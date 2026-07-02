//! Spectrum analysis — the Rust port of the C++ `AudioProcessor` FFT path
//! (`app/api/src/audio/audio_processor.cpp`), constants and ballistics
//! preserved:
//!
//!   * 4096-sample frame, Hann window, amplitude-corrected power per bin
//!   * log-spaced buckets between 30Hz and min(20kHz, Nyquist)
//!   * dB (power domain) normalised 0..1 over a 70dB display range
//!   * instant attack / 60dB-per-second release on the bars
//!   * peak-hold ~0.5s, then a 45dB-per-second fall
//!
//! `rustfft` replaces KissFFT (complex transform on real input — bins 0..N/2
//! match `kiss_fftr`'s output).

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub const FRAME_SAMPLES: usize = 4096;
pub const FFT_DECIBEL_RANGE: f32 = 70.0;
pub const REFRESH_RATE: f32 = 60.0;
pub const SPECTRUM_FREQ_MIN: f32 = 30.0;
pub const SPECTRUM_FREQ_MAX_LIMIT: f32 = 20_000.0;

const RELEASE_PER_FRAME: f32 = 60.0 / FFT_DECIBEL_RANGE / REFRESH_RATE;
const PEAK_FALL_PER_FRAME: f32 = 45.0 / FFT_DECIBEL_RANGE / REFRESH_RATE;
const PEAK_HOLD_FRAMES: u32 = (REFRESH_RATE / 2.0) as u32; // ~0.5s

/// One channel's spectrum output for a frame.
#[derive(Debug, Clone, Default)]
pub struct SpectrumFrame {
    /// Smoothed bar heights, 0..1, one per bucket.
    pub bars: Vec<f32>,
    /// Peak-hold markers, 0..1, one per bucket.
    pub peaks: Vec<f32>,
    pub freq_min: f32,
    pub freq_max: f32,
}

pub struct SpectrumProcessor {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    total_win: f32,
    scratch: Vec<Complex<f32>>,
    partitions: Vec<usize>,
    last_partitions: (usize, u32),
    smoothed: Vec<f32>,
    peak: Vec<f32>,
    peak_age: Vec<u32>,
}

impl SpectrumProcessor {
    pub fn new() -> SpectrumProcessor {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FRAME_SAMPLES);
        // Hann window, exactly as the C++ createWindow.
        let window: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|i| {
                0.5 * (1.0
                    - (2.0 * std::f32::consts::PI * i as f32 / (FRAME_SAMPLES as f32 - 1.0)).cos())
            })
            .collect();
        let total_win = window.iter().sum();
        SpectrumProcessor {
            fft,
            window,
            total_win,
            scratch: vec![Complex::default(); FRAME_SAMPLES],
            partitions: Vec::new(),
            last_partitions: (0, 0),
            smoothed: Vec::new(),
            peak: Vec::new(),
            peak_age: Vec::new(),
        }
    }

    /// Bucket edges (bin indices) log-spaced between SPECTRUM_FREQ_MIN and
    /// the Nyquist-clamped max — mirrors `GenFreqPartitions`.
    fn gen_partitions(&mut self, buckets: usize, sample_rate: u32) {
        if self.last_partitions == (buckets, sample_rate) && !self.partitions.is_empty() {
            return;
        }
        self.last_partitions = (buckets, sample_rate);
        let f_hi = SPECTRUM_FREQ_MAX_LIMIT.min(sample_rate as f32 * 0.5);
        let bins = FRAME_SAMPLES / 2;
        self.partitions = (0..=buckets)
            .map(|k| {
                let f = SPECTRUM_FREQ_MIN
                    * (f_hi / SPECTRUM_FREQ_MIN).powf(k as f32 / buckets as f32);
                let bin = (f / (sample_rate as f32 / FRAME_SAMPLES as f32)).round() as usize;
                bin.min(bins)
            })
            .collect();
    }

    /// Process one frame of mono samples (the newest `FRAME_SAMPLES`; shorter
    /// input is zero-padded at the front) and advance the ballistics one
    /// display frame.
    pub fn process(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
        max_buckets: usize,
    ) -> SpectrumFrame {
        let spectrum_samples = FRAME_SAMPLES / 2;
        let buckets = (spectrum_samples / 8).min(max_buckets).max(4);
        self.gen_partitions(buckets, sample_rate);

        // Window × audio (front-padded with zeros when short).
        let pad = FRAME_SAMPLES.saturating_sub(samples.len());
        for i in 0..FRAME_SAMPLES {
            let s = if i < pad { 0.0 } else { samples[i - pad] };
            self.scratch[i] = Complex::new(s * self.window[i], 0.0);
        }
        self.fft.process(&mut self.scratch);
        // Bin 0 is the all-frequency (DC) component — zeroed like the C++.
        self.scratch[0] = Complex::default();

        let power: Vec<f32> = self.scratch[..spectrum_samples]
            .iter()
            .map(|c| {
                let amp = c.norm() * 2.0 / self.total_win;
                amp * amp
            })
            .collect();

        if self.smoothed.len() != buckets {
            self.smoothed = vec![0.0; buckets];
            self.peak = vec![0.0; buckets];
            self.peak_age = vec![0; buckets];
        }

        let mut frame = SpectrumFrame {
            bars: vec![0.0; buckets],
            peaks: vec![0.0; buckets],
            freq_min: SPECTRUM_FREQ_MIN,
            freq_max: SPECTRUM_FREQ_MAX_LIMIT.min(sample_rate as f32 * 0.5),
        };

        for k in 0..buckets {
            let b0 = self.partitions[k];
            let b1 = (b0 + 1).max(self.partitions[k + 1]).min(spectrum_samples);
            let b0 = b0.min(b1 - 1);
            let mean_power = (power[b0..b1].iter().sum::<f32>() / (b1 - b0) as f32)
                .max(f32::MIN_POSITIVE);

            // dB (power domain), normalised 0..1 over the display range.
            let v = ((10.0 * mean_power.log10() + FFT_DECIBEL_RANGE) / FFT_DECIBEL_RANGE)
                .clamp(0.0, 1.0);

            // Instant attack, timed release.
            let smoothed = v.max(self.smoothed[k] - RELEASE_PER_FRAME);
            self.smoothed[k] = smoothed;
            frame.bars[k] = smoothed;

            // Peak-hold: sit at the recent maximum, then fall slowly.
            if v >= self.peak[k] {
                self.peak[k] = v;
                self.peak_age[k] = 0;
            } else {
                self.peak_age[k] += 1;
                if self.peak_age[k] > PEAK_HOLD_FRAMES {
                    self.peak[k] = smoothed.max(self.peak[k] - PEAK_FALL_PER_FRAME);
                }
            }
            frame.peaks[k] = self.peak[k];
        }
        frame
    }
}

