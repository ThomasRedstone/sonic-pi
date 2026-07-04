//! Latency probe — measures the numbers that define the "live" feel, against
//! the REAL runtime (foundation-phase harness, plan v2 item 1a):
//!
//!   * boot → Spider ready        (cold-start wait before first sound)
//!   * run  → first Spider reply  (the run round-trip a performer feels)
//!   * run  → scope slot active   (proxy for run → audible)
//!   * external cue → /incoming/osc event (network cue latency)
//!
//! Prints each number and exits non-zero if any misses its (generous)
//! ceiling — ratchet these down as the stack improves.
//!
//!   cargo run --example latency_probe

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sonicpi_core::audio::ScopeSlotReader;
use sonicpi_core::osc::UdpOscSender;
use sonicpi_core::rosc::{OscMessage, OscType};
use sonicpi_core::supervisor::{BootMode, Supervisor};
use sonicpi_core::{ApiClient, ClientEvent, PortId, Session};

const QUIET_RUN: &str = "live_loop :probe do\n  sample :bd_haus, amp: 0\n  sleep 0.25\nend";

/// Ceilings (ms). Generous on purpose — the probe's first job is a baseline;
/// tighten once the numbers are known and stable.
const MAX_BOOT_MS: u128 = 30_000;
const MAX_RUN_ACK_MS: u128 = 2_000;
const MAX_SCOPE_MS: u128 = 4_000;
const MAX_CUE_MS: u128 = 500;

/// Millisecond timestamps (relative to `t0`) of the first sighting of each
/// event kind; 0 = not seen yet.
#[derive(Default)]
struct Stamps {
    spider_ready: AtomicU64,
    first_reply: AtomicU64,
    cue_event: AtomicU64,
}

struct Probe {
    t0: Instant,
    stamps: Arc<Stamps>,
}

impl Probe {
    fn mark(slot: &AtomicU64, t0: Instant) {
        let _ = slot.compare_exchange(
            0,
            t0.elapsed().as_millis().max(1) as u64,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
}

impl ApiClient for Probe {
    fn on_event(&self, e: ClientEvent) {
        match e {
            ClientEvent::SpiderReady => Self::mark(&self.stamps.spider_ready, self.t0),
            ClientEvent::Report(_) => Self::mark(&self.stamps.first_reply, self.t0),
            ClientEvent::Cue(_) => Self::mark(&self.stamps.cue_event, self.t0),
            _ => {}
        }
    }
}

fn wait_for(slot: &AtomicU64, timeout: Duration) -> Option<u64> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let v = slot.load(Ordering::SeqCst);
        if v != 0 {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
    let mut failures = Vec::new();

    // ── Boot → Spider ready ─────────────────────────────────────────────
    let t0 = Instant::now();
    let mut sup = Supervisor::boot(&root, BootMode::Normal).expect("supervisor boot");
    let stamps = Arc::new(Stamps::default());
    let session = Session::connect(
        &sup.ports,
        Arc::new(Probe { t0, stamps: stamps.clone() }),
    )
    .expect("session");

    let boot_ms = wait_for(&stamps.spider_ready, Duration::from_millis(MAX_BOOT_MS as u64))
        .map(u128::from);
    match boot_ms {
        Some(ms) => {
            println!("boot → spider ready:   {ms} ms");
            if ms > MAX_BOOT_MS {
                failures.push(format!("boot {ms}ms > {MAX_BOOT_MS}ms"));
            }
        }
        None => failures.push("spider never signalled ready".into()),
    }

    // ── Run → first reply, and → scope activity ────────────────────────
    let scsynth = sup.ports.get(PortId::Scsynth);
    let shm = format!("/SuperSonic_{scsynth}");
    let run_at = Instant::now();
    let run_at_ms = t0.elapsed().as_millis() as u64;
    session.run("probe", QUIET_RUN).expect("run");

    match wait_for(&stamps.first_reply, Duration::from_millis(MAX_RUN_ACK_MS as u64 * 2)) {
        Some(ms) => {
            let delta = ms.saturating_sub(run_at_ms) as u128;
            println!("run  → first reply:    {delta} ms");
            if delta > MAX_RUN_ACK_MS {
                failures.push(format!("run ack {delta}ms > {MAX_RUN_ACK_MS}ms"));
            }
        }
        None => failures.push("no spider reply to the run".into()),
    }

    // Scope: attach (segment already exists) and poll for the slot to
    // publish — the closest wire-visible proxy for "sound is flowing".
    let scope_deadline = Instant::now() + Duration::from_millis(MAX_SCOPE_MS as u64 * 2);
    let mut scope_ms: Option<u128> = None;
    let mut reader: Option<ScopeSlotReader> = None;
    let mut buf = Vec::new();
    while Instant::now() < scope_deadline {
        if reader.is_none() {
            reader = ScopeSlotReader::open(&shm, 0).ok();
        }
        if let Some(r) = reader.as_mut() {
            if r.pull_latest_mono(&mut buf) && !buf.is_empty() {
                scope_ms = Some(run_at.elapsed().as_millis());
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    match scope_ms {
        Some(ms) => {
            println!("run  → scope active:   {ms} ms");
            if ms > MAX_SCOPE_MS {
                failures.push(format!("scope {ms}ms > {MAX_SCOPE_MS}ms"));
            }
        }
        None => failures.push("scope slot never published".into()),
    }

    // ── External cue → /incoming/osc ────────────────────────────────────
    // The supervisor binds the osc-cues port (well-known 4560 preferred).
    let cue_port = sup.ports.get(PortId::TauOscCues);
    let cue_tx = UdpOscSender::to_localhost(cue_port).expect("cue sender");
    let cue_at_ms = t0.elapsed().as_millis() as u64;
    cue_tx
        .send(&OscMessage {
            addr: "/latency/probe".into(),
            args: vec![OscType::Int(1)],
        })
        .expect("cue send");
    match wait_for(&stamps.cue_event, Duration::from_millis(MAX_CUE_MS as u64 * 4)) {
        Some(ms) => {
            let delta = ms.saturating_sub(cue_at_ms) as u128;
            println!("cue  → incoming event: {delta} ms");
            if delta > MAX_CUE_MS {
                failures.push(format!("cue {delta}ms > {MAX_CUE_MS}ms"));
            }
        }
        None => failures.push("external cue never surfaced".into()),
    }

    // ── Teardown ────────────────────────────────────────────────────────
    session.stop().ok();
    std::thread::sleep(Duration::from_millis(500));
    sup.shutdown();
    if !sup.shutdown_verified() {
        failures.push("a supervisor child survived shutdown".into());
    }

    if failures.is_empty() {
        println!("PASS");
    } else {
        for f in &failures {
            eprintln!("FAIL: {f}");
        }
        std::process::exit(1);
    }
}
