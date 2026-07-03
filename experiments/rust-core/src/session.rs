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
    scsynth_port: u16,
    server: OscServer,
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

        // Register for device pushes (and get an immediate report) — this is
        // how the device lists reach the client in both boot modes.
        let _ = supersonic.send(&protocol::out::supersonic_devices_report(
            ports.get(PortId::GuiListenToSpider),
        ));
        // Driver enumeration: the reply routes to the request's source, so it
        // must leave from the incoming server's socket.
        let _ = server
            .send_from(ports.get(PortId::Scsynth), &protocol::out::supersonic_drivers_list());

        Ok(Session {
            spider,
            daemon,
            supersonic,
            token,
            scsynth_port: ports.get(PortId::Scsynth),
            server,
            _keep_alive: keep_alive,
        })
    }

    /// Hot-swap audio devices directly on the engine
    /// (`/supersonic/devices/switch`) — the supervisor-mode path, where
    /// there is no daemon to forward `/daemon/audio/switch-device`.
    pub fn switch_audio_device_direct(
        &self,
        output: &str,
        sample_rate: f32,
        buffer_size: i32,
        input: &str,
    ) -> Result<(), CoreError> {
        self.supersonic.send(&protocol::out::supersonic_devices_switch(
            output,
            sample_rate,
            buffer_size,
            input,
        ))
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

    /// Set the master volume (`/mixer-amp`, amp 0.0..=2.0 like the Qt slider).
    pub fn set_mixer_amp(&self, amp: f32, silent: bool) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::mixer_amp(self.token, amp, silent))
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

    /// Enable/disable Spider's MIDI subsystem (`/midi-start` / `/midi-stop`).
    pub fn set_midi_enabled(&self, enabled: bool, silent: bool) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::midi_enabled(self.token, enabled, silent))
    }

    /// Start/stop the incoming-OSC cue server (`/cue-port-start|stop`).
    pub fn set_cue_server_enabled(&self, enabled: bool) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::cue_server_enabled(self.token, enabled))
    }

    /// Cue server reachability: remote hosts vs localhost only
    /// (`/cue-port-external|internal`).
    pub fn set_cue_server_external(&self, external: bool) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::cue_server_external(self.token, external))
    }

    /// Invert the stereo field (`/mixer-invert-stereo|standard-stereo`).
    pub fn set_mixer_invert_stereo(&self, invert: bool) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::mixer_invert_stereo(self.token, invert))
    }

    /// Force mono output (`/mixer-mono-mode|stereo-mode`).
    pub fn set_mixer_force_mono(&self, mono: bool) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::mixer_force_mono(self.token, mono))
    }

    /// Master high-pass filter: `Some(freq)` enables (MIDI-note cutoff),
    /// `None` disables (`/mixer-hpf-enable|disable`).
    pub fn set_mixer_hpf(&self, freq: Option<f32>) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::mixer_hpf(self.token, freq))
    }

    /// Master low-pass filter (`/mixer-lpf-enable|disable`).
    pub fn set_mixer_lpf(&self, freq: Option<f32>) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::mixer_lpf(self.token, freq))
    }

    /// Version-check opt-in (`/enable-update-checking|disable-…`).
    pub fn set_update_checking(&self, enabled: bool) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::update_checking(self.token, enabled))
    }

    /// Enable/disable Spider's gamepad subsystem (`/gamepad-start|stop`).
    pub fn set_gamepad_enabled(&self, enabled: bool, silent: bool) -> Result<(), CoreError> {
        self.spider.send(&protocol::out::gamepad_enabled(self.token, enabled, silent))
    }

    /// Daemon-brokered audio driver switch (`/daemon/audio/switch-driver`).
    pub fn switch_audio_driver(&self, driver: &str) -> Result<(), CoreError> {
        self.daemon.send(&protocol::out::audio_switch_driver(self.token, driver))
    }

    /// Direct engine driver switch (`/supersonic/drivers/switch`) — the
    /// supervisor-mode path.
    pub fn switch_audio_driver_direct(&self, driver: &str) -> Result<(), CoreError> {
        self.supersonic.send(&protocol::out::supersonic_drivers_switch(driver))
    }

    /// Re-request the driver enumeration (reply surfaces as
    /// `ClientEvent::AudioDrivers`).
    pub fn request_audio_drivers(&self) -> Result<(), CoreError> {
        self.server.send_from(self.scsynth_port, &protocol::out::supersonic_drivers_list())
    }

    pub fn token(&self) -> i32 {
        self.token
    }
}