impl Default for SpectrumProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sample_rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn sine_peaks_in_the_right_bucket() {
        let mut p = SpectrumProcessor::new();
        let samples = sine(1000.0, 48_000, FRAME_SAMPLES);
        let frame = p.process(&samples, 48_000, 64);
        assert_eq!(frame.bars.len(), 64);

        // The loudest bucket must be the one whose log-spaced range holds 1kHz.
        let loudest = frame
            .bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        let expected = (64.0
            * (1000.0f32 / SPECTRUM_FREQ_MIN).ln()
            / (SPECTRUM_FREQ_MAX_LIMIT.min(24_000.0) / SPECTRUM_FREQ_MIN).ln())
        .floor() as usize;
        assert!(
            loudest.abs_diff(expected) <= 1,
            "1kHz landed in bucket {loudest}, expected ~{expected}"
        );
        // And it should be strongly above the quietest bucket.
        let quietest = frame.bars.iter().cloned().fold(f32::MAX, f32::min);
        assert!(frame.bars[loudest] > quietest + 0.3, "no clear peak");
    }

    #[test]
    fn silence_is_near_zero() {
        let mut p = SpectrumProcessor::new();
        let frame = p.process(&vec![0.0; FRAME_SAMPLES], 48_000, 32);
        assert!(frame.bars.iter().all(|&v| v == 0.0), "silence must clamp to 0");
    }

    #[test]
    fn ballistics_release_and_peak_hold() {
        let mut p = SpectrumProcessor::new();
        let loud = sine(1000.0, 48_000, FRAME_SAMPLES);
        let first = p.process(&loud, 48_000, 32);
        let k = first
            .bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        let loud_level = first.bars[k];

        // One silent frame: bar releases by exactly one frame's worth;
        // peak holds.
        let silent = p.process(&vec![0.0; FRAME_SAMPLES], 48_000, 32);
        assert!((silent.bars[k] - (loud_level - RELEASE_PER_FRAME)).abs() < 1e-4);
        assert_eq!(silent.peaks[k], first.peaks[k], "peak must hold");

        // After the hold window expires the peak starts falling.
        let mut last_peak = silent.peaks[k];
        for _ in 0..(PEAK_HOLD_FRAMES + 5) {
            last_peak = p.process(&vec![0.0; FRAME_SAMPLES], 48_000, 32).peaks[k];
        }
        assert!(last_peak < silent.peaks[k], "peak must fall after the hold");
    }
}
