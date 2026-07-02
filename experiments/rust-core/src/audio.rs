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

pub mod shm;
pub mod spectrum;

pub use shm::{
    metrics_idx, MetricsReader, ScopeReader, ScopeSlotReader, ScopeWriter, SCOPE_SHM_NAME,
    SHM_AUDIO_CHANNELS, SHM_AUDIO_FRAMES, SHM_AUDIO_SAMPLE_RATE,
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
pub struct AudioProcessor {
    scope: Option<ScopeReader>,
}

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

impl Default for AudioProcessor {
    fn default() -> Self {
        Self::new()
    }
}
