//! The OSC vocabulary — addresses, outgoing builders, and the incoming parser.
//!
//! Addresses and argument shapes are taken verbatim from the C++:
//!   * outgoing (core → spider/daemon/supersonic): `sonicpi_api.cpp`
//!   * incoming (spider → core): `osc/osc_handler.cpp`
//!
//! Convention (matches the C++): every spider-bound message is prefixed with
//! the session token as an `Int(i32)`.

use rosc::{OscMessage, OscType};

use crate::client::*;

/// Known OSC addresses.
pub mod addr {
    // ── Outgoing: core → Spider runtime (port `gui_send_to_spider`) ──────────
    pub const SAVE_AND_RUN_BUFFER: &str = "/save-and-run-buffer";
    pub const STOP_ALL_JOBS: &str = "/stop-all-jobs";
    pub const PING: &str = "/ping";
    pub const LOAD_BUFFER: &str = "/load-buffer";
    pub const SAVE_BUFFER: &str = "/save-buffer";
    pub const BUFFER_NEWLINE_AND_INDENT: &str = "/buffer-newline-and-indent";
    pub const MIXER_AMP: &str = "/mixer-amp";
    pub const SET_GLOBAL_TIMEWARP: &str = "/set-global-timewarp";

    // ── Outgoing: core → Boot daemon (port `daemon`) ─────────────────────────
    pub const DAEMON_KEEP_ALIVE: &str = "/daemon/keep-alive";
    pub const DAEMON_EXIT: &str = "/daemon/exit";
    pub const DAEMON_AUDIO_SWITCH_DEVICE: &str = "/daemon/audio/switch-device";

    // ── Outgoing: core → SuperSonic (port `scsynth`) ─────────────────────────
    pub const CLOCK_TEMPO_SET: &str = "/clock/tempo/set";
    pub const CLOCK_VISIBILITY: &str = "/clock/visibility";
    pub const CLOCK_AUDIO_PUBLISH_SET: &str = "/clock/audio/publish/set";
    pub const CLOCK_PEER_NAME_SET: &str = "/clock/peer_name/set";
    pub const SUPERSONIC_DEVICES_REPORT: &str = "/supersonic/devices/report";
    pub const SUPERSONIC_DEVICES_SWITCH: &str = "/supersonic/devices/switch";
    pub const SUPERSONIC_RECORD_START: &str = "/supersonic/record/start";
    pub const SUPERSONIC_RECORD_STOP: &str = "/supersonic/record/stop";

    // ── Incoming: Spider/SuperSonic → core (port `gui_listen_to_spider`) ─────
    pub const LOG_MULTI_MESSAGE: &str = "/log/multi_message";
    pub const LOG_INFO: &str = "/log/info";
    pub const ERROR: &str = "/error";
    pub const SYNTAX_ERROR: &str = "/syntax_error";
    pub const ACK: &str = "/ack";
    pub const RUNS_ALL_COMPLETED: &str = "/runs/all-completed";
    pub const EXITED: &str = "/exited";
    pub const EXITED_WITH_BOOT_ERROR: &str = "/exited-with-boot-error";
    pub const VERSION: &str = "/version";
    pub const LINK_BPM: &str = "/link-bpm";
    pub const LINK_NUM_PEERS: &str = "/link-num-peers";
    pub const SPIDER_READY: &str = "/spider/ready";
    pub const INCOMING_OSC: &str = "/incoming/osc";
    pub const MIDI_IN_PORTS: &str = "/midi/in-ports";
    pub const MIDI_OUT_PORTS: &str = "/midi/out-ports";
    pub const GAMEPAD_DEVICES_LIST: &str = "/gamepad/devices-list";
    pub const UPDATE_INFO_TEXT: &str = "/update-info-text";
    pub const SUPERSONIC_DEVICES: &str = "/supersonic/devices";
    pub const SUPERSONIC_INPUT_DEVICES: &str = "/supersonic/input-devices";
    pub const SUPERSONIC_INFO: &str = "/supersonic/info";
}

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage { addr: addr.to_string(), args }
}

