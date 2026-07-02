//! E2E check of the Phase-4 Rust supervisor (daemon.rb replacement): boot
//! Spider + SuperSonic directly, connect a Session on the allocated ports,
//! run a silent loop, expect live log traffic back, shut down, and verify
//! nothing is left running. Exits non-zero on failure so it can gate CI.
//!
//!   cargo run --example supervisor_check

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sonicpi_core::supervisor::Supervisor;
use sonicpi_core::{ApiClient, ClientEvent, Session};

const QUIET_RUN: &str = "live_loop :sup do\n  sample :bd_haus, amp: 0\n  sleep 0.25\nend";

struct Counter {
    events: Arc<AtomicUsize>,
    device_pushes: Arc<AtomicUsize>,
}
impl ApiClient for Counter {
    fn on_event(&self, e: ClientEvent) {
        self.events.fetch_add(1, Ordering::Relaxed);
        if matches!(e, ClientEvent::AudioDevices(_)) {
            self.device_pushes.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
    println!("booting via Rust supervisor (no daemon.rb)…");
    let mut sup = Supervisor::boot(&root).expect("supervisor boot");
    let scsynth = sup.ports.get(sonicpi_core::PortId::Scsynth);
    println!("engine on {scsynth}, token {}", sup.ports.token);

    let events = Arc::new(AtomicUsize::new(0));
    let device_pushes = Arc::new(AtomicUsize::new(0));
    let session = Session::connect(
        &sup.ports,
        Arc::new(Counter { events: events.clone(), device_pushes: device_pushes.clone() }),
    )
    .expect("session");

    // Give Spider time to boot, then drive a run.
    std::thread::sleep(Duration::from_secs(10));
    session.run("sup", QUIET_RUN).expect("run");
    std::thread::sleep(Duration::from_secs(5));
    session.stop().expect("stop");
    std::thread::sleep(Duration::from_secs(2));

    let n = events.load(Ordering::Relaxed);
    let devices = device_pushes.load(Ordering::Relaxed);
    let (spider_up, engine_up) = sup.children_running();
    println!(
        "events received: {n} (device pushes: {devices}); spider up: {spider_up}; engine up: {engine_up}"
    );

    sup.shutdown();
    let (spider_after, engine_after) = sup.children_running();
    let shm_gone = !std::path::Path::new(&format!("/dev/shm/SuperSonic_{scsynth}")).exists();
    println!(
        "after shutdown — spider: {spider_after}, engine: {engine_after}, shm gone: {shm_gone}"
    );

    // Device pushes prove the /supersonic/devices/report registration works
    // without a daemon in the middle (the settings pane depends on it).
    let ok = n > 0
        && devices > 0
        && spider_up
        && engine_up
        && !spider_after
        && !engine_after
        && shm_gone;
    println!("{}", if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}
