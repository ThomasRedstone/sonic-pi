//! Golden-traffic regression test: every OSC message captured from a real
//! session (`cargo run --example golden_capture`) must still decode and be
//! recognised by `protocol::parse_incoming`. Guards the parser against
//! regressions using real wire bytes, not hand-built messages.

use std::path::PathBuf;

use sonicpi_core::rosc::{decoder, OscPacket};
use sonicpi_core::{protocol, ClientEvent};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden_osc.hex")
}

fn unhex(line: &str) -> Vec<u8> {
    (0..line.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn golden_capture_still_parses() {
    let path = fixture_path();
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing fixture {} — run the golden_capture example", path.display()));

    let mut total = 0usize;
    let mut parsed = 0usize;
    let mut unhandled: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut seen_addrs: Vec<String> = Vec::new();

    for line in data.lines().filter(|l| !l.trim().is_empty()) {
        let bytes = unhex(line.trim());
        let (_, packet) = decoder::decode_udp(&bytes).expect("captured bytes must decode");
        let OscPacket::Message(m) = packet else {
            continue; // bundles aren't part of the GUI contract today
        };
        total += 1;
        if !seen_addrs.contains(&m.addr) {
            seen_addrs.push(m.addr.clone());
        }
        match protocol::parse_incoming(&m) {
            Some(ClientEvent::Unhandled { addr }) => unhandled.push(addr),
            Some(_) => parsed += 1,
            None => unknown.push(m.addr.clone()),
        }
    }

    assert!(total > 0, "fixture is empty");

    // Every address the engine sent must at least be *recognised* — brand-new
    // unknown addresses mean the vocabulary drifted and the parser is silent
    // about it.
    unknown.sort();
    unknown.dedup();
    assert!(
        unknown.is_empty(),
        "addresses not recognised at all by parse_incoming: {unknown:?}\n(all seen: {seen_addrs:?})"
    );

    // The overwhelming majority of live traffic must be fully modelled (the
    // Unhandled bucket is allowed to exist but must stay small).
    let fully = parsed as f64 / total as f64;
    assert!(
        fully >= 0.8,
        "only {:.0}% of {} captured messages fully parse (unhandled: {:?})",
        fully * 100.0,
        total,
        {
            let mut u = unhandled.clone();
            u.sort();
            u.dedup();
            u
        }
    );

    // Core vocabulary sanity: a real session always logs and completes runs.
    for must in ["/log/multi_message", "/runs/all-completed"] {
        assert!(
            seen_addrs.iter().any(|a| a == must),
            "expected {must} in the captured session (saw: {seen_addrs:?})"
        );
    }
}
