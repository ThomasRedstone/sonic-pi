//! Scripted-session conformance (foundation harness, plan v2 item 1b):
//! drive every settings-protocol path the GUI can send against the REAL
//! runtime and assert the expected reply families arrive. This is what
//! runtime-tests the wire formats mirrored from mainwindow.cpp — unit tests
//! prove the bytes, this proves the receivers accept them (a token in the
//! wrong slot would earn an "Invalid token" log and no effect).
//!
//!   cargo run --example conformance_check

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sonicpi_core::supervisor::{BootMode, Supervisor};
use sonicpi_core::{ApiClient, ClientEvent, Session};

#[derive(Default)]
struct Seen {
    spider_ready: AtomicUsize,
    reports: AtomicUsize,
    drivers_lists: AtomicUsize,
    driver_switches: AtomicUsize,
    device_pushes: AtomicUsize,
    current_driver: Mutex<Option<String>>,
    errors: Mutex<Vec<String>>,
}

impl ApiClient for Seen {
    fn on_event(&self, e: ClientEvent) {
        match e {
            ClientEvent::SpiderReady => {
                self.spider_ready.fetch_add(1, Ordering::Relaxed);
            }
            ClientEvent::Report(m) => {
                self.reports.fetch_add(1, Ordering::Relaxed);
                // Token rejections surface as plain log lines — catch them.
                if m.text.contains("Invalid token") {
                    self.errors.lock().unwrap().push(m.text);
                }
            }
            ClientEvent::AudioDrivers(d) => {
                self.drivers_lists.fetch_add(1, Ordering::Relaxed);
                *self.current_driver.lock().unwrap() = Some(d.current);
            }
            ClientEvent::DriverSwitched { ok, detail } => {
                self.driver_switches.fetch_add(1, Ordering::Relaxed);
                if !ok {
                    // Switching to the CURRENT driver must succeed; a failure
                    // means the wire format (not the driver) is wrong.
                    self.errors.lock().unwrap().push(format!("driver switch failed: {detail}"));
                }
            }
            ClientEvent::AudioDevices(_) => {
                self.device_pushes.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

fn wait_until(deadline: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app");
    println!("booting via Rust supervisor…");
    let mut sup = Supervisor::boot(&root, BootMode::Normal).expect("supervisor boot");
    let seen = Arc::new(Seen::default());
    let session = Session::connect(&sup.ports, seen.clone()).expect("session");
    let mut failures: Vec<String> = Vec::new();

    // Spider must come up before spider-bound settings mean anything.
    if !wait_until(Duration::from_secs(30), || {
        seen.spider_ready.load(Ordering::Relaxed) > 0
            || seen.reports.load(Ordering::Relaxed) > 0
    }) {
        eprintln!("FAIL: spider never became ready");
        sup.shutdown();
        std::process::exit(1);
    }

    // ── Driver enumeration (send_from reply routing) ────────────────────
    // Session::connect already requested it; re-request explicitly too.
    session.request_audio_drivers().expect("drivers list request");
    if !wait_until(Duration::from_secs(5), || seen.drivers_lists.load(Ordering::Relaxed) > 0) {
        failures.push("no /supersonic/drivers/list.reply (send_from routing?)".into());
    }

    // ── Spider-bound settings sweep ─────────────────────────────────────
    // Each pair toggles on then back to default. A wrong wire format shows
    // up as an "Invalid token" log (caught above) or a spider error report.
    session.set_mixer_amp(1.0, true).expect("mixer amp");
    session.set_mixer_invert_stereo(true).expect("invert on");
    session.set_mixer_invert_stereo(false).expect("invert off");
    session.set_mixer_force_mono(true).expect("mono on");
    session.set_mixer_force_mono(false).expect("mono off");
    session.set_mixer_hpf(Some(80.0)).expect("hpf on");
    session.set_mixer_hpf(None).expect("hpf off");
    session.set_mixer_lpf(Some(100.0)).expect("lpf on");
    session.set_mixer_lpf(None).expect("lpf off");
    session.set_midi_enabled(false, true).expect("midi off");
    session.set_midi_enabled(true, true).expect("midi on");
    session.set_cue_server_enabled(true).expect("cue server on");
    session.set_cue_server_external(false).expect("cue internal");
    session.set_update_checking(false).expect("update check off");
    session.set_gamepad_enabled(false, true).expect("gamepad off");

    // ── Driver switch to the CURRENT driver (a no-op hot-swap) ──────────
    // JUCE treats switching to the active driver as success, so this
    // validates /supersonic/drivers/switch + its .reply parse with no risk
    // of leaving the machine on a different audio API.
    let current = seen.current_driver.lock().unwrap().clone();
    match current {
        Some(name) => {
            session.switch_audio_driver_direct(&name).expect("driver switch");
            if !wait_until(Duration::from_secs(10), || {
                seen.driver_switches.load(Ordering::Relaxed) > 0
            }) {
                failures.push("no /supersonic/drivers/switch.reply".into());
            }
        }
        None => failures.push("no current driver learned — switch leg skipped".into()),
    }

    // Give the runtime a moment to process the sweep and log any rejections.
    std::thread::sleep(Duration::from_secs(3));

    let errors = seen.errors.lock().unwrap().clone();
    for e in &errors {
        failures.push(format!("runtime rejected a message: {e}"));
    }

    println!(
        "reports: {}, drivers lists: {}, device pushes: {}, rejections: {}",
        seen.reports.load(Ordering::Relaxed),
        seen.drivers_lists.load(Ordering::Relaxed),
        seen.device_pushes.load(Ordering::Relaxed),
        errors.len()
    );

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
