//! Beat dip: how far a source's level falls over the beat, which a listener
//! hears as a sidechain pump.
//!
//! Each source is summed to mono and high-passed at 300 Hz by two one-pole
//! stages, which leaves out the kick and the sub that key a pump and would
//! fill its dip. Every whole quarter note of the analysed window is cut into
//! 24 equal parts by frame. A span's dip folds its whole quarter notes onto
//! one, as the mean power of each part across them, and compares its
//! quietest part with its upper quartile, in dB. A tempo change moves the
//! parts with the beat.
//!
//! The upper quartile is the level the music returns to between dips. A drum
//! hit fills only a few parts of the beat, so it does not raise it, while a
//! pump's dip lasts long enough to reach the quietest part.

use super::round2;

/// The parts of a quarter note.
pub(super) const PARTS: usize = 24;
const HIGH_PASS_HZ: f64 = 300.0;
/// A span needs this many whole quarter notes.
const MIN_BEATS: usize = 4;
/// A span is measured when its mean square above 300 Hz exceeds -60 dBFS.
const AUDIBLE_MEAN_SQUARE: f64 = 1e-6;
/// The quietest part counts as at most 60 dB under the upper quartile.
const MAX_DIP_DB: f64 = 60.0;

/// The analysed window's whole quarter notes and each source's power in
/// their parts.
pub(super) struct BeatFold {
    /// Each whole quarter note's first frame and the frame after it.
    beats: Vec<(u64, u64)>,
    /// Frames in each part of each quarter note.
    counts: Vec<[u64; PARTS]>,
    sources: Vec<SourceFold>,
    /// The high-pass stages' pole.
    pole: f64,
    /// The quarter note that holds or follows the current frame.
    beat: usize,
}

struct SourceFold {
    lows: [f64; 2],
    /// Each quarter note's high-passed power in each part.
    parts: Vec<[f64; PARTS]>,
}

impl BeatFold {
    /// `beats` must be ascending and not overlap.
    pub fn new(rate: u32, beats: Vec<(u64, u64)>, sources: usize) -> Self {
        let parts = vec![[0.0; PARTS]; beats.len()];
        Self {
            counts: vec![[0; PARTS]; beats.len()],
            sources: (0..sources)
                .map(|_| SourceFold {
                    lows: [0.0; 2],
                    parts: parts.clone(),
                })
                .collect(),
            beats,
            pole: (-2.0 * std::f64::consts::PI * HIGH_PASS_HZ / f64::from(rate)).exp(),
            beat: 0,
        }
    }

    /// Add one frame: its index and each source's mono sample.
    pub fn push(&mut self, frame: u64, mono: impl Iterator<Item = f64>) {
        let pole = self.pole;
        while self.beat < self.beats.len() && self.beats[self.beat].1 <= frame {
            self.beat += 1;
        }
        let part = self.beats.get(self.beat).and_then(|&(start, end)| {
            (start <= frame).then(|| ((frame - start) * PARTS as u64 / (end - start)) as usize)
        });
        if let Some(part) = part {
            self.counts[self.beat][part] += 1;
        }
        for (source, mut x) in self.sources.iter_mut().zip(mono) {
            for low in &mut source.lows {
                *low = (1.0 - pole) * x + pole * *low;
                x -= *low;
            }
            if let Some(part) = part {
                source.parts[self.beat][part] += x * x;
            }
        }
    }

    /// Source `source`'s dip over the whole quarter notes in `[start, end)`,
    /// in dB.
    pub fn dip(&self, source: usize, start: u64, end: u64) -> Option<f64> {
        let first = self.beats.partition_point(|beat| beat.0 < start);
        let last = self.beats.partition_point(|beat| beat.1 <= end);
        if last < first + MIN_BEATS {
            return None;
        }
        let mut power = [0.0; PARTS];
        let mut frames = [0u64; PARTS];
        for beat in first..last {
            for part in 0..PARTS {
                power[part] += self.sources[source].parts[beat][part];
                frames[part] += self.counts[beat][part];
            }
        }
        if frames.contains(&0) {
            return None;
        }
        let total: u64 = frames.iter().sum();
        if power.iter().sum::<f64>() / total as f64 <= AUDIBLE_MEAN_SQUARE {
            return None;
        }
        let mut means: Vec<f64> = power
            .iter()
            .zip(frames)
            .map(|(power, frames)| power / frames as f64)
            .collect();
        means.sort_by(f64::total_cmp);
        // The nearest-rank 75th percentile: the 18th of 24 parts.
        let upper = means[(PARTS * 3).div_ceil(4) - 1];
        Some(round2((10.0 * (upper / means[0]).log10()).min(MAX_DIP_DB)))
    }
}
