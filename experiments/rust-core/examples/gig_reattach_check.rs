//! Gig-hardening e2e check (`plan/04-gig-hardening.md`): boot a REAL gig
//! session, simulate a GUI crash (drop the `Supervisor` without shutting
//! down — no `Child::kill()` involved, exactly what happens when a GUI
//! process dies), then reattach a brand-new `Session` using only the
//! on-disk lock — proving the whole "GUI dies, music keeps playing,
//! relaunch reconnects" story end to end against the real runtime, without
//! touching any other running instance (its own throwaway lock file, its
//! own dynamically-allocated ports).
//!
//!   cargo run --example gig_reattach_check

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sonicpi_core::session_lock::SessionLock;
use sonicpi_core::supervisor::{BootMode, Supervisor};
use sonicpi_core::{ApiClient, ClientEvent, Session};

const QUIET_RUN: &str = "live_loop :gig do\n  sample :bd_haus, amp: 0\n  sleep 0.25\nend";

struct Counter(Arc<AtomicUsize>);
impl ApiClient for Counter {
    fn on_event(&self, _: ClientEvent) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
    let lock_path = std::env::temp_dir().join("sonic_oxide_gig_reattach_check.lock");
    let _ = std::fs::remove_file(&lock_path);
    let mut failures = Vec::new();

    println!("booting a GIG-MODE supervisor…");
    let sup = Supervisor::boot(&root, BootMode::Gig).expect("gig supervisor boot");
    sup.write_session_lock(&lock_path).expect("write session lock");
    let (spider_pid, supersonic_pid);
    {
        let lock = SessionLock::load(&lock_path).expect("load session lock");
        assert_eq!(lock.token, sup.ports.token);
        if !lock.still_live() {
            failures.push("freshly-written lock does not verify as live".to_string());
        }
        spider_pid = lock.spider_pid;
        supersonic_pid = lock.supersonic_pid;
    }

    // Prove it's actually up: run something through the FIRST session.
    let events_a = Arc::new(AtomicUsize::new(0));
    let session_a =
        Session::connect(&sup.ports, Arc::new(Counter(events_a.clone()))).expect("session a");
    session_a.run("gig-a", QUIET_RUN).expect("run before crash");
    std::thread::sleep(Duration::from_secs(3));
    if events_a.load(Ordering::Relaxed) == 0 {
        failures.push("no events on the pre-crash session — engine never came up".into());
    }
    drop(session_a);

    // ── Simulate a GUI crash: drop the Supervisor WITHOUT shutdown() ──────
    // This is exactly what `kill -9`-ing the GUI does in gig mode: no
    // Child::kill() call happens (Drop is a no-op for BootMode::Gig — see
    // supervisor.rs), so the children must still be running afterward.
    println!("simulating a GUI crash (dropping the Supervisor, no shutdown)…");
    drop(sup);
    std::thread::sleep(Duration::from_millis(500));

    let lock = SessionLock::load(&lock_path).expect("load session lock after crash");
    if !lock.still_live() {
        failures.push("children did not survive the simulated crash".into());
    }

    // ── Reattach: a brand-new Session, from nothing but the lock file ─────
    println!("reattaching from the lock file alone…");
    let events_b = Arc::new(AtomicUsize::new(0));
    let ports = lock.to_ports();
    let session_b =
        Session::connect(&ports, Arc::new(Counter(events_b.clone()))).expect("session b");
    session_b.ping().expect("ping the reattached session");
    std::thread::sleep(Duration::from_millis(500));
    if events_b.load(Ordering::Relaxed) == 0 {
        failures.push("reattached session got no reply to /ping".into());
    }
    session_b.run("gig-b", QUIET_RUN).expect("run after reattach");
    std::thread::sleep(Duration::from_secs(2));

    // ── Explicit "Stop performance": the only path that actually ends it ──
    println!("stopping the performance (SIGTERM, then SIGKILL fallback)…");
    unsafe {
        libc::kill(spider_pid as libc::pid_t, libc::SIGTERM);
        libc::kill(supersonic_pid as libc::pid_t, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(500));
    unsafe {
        libc::kill(spider_pid as libc::pid_t, libc::SIGKILL);
        libc::kill(supersonic_pid as libc::pid_t, libc::SIGKILL);
    }
    // Reap them: THIS test process is the real OS-level parent of both
    // children the whole time (dropping the Rust `Supervisor` value earlier
    // only stopped tracking them via that handle — it never exited the
    // process, so no reparenting-to-init ever happened). A killed child
    // stays a zombie — visible in /proc, same start time — until its parent
    // calls wait() on it, so without this reap `still_live()` below would
    // (correctly, for THIS process) still see them as "alive". In the real
    // crash/relaunch scenario the app process actually exits, the kernel
    // reparents the orphans to init, and init reaps them promptly — so a
    // genuinely separate relaunched app is never in this position and, not
    // being the parent, has no `wait()` to call (nor should it try one).
    unsafe {
        libc::waitpid(spider_pid as libc::pid_t, std::ptr::null_mut(), 0);
        libc::waitpid(supersonic_pid as libc::pid_t, std::ptr::null_mut(), 0);
    }
    std::thread::sleep(Duration::from_millis(300));
    SessionLock::remove(&lock_path);

    if lock.still_live() {
        failures.push("session still verifies as live after Stop Performance".into());
    }
    if lock_path.exists() {
        failures.push("lock file survived removal".into());
    }

    if failures.is_empty() {
        println!("PASS");
    } else {
        for f in &failures {
            eprintln!("FAIL: {f}");
        }
        std::process::exit(1);
    }
}
