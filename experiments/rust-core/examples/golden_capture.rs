//! Golden OSC capture — boots the real daemon, drives a short (silent) run,
//! and records every incoming OSC message hex-encoded, one per line. The
//! output becomes `tests/fixtures/golden_osc.hex`, the regression corpus for
//! `protocol::parse_incoming` (the Phase-2 validation strategy).
//!
//! Usage (needs the full runtime in ../../app):
//!   cargo run --example golden_capture [seconds] [out_file]

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sonicpi_core::osc::{OscServer, UdpOscSender};
use sonicpi_core::paths::{resolve, SonicPiPath};
use sonicpi_core::process::Daemon;
use sonicpi_core::rosc;
use sonicpi_core::{protocol, KeepAlive, PortId};

// Full traffic, no audible interference with a live session.
const QUIET_RUN: &str = "live_loop :golden do\n  sample :bd_haus, amp: 0\n  sleep 0.25\nend";

fn main() {
    let secs: u64 =
        std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let out_path = std::env::args().nth(2).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden_osc.hex")
    });

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
    let paths = resolve(&root);
    let mut daemon = Daemon::boot(&paths[&SonicPiPath::Ruby], &paths[&SonicPiPath::BootDaemon])
        .expect("daemon boot (is the runtime built?)");
    let token = daemon.ports.token;
    println!("daemon up, token {token}");

    let captured: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let _server = OscServer::start(daemon.ports.get(PortId::GuiListenToSpider), move |m| {
        if let Ok(bytes) = rosc::encoder::encode(&rosc::OscPacket::Message(m.clone())) {
            sink.lock().unwrap().push(bytes);
        }
    })
    .expect("gui osc server");

    let daemon_tx = UdpOscSender::to_localhost(daemon.ports.get(PortId::Daemon)).unwrap();
    let spider_tx = UdpOscSender::to_localhost(daemon.ports.get(PortId::GuiSendToSpider)).unwrap();
    let _keep_alive = KeepAlive::start(
        UdpOscSender::to_localhost(daemon.ports.get(PortId::Daemon)).unwrap(),
        token,
    );

    // Wait for Spider to come up, then run → let it loop → stop.
    println!("waiting {}s while the run generates traffic…", secs);
    std::thread::sleep(Duration::from_secs(8)); // spider boot
    let _ = spider_tx.send(&protocol::out::run_buffer(token, "golden", QUIET_RUN));
    std::thread::sleep(Duration::from_secs(secs));
    let _ = spider_tx.send(&protocol::out::stop_all_jobs(token));
    std::thread::sleep(Duration::from_secs(2));

    // Polite shutdown.
    let _ = daemon_tx.send(&protocol::out::daemon_exit(token));
    if !daemon.wait_timeout(Duration::from_secs(3)) {
        daemon.kill();
    }

    let msgs = captured.lock().unwrap();
    std::fs::create_dir_all(out_path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(&out_path).unwrap();
    for bytes in msgs.iter() {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        writeln!(f, "{hex}").unwrap();
    }
    println!("captured {} messages → {}", msgs.len(), out_path.display());
}
