//! Session integration test — the full wiring against in-process stub
//! servers: three OscServers stand in for Spider / daemon / SuperSonic and
//! capture what the Session sends; a message pushed at the gui-listen port
//! must come back decoded through the ApiClient.

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sonicpi_core::osc::{OscServer, UdpOscSender};
use sonicpi_core::rosc::{OscMessage, OscType};
use sonicpi_core::{protocol, ApiClient, ClientEvent, PortId, Ports, Session};

type Captured = Arc<Mutex<Vec<OscMessage>>>;

fn capturing_server() -> (OscServer, Captured) {
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let server = OscServer::start(0, move |m| sink.lock().unwrap().push(m)).unwrap();
    (server, captured)
}

fn wait_for<F: Fn() -> bool>(cond: F, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn addrs(captured: &Captured) -> Vec<String> {
    captured.lock().unwrap().iter().map(|m| m.addr.clone()).collect()
}

struct Collect(Arc<Mutex<Vec<ClientEvent>>>);
impl ApiClient for Collect {
    fn on_event(&self, e: ClientEvent) {
        self.0.lock().unwrap().push(e);
    }
}

#[test]
fn session_wires_all_senders_and_the_incoming_server() {
    let (spider, spider_rx) = capturing_server();
    let (daemon, daemon_rx) = capturing_server();
    let (supersonic, supersonic_rx) = capturing_server();
    let gui_listen =
        UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();

    let ports = Ports::from_parts(
        daemon.port(),
        gui_listen,
        spider.port(),
        supersonic.port(),
        4560,
        777,
    );
    assert_eq!(ports.get(PortId::TauOscCues), 4560);

    let events: Arc<Mutex<Vec<ClientEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let session = Session::connect(&ports, Arc::new(Collect(events.clone()))).unwrap();
    assert_eq!(session.token(), 777);

    // Connect registers for device pushes with the engine.
    wait_for(
        || addrs(&supersonic_rx).iter().any(|a| a == protocol::addr::SUPERSONIC_DEVICES_REPORT),
        "devices/report registration",
    );

    // Spider-bound sends.
    session.run("buffer0", "play 60").unwrap();
    session.stop().unwrap();
    session.ping().unwrap();
    session.set_mixer_amp(1.5, false).unwrap();
    wait_for(
        || {
            let a = addrs(&spider_rx);
            a.iter().any(|x| x == protocol::addr::SAVE_AND_RUN_BUFFER)
                && a.iter().any(|x| x == protocol::addr::STOP_ALL_JOBS)
                && a.iter().any(|x| x == protocol::addr::PING)
                && a.iter().any(|x| x == protocol::addr::MIXER_AMP)
        },
        "spider messages",
    );
    // Run carries the token + code.
    {
        let msgs = spider_rx.lock().unwrap();
        let run = msgs.iter().find(|m| m.addr == protocol::addr::SAVE_AND_RUN_BUFFER).unwrap();
        assert!(matches!(run.args[0], OscType::Int(777)));
        assert!(matches!(&run.args[2], OscType::String(s) if s == "play 60"));
    }

    // Engine-bound sends: raw + the direct device switch.
    session
        .send_to_supersonic(&protocol::out::clock_tempo_set(120.0))
        .unwrap();
    session.switch_audio_device_direct("Out", 48000.0, 1024, "__none__").unwrap();
    wait_for(
        || {
            let a = addrs(&supersonic_rx);
            a.iter().any(|x| x == protocol::addr::CLOCK_TEMPO_SET)
                && a.iter().any(|x| x == protocol::addr::SUPERSONIC_DEVICES_SWITCH)
        },
        "supersonic messages",
    );

    // Daemon-bound sends: device switch (forwarded form), keep-alive
    // (background loop), and shutdown.
    session.switch_audio_device("Out", 0.0, 0, "").unwrap();
    session.shutdown().unwrap();
    wait_for(
        || {
            let a = addrs(&daemon_rx);
            a.iter().any(|x| x == protocol::addr::DAEMON_AUDIO_SWITCH_DEVICE)
                && a.iter().any(|x| x == protocol::addr::DAEMON_EXIT)
                && a.iter().any(|x| x == protocol::addr::DAEMON_KEEP_ALIVE)
        },
        "daemon messages (incl. keep-alive)",
    );

    // Incoming: a message at the gui-listen port arrives decoded.
    let to_gui = UdpOscSender::to_localhost(gui_listen).unwrap();
    to_gui
        .send(&OscMessage {
            addr: protocol::addr::LOG_INFO.to_string(),
            args: vec![OscType::Int(0), OscType::String("=> hello".into())],
        })
        .unwrap();
    wait_for(
        || {
            events.lock().unwrap().iter().any(
                |e| matches!(e, ClientEvent::Report(m) if m.text == "=> hello"),
            )
        },
        "decoded incoming event",
    );

    drop(session); // KeepAlive Drop path (stop + join)
}