/// Outgoing message builders.
pub mod out {
    use super::*;

    /// `/save-and-run-buffer [token] [name] [code] [name]` — this is how the
    /// GUI runs a buffer (see `SonicPiAPI::Run`/`SaveAndRunBuffer`).
    pub fn run_buffer(token: i32, name: &str, code: &str) -> OscMessage {
        msg(
            addr::SAVE_AND_RUN_BUFFER,
            vec![
                OscType::Int(token),
                OscType::String(name.to_string()),
                OscType::String(code.to_string()),
                OscType::String(name.to_string()),
            ],
        )
    }

    /// `/stop-all-jobs [token]`.
    pub fn stop_all_jobs(token: i32) -> OscMessage {
        msg(addr::STOP_ALL_JOBS, vec![OscType::Int(token)])
    }

    /// `/ping [token] [tag]` — used to detect the server coming up.
    pub fn ping(token: i32, tag: &str) -> OscMessage {
        msg(addr::PING, vec![OscType::Int(token), OscType::String(tag.to_string())])
    }

    /// `/daemon/keep-alive [token]` — sent to the daemon every ~4s; if it stops
    /// arriving the daemon tears down Spider + SuperSonic (the kill switch).
    pub fn keep_alive(token: i32) -> OscMessage {
        msg(addr::DAEMON_KEEP_ALIVE, vec![OscType::Int(token)])
    }

    /// `/daemon/exit [token]` — clean shutdown request to the daemon.
    pub fn daemon_exit(token: i32) -> OscMessage {
        msg(addr::DAEMON_EXIT, vec![OscType::Int(token)])
    }

    /// `/set-global-timewarp [token] [time]`.
    pub fn set_global_timewarp(token: i32, time: f64) -> OscMessage {
        msg(addr::SET_GLOBAL_TIMEWARP, vec![OscType::Int(token), OscType::Double(time)])
    }

    /// `/clock/tempo/set [bpm]` — straight to SuperSonic (mirrors
    /// `link_api.rb`'s `@link_comms.send("/clock/tempo/set", bpm.to_f)`).
    pub fn clock_tempo_set(bpm: f32) -> OscMessage {
        msg(addr::CLOCK_TEMPO_SET, vec![OscType::Float(bpm)])
    }

    /// `/supersonic/record/start [path] [format] [depth]` — SuperSonic's
    /// JUCE-side recorder taps the main output mix (mirrors
    /// `Studio#recording_start`; Spider sends `path, "wav", 24`).
    pub fn record_start(path: &str, format: &str, depth: i32) -> OscMessage {
        msg(
            addr::SUPERSONIC_RECORD_START,
            vec![
                OscType::String(path.to_string()),
                OscType::String(format.to_string()),
                OscType::Int(depth),
            ],
        )
    }

    /// `/supersonic/record/stop`.
    pub fn record_stop() -> OscMessage {
        msg(addr::SUPERSONIC_RECORD_STOP, vec![])
    }

    /// `/supersonic/devices/report [gui_listen_port]` — registers the port
    /// as a device-push notify target and triggers an immediate report
    /// (mirrors `SonicPiAPI::RequestAudioDevices`).
    pub fn supersonic_devices_report(gui_listen_port: u16) -> OscMessage {
        msg(addr::SUPERSONIC_DEVICES_REPORT, vec![OscType::Int(gui_listen_port as i32)])
    }

    /// `/supersonic/devices/switch [output] [sampleRate] [bufferSize]
    /// [input]` — the engine hot-swaps devices in place. This is what the
    /// daemon forwards `/daemon/audio/switch-device` to (token stripped).
    pub fn supersonic_devices_switch(
        output: &str,
        sample_rate: f32,
        buffer_size: i32,
        input: &str,
    ) -> OscMessage {
        msg(
            addr::SUPERSONIC_DEVICES_SWITCH,
            vec![
                OscType::String(output.to_string()),
                OscType::Float(sample_rate),
                OscType::Int(buffer_size),
                OscType::String(input.to_string()),
            ],
        )
    }

