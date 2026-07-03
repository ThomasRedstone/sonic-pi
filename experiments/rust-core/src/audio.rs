//! Audio — scope shared-memory reader + FFT/spectrum for `ProcessedAudio`.
//!
//! The hard part — reading SuperSonic's audio ring from shared memory with a
//! byte-for-byte `#[repr(C)]` layout and correct atomic ordering — lives in the
//! [`shm`] submodule (proven in the Phase-1 GPUI spike, now folded in here as
//! its permanent home; see plan/01-spike-results.md).
//!
//! Still TODO: the FFT/spectrum stage (KissFFT → `rustfft`) that turns the raw
//! window into the log-spaced spectrum buckets + peaks of `ProcessedAudio`, plus
//! the display ballistics (instant attack, timed release, peak-hold) the C++
//! `AudioProcessor` applies before handing frames to the client.

// POSIX shm only — the Windows CreateFileMapping backend is future work
// (plan v2, cross-platform); everything else in the crate is portable.
#[cfg(unix)]
pub mod shm;
pub mod spectrum;

#[cfg(unix)]
pub use shm::{
    metrics_idx, MetricsReader, NodeInfo, NodeTreeReader, ScopeReader, ScopeSlotReader,
    ScopeWriter, SCOPE_SHM_NAME, SHM_AUDIO_CHANNELS, SHM_AUDIO_FRAMES, SHM_AUDIO_SAMPLE_RATE,
};
pub use spectrum::{SpectrumFrame, SpectrumProcessor};

/// The processed per-frame scope/spectrum snapshot handed to the client.
/// Mirrors the C++ `ProcessedAudio`.
#[derive(Debug, Clone, Default)]
pub struct ProcessedAudio {
    pub samples: [Vec<f32>; 2],
    pub mono_samples: Vec<f32>,
    pub spectrum_quantized: [Vec<f32>; 2],
    pub spectrum_peaks: [Vec<f32>; 2],
    pub spectrum_freq_min: f32,
    pub spectrum_freq_max: f32,
}

/// Reads the engine's scope shared memory and (eventually) produces
/// `ProcessedAudio`. The shm read half is real; the FFT/spectrum half is TODO.
/// POSIX-shm-backed, so unix-only like the [`shm`] module.
#[cfg(unix)]
pub struct AudioProcessor {
    scope: Option<ScopeReader>,
}

#[cfg(unix)]
impl AudioProcessor {
    pub fn new() -> Self {
        AudioProcessor { scope: None }
    }

    /// Attach to the engine's scope shm segment (e.g. [`SCOPE_SHM_NAME`], or the
    /// real `SuperSonic_<port>` segment).
    pub fn connect(&mut self, name: &str) -> std::io::Result<()> {
        self.scope = Some(ScopeReader::open(name)?);
        Ok(())
    }

    /// True once attached and the producer is actively writing.
    pub fn is_active(&self) -> bool {
        self.scope.as_ref().map_or(false, |r| r.is_active())
    }

    /// Fill `out` with the most recent `frames` of channel 0. Returns false when
    /// not connected or no data is flowing. This is what a scope view calls each
    /// frame; the spectrum path will build on the same read.
    pub fn read_scope_mono(&self, out: &mut Vec<f32>, frames: usize) -> bool {
        self.scope
            .as_ref()
            .map_or(false, |r| r.read_latest_mono(out, frames))
    }
}

#[cfg(unix)]
impl Default for AudioProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facade_reads_a_live_ring_and_reports_disconnected_states() {
        let mut p = AudioProcessor::default();
        // Disconnected: inert.
        assert!(!p.is_active());
        let mut out = Vec::new();
        assert!(!p.read_scope_mono(&mut out, 64));
        assert!(p.connect("/sonic_oxide_no_such_segment").is_err());

        // Connected to a live fake ring: data flows through the facade.
        let name = "/sonic_oxide_audio_facade_test";
        let mut w = ScopeWriter::create(name).unwrap();
        let block = vec![0.25f32; 128 * SHM_AUDIO_CHANNELS as usize];
        w.write_interleaved(&block, 128);
        p.connect(name).unwrap();
        assert!(p.is_active());
        assert!(p.read_scope_mono(&mut out, 64));
        assert_eq!(out.len(), 64);
        assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-6));
    }
}
