//! Gig-mode session lockfile (plan `04-gig-hardening.md`) — lets a
//! relaunched GUI **reattach** to a runtime that outlived a previous GUI
//! process instead of spawning a second one on top of it.
//!
//! Format: flat `key = value` lines, one per field — the same convention
//! `gpui-spike/src/store.rs` uses for prefs.conf. Five fields don't earn a
//! new serde dependency.
//!
//! Safety property this module exists to provide: **fail closed**. A wrong
//! "yes, that session is still alive" is far worse than a wrong "no" (it
//! would mean spawning a second SuperSonic on top of a live one, fighting
//! over the same audio device) — so any ambiguity anywhere in this module
//! (unparseable file, unreadable `/proc`, unsupported OS) resolves to "not
//! the same process" / "can't verify", never to "assume alive".

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::ports::Ports;

/// Everything a relaunched GUI needs to reconnect to a running gig session
/// without respawning it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLock {
    pub token: i32,
    pub spider_pid: u32,
    pub spider_started: u64,
    pub supersonic_pid: u32,
    pub supersonic_started: u64,
    pub daemon_port: u16,
    pub gui_listen: u16,
    pub gui_send: u16,
    pub scsynth: u16,
    pub osc_cues: u16,
}

impl SessionLock {
    /// Rebuild the `Ports` table `Session::connect` needs — no daemon
    /// handshake required, since the ports were allocated (and are now
    /// remembered) by the supervisor that's still running.
    pub fn to_ports(&self) -> Ports {
        Ports::from_parts(
            self.daemon_port,
            self.gui_listen,
            self.gui_send,
            self.scsynth,
            self.osc_cues,
            self.token,
        )
    }

    /// True only if BOTH recorded processes are still the SAME processes
    /// (not a PID recycled by an unrelated program since the lock was
    /// written). This is the reattach gate — callers must not reattach on
    /// `false`.
    pub fn still_live(&self) -> bool {
        is_same_process(self.spider_pid, self.spider_started)
            && is_same_process(self.supersonic_pid, self.supersonic_started)
    }

    fn serialize(&self) -> String {
        let fields: [(&str, String); 10] = [
            ("token", self.token.to_string()),
            ("spider_pid", self.spider_pid.to_string()),
            ("spider_started", self.spider_started.to_string()),
            ("supersonic_pid", self.supersonic_pid.to_string()),
            ("supersonic_started", self.supersonic_started.to_string()),
            ("daemon_port", self.daemon_port.to_string()),
            ("gui_listen", self.gui_listen.to_string()),
            ("gui_send", self.gui_send.to_string()),
            ("scsynth", self.scsynth.to_string()),
            ("osc_cues", self.osc_cues.to_string()),
        ];
        fields.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\n") + "\n"
    }