    /// `/mixer-amp [token] [amp] [silent]` — the master volume, sent to
    /// Spider (mirrors `MainWindow::changeSystemPreAmp`; the Qt slider maps
    /// 0..100 → 0.0..2.0).
    pub fn mixer_amp(token: i32, amp: f32, silent: bool) -> OscMessage {
        msg(
            addr::MIXER_AMP,
            vec![OscType::Int(token), OscType::Float(amp), OscType::Int(silent as i32)],
        )
    }

    /// `/daemon/audio/switch-device [token] [output] [sampleRate] [bufferSize]
    /// [input]` — sent to the daemon (mirrors `MainWindow::sendDeviceSwitch`).
    /// Empty strings / zeros mean "leave unchanged"; input `"__none__"`
    /// disables audio inputs.
    pub fn audio_switch_device(
        token: i32,
        output: &str,
        sample_rate: f32,
        buffer_size: i32,
        input: &str,
    ) -> OscMessage {
        msg(
            addr::DAEMON_AUDIO_SWITCH_DEVICE,
            vec![
                OscType::Int(token),
                OscType::String(output.to_string()),
                OscType::Float(sample_rate),
                OscType::Int(buffer_size),
                OscType::String(input.to_string()),
            ],
        )
    }
}

// ── Incoming parsing ─────────────────────────────────────────────────────────

fn arg_i32(m: &OscMessage, i: usize) -> Option<i32> {
    match m.args.get(i)? {
        OscType::Int(v) => Some(*v),
        _ => None,
    }
}
fn arg_str(m: &OscMessage, i: usize) -> Option<String> {
    match m.args.get(i)? {
        OscType::String(v) => Some(v.clone()),
        _ => None,
    }
}
fn arg_f64(m: &OscMessage, i: usize) -> Option<f64> {
    match m.args.get(i)? {
        OscType::Double(v) => Some(*v),
        OscType::Float(v) => Some(*v as f64),
        OscType::Int(v) => Some(*v as f64),
        _ => None,
    }
}

