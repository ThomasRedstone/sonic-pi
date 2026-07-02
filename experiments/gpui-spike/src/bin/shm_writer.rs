//! Fake SuperSonic: writes a moving audio trace into the scope shared-memory
//! ring, so the GUI spike's Scope pane shows live data from another process.
//!
//! Usage: run this, then run the GUI spike (`cargo run`) in another terminal.
//! Ctrl-C (or SIGTERM) to stop — a signal handler breaks the loop so the
//! segment is unlinked cleanly on the way out.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use sonicpi_core::audio::{ScopeWriter, SCOPE_SHM_NAME, SHM_AUDIO_CHANNELS};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn main() {
    // Handle Ctrl-C / kill so we break the loop and let ScopeWriter's Drop
    // unlink the segment (a plain SIGTERM would skip destructors otherwise).
    unsafe {
        let handler = on_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }

    let mut w = ScopeWriter::create(SCOPE_SHM_NAME).expect("failed to create scope shm");
    println!("fake-supersonic: streaming scope audio to shm '{SCOPE_SHM_NAME}'");
    println!("now run the GUI spike (`cargo run`) in another terminal. Ctrl-C to stop.");

    let sr = 48_000f32;
    let ch = SHM_AUDIO_CHANNELS as usize;
    let block: u32 = 480; // 10 ms @ 48 kHz
    let mut phase = 0f32;
    let mut t: u64 = 0; // global frame counter, drives a slow frequency sweep
    let mut buf = vec![0f32; block as usize * ch];

    loop {
        for f in 0..block as usize {
            // Frequency slowly sweeps ~150–650 Hz so the trace visibly moves.
            let freq = 400.0 + 250.0 * (t as f32 * 0.0008).sin();
            phase += TAU * freq / sr;
            if phase > TAU {
                phase -= TAU;
            }
            let env = 0.7;
            buf[f * ch] = phase.sin() * env; // channel 0 (what the scope reads)
            if ch > 1 {
                buf[f * ch + 1] = (phase * 1.5).sin() * env * 0.6; // a little stereo variety
            }
            t += 1;
        }
        w.write_interleaved(&buf, block);
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        sleep(Duration::from_millis(10));
    }
    println!("\nfake-supersonic: stopping, unlinking shm segment.");
    // `w` drops here → deactivates slot 0 and unlinks the segment.
}
