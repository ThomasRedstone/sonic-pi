//! Soak test (`plan/04-gig-hardening.md`): boot the real runtime, run a
//! `live_loop` for an extended period, and periodically check that both
//! children are still alive, the scope is still publishing, and memory
//! isn't drifting. Defaults to a short CI-friendly duration; override for
//! the real multi-hour hardware run the plan calls for before trusting gig
//! mode on a stage.
//!
//!   cargo run --release --example soak                       # 60s default
//!   SONIC_OXIDE_SOAK_SECS=3600 cargo run --release --example soak   # 1h
//!   SONIC_OXIDE_SOAK_SECS=86400 cargo run --release --example soak  # 24h
//!
//! Not wired into any CI job (deliberately — see `sonic-oxide.yml`'s
//! comments on keeping CI usage sparing, and this needs the real SuperSonic
//! binary + Ruby runtime CI doesn't build). Local + `make soak` only.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sonicpi_core::audio::ScopeSlotReader;
use sonicpi_core::supervisor::{BootMode, Supervisor};
use sonicpi_core::{ApiClient, ClientEvent, PortId, Session};

const LOOP_CODE: &str =
    "live_loop :soak do\n  sample :bd_haus, amp: 0\n  sleep 0.5\nend";

struct Counter(Arc<AtomicUsize>);
impl ApiClient for Counter {
    fn on_event(&self, _: ClientEvent) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// `VmRSS` in KiB from `/proc/<pid>/status`, or `None` if unreadable
/// (non-Linux, or the process already exited).
fn rss_kib(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    text.lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse().ok())
}

fn env_secs(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
    let total = Duration::from_secs(env_secs("SONIC_OXIDE_SOAK_SECS", 60));
    let sample_every = Duration::from_secs(env_secs("SONIC_OXIDE_SOAK_SAMPLE_SECS", 10));
    println!(
        "soak: running for {}s, sampling every {}s",
        total.as_secs(),
        sample_every.as_secs()
    );

    let mut sup = Supervisor::boot(&root, BootMode::Normal).expect("supervisor boot");
    let events = Arc::new(AtomicUsize::new(0));
    let session =
        Session::connect(&sup.ports, Arc::new(Counter(events.clone()))).expect("session");
    session.run("soak", LOOP_CODE).expect("start the loop");

    let scsynth = sup.ports.get(PortId::Scsynth);
    let shm = format!("/SuperSonic_{scsynth}");
    let spider_pid = sup.spider_pid();
    let supersonic_pid = sup.supersonic_pid();

    let mut failures = Vec::new();
    let mut samples: Vec<(u64, u64)> = Vec::new(); // (spider_rss, engine_rss) KiB
    let start = Instant::now();
    let mut next_sample = start;
    let mut scope_ever_live = false;

    while start.elapsed() < total {
        if Instant::now() >= next_sample {
            next_sample += sample_every;
            let (spider_up, engine_up) = sup.children_running();
            if !spider_up || !engine_up {
                failures.push(format!(
                    "child died at {}s (spider up: {spider_up}, engine up: {engine_up})",
                    start.elapsed().as_secs()
                ));
                break;
            }
            let scope_live = ScopeSlotReader::open(&shm, 0)
                .map(|mut r| {
                    let mut buf = Vec::new();
                    r.pull_latest_mono(&mut buf) || !buf.is_empty()
                })
                .unwrap_or(false);
            scope_ever_live |= scope_live;

            let s_rss = rss_kib(spider_pid).unwrap_or(0);
            let e_rss = rss_kib(supersonic_pid).unwrap_or(0);
            samples.push((s_rss, e_rss));
            println!(
                "t={:>5}s  events={:>6}  spider_rss={s_rss}KiB  engine_rss={e_rss}KiB  scope_live={scope_live}",
                start.elapsed().as_secs(),
                events.load(Ordering::Relaxed),
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if !scope_ever_live {
        failures.push("scope never published a live window during the whole run".into());
    }
    if events.load(Ordering::Relaxed) == 0 {
        failures.push("no engine events arrived during the whole run".into());
    }

    // Memory-drift check: compare the last sample to the first (after
    // letting warmup settle — skip the very first sample). Generous
    // threshold (2x) — this is a smoke check for leaks, not a tight budget.
    if samples.len() > 2 {
        let (s0, e0) = samples[1];
        let (sn, en) = samples[samples.len() - 1];
        if s0 > 0 && sn > s0 * 2 {
            failures.push(format!("spider RSS more than doubled: {s0}KiB -> {sn}KiB"));
        }
        if e0 > 0 && en > e0 * 2 {
            failures.push(format!("engine RSS more than doubled: {e0}KiB -> {en}KiB"));
        }
    }

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