/// Parse an incoming OSC message into a `ClientEvent`, or `None` if it's not a
/// message we route. Mirrors the dispatch in `osc_handler.cpp`.
pub fn parse_incoming(m: &OscMessage) -> Option<ClientEvent> {
    use ClientEvent as E;
    match m.addr.as_str() {
        addr::ACK => Some(E::Status(StatusInfo {
            kind: StatusType::Ack,
            id: arg_str(m, 0).unwrap_or_default(),
        })),
        addr::RUNS_ALL_COMPLETED => Some(E::Status(StatusInfo {
            kind: StatusType::AllComplete,
            id: arg_str(m, 0).unwrap_or_default(),
        })),
        addr::EXITED => Some(E::Status(StatusInfo {
            kind: StatusType::Exited,
            id: arg_str(m, 0).unwrap_or_default(),
        })),
        addr::EXITED_WITH_BOOT_ERROR => {
            Some(E::BootError(arg_str(m, 0).unwrap_or_default()))
        }
        addr::SPIDER_READY => Some(E::SpiderReady),
        addr::LINK_BPM => arg_f64(m, 0).map(E::Bpm),
        addr::LINK_NUM_PEERS => arg_i32(m, 0).map(E::ActiveLinks),
        addr::LOG_INFO => Some(E::Report(MessageInfo {
            style: arg_i32(m, 0).unwrap_or(0),
            ..MessageInfo::simple(MessageType::Info, arg_str(m, 1).unwrap_or_default())
        })),
        addr::ERROR => Some(E::Report(parse_error(m, MessageType::RuntimeError))),
        addr::SYNTAX_ERROR => Some(E::Report(parse_error(m, MessageType::SyntaxError))),
        addr::LOG_MULTI_MESSAGE => Some(E::Report(parse_multi(m))),
        // `/incoming/osc [time(str), id(i32), address(str), args(str)]` —
        // order mirrored from osc_handler.cpp.
        addr::INCOMING_OSC => Some(E::Cue(CueInfo {
            time: arg_str(m, 0).unwrap_or_default(),
            id: arg_i32(m, 1).unwrap_or(0),
            address: arg_str(m, 2).unwrap_or_default(),
            args: arg_str(m, 3).unwrap_or_default(),
            index: 0,
        })),
        addr::VERSION => Some(E::Version(VersionInfo {
            version: arg_str(m, 0).unwrap_or_default(),
            num: arg_i32(m, 1).unwrap_or(0),
            latest_version: arg_str(m, 2).unwrap_or_default(),
            latest_num: arg_i32(m, 3).unwrap_or(0),
            platform: arg_str(m, 7).unwrap_or_default(),
        })),
        addr::MIDI_IN_PORTS => Some(E::Midi(MidiInfo {
            kind: MidiType::In,
            port_info: arg_str(m, 0).unwrap_or_default(),
        })),
        addr::MIDI_OUT_PORTS => Some(E::Midi(MidiInfo {
            kind: MidiType::Out,
            port_info: arg_str(m, 0).unwrap_or_default(),
        })),
        addr::GAMEPAD_DEVICES_LIST => {
            Some(E::GamepadDevices(arg_str(m, 0).unwrap_or_default()))
        }
        addr::SUPERSONIC_DEVICES => Some(E::AudioDevices(parse_audio_devices(m))),
        addr::SUPERSONIC_INPUT_DEVICES => {
            Some(E::AudioInputDevices(parse_audio_input_devices(m)))
        }
        addr::SUPERSONIC_INFO => Some(E::Scsynth(ScsynthInfo {
            text: arg_str(m, 0).unwrap_or_default(),
            sample_rate: arg_i32(m, 1),
            buffer_size: arg_i32(m, 2),
        })),
        // Recognised families we haven't fully modelled yet (setup,
        // statechange, info text, buffer/* …).
        a if a.starts_with("/supersonic/")
            || a.starts_with("/buffer/")
            || a == addr::UPDATE_INFO_TEXT =>
        {
            Some(E::Unhandled { addr: a.to_string() })
        }
        _ => None,
    }
}

/// `/supersonic/devices`: mode(s), current(s), name(s)…, sampleRate(i),
/// rate-compat(i)…, driver-type(s)… — device names run until the first int
/// arg (mirrors the C++ `ArgReader` loop in `osc_handler.cpp`).
fn parse_audio_devices(m: &OscMessage) -> AudioDevicesInfo {
    let mut info = AudioDevicesInfo {
        mode: arg_str(m, 0).unwrap_or_default(),
        current_device: arg_str(m, 1).unwrap_or_default(),
        ..Default::default()
    };
    let mut i = 2;
    while let Some(name) = arg_str(m, i) {
        info.devices.push(name);
        i += 1;
    }
    info.sample_rate = arg_i32(m, i).unwrap_or(0);
    info
}

/// `/supersonic/input-devices`: current(s), numDevices(i), name(s)…, type(s)…
fn parse_audio_input_devices(m: &OscMessage) -> AudioInputDevicesInfo {
    let mut info = AudioInputDevicesInfo {
        current_device: arg_str(m, 0).unwrap_or_default(),
        ..Default::default()
    };
    let n = arg_i32(m, 1).unwrap_or(0).max(0) as usize;
    for i in 0..n {
        if let Some(name) = arg_str(m, 2 + i) {
            info.devices.push(name);
        }
    }
    info
}

fn parse_error(m: &OscMessage, kind: MessageType) -> MessageInfo {
    // /error and /syntax_error: [job_id, message, backtrace, line]
    MessageInfo {
        job_id: arg_i32(m, 0).unwrap_or(0),
        backtrace: arg_str(m, 2).unwrap_or_default(),
        line: arg_i32(m, 3).unwrap_or(0),
        ..MessageInfo::simple(kind, arg_str(m, 1).unwrap_or_default())
    }
}

