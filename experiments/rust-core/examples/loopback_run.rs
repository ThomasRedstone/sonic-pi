//! Headless proof of the Phase-3a frontend↔core loop, using the real core
//! `osc` + `protocol` modules — the same wiring the GPUI spike drives from its
//! "Run ▶" button.
//!
//!   frontend --/save-and-run-buffer--> [spider loopback] --/log/*--> frontend
//!
//! Run with: cargo run --example loopback_run

use std::sync::mpsc;
use std::time::Duration;

use sonicpi_core::osc::{OscServer, UdpOscSender};
use sonicpi_core::protocol::{self, addr};
use sonicpi_core::rosc::{OscMessage, OscType};
use sonicpi_core::ClientEvent;

fn osc(a: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage { addr: a.to_string(), args }
}

fn main() {
    // Frontend's incoming server: decode engine OSC → ClientEvent.
    let (tx, rx) = mpsc::channel();
    let gui = OscServer::start(0, move |m| {
        if let Some(ev) = protocol::parse_incoming(&m) {
            let _ = tx.send(ev);
        }
    })
    .unwrap();
    let gui_port = gui.port();

    // Loopback "spider": on a run, echo back a few engine log messages.
    let reply = UdpOscSender::to_localhost(gui_port).unwrap();
    let spider = OscServer::start(0, move |m| {
        if m.addr == addr::SAVE_AND_RUN_BUFFER {
            let code = match m.args.get(2) {
                Some(OscType::String(s)) => s.clone(),
                _ => String::new(),
            };
            let first = code.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            let _ = reply.send(&osc(
                addr::LOG_INFO,
                vec![OscType::Int(0), OscType::String("=> Run started".into())],
            ));
            let _ = reply.send(&osc(
                addr::LOG_MULTI_MESSAGE,
                vec![
                    OscType::Int(1),
                    OscType::String(":main".into()),
                    OscType::String("0.0".into()),
                    OscType::Int(1),
                    OscType::Int(0),
                    OscType::String(format!("run → {first}")),
                ],
            ));
            let _ = reply.send(&osc(
                addr::RUNS_ALL_COMPLETED,
                vec![OscType::String("run".into())],
            ));
        }
    })
    .unwrap();

    // Frontend sends the editor buffer.
    let run_tx = UdpOscSender::to_localhost(spider.port()).unwrap();
    let code = "play :c4\nsleep 1";
    println!("→ frontend sends /save-and-run-buffer for {code:?}");
    run_tx
        .send(&protocol::out::run_buffer(1, "buffer0", code))
        .unwrap();

    println!("← frontend decodes engine events:");
    let mut n = 0;
    while let Ok(ev) = rx.recv_timeout(Duration::from_secs(2)) {
        match ev {
            ClientEvent::Report(m) if !m.multi.is_empty() => println!(
                "   Report(multi): {}",
                m.multi.iter().map(|d| d.text.clone()).collect::<Vec<_>>().join(" ")
            ),
            ClientEvent::Report(m) => println!("   Report: {}", m.text),
            ClientEvent::Status(s) => println!("   Status: {:?} {}", s.kind, s.id),
            other => println!("   {other:?}"),
        }
        n += 1;
        if n >= 3 {
            break;
        }
    }
    println!("done — {n} events round-tripped through the core.");
}
