//! Dependency-free fuzz-lite harness for the OSC decode+parse path
//! (`plan/04-gig-hardening.md`: harden the parser). Not a substitute for a
//! real corpus-guided `cargo fuzz` run — this is the cheap, always-in-CI
//! floor: a fixed set of known-tricky byte patterns, many random trials
//! from a deterministic seed (reproducible on failure), and mutations of
//! real encoded messages (more likely to reach `parse_incoming`'s
//! per-address arg-index logic than pure noise). Every one of them just
//! asserts the decode+parse path never panics, however malformed the
//! input — no new dependency, runs on stable, works everywhere `cargo test`
//! does.
//!
//! Why this matters more now than it used to: a gig-mode runtime (plan
//! phase 6) is meant to stay up for hours unattended, which makes a
//! flaky/confused/malicious sender on the cue port or a buggy MIDI bridge
//! a real (if unlikely) attack surface — one a dev-loop process that
//! restarts constantly never had to worry about.

use sonicpi_core::protocol::parse_incoming;
use sonicpi_core::rosc::{self, OscMessage, OscPacket, OscType};

/// xorshift64 — tiny, deterministic, no dependency. Fine for byte-soup
/// fuzzing; not for anything cryptographic.
struct Xorshift64(u64);
impl Xorshift64 {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next() & 0xff) as u8).collect()
    }
}

/// Decode raw bytes as OSC (as `OscServer`'s receive loop does) and, for
/// every message a bundle might contain, run it through `parse_incoming` —
/// exactly the two-stage pipeline live traffic goes through. Anything that
/// fails to decode is silently ignored (matches `OscServer::dispatch`,
/// which does the same for garbage datagrams); the only thing under test is
/// "does this ever panic".
fn try_decode_and_parse(bytes: &[u8]) {
    if let Ok((_, packet)) = rosc::decoder::decode_udp(bytes) {
        fn walk(packet: OscPacket) {
            match packet {
                OscPacket::Message(m) => {
                    let _ = parse_incoming(&m);
                }
                OscPacket::Bundle(b) => {
                    for inner in b.content {
                        walk(inner);
                    }
                }
            }
        }
        walk(packet);
    }
}

#[test]
fn known_tricky_inputs_never_panic() {
    let cases: &[&[u8]] = &[
        b"",
        b"\0",
        b"not osc at all, just text",
        b"/",
        b"/\0\0\0,\0\0\0",                // address + empty typetag, no args
        b"/x\0\0,i\0\0\xff\xff\xff\xff",  // int arg with garbage payload
        b"#bundle\0",                     // truncated bundle header
        b"/error\0\0,s\0\0",              // typetag promises a string that isn't there
    ];
    for case in cases {
        try_decode_and_parse(case);
    }
}

#[test]
fn random_byte_soup_never_panics() {
    // Deterministic seed: a failure here is reproducible by just rerunning
    // this test — no corpus file to capture and check in.
    let mut rng = Xorshift64(0xC0FFEE_A55D00D_u64 | 1);
    for _ in 0..20_000 {
        let len = (rng.next() % 128) as usize;
        let bytes = rng.bytes(len);
        try_decode_and_parse(&bytes);
    }
}

#[test]
fn mutated_real_messages_never_panic() {
    // Seed from VALID encodings of addresses `parse_incoming` actually
    // models (including ones with address-specific arg-index assumptions),
    // then flip random bytes — mutating known-good input reaches deeper
    // into the parser than noise alone typically does.
    let seeds: Vec<Vec<u8>> = [
        OscMessage { addr: "/ack".into(), args: vec![OscType::String("id".into())] },
        OscMessage {
            addr: "/error".into(),
            args: vec![
                OscType::Int(1),
                OscType::String("boom".into()),
                OscType::String(String::new()),
            ],
        },
        OscMessage {
            addr: "/supersonic/drivers/list.reply".into(),
            args: vec![OscType::String("ALSA".into()), OscType::String("JACK".into())],
        },
        OscMessage {
            addr: "/supersonic/drivers/switch.reply".into(),
            args: vec![OscType::Int(1), OscType::String("JACK".into()), OscType::Float(48000.0), OscType::Int(256)],
        },
        OscMessage { addr: "/midi/in-ports".into(), args: vec![OscType::String("p1".into())] },
        OscMessage {
            addr: "/incoming/osc".into(),
            args: vec![
                OscType::String("1/2".into()),
                OscType::String("/beat".into()),
                OscType::String("[1, 2]".into()),
            ],
        },
    ]
    .into_iter()
    .map(|m| rosc::encoder::encode(&OscPacket::Message(m)).unwrap())
    .collect();

    let mut rng = Xorshift64(0xBADC0DE_u64 | 1);
    for seed in &seeds {
        for _ in 0..2_000 {
            let mut mutated = seed.clone();
            let flips = 1 + (rng.next() % 4) as usize;
            for _ in 0..flips {
                if mutated.is_empty() {
                    break;
                }
                let i = (rng.next() as usize) % mutated.len();
                mutated[i] = (rng.next() & 0xff) as u8;
            }
            try_decode_and_parse(&mutated);
        }
    }
}

#[test]
fn oversized_and_truncated_length_prefixed_strings_never_panic() {
    // OSC strings/blobs are length-implied by NUL padding, not an explicit
    // length prefix, but bundles DO carry explicit i32 element lengths —
    // the classic "claims to be huge, isn't" fuzzing target.
    let mut rng = Xorshift64(0x5EED_u64 | 1);
    for _ in 0..5_000 {
        let mut bytes = b"#bundle\0".to_vec();
        bytes.extend_from_slice(&[0u8; 8]); // fake NTP timetag
        // A bogus (possibly huge, possibly negative) element length,
        // followed by a short amount of real garbage.
        let claimed_len = rng.next() as i32;
        bytes.extend_from_slice(&claimed_len.to_be_bytes());
        let tail_len = (rng.next() % 32) as usize;
        bytes.extend(rng.bytes(tail_len));
        try_decode_and_parse(&bytes);
    }
}