/// /log/multi_message: [job_id, thread_name, runtime, count, (style, text)*].
fn parse_multi(m: &OscMessage) -> MessageInfo {
    let mut info = MessageInfo::simple(MessageType::Multi, String::new());
    info.job_id = arg_i32(m, 0).unwrap_or(0);
    info.thread_name = arg_str(m, 1).unwrap_or_default();
    info.runtime = arg_str(m, 2).unwrap_or_default();
    let count = arg_i32(m, 3).unwrap_or(0).max(0) as usize;
    for k in 0..count {
        let base = 4 + k * 2;
        info.multi.push(MessageData {
            style: arg_i32(m, base).unwrap_or(0),
            text: arg_str(m, base + 1).unwrap_or_default(),
        });
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{OscPacket, OscType};

    #[test]
    fn run_buffer_roundtrips_through_the_wire() {
        let m = out::run_buffer(42, "buffer0", "play 60");
        let bytes = rosc::encoder::encode(&OscPacket::Message(m.clone())).unwrap();
        let (_, packet) = rosc::decoder::decode_udp(&bytes).unwrap();
        let OscPacket::Message(back) = packet else { panic!("not a message") };
        assert_eq!(back.addr, addr::SAVE_AND_RUN_BUFFER);
        assert_eq!(back.args.len(), 4);
        assert!(matches!(back.args[0], OscType::Int(42)));
        assert!(matches!(&back.args[1], OscType::String(s) if s == "buffer0"));
        assert!(matches!(&back.args[2], OscType::String(s) if s == "play 60"));
    }

    #[test]
    fn parses_ack_status() {
        let m = OscMessage {
            addr: addr::ACK.into(),
            args: vec![OscType::String("run-7".into())],
        };
        match parse_incoming(&m) {
            Some(ClientEvent::Status(s)) => {
                assert_eq!(s.kind, StatusType::Ack);
                assert_eq!(s.id, "run-7");
            }
            other => panic!("expected Ack status, got {other:?}"),
        }
    }

    #[test]
    fn parses_multi_message_lines() {
        let m = OscMessage {
            addr: addr::LOG_MULTI_MESSAGE.into(),
            args: vec![
                OscType::Int(3),                       // job_id
                OscType::String(":live_loop".into()),  // thread
                OscType::String("0.5".into()),         // runtime
                OscType::Int(2),                        // count
                OscType::Int(0),
                OscType::String("synth :beep".into()),
                OscType::Int(1),
                OscType::String("sample :bd_haus".into()),
            ],
        };
        match parse_incoming(&m) {
            Some(ClientEvent::Report(info)) => {
                assert_eq!(info.kind, MessageType::Multi);
                assert_eq!(info.job_id, 3);
                assert_eq!(info.multi.len(), 2);
                assert_eq!(info.multi[1].text, "sample :bd_haus");
                assert_eq!(info.multi[1].style, 1);
            }
            other => panic!("expected multi Report, got {other:?}"),
        }
    }

    #[test]
    fn all_outgoing_builders_carry_the_documented_shapes() {
        let m = out::ping(9, "tag");
        assert_eq!(m.addr, addr::PING);
        assert!(matches!(&m.args[1], OscType::String(s) if s == "tag"));

        assert_eq!(out::keep_alive(9).addr, addr::DAEMON_KEEP_ALIVE);
        assert_eq!(out::daemon_exit(9).addr, addr::DAEMON_EXIT);

        let m = out::set_global_timewarp(9, 0.25);
        assert_eq!(m.addr, addr::SET_GLOBAL_TIMEWARP);
        assert!(matches!(m.args[1], OscType::Double(d) if d == 0.25));

        let m = out::clock_tempo_set(128.0);
        assert_eq!(m.addr, addr::CLOCK_TEMPO_SET);
        assert!(matches!(m.args[0], OscType::Float(f) if f == 128.0));

        let m = out::record_start("/tmp/x.wav", "wav", 24);
        assert_eq!(m.addr, addr::SUPERSONIC_RECORD_START);
        assert!(matches!(m.args[2], OscType::Int(24)));
        assert!(out::record_stop().args.is_empty());

        let m = out::supersonic_devices_report(1234);
        assert_eq!(m.addr, addr::SUPERSONIC_DEVICES_REPORT);
        assert!(matches!(m.args[0], OscType::Int(1234)));

        let m = out::supersonic_devices_switch("Out", 44100.0, 512, "In");
        assert_eq!(m.addr, addr::SUPERSONIC_DEVICES_SWITCH);
        assert!(matches!(&m.args[3], OscType::String(s) if s == "In"));
    }

    #[test]
    fn parses_the_remaining_incoming_vocabulary() {
        let msg = |addr: &str, args: Vec<OscType>| OscMessage { addr: addr.into(), args };
        let s = |v: &str| OscType::String(v.into());

        match parse_incoming(&msg(addr::LOG_INFO, vec![OscType::Int(1), s("=> hi")])) {
            Some(ClientEvent::Report(m)) => {
                assert_eq!(m.text, "=> hi");
                assert_eq!(m.style, 1);
            }
            other => panic!("log/info: {other:?}"),
        }
        for (a, kind) in
            [(addr::ERROR, MessageType::RuntimeError), (addr::SYNTAX_ERROR, MessageType::SyntaxError)]
        {
            match parse_incoming(&msg(a, vec![OscType::Int(3), s("boom"), s("bt"), OscType::Int(7)])) {
                Some(ClientEvent::Report(m)) => {
                    assert_eq!(m.kind, kind);
                    assert_eq!(m.job_id, 3);
                    assert_eq!(m.line, 7);
                    assert_eq!(m.backtrace, "bt");
                }
                other => panic!("{a}: {other:?}"),
            }
        }
        assert!(matches!(
            parse_incoming(&msg(addr::RUNS_ALL_COMPLETED, vec![s("x")])),
            Some(ClientEvent::Status(st)) if st.kind == StatusType::AllComplete
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::EXITED, vec![s("x")])),
            Some(ClientEvent::Status(st)) if st.kind == StatusType::Exited
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::EXITED_WITH_BOOT_ERROR, vec![s("no engine")])),
            Some(ClientEvent::BootError(e)) if e == "no engine"
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::SPIDER_READY, vec![])),
            Some(ClientEvent::SpiderReady)
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::LINK_BPM, vec![OscType::Double(99.5)])),
            Some(ClientEvent::Bpm(b)) if b == 99.5
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::LINK_NUM_PEERS, vec![OscType::Int(2)])),
            Some(ClientEvent::ActiveLinks(2))
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::MIDI_IN_PORTS, vec![s("in1")])),
            Some(ClientEvent::Midi(m)) if m.kind == MidiType::In && m.port_info == "in1"
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::MIDI_OUT_PORTS, vec![s("out1")])),
            Some(ClientEvent::Midi(m)) if m.kind == MidiType::Out
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::GAMEPAD_DEVICES_LIST, vec![s("pads")])),
            Some(ClientEvent::GamepadDevices(g)) if g == "pads"
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::ACK, vec![s("id")])),
            Some(ClientEvent::Status(st)) if st.kind == StatusType::Ack
        ));
        // Recognised-but-unmodelled families stay visible…
        assert!(matches!(
            parse_incoming(&msg("/buffer/replace-lines", vec![])),
            Some(ClientEvent::Unhandled { .. })
        ));
        assert!(matches!(
            parse_incoming(&msg(addr::UPDATE_INFO_TEXT, vec![])),
            Some(ClientEvent::Unhandled { .. })
        ));
        // …and unknown addresses are dropped.
        assert!(parse_incoming(&msg("/no/such/address", vec![])).is_none());
    }

    #[test]
    fn parses_incoming_osc_cue_in_wire_order() {
        // time, id, address, args — as osc_handler.cpp pops them.
        let m = OscMessage {
            addr: addr::INCOMING_OSC.into(),
            args: vec![
                OscType::String("1783000703/1000".into()),
                OscType::Int(4),
                OscType::String("/link/tempo-change".into()),
                OscType::String("[120.0]".into()),
            ],
        };
        match parse_incoming(&m) {
            Some(ClientEvent::Cue(c)) => {
                assert_eq!(c.time, "1783000703/1000");
                assert_eq!(c.id, 4);
                assert_eq!(c.address, "/link/tempo-change");
                assert_eq!(c.args, "[120.0]");
            }
            other => panic!("expected Cue, got {other:?}"),
        }
    }

    #[test]
    fn audio_switch_device_matches_the_qt_wire_format() {
        // Mirrors MainWindow::sendDeviceSwitch: token, output, sr(f32),
        // buf(i32), input.
        let m = out::audio_switch_device(42, "Built-in", 48000.0, 1024, "__none__");
        assert_eq!(m.addr, addr::DAEMON_AUDIO_SWITCH_DEVICE);
        assert!(matches!(m.args[0], OscType::Int(42)));
        assert!(matches!(&m.args[1], OscType::String(s) if s == "Built-in"));
        assert!(matches!(m.args[2], OscType::Float(f) if f == 48000.0));
        assert!(matches!(m.args[3], OscType::Int(1024)));
        assert!(matches!(&m.args[4], OscType::String(s) if s == "__none__"));
    }

    #[test]
    fn parses_supersonic_devices_names_until_first_int() {
        let m = OscMessage {
            addr: addr::SUPERSONIC_DEVICES.into(),
            args: vec![
                OscType::String("auto".into()),
                OscType::String("Built-in Audio".into()),
                OscType::String("Built-in Audio".into()),
                OscType::String("HDMI Out".into()),
                OscType::Int(48000),
                OscType::Int(1),
                OscType::Int(1),
                OscType::String("alsa".into()),
                OscType::String("alsa".into()),
            ],
        };
        match parse_incoming(&m) {
            Some(ClientEvent::AudioDevices(d)) => {
                assert_eq!(d.mode, "auto");
                assert_eq!(d.current_device, "Built-in Audio");
                assert_eq!(d.devices, vec!["Built-in Audio", "HDMI Out"]);
                assert_eq!(d.sample_rate, 48000);
            }
            other => panic!("expected AudioDevices, got {other:?}"),
        }
    }

    #[test]
    fn parses_supersonic_input_devices() {
        let m = OscMessage {
            addr: addr::SUPERSONIC_INPUT_DEVICES.into(),
            args: vec![
                OscType::String("Mic".into()),
                OscType::Int(2),
                OscType::String("Mic".into()),
                OscType::String("Line In".into()),
                OscType::String("alsa".into()),
                OscType::String("alsa".into()),
            ],
        };
        match parse_incoming(&m) {
            Some(ClientEvent::AudioInputDevices(d)) => {
                assert_eq!(d.current_device, "Mic");
                assert_eq!(d.devices, vec!["Mic", "Line In"]);
            }
            other => panic!("expected AudioInputDevices, got {other:?}"),
        }
    }

    #[test]
    fn parses_supersonic_info_simple_and_extended() {
        let simple = OscMessage {
            addr: addr::SUPERSONIC_INFO.into(),
            args: vec![OscType::String("SuperSonic booted".into())],
        };
        match parse_incoming(&simple) {
            Some(ClientEvent::Scsynth(i)) => {
                assert_eq!(i.text, "SuperSonic booted");
                assert_eq!(i.sample_rate, None);
            }
            other => panic!("expected Scsynth, got {other:?}"),
        }
        let extended = OscMessage {
            addr: addr::SUPERSONIC_INFO.into(),
            args: vec![
                OscType::String("running".into()),
                OscType::Int(48000),
                OscType::Int(1024),
            ],
        };
        match parse_incoming(&extended) {
            Some(ClientEvent::Scsynth(i)) => {
                assert_eq!(i.sample_rate, Some(48000));
                assert_eq!(i.buffer_size, Some(1024));
            }
            other => panic!("expected Scsynth, got {other:?}"),
        }
    }
}
