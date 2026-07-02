//! E2E check of the recording path: boot the real runtime, run a silent
//! loop, record ~5s through SuperSonic's JUCE-side recorder, stop, and
//! verify a well-formed WAV landed on disk. Exits non-zero on failure so it
//! can gate CI.
//!
//!   cargo run --example record_check

use std::path::PathBuf;
use std::time::Duration;

use sonicpi_core::osc::{OscServer, UdpOscSender};
use sonicpi_core::paths::{resolve, SonicPiPath};
use sonicpi_core::process::Daemon;
use sonicpi_core::{protocol, KeepAlive, PortId};

const QUIET_RUN: &str = "live_loop :rec do\n  sample :bd_haus, amp: 0\n  sleep 0.25\nend";

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
    let paths = resolve(&root);
    let mut daemon = Daemon::boot(&paths[&SonicPiPath::Ruby], &paths[&SonicPiPath::BootDaemon])
        .expect("daemon boot");
    let token = daemon.ports.token;

    // Keep the incoming port open so Spider doesn't log send errors.
    let _server = OscServer::start(daemon.ports.get(PortId::GuiListenToSpider), |_| {}).unwrap();
    let daemon_tx = UdpOscSender::to_localhost(daemon.ports.get(PortId::Daemon)).unwrap();
    let spider_tx = UdpOscSender::to_localhost(daemon.ports.get(PortId::GuiSendToSpider)).unwrap();
    let engine_tx = UdpOscSender::to_localhost(daemon.ports.get(PortId::Scsynth)).unwrap();
    let _keep_alive = KeepAlive::start(
        UdpOscSender::to_localhost(daemon.ports.get(PortId::Daemon)).unwrap(),
        token,
    );

    println!("waiting for the runtime to boot…");
    std::thread::sleep(Duration::from_secs(10));
    let _ = spider_tx.send(&protocol::out::run_buffer(token, "rec", QUIET_RUN));
    std::thread::sleep(Duration::from_secs(2));

    let wav = std::env::temp_dir().join("sonic_oxide_record_check.wav");
    let _ = std::fs::remove_file(&wav);
    println!("recording 5s to {}…", wav.display());
    let _ = engine_tx.send(&protocol::out::record_start(wav.to_str().unwrap(), "wav", 24));
    std::thread::sleep(Duration::from_secs(5));
    let _ = engine_tx.send(&protocol::out::record_stop());
    std::thread::sleep(Duration::from_secs(2));
    let _ = spider_tx.send(&protocol::out::stop_all_jobs(token));

    // Polite shutdown before judging the result.
    let _ = daemon_tx.send(&protocol::out::daemon_exit(token));
    if !daemon.wait_timeout(Duration::from_secs(3)) {
        daemon.kill();
    }

    let bytes = std::fs::read(&wav).unwrap_or_default();
    let ok = bytes.len() > 44 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE";
    println!(
        "recording: {} bytes, RIFF/WAVE header {} → {}",
        bytes.len(),
        if ok { "valid" } else { "INVALID" },
        if ok { "PASS" } else { "FAIL" }
    );
    std::process::exit(if ok { 0 } else { 1 });
}
