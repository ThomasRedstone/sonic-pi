//! Demonstrates the core's contract handling without a full Sonic Pi runtime:
//!   * parse a daemon handshake line into ports + token,
//!   * build an outgoing "run buffer" message and show it on the wire,
//!   * decode a couple of representative incoming messages into ClientEvents.
//!
//! Run with: cargo run --example contract_demo

use rosc::{OscMessage, OscPacket, OscType};
use sonicpi_core::{
    ports::Ports,
    protocol::{self, addr},
    ClientEvent, PortId,
};

fn main() {
    println!("== daemon handshake ==");
    let line = "37000 37001 37002 37003 37004 987654321";
    let ports = Ports::parse_daemon_line(line).expect("parse");
    println!("  line: {line:?}");
    println!("  daemon               = {}", ports.get(PortId::Daemon));
    println!("  gui_listen_to_spider = {}", ports.get(PortId::GuiListenToSpider));
    println!("  gui_send_to_spider   = {}", ports.get(PortId::GuiSendToSpider));
    println!("  scsynth              = {}", ports.get(PortId::Scsynth));
    println!("  token                = {}", ports.token);

    println!("\n== outgoing: run a buffer ==");
    let run = protocol::out::run_buffer(ports.token, "buffer0", "play :c4\nsleep 1");
    let bytes = rosc::encoder::encode(&OscPacket::Message(run.clone())).unwrap();
    println!("  addr = {}", run.addr);
    println!("  args = {} value(s)", run.args.len());
    println!("  encoded to {} bytes on the wire", bytes.len());

    println!("\n== incoming: decode representative messages ==");
    let samples = vec![
        OscMessage { addr: addr::SPIDER_READY.into(), args: vec![] },
        OscMessage { addr: addr::ACK.into(), args: vec![OscType::String("run-1".into())] },
        OscMessage { addr: addr::LINK_BPM.into(), args: vec![OscType::Double(128.0)] },
        OscMessage {
            addr: addr::LOG_MULTI_MESSAGE.into(),
            args: vec![
                OscType::Int(1),
                OscType::String(":main".into()),
                OscType::String("0.0".into()),
                OscType::Int(1),
                OscType::Int(0),
                OscType::String("synth :beep, note: 60".into()),
            ],
        },
        OscMessage { addr: "/supersonic/info".into(), args: vec![] },
    ];

    for m in &samples {
        match protocol::parse_incoming(m) {
            Some(ClientEvent::Report(info)) => {
                println!("  {} -> Report({:?}, {} line(s))", m.addr, info.kind, info.multi.len().max(1));
            }
            Some(other) => println!("  {} -> {other:?}", m.addr),
            None => println!("  {} -> (unrouted)", m.addr),
        }
    }
}
