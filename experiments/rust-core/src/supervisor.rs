//! Phase-4 supervisor — the Rust replacement for `daemon.rb`.
//!
//! Where the Ruby daemon is a separate process (port allocation, spawning
//! Spider + SuperSonic, a keep-alive kill switch), the supervisor folds that
//! job into the core: the app process itself allocates ports and owns the
//! children (4 processes → 3).
//!
//! Crash-safety replaces the keep-alive dance: children are spawned with
//! `PR_SET_PDEATHSIG(SIGTERM)` (Linux), so if the app dies — even SIGKILL —
//! the kernel terminates Spider and SuperSonic. No 40s+ kill-switch window.
//!
//! Spawn invocations mirror `daemon.rb` (no shell involved anywhere —
//! `Command` passes args as a vector):
//!   * SuperSonic: `supersonic -u <scsynth> -a 1024 -m 131072 -D 0 -R 0
//!     -l 1 -b 4096 -B 127.0.0.1 -Z 1024` (`SupersonicBooter::DEFAULT_OPTS`;
//!     audio-settings.toml overrides are TODO)
//!   * Spider: `ruby --enable-frozen-string-literal -E utf-8 --yjit
//!     spider-server.rb -u <spider-listen> <spider-send> <scsynth>
//!     <scsynth-send> <osc-cues> <token>` (`SpiderBooter`), where the pairs
//!     collapse exactly as the daemon's port table pairs them.
//!
//! Not yet ported: `/daemon/audio/switch-device` (needs a SuperSonic restart
//! path) and TOML audio options — the daemon.rb route stays available as the
//! A/B fallback until they land.

use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::paths::{resolve, SonicPiPath};
use crate::ports::Ports;
use crate::CoreError;

/// `SupersonicBooter::DEFAULT_OPTS`, in order.
const SUPERSONIC_DEFAULT_OPTS: &[&str] = &[
    "-a", "1024", "-m", "131072", "-D", "0", "-R", "0", "-l", "1", "-b", "4096", "-B",
    "127.0.0.1", "-Z", "1024",
];

pub struct Supervisor {
    pub ports: Ports,
    supersonic: Child,
    spider: Child,
}

/// Ask the OS for a free UDP port on localhost. Same freshness guarantee as
/// daemon.rb's check-then-use (a race is possible but the window is tiny and
/// the children bind immediately).
fn free_udp_port() -> Result<u16, CoreError> {
    let sock = UdpSocket::bind("127.0.0.1:0")
        .map_err(|e| CoreError::Spawn(format!("allocating port: {e}")))?;
    let port = sock
        .local_addr()
        .map_err(|e| CoreError::Spawn(format!("reading allocated port: {e}")))?
        .port();
    Ok(port)
}

/// Die-with-parent on Linux: the kernel SIGTERMs the child if this process
/// exits for any reason. This is the crash-safety net that replaces the
/// daemon's keep-alive kill switch.
fn die_with_parent(cmd: &mut Command) {
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
}

impl Supervisor {
    /// Allocate ports, boot SuperSonic (waiting for its shm segment to
    /// publish), then boot Spider. `app_root` is the Sonic Pi `app/` dir.
    pub fn boot(app_root: &Path) -> Result<Supervisor, CoreError> {
        let paths = resolve(app_root);
        let supersonic_bin = app_root.join("server/native/supersonic");
        let spider_rb = paths[&SonicPiPath::ServerBin].join("spider-server.rb");
        if !supersonic_bin.exists() {
            return Err(CoreError::Spawn(format!("missing {}", supersonic_bin.display())));
        }
        if !spider_rb.exists() {
            return Err(CoreError::Spawn(format!("missing {}", spider_rb.display())));
        }

        let daemon = free_udp_port()?; // wire-compat slot; no listener yet
        let gui_listen = free_udp_port()?;
        let gui_send = free_udp_port()?;
        let scsynth = free_udp_port()?;
        let osc_cues = free_udp_port()?;
        // Token: daemon.rb uses rand(max i32); time^pid is enough entropy for
        // a localhost session credential.
        let token = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0) as i32)
            .wrapping_mul(31)
            .wrapping_add(std::process::id() as i32)
            .wrapping_abs()
            | 1;
        let ports = Ports::from_parts(daemon, gui_listen, gui_send, scsynth, osc_cues, token);

        // SuperSonic first (Spider needs a live engine).
        let mut cmd = Command::new(&supersonic_bin);
        cmd.arg("-u").arg(scsynth.to_string()).args(SUPERSONIC_DEFAULT_OPTS);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        die_with_parent(&mut cmd);
        let supersonic =
            cmd.spawn().map_err(|e| CoreError::Spawn(format!("spawning supersonic: {e}")))?;

        // The engine publishes /dev/shm/SuperSonic_<port> once it is up —
        // the same readiness signal the shm readers use.
        let shm_path = PathBuf::from(format!("/dev/shm/SuperSonic_{scsynth}"));
        let deadline = Instant::now() + Duration::from_secs(15);
        while !shm_path.exists() {
            if Instant::now() >= deadline {
                let mut child = supersonic;
                let _ = child.kill();
                let _ = child.wait();
                return Err(CoreError::Spawn("supersonic never published its shm".into()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // Spider (mirrors SpiderBooter, with the port pairs collapsed:
        // spider-listen = gui-send, spider-send = gui-listen,
        // scsynth-send = scsynth).
        let mut cmd = Command::new(&paths[&SonicPiPath::Ruby]);
        cmd.arg("--enable-frozen-string-literal")
            .arg("-E")
            .arg("utf-8")
            .arg("--yjit")
            .arg(&spider_rb)
            .arg("-u")
            .arg(gui_send.to_string())
            .arg(gui_listen.to_string())
            .arg(scsynth.to_string())
            .arg(scsynth.to_string())
            .arg(osc_cues.to_string())
            .arg(token.to_string());
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        die_with_parent(&mut cmd);
        let spider = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let mut child = supersonic;
                let _ = child.kill();
                let _ = child.wait();
                return Err(CoreError::Spawn(format!("spawning spider: {e}")));
            }
        };

        Ok(Supervisor { ports, supersonic, spider })
    }

    /// Graceful shutdown: TERM Spider first (it says goodbye to the engine),
    /// then TERM SuperSonic; KILL whatever ignores the request.
    pub fn shutdown(&mut self) {
        for child in [&mut self.spider, &mut self.supersonic] {
            #[cfg(unix)]
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        }
    }

    pub fn children_running(&mut self) -> (bool, bool) {
        let spider = matches!(self.spider.try_wait(), Ok(None));
        let supersonic = matches!(self.supersonic.try_wait(), Ok(None));
        (spider, supersonic)
    }

    /// `shutdown()` + verify: returns true only when both children are
    /// confirmed gone. Callers should log a warning on false rather than
    /// assume success (one unexplained orphan incident is on record).
    pub fn shutdown_verified(&mut self) -> bool {
        self.shutdown();
        let (spider, supersonic) = self.children_running();
        !spider && !supersonic
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_free_ports_that_rebind() {
        let a = free_udp_port().unwrap();
        assert!(a > 0);
        let _s = UdpSocket::bind(("127.0.0.1", a)).expect("port should be free to bind");
    }

    #[test]
    fn boot_fails_cleanly_without_a_runtime() {
        let err = Supervisor::boot(Path::new("/nonexistent/app"));
        assert!(err.is_err());
    }
}
