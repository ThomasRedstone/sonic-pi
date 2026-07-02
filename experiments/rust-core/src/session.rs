//! `Session` — ties senders, the incoming server, and the keep-alive loop
//! together behind the ports/token from a daemon handshake. This is the live
//! wiring the Qt GUI would talk to (the Phase-2 drop-in replacement for the
//! parts of `SonicPiAPI` that own the OSC plumbing).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use rosc::OscMessage;

use crate::client::ApiClient;
use crate::osc::{OscServer, UdpOscSender};
use crate::ports::{PortId, Ports};
use crate::protocol;
use crate::CoreError;

/// Background loop that pings the daemon so it doesn't tear everything down.
/// Stops on drop.
pub struct KeepAlive {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl KeepAlive {
    pub fn start(sender: UdpOscSender, token: i32) -> KeepAlive {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                let _ = sender.send(&protocol::out::keep_alive(token));
                // The C++ pings every 4s; poll the stop flag more often so
                // shutdown is snappy.
                for _ in 0..40 {
                    if stop_thread.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
        KeepAlive { stop, handle: Some(handle) }
    }
}

impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// A connected session: three senders (Spider / daemon / SuperSonic), the
/// incoming OSC server, and the keep-alive loop.
pub struct Session {
    spider: UdpOscSender,
    daemon: UdpOscSender,
    supersonic: UdpOscSender,
    token: i32,
    _server: OscServer,
    _keep_alive: KeepAlive,
}

impl Session {
    /// Wire up all sockets from resolved `ports`, routing decoded incoming
    /// messages to `client`. Does not spawn the daemon (see `process::Daemon`);
    /// this is the "given a handshake, connect" half.
    pub fn connect(ports: &Ports, client: Arc<dyn ApiClient>) -> Result<Session, CoreError> {
        let token = ports.token;

        let spider = UdpOscSender::to_localhost(ports.get(PortId::GuiSendToSpider))?;
        let daemon = UdpOscSender::to_localhost(ports.get(PortId::Daemon))?;
        let supersonic = UdpOscSender::to_localhost(ports.get(PortId::Scsynth))?;

        let server = OscServer::start(ports.get(PortId::GuiListenToSpider), move |m| {
            if let Some(event) = protocol::parse_incoming(&m) {
                client.on_event(event);
            }
        })?;

        let keep_alive =
            KeepAlive::start(UdpOscSender::to_localhost(ports.get(PortId::Daemon))?, token);

        Ok(Session {
            spider,
            daemon,
            supersonic,
            token,
            _server: server,
            _keep_alive: keep_alive,
        })
    }

    /// Run a buffer (`/save-and-run-buffer`).
    pub fn run(&self, name: &str, code: &str) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::run_buffer(self.token, name, code))
    }

    /// Stop all jobs (`/stop-all-jobs`).
    pub fn stop(&self) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::stop_all_jobs(self.token))
    }

    /// Probe whether Spider is up yet (`/ping`).
    pub fn ping(&self) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::ping(self.token, "core/1/hello"))
    }

    /// Send a raw message straight to SuperSonic (e.g. `/clock/*`,
    /// `/supersonic/*`).
    pub fn send_to_supersonic(&self, m: &OscMessage) -> Result<(), CoreError> {
        self.supersonic.send(m)
    }

    /// Ask the daemon to exit cleanly (`/daemon/exit`).
    pub fn shutdown(&self) -> Result<(), CoreError> {
        self.daemon.send(&protocol::out::daemon_exit(self.token))
    }

    /// Switch audio devices (`/daemon/audio/switch-device`). Empty strings /
    /// zeros leave that aspect unchanged; input `"__none__"` disables inputs.
    pub fn switch_audio_device(
        &self,
        output: &str,
        sample_rate: f32,
        buffer_size: i32,
        input: &str,
    ) -> Result<(), CoreError> {
        self.daemon.send(&protocol::out::audio_switch_device(
            self.token,
            output,
            sample_rate,
            buffer_size,
            input,
        ))
    }

    pub fn token(&self) -> i32 {
        self.token
    }
}
