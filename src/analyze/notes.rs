//! Score–audio agreement: whether each note sounds, whether its part is
//! buried in its own register, and whether a lone note sounds at its written
//! pitch.
//!
//! Every note or hit whose target is an analysed source gets a window: a note
//! from its onset to its release, at most [`NOTE_MAX_S`]; a hit for
//! [`HIT_S`]. While rendering, the window adds up the target's K-weighted and
//! octave-band energy, and the same at the target's mix point; the target is
//! judged in its strongest band. A note that no
//! other note on its node overlaps while its pitch is measured, and that has
//! no pitch expression, also keeps the target's samples for a pitch estimate.

use serde::Serialize;

use super::{round2, round3};
use crate::production_analysis::loudness;

/// How much of each note is measured: until its release, at most this long,
/// so that a slow attack is not read as a quiet note.
pub(super) const NOTE_MAX_S: f64 = 2.0;
/// How much of each hit is measured.
pub(super) const HIT_S: f64 = 0.25;
/// The attack skipped before estimating pitch.
const PITCH_SKIP_S: f64 = 0.03;
/// A pitch estimate needs this many periods of the written pitch for its
/// integration window, and as many again for its lag range.
const PITCH_PERIODS: f64 = 2.0;
/// YIN: the first dip of the normalized difference below this is the period,
/// and a dip this deep at half the written period means an octave too high.
const YIN_THRESHOLD: f64 = 0.15;
/// A dip this deep within [`NEAR_SEMITONES`] of the written period is taken
/// as the period, even when a longer one fits better: a sub-octave layer
/// makes a waveform repeat at twice the period, but it is heard at the
/// written pitch.
const NEAR_THRESHOLD: f64 = 0.5;
const NEAR_SEMITONES: f64 = 1.5;
/// A window whose chosen dip is above this is not clearly periodic. Clean
/// harmonic tones measure under about 0.03; bells, toms, electric-piano tines
/// and driven resonances measure 0.03 to 0.2 and read off pitch by design.
pub(super) const APERIODIC: f64 = 0.03;

/// Samples a pitch estimate of `hz` needs at `rate`.
pub(super) fn pitch_samples(hz: f64, rate: u32) -> u64 {
    let period = f64::from(rate) / hz;
    (PITCH_PERIODS * period * 2.0).ceil() as u64 + (period * 2.0).ceil() as u64 + 2
}

/// Frames skipped before the pitch estimate.
pub(super) fn pitch_skip(rate: u32) -> u64 {
    (PITCH_SKIP_S * f64::from(rate)).round() as u64
}

/// One measured note.
pub(super) struct Window {
    pub address: String,
    /// Index of the target source, and of its mix point if it has one.
    pub target: usize,
    pub mix: Option<usize>,
    pub start: u64,
    pub end: u64,
    /// The written pitch, when this note's pitch is estimated.
    pub pitch_hz: Option<f64>,
    pub pitch_from: u64,
    pub pitch_needed: usize,
    pub frames: u64,
    pub target_k: f64,
    pub mix_k: f64,
    pub target_bands: [f64; 10],
    pub mix_bands: [f64; 10],
    pub samples: Vec<f64>,
}

impl Window {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        address: String,
        target: usize,
        mix: Option<usize>,
        start: u64,
        end: u64,
        pitch_hz: Option<f64>,
        lone: bool,
        rate: u32,
    ) -> Self {
        let measured = pitch_hz.filter(|_| lone);
        Self {
            address,
            target,
            mix,
            start,
            end,
            pitch_hz: measured,
            pitch_from: start + pitch_skip(rate),
            pitch_needed: measured.map_or(0, |hz| pitch_samples(hz, rate) as usize),
            frames: 0,
            target_k: 0.0,
            mix_k: 0.0,
            target_bands: [0.0; 10],
            mix_bands: [0.0; 10],
            samples: Vec::new(),
        }
    }

    /// Add frame `frame` of every source.
    pub fn add(&mut self, frame: u64, values: &[FrameValues]) {
        let target = &values[self.target];
        self.frames += 1;
        self.target_k += target.k;
        for (total, value) in self.target_bands.iter_mut().zip(target.bands) {
            *total += value;
        }
        if let Some(mix) = self.mix {
            self.mix_k += values[mix].k;
            for (total, value) in self.mix_bands.iter_mut().zip(values[mix].bands) {
                *total += value;
            }
        }
        if frame >= self.pitch_from && self.samples.len() < self.pitch_needed {
            self.samples.push(target.mono);
        }
    }
}