    fn parse(text: &str) -> Option<SessionLock> {
        let mut map: HashMap<&str, &str> = HashMap::new();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim(), v.trim());
            }
        }
        let get_u16 = |k: &str| map.get(k)?.parse::<u16>().ok();
        let get_u32 = |k: &str| map.get(k)?.parse::<u32>().ok();
        let get_u64 = |k: &str| map.get(k)?.parse::<u64>().ok();
        let get_i32 = |k: &str| map.get(k)?.parse::<i32>().ok();
        Some(SessionLock {
            token: get_i32("token")?,
            spider_pid: get_u32("spider_pid")?,
            spider_started: get_u64("spider_started")?,
            supersonic_pid: get_u32("supersonic_pid")?,
            supersonic_started: get_u64("supersonic_started")?,
            daemon_port: get_u16("daemon_port")?,
            gui_listen: get_u16("gui_listen")?,
            gui_send: get_u16("gui_send")?,
            scsynth: get_u16("scsynth")?,
            osc_cues: get_u16("osc_cues")?,
        })
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.serialize())
    }

    /// `None` for anything short of a fully-formed lock: missing file,
    /// unreadable, or truncated/corrupt — never guess at partial data.
    pub fn load(path: &Path) -> Option<SessionLock> {
        let text = std::fs::read_to_string(path).ok()?;
        Self::parse(&text)
    }

    pub fn remove(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

/// Where the lockfile lives for a given workspace store directory
/// (mirrors `store::default_store_dir()` in the GPUI spike — this crate
/// doesn't depend on that module, so callers pass the dir in).
pub fn lock_path(store_dir: &Path) -> PathBuf {
    store_dir.join("session.lock")
}

/// True iff a process with `pid` is running right now AND its start time
/// matches `expected_start` — the PID-reuse guard (`pid_start_time` is the
/// portable-but-Linux-only half of this; other platforms always return
/// `None`, so this is always `false` there, which is the correct fail-closed
/// behaviour until a phase-9 port adds a real implementation).
pub fn is_same_process(pid: u32, expected_start: u64) -> bool {
    pid_start_time(pid) == Some(expected_start)
}

/// Field 22 (`starttime`, clock ticks since boot) of `/proc/<pid>/stat` —
/// monotonic, already available, immune to PID reuse because a NEW process
/// reusing an old PID gets a NEW start time. `None` for "can't verify"
/// (process gone, unreadable, or an OS with no `/proc`).
#[cfg(target_os = "linux")]
pub fn pid_start_time(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the process name can't be split on plain whitespace
    // naively — the name itself (in parens) may contain spaces or even
    // parens. Comm is delimited by the FIRST '(' and the LAST ')' (the
    // kernel's own parsing rule), so everything after that last ')' is the
    // remaining space-separated fields, with state as field 3.
    let after_comm = text.rsplit_once(')')?.1;
    // starttime is field 22 overall == the 20th field after state (field 3).
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
pub fn pid_start_time(_pid: u32) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SessionLock {
        SessionLock {
            token: 123456,
            spider_pid: 111,
            spider_started: 222,
            supersonic_pid: 333,
            supersonic_started: 444,
            daemon_port: 5000,
            gui_listen: 5001,
            gui_send: 5002,
            scsynth: 5003,
            osc_cues: 4560,
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join("sonic_oxide_session_lock_test_rt");
        let _ = std::fs::remove_dir_all(&dir);
        let path = lock_path(&dir);
        let lock = sample();
        lock.save(&path).unwrap();
        let loaded = SessionLock::load(&path).unwrap();
        assert_eq!(loaded, lock);
        SessionLock::remove(&path);
        assert!(SessionLock::load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_corrupt_file_yields_none_not_a_panic() {
        assert!(SessionLock::load(Path::new("/nonexistent/session.lock")).is_none());
        let dir = std::env::temp_dir().join("sonic_oxide_session_lock_test_corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = lock_path(&dir);
        std::fs::write(&path, "token = 1\nspider_pid = 2\n").unwrap(); // missing fields
        assert!(SessionLock::load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn to_ports_carries_the_token_and_every_port() {
        let lock = sample();
        let ports = lock.to_ports();
        assert_eq!(ports.token, 123456);
        assert_eq!(ports.get(crate::ports::PortId::Scsynth), 5003);
        assert_eq!(ports.get(crate::ports::PortId::TauOscCues), 4560);
    }

    #[test]
    fn a_bogus_pid_never_verifies_as_live() {
        // PID 0 is never a real process to wait on; a start-time lookup for
        // it must fail closed (None), not panic or (worse) coincidentally
        // match some default.
        assert!(!is_same_process(0, 0));
        assert!(!is_same_process(0, 999_999_999));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn reads_our_own_real_start_time_stably() {
        // Exercises the real /proc parser (no fake filesystem needed — our
        // own process is a real, stable target): the same PID's start time
        // must be identical across repeated reads within one test run.
        let pid = std::process::id();
        let a = pid_start_time(pid).expect("must read our own /proc/self/stat");
        let b = pid_start_time(pid).expect("second read");
        assert_eq!(a, b);
        assert!(is_same_process(pid, a));
        assert!(!is_same_process(pid, a.wrapping_add(1)));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn an_exited_pid_reads_as_gone() {
        // Spawn a process that exits immediately, wait for it, then its PID
        // (now free — or reused by something else entirely) must not read
        // back as verifiably alive with a start time we never recorded.
        let mut child = std::process::Command::new("true").spawn().expect("spawn `true`");
        let pid = child.id();
        child.wait().expect("wait");
        // Give the kernel a moment to actually reap /proc/<pid>.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(pid_start_time(pid).is_none() || !is_same_process(pid, 0));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn non_linux_never_verifies_liveness_yet() {
        // Documents the intentional current limitation (see 04-gig-hardening.md,
        // phase 9): gig-mode reattach doesn't claim to work outside Linux.
        assert_eq!(pid_start_time(std::process::id()), None);
        assert!(!is_same_process(std::process::id(), 1));
    }
}
