//! macOS crash-safety primitive (`plan/07-platforms-release.md`, phase 9).
//!
//! Linux gets `PR_SET_PDEATHSIG` (one syscall, `supervisor.rs`); Windows
//! gets a kill-on-close Job Object (`WinJob`, also `supervisor.rs`). macOS
//! has no single-syscall equivalent — the standard idiom is a small
//! separate watchdog process that `kqueue`-watches the app's PID via
//! `EVFILT_PROC`/`NOTE_EXIT` and kills the audio children the moment it
//! sees the app exit (for any reason, including a crash — a kqueue process
//! filter fires on exit regardless of cause).
//!
//! This module ships the actual watching PRIMITIVE — the piece with no
//! precedent elsewhere in the codebase and the one genuinely unverifiable
//! without either a real Mac or (as done here) macOS CI. **Not yet wired
//! into `Supervisor::boot`**: turning this into a real watchdog means
//! spawning it as a genuinely separate OS process (so it outlives the app
//! if the app itself is what died), which in turn needs a way to locate
//! the watchdog's own executable at runtime (a `paths.rs`-shaped problem)
//! and a macOS packaging story — neither exists yet. Landing the primitive
//! now, verified for real on `macos-latest` CI, de-risks that follow-up
//! without having to make packaging decisions today.

#![cfg(target_os = "macos")]

use std::io;
use std::time::{Duration, Instant};

/// Block until `pid` exits or `timeout` elapses. `Ok(true)` = exit
/// observed, `Ok(false)` = timed out with the process presumably still
/// running, `Err` = `kqueue`/`kevent` setup failed (caller should treat
/// that as "can't supervise this process" rather than assume anything
/// about its liveness).
///
/// Uses a single `EVFILT_PROC`/`NOTE_EXIT` registration + a blocking
/// `kevent()` wait — the same mechanism a real watchdog would use, just
/// invoked synchronously here rather than in a standalone process.
pub fn wait_for_process_exit(pid: u32, timeout: Duration) -> io::Result<bool> {
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut change: libc::kevent = std::mem::zeroed();
        change.ident = pid as libc::uintptr_t;
        change.filter = libc::EVFILT_PROC;
        change.flags = libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT;
        change.fflags = libc::NOTE_EXIT;

        // Register the filter. If `pid` has ALREADY exited by this point,
        // kevent() itself fails with ESRCH — treat that as "exit observed"
        // (there's nothing left to watch, which is the same outcome).
        let mut event: libc::kevent = std::mem::zeroed();
        let registered = libc::kevent(
            kq,
            &change,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        );
        if registered < 0 {
            let e = io::Error::last_os_error();
            libc::close(kq);
            return Ok(e.raw_os_error() == Some(libc::ESRCH));
        }

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                libc::close(kq);
                return Ok(false);
            }
            let ts = libc::timespec {
                tv_sec: remaining.as_secs() as libc::time_t,
                tv_nsec: remaining.subsec_nanos() as i64,
            };
            let n = libc::kevent(kq, std::ptr::null(), 0, &mut event, 1, &ts);
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue; // EINTR — a signal landed mid-wait, just retry
                }
                libc::close(kq);
                return Err(err);
            }
            libc::close(kq);
            // n == 0 is a timeout (shouldn't happen given the deadline
            // check above, but treat it the same for safety); n == 1 with
            // NOTE_EXIT set is the real signal.
            return Ok(n > 0 && event.fflags & libc::NOTE_EXIT != 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_real_process_exiting() {
        // /bin/sleep exists on every macOS install; 1s is short enough for
        // a fast CI test but long enough that "detected instantly" would
        // be suspicious (i.e. this genuinely waits, not a stub returning
        // true unconditionally).
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("1")
            .spawn()
            .expect("spawn /bin/sleep");
        let pid = child.id();

        let start = Instant::now();
        let exited = wait_for_process_exit(pid, Duration::from_secs(10)).expect("kqueue wait");
        assert!(exited, "expected the sleep process's exit to be observed");
        assert!(
            start.elapsed() >= Duration::from_millis(800),
            "detected exit suspiciously fast ({:?}) — is this actually watching?",
            start.elapsed()
        );

        let _ = child.wait(); // reap; avoid a zombie in the test process
    }

    #[test]
    fn times_out_on_a_process_that_outlives_the_wait() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("spawn /bin/sleep");
        let pid = child.id();

        let exited = wait_for_process_exit(pid, Duration::from_millis(300)).expect("kqueue wait");
        assert!(!exited, "expected a timeout, not an exit, within 300ms of a 5s sleep");

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn an_already_exited_pid_resolves_immediately() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("0")
            .spawn()
            .expect("spawn /bin/sleep 0");
        let pid = child.id();
        let _ = child.wait(); // let it actually exit and get reaped first

        let start = Instant::now();
        let exited = wait_for_process_exit(pid, Duration::from_secs(5)).expect("kqueue wait");
        assert!(exited, "an already-dead pid must resolve as exited, not hang");
        assert!(start.elapsed() < Duration::from_secs(1), "should resolve near-instantly");
    }
}
