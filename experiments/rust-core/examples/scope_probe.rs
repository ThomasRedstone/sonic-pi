//! Probe a live engine's scope shm segment: attach, report whether the master
//! audio ring is enabled/flowing, and print a few samples. Usage:
//!   cargo run --example scope_probe -- /SuperSonic_<port>

use std::time::Duration;

use sonicpi_core::audio::{ScopeReader, ScopeSlotReader};

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "/sonic_scope_spike".into());
    let reader = match ScopeReader::open(&name) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("open {name}: {e}");
            std::process::exit(1);
        }
    };
    println!("attached to {name}, slot0 active = {}", reader.is_active());

    let mut samples = Vec::new();
    for i in 0..5 {
        let live = reader.read_latest_mono(&mut samples, 32);
        let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        println!("audio ring poll {i}: live={live} n={} peak={peak:.5}", samples.len());
        std::thread::sleep(Duration::from_millis(100));
    }

    // Scan the triple-buffered scope slots (the path the real engine writes).
    for index in 0..16 {
        match ScopeSlotReader::open(&name, index) {
            Ok(mut r) => {
                if !r.is_active() {
                    continue;
                }
                let mut out = Vec::new();
                let mut pulls = 0;
                let mut peak = 0.0f32;
                for _ in 0..10 {
                    if r.pull_latest_mono(&mut out) {
                        pulls += 1;
                        peak = out.iter().fold(peak, |a, s| a.max(s.abs()));
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
                println!(
                    "scope slot {index}: ACTIVE, {pulls}/10 pulls had new data, n={} peak={peak:.5}",
                    out.len()
                );
            }
            Err(e) => {
                println!("scope slot {index}: {e}");
                break;
            }
        }
    }
}