/// What one frame of one source adds to a note window: K-weighted energy and
/// octave-band energy, each summed over channels, and the channel mean.
#[derive(Clone, Copy, Default)]
pub(super) struct FrameValues {
    pub k: f64,
    pub bands: [f64; 10],
    pub mono: f64,
}

/// A pitch estimate: the deviation from the written pitch in cents and the
/// normalized difference at the chosen lag, 0 for a perfectly periodic signal.
pub(super) struct Pitch {
    pub cents: f64,
    pub aperiodicity: f64,
}

/// YIN (de Cheveigné and Kawahara, 2002) on `samples`, around the written
/// period `P`:
///
/// 1. a dip under [`YIN_THRESHOLD`] near `P/2` is an octave too high;
/// 2. otherwise a dip under [`NEAR_THRESHOLD`] near `P` is the period;
/// 3. otherwise the first dip under [`YIN_THRESHOLD`] from `P/2` to `2P`, or
///    the deepest one there.
///
/// "Near" is within [`NEAR_SEMITONES`]. `None` when there are too few samples.
pub(super) fn yin(samples: &[f64], rate: u32, written_hz: f64) -> Option<Pitch> {
    let period = f64::from(rate) / written_hz;
    let min_lag = ((period / 2.0).floor() as usize).max(2);
    let max_lag = (period * 2.0).ceil() as usize;
    let width = (PITCH_PERIODS * period * 2.0).ceil() as usize;
    if samples.len() < width + max_lag + 1 {
        return None;
    }
    // d(τ) for τ = 1..=max_lag + 1, then the cumulative mean normalized d'.
    let mut normalized = vec![1.0; max_lag + 2];
    let mut running = 0.0;
    for (lag, value) in normalized.iter_mut().enumerate().skip(1) {
        let d: f64 = (0..width)
            .map(|j| {
                let e = samples[j] - samples[j + lag];
                e * e
            })
            .sum();
        running += d;
        *value = if running > 0.0 {
            d * lag as f64 / running
        } else {
            1.0
        };
    }
    let deepest = |range: std::ops::RangeInclusive<usize>| {
        range.min_by(|&a, &b| normalized[a].total_cmp(&normalized[b]))
    };
    let near = |center: f64| {
        let spread = 2f64.powf(NEAR_SEMITONES / 12.0);
        let low = ((center / spread).floor() as usize).max(2);
        let high = ((center * spread).ceil() as usize).min(max_lag);
        deepest(low..=high)
    };
    let chosen = if let Some(lag) = near(period / 2.0).filter(|&l| normalized[l] < YIN_THRESHOLD) {
        lag
    } else if let Some(lag) = near(period).filter(|&l| normalized[l] <= NEAR_THRESHOLD) {
        lag
    } else {
        let range = min_lag..=max_lag;
        let mut lag = range
            .clone()
            .find(|&lag| normalized[lag] < YIN_THRESHOLD)
            .or_else(|| deepest(range))?;
        while lag < max_lag && normalized[lag + 1] < normalized[lag] {
            lag += 1;
        }
        lag
    };
    // Parabolic interpolation around the dip.
    let (a, b, c) = (
        normalized[chosen - 1],
        normalized[chosen],
        normalized[chosen + 1],
    );
    let denominator = a - 2.0 * b + c;
    let offset = if denominator > 0.0 {
        (0.5 * (a - c) / denominator).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let lag = chosen as f64 + offset;
    Some(Pitch {
        cents: 1200.0 * (f64::from(rate) / lag / written_hz).log2(),
        aperiodicity: b.max(0.0),
    })
}

/// Results for one note, as listed with `--notes`.
#[derive(Clone, Debug, Serialize)]
pub struct NoteReport {
    /// The event's source address.
    pub event: String,
    /// The target source, `node:port`.
    pub source: String,
    pub start_frame: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// K-weighted loudness of the target over the window; `None` when silent.
    pub level_lufs: Option<f64>,
    /// The target's share of its mix point in the octave band where the
    /// target is strongest over the note, in dB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_db: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cents: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperiodicity: Option<f64>,
    /// `ok`, `silent`, `buried`, `pitch_mismatch`, or `unpitched`.
    pub status: &'static str,
}

/// Per-part counts over a span of notes.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct NoteSummary {
    /// Notes and hits measured.
    pub measured: usize,
    pub silent: usize,
    pub buried: usize,
    /// Notes whose pitch was estimated, and those that were not clearly
    /// periodic. The rest overlap another note, have a pitch expression, are
    /// hits, or are too short.
    pub pitched: usize,
    pub unpitched: usize,
    pub pitch_mismatches: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_cents: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_abs_cents: Option<f64>,
}

