//! Boot-daemon supervision — Rust mirror of `StartBootDaemon`.
//!
//! Scaffold: `boot()` spawns the Ruby daemon and parses its handshake line.
//! Fully exercising it needs the real Sonic Pi runtime (bundled Ruby + Spider +
//! SuperSonic), so this is structurally complete but not yet integration-tested.
//! In the Phase-4 end state, this module also absorbs the daemon's own job
//! (port allocation, kill switch) so the Ruby daemon process disappears.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::ports::Ports;
use crate::CoreError;

/// A spawned boot daemon plus the ports it reported.
pub struct Daemon {
    pub child: Child,
    pub ports: Ports,
}

impl Daemon {
    /// Spawn `ruby <daemon_script>` and read the single handshake line it prints
    /// to stdout, returning the parsed ports + token.
    pub fn boot(ruby: &Path, daemon_script: &Path) -> Result<Daemon, CoreError> {
        let mut child = Command::new(ruby)
            .arg(daemon_script)
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| CoreError::Spawn(format!("spawning daemon: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Spawn("daemon produced no stdout".into()))?;

        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .map_err(|e| CoreError::Spawn(format!("reading handshake: {e}")))?;

        let ports = Ports::parse_daemon_line(line.trim())?;
        Ok(Daemon { child, ports })
    }

    /// Request a clean shutdown by killing the child. (The polite path is to
    /// send `/daemon/exit` first via the daemon sender; see `Session::shutdown`.)
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Wait up to `timeout` for the daemon to exit on its own (e.g. after a
    /// `/daemon/exit` sent via `Session::shutdown`). Returns `true` if it
    /// exited within the window; the caller should `kill()` on `false`.
    pub fn wait_timeout(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Backstop: never leave an orphaned daemon behind. The polite quit path
/// (`/daemon/exit` + `wait_timeout`) should already have reaped the child, in
/// which case this is a no-op.
impl Drop for Daemon {
    fn drop(&mut self) {
        self.kill();
    }
}