/// Measured values for a finished window.
pub(super) struct Outcome {
    pub level_lufs: Option<f64>,
    pub share_db: Option<f64>,
    pub pitch: Option<Pitch>,
}

pub(super) fn outcome(window: &Window, channels: &[u8], rate: u32) -> Outcome {
    let level_lufs = (window.frames > 0 && window.target_k > 0.0)
        .then(|| loudness(window.target_k / window.frames as f64).ok())
        .flatten();
    let per_channel = |energy: f64, source: usize| energy / f64::from(channels[source]);
    let share_db = window.mix.and_then(|mix| {
        // Judge the part where it is strongest: a written pitch says little
        // about where a hat or a bright lead carries its energy.
        let band = (0..10)
            .max_by(|&a, &b| window.target_bands[a].total_cmp(&window.target_bands[b]))
            .expect("ten bands");
        let (target, total) = (window.target_bands[band], window.mix_bands[band]);
        (target > 0.0 && total > 0.0).then(|| {
            round2(10.0 * (per_channel(target, window.target) / per_channel(total, mix)).log10())
        })
    });
    let pitch = window
        .pitch_hz
        .and_then(|hz| yin(&window.samples, rate, hz))
        .map(|pitch| Pitch {
            cents: round2(pitch.cents),
            aperiodicity: round3(pitch.aperiodicity),
        });
    Outcome {
        level_lufs: level_lufs.map(round2),
        share_db,
        pitch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, seconds: f64) -> Vec<f64> {
        let n = (seconds * f64::from(rate)) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                // A fundamental with two harmonics, like a plucked or bowed tone.
                (2.0 * std::f64::consts::PI * hz * t).sin()
                    + 0.5 * (4.0 * std::f64::consts::PI * hz * t).sin()
                    + 0.25 * (6.0 * std::f64::consts::PI * hz * t).sin()
            })
            .collect()
    }

    #[test]
    fn yin_measures_tones_at_their_pitch_and_off_it() {
        for written in [41.2, 110.0, 440.0, 1760.0] {
            let pitch = yin(&tone(written, 48_000, 0.22), 48_000, written).unwrap();
            assert!(pitch.cents.abs() < 2.0, "{written}: {}", pitch.cents);
            assert!(pitch.aperiodicity < 0.05, "{}", pitch.aperiodicity);
        }
        let sharp = 440.0 * 2f64.powf(1.0 / 12.0);
        let pitch = yin(&tone(sharp, 48_000, 0.22), 48_000, 440.0).unwrap();
        assert!((pitch.cents - 100.0).abs() < 2.0, "{}", pitch.cents);
        let octave = yin(&tone(880.0, 48_000, 0.22), 48_000, 440.0).unwrap();
        assert!((octave.cents - 1200.0).abs() < 2.0, "{}", octave.cents);
        let below = yin(&tone(220.0, 48_000, 0.22), 48_000, 440.0).unwrap();
        assert!((below.cents + 1200.0).abs() < 2.0, "{}", below.cents);
    }

    #[test]
    fn a_sub_octave_layer_is_heard_at_the_written_pitch() {
        let rate = 48_000;
        let written = 65.41;
        let layered: Vec<f64> = tone(written, rate, 0.25)
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let t = i as f64 / f64::from(rate);
                x + 0.4 * (std::f64::consts::PI * written * t).sin()
            })
            .collect();
        // The layer biases the dip by a few cents, far inside the limit.
        let pitch = yin(&layered, rate, written).unwrap();
        assert!(pitch.cents.abs() < 10.0, "{}", pitch.cents);
    }

    #[test]
    fn yin_needs_enough_periods() {
        assert!(yin(&tone(41.2, 48_000, 0.05), 48_000, 41.2).is_none());
    }

    #[test]
    fn noise_is_not_periodic() {
        // A deterministic linear congruential sequence.
        let mut state: u64 = 1;
        let noise: Vec<f64> = (0..10_000)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
            })
            .collect();
        let pitch = yin(&noise, 48_000, 440.0).unwrap();
        assert!(pitch.aperiodicity > APERIODIC, "{}", pitch.aperiodicity);
    }
}
