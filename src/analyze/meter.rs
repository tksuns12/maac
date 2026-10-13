//! Streaming per-source measurement in 100 ms blocks.
//!
//! Every source is measured on one block grid that starts at the first
//! rendered frame, so block `i` of any source covers the same frames as block
//! `i` of the project output. Sections aggregate the blocks whose first frame
//! lies inside them.

use crate::production_analysis::{integrated, k_weighting, loudness, Biquad, Interpolator};

use super::image::Spectrogram;
use super::notes::FrameValues;
use super::{round2, round3, Measures, Position, PositionMap, Stereo, BAND_CENTERS_HZ};

/// Samples at or beyond full scale clip when encoded as PCM.
const CLIP_LEVEL: f64 = 1.0;
/// A block is active when its mean square exceeds -60 dBFS.
const ACTIVE_MEAN_SQUARE: f64 = 1e-6;
/// Reported instead of minus infinity for an exact zero.
const FLOOR_DB: f64 = -120.0;
/// Like the BS.1770 absolute gate, blocks at or below -70 LUFS do not count
/// toward the floor or the block range.
const GATE_LUFS: f64 = -70.0;
/// The floor is the quietest tenth of the gated blocks, and needs at least
/// one second of them.
const FLOOR_FRACTION: f64 = 0.1;
const FLOOR_MIN_BLOCKS: usize = 10;
/// The block range runs from the floor percentile to this one.
const RANGE_HIGH: f64 = 0.95;

#[derive(Clone, Debug, Default)]
pub(super) struct Block {
    pub start: u64,
    pub frames: u64,
    /// Sum over channels and frames of the squared K-weighted sample.
    pub k: f64,
    pub sq: [f64; 2],
    pub lr: f64,
    pub peak: f64,
    pub true_peak: f64,
    pub clipped: u64,
    pub first_clip: Option<u64>,
    pub bands: [f64; 10],
}

/// A one-octave constant-peak-gain band-pass biquad (RBJ), direct form I.
#[derive(Clone, Copy)]
struct Band {
    b: [f64; 3],
    a: [f64; 3],
    z: [f64; 4],
}

impl Band {
    fn new(center: f64, rate: u32) -> Option<Self> {
        let nyquist = f64::from(rate) / 2.0;
        if center * std::f64::consts::SQRT_2 >= nyquist {
            return None;
        }
        let w0 = 2.0 * std::f64::consts::PI * center / f64::from(rate);
        let alpha = w0.sin() * (std::f64::consts::LN_2 / 2.0 * w0 / w0.sin()).sinh();
        let a0 = 1.0 + alpha;
        Some(Self {
            b: [alpha / a0, 0.0, -alpha / a0],
            a: [1.0, -2.0 * w0.cos() / a0, (1.0 - alpha) / a0],
            z: [0.0; 4],
        })
    }

    fn process(&mut self, x: f64) -> f64 {
        let [x1, x2, y1, y2] = self.z;
        let y = self.b[0] * x + self.b[1] * x1 + self.b[2] * x2 - self.a[1] * y1 - self.a[2] * y2;
        self.z = [x, x1, y, y1];
        y
    }
}

/// Which octave bands exist below the Nyquist frequency at `rate`.
pub(super) fn available_bands(rate: u32) -> [bool; 10] {
    BAND_CENTERS_HZ.map(|center| Band::new(center, rate).is_some())
}

pub(super) struct SourceMeter {
    channels: usize,
    hop: u64,
    k: Vec<[Biquad; 2]>,
    bands: Vec<[Option<Band>; 10]>,
    interpolator: Interpolator,
    current: Block,
    blocks: Vec<Block>,
    frame: u64,
    spectrogram: Option<Spectrogram>,
    error: Option<String>,
}

impl SourceMeter {
    pub fn new(rate: u32, channels: u8, hop: u64, spectrogram: Option<Spectrogram>) -> Self {
        let channels = usize::from(channels);
        Self {
            channels,
            hop,
            k: (0..channels).map(|_| k_weighting(rate)).collect(),
            bands: (0..channels)
                .map(|_| BAND_CENTERS_HZ.map(|center| Band::new(center, rate)))
                .collect(),
            interpolator: Interpolator::default(),
            current: Block::default(),
            blocks: Vec::new(),
            frame: 0,
            spectrogram,
            error: None,
        }
    }

    /// Count frames from `frame` instead of 0, so block starts are positions
    /// in the piece when only a window is measured.
    pub fn starting_at(mut self, frame: u64) -> Self {
        self.frame = frame;
        self
    }

    /// Measure one frame, and return what it adds to a note window.
    pub fn push(&mut self, frame: &[f64]) -> FrameValues {
        let mut values = FrameValues::default();
        if frame.len() != self.channels {
            self.error
                .get_or_insert_with(|| "rendered port changed its channel count".into());
            return values;
        }
        let mut input = [0.0; 2];
        let block = &mut self.current;
        for (c, &x) in frame.iter().enumerate() {
            input[c] = x;
            let magnitude = x.abs();
            block.peak = block.peak.max(magnitude);
            if magnitude >= CLIP_LEVEL {
                block.clipped += 1;
                block.first_clip.get_or_insert(self.frame);
            }
            block.sq[c] += x * x;
            let [shelf, pass] = &mut self.k[c];
            match shelf.process(x).and_then(|y| pass.process(y)) {
                Ok(y) => {
                    block.k += y * y;
                    values.k += y * y;
                }
                Err(failure) => {
                    self.error.get_or_insert(failure.message);
                }
            }
            for ((energy, value), band) in block
                .bands
                .iter_mut()
                .zip(values.bands.iter_mut())
                .zip(self.bands[c].iter_mut())
            {
                if let Some(band) = band {
                    let y = band.process(x);
                    *energy += y * y;
                    *value += y * y;
                }
            }
        }
        if self.channels == 2 {
            block.lr += frame[0] * frame[1];
        }
        match self.interpolator.push(input, self.channels) {
            Ok(outputs) => {
                for output in outputs {
                    for value in output.iter().take(self.channels) {
                        block.true_peak = block.true_peak.max(value.abs());
                    }
                }
            }
            Err(failure) => {
                self.error.get_or_insert(failure.message);
            }
        }
        values.mono = frame.iter().sum::<f64>() / self.channels as f64;
        if let Some(spectrogram) = &mut self.spectrogram {
            spectrogram.push(values.mono);
        }
        block.frames += 1;
        self.frame += 1;
        if block.frames == self.hop {
            let next = Block {
                start: self.frame,
                ..Block::default()
            };
            self.blocks.push(std::mem::replace(&mut self.current, next));
        }
        values
    }

    /// Flush the true-peak interpolator into the last block.
    pub fn finish(mut self) -> Result<(Vec<Block>, Option<Spectrogram>), String> {
        let mut flushed = 0.0_f64;
        for _ in 0..11 {
            match self.interpolator.push([0.0; 2], self.channels) {
                Ok(outputs) => {
                    for output in outputs {
                        for value in output.iter().take(self.channels) {
                            flushed = flushed.max(value.abs());
                        }
                    }
                }
                Err(failure) => {
                    self.error.get_or_insert(failure.message);
                }
            }
        }
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.current.frames > 0 {
            self.blocks.push(self.current);
        }
        if let Some(last) = self.blocks.last_mut() {
            last.true_peak = last.true_peak.max(flushed);
        }
        let spectrogram = self.spectrogram.map(Spectrogram::finish);
        Ok((self.blocks, spectrogram))
    }
}

impl Block {
    /// BS.1770 energy: K-weighted mean square summed over channels.
    fn energy(&self) -> f64 {
        if self.frames == 0 {
            0.0
        } else {
            self.k / self.frames as f64
        }
    }
}

/// The full blocks above the gate, as (index, loudness), quietest first; ties
/// keep block order.
fn gated(blocks: &[Block], hop: u64) -> Vec<(usize, f64)> {
    let mut gated: Vec<(usize, f64)> = blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.frames == hop && block.energy() > 0.0)
        .filter_map(|(index, block)| loudness(block.energy()).ok().map(|l| (index, l)))
        .filter(|(_, loudness)| *loudness > GATE_LUFS)
        .collect();
    gated.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    gated
}

/// The nearest-rank percentile `p` of ascending `values`.
fn percentile(values: &[(usize, f64)], p: f64) -> f64 {
    let rank = (p * values.len() as f64).ceil() as usize;
    values[rank.clamp(1, values.len()) - 1].1
}

/// The indices of the quietest tenth of the gated blocks, in block order, or
/// `None` with fewer than [`FLOOR_MIN_BLOCKS`] gated blocks.
pub(super) fn floor_blocks(blocks: &[Block], hop: u64) -> Option<Vec<usize>> {
    let gated = gated(blocks, hop);
    if gated.len() < FLOOR_MIN_BLOCKS {
        return None;
    }
    let count = (FLOOR_FRACTION * gated.len() as f64).ceil() as usize;
    let mut floor: Vec<usize> = gated[..count].iter().map(|(index, _)| *index).collect();
    floor.sort_unstable();
    Some(floor)
}

/// The mean BS.1770 energy of `floor`'s blocks, per channel.
pub(super) fn floor_energy(blocks: &[Block], floor: &[usize], channels: u8) -> f64 {
    floor.iter().map(|&i| blocks[i].energy()).sum::<f64>()
        / floor.len() as f64
        / f64::from(channels)
}

/// Loudness of a BS.1770 energy, if it is positive.
pub(super) fn loudness_of(energy: f64) -> Option<f64> {
    (energy > 0.0).then(|| loudness(energy).ok()).flatten()
}

/// The loudness of `floor`'s blocks taken together.
pub(super) fn floor_loudness(blocks: &[Block], floor: &[usize]) -> Option<f64> {
    let energy = floor.iter().map(|&i| blocks[i].energy()).sum::<f64>() / floor.len() as f64;
    loudness(energy).ok()
}

/// How far block loudness spreads, from the floor percentile to the 95th, in
/// LU; `None` with too few gated blocks.
pub(super) fn block_range_lu(blocks: &[Block], hop: u64) -> Option<f64> {
    let gated = gated(blocks, hop);
    (gated.len() >= FLOOR_MIN_BLOCKS)
        .then(|| percentile(&gated, RANGE_HIGH) - percentile(&gated, FLOOR_FRACTION))
}

/// The fraction of full blocks above the gate.
pub(super) fn presence(blocks: &[Block], hop: u64) -> f64 {
    let full = blocks.iter().filter(|block| block.frames == hop).count();
    if full == 0 {
        0.0
    } else {
        gated(blocks, hop).len() as f64 / full as f64
    }
}

/// Octave-band energy summed over `indices`, per channel.
pub(super) fn band_energy(blocks: &[Block], indices: &[usize], channels: u8) -> [f64; 10] {
    let mut bands = [0.0; 10];
    for &index in indices {
        for (total, value) in bands.iter_mut().zip(blocks[index].bands) {
            *total += value;
        }
    }
    bands.map(|value| value / f64::from(channels))
}

pub(super) fn db_power(ratio: f64) -> f64 {
    if ratio > 0.0 {
        10.0 * ratio.log10()
    } else {
        FLOOR_DB
    }
}

fn db_amplitude(amplitude: f64) -> Option<f64> {
    (amplitude > 0.0).then(|| 20.0 * amplitude.log10())
}

/// Measure `blocks`, all on one grid; `output` is the project output's blocks
/// over the same range, for band shares.
pub(super) fn measures(
    blocks: &[Block],
    channels: u8,
    rate: u32,
    hop: u64,
    available: [bool; 10],
    output: Option<&[Block]>,
    positions: &PositionMap<'_>,
) -> Measures {
    let channels_f = f64::from(channels);
    let frames: u64 = blocks.iter().map(|block| block.frames).sum();
    let mut sq = [0.0; 2];
    let mut lr = 0.0;
    let mut bands = [0.0; 10];
    let mut peak = 0.0_f64;
    let mut true_peak = 0.0_f64;
    let mut clipped = 0;
    let mut first_clip = None;
    for block in blocks {
        for (total, value) in sq.iter_mut().zip(block.sq) {
            *total += value;
        }
        lr += block.lr;
        for (total, value) in bands.iter_mut().zip(block.bands) {
            *total += value;
        }
        peak = peak.max(block.peak);
        true_peak = true_peak.max(block.true_peak);
        clipped += block.clipped;
        first_clip = first_clip.or(block.first_clip);
    }
    true_peak = true_peak.max(peak);
    let mean_square = if frames == 0 {
        0.0
    } else {
        (sq[0] + sq[1]) / (frames as f64 * channels_f)
    };
    let rms_dbfs = (mean_square > 0.0).then(|| db_power(mean_square));
    let sample_peak_dbfs = db_amplitude(peak);

    // 400 ms momentary blocks are four consecutive full 100 ms blocks; 3 s
    // short-term windows are thirty.
    let full = |window: &[Block]| window.iter().all(|block| block.frames == hop);
    let energy = |window: &[Block]| {
        let n: u64 = window.iter().map(|block| block.frames).sum();
        window.iter().map(|block| block.k).sum::<f64>() / n as f64
    };
    let momentary: Vec<f64> = blocks
        .windows(4)
        .filter(|window| full(window))
        .map(energy)
        .collect();
    // Like the BS.1770 absolute gate, nothing below -70 LUFS is reported.
    let max_loudness = |energies: &mut dyn Iterator<Item = f64>| {
        energies
            .filter(|energy| *energy > 0.0)
            .filter_map(|energy| loudness(energy).ok())
            .reduce(f64::max)
            .filter(|value| *value > -70.0)
    };
    let max_momentary = max_loudness(&mut momentary.iter().copied());
    let max_short_term =
        max_loudness(&mut blocks.windows(30).filter(|window| full(window)).map(energy));
    let integrated_lufs = integrated(&momentary, peak == 0.0)
        .ok()
        .and_then(|measurement| measurement.value);

    let band_total: f64 = bands.iter().sum();
    let bands_db = (0..10)
        .map(|b| {
            (available[b] && band_total > 0.0).then(|| round2(db_power(bands[b] / band_total)))
        })
        .collect();
    let master_share = output.map(|output| {
        let mut totals = [0.0; 10];
        for block in output {
            for (total, value) in totals.iter_mut().zip(block.bands) {
                *total += value;
            }
        }
        (0..10)
            .map(|b| (available[b] && totals[b] > 0.0).then(|| round3(bands[b] / totals[b])))
            .collect()
    });

    let floor = floor_blocks(blocks, hop);
    let floor_lufs = floor
        .as_ref()
        .and_then(|floor| floor_loudness(blocks, floor));
    let floor_bands_db = floor.as_ref().map(|floor| {
        let bands = band_energy(blocks, floor, channels);
        let total: f64 = bands.iter().sum();
        (0..10)
            .map(|b| (available[b] && total > 0.0).then(|| round2(db_power(bands[b] / total))))
            .collect()
    });

    let stereo = (channels == 2).then(|| {
        let (l, r) = (sq[0], sq[1]);
        let mid = (l + r + 2.0 * lr) / 4.0;
        let side = (l + r - 2.0 * lr) / 4.0;
        Stereo {
            balance_db: (l > 0.0 && r > 0.0).then(|| round2(db_power(l / r))),
            correlation: (l > 0.0 && r > 0.0).then(|| round3(lr / (l * r).sqrt())),
            side_to_mid_db: (mid > 0.0).then(|| round2(db_power(side.max(0.0) / mid))),
            mono_loss_db: (l + r > 0.0).then(|| round2(db_power(mid.max(0.0) / ((l + r) / 2.0)))),
        }
    });

    let active: Vec<bool> = blocks
        .iter()
        .map(|block| {
            block.frames > 0
                && (block.sq[0] + block.sq[1]) / (block.frames as f64 * channels_f)
                    > ACTIVE_MEAN_SQUARE
        })
        .collect();
    let block_seconds = hop as f64 / f64::from(rate);
    let total_seconds = frames as f64 / f64::from(rate);
    let (leading, trailing) = match (
        active.iter().position(|&a| a),
        active.iter().rposition(|&a| a),
    ) {
        (Some(first), Some(last)) => (
            first as f64 * block_seconds,
            (active.len() - 1 - last) as f64 * block_seconds,
        ),
        _ => (total_seconds, total_seconds),
    };

    Measures {
        integrated_lufs: integrated_lufs.map(round2),
        max_momentary_lufs: max_momentary.map(round2),
        max_short_term_lufs: max_short_term.map(round2),
        rms_dbfs: rms_dbfs.map(round2),
        crest_db: sample_peak_dbfs
            .zip(rms_dbfs)
            .map(|(peak, rms)| round2(peak - rms)),
        sample_peak_dbfs: sample_peak_dbfs.map(round2),
        true_peak_dbtp: db_amplitude(true_peak).map(round2),
        clipped_samples: clipped,
        first_clip: first_clip.map(|frame| positions.at(frame)),
        bands_db,
        master_share,
        floor_lufs: floor_lufs.map(round2),
        block_range_lu: block_range_lu(blocks, hop).map(round2),
        floor_bands_db,
        stereo,
        notes: None,
        lead: None,
        dynamics: None,
        active_fraction: if active.is_empty() {
            0.0
        } else {
            round3(active.iter().filter(|&&a| a).count() as f64 / active.len() as f64)
        },
        leading_silence_s: round2(leading.min(total_seconds)),
        trailing_silence_s: round2(trailing.min(total_seconds)),
    }
}

impl Measures {
    pub(super) fn is_active(&self) -> bool {
        self.active_fraction > 0.0
    }
}

impl Position {
    pub(super) fn seconds_of(frame: u64, rate: u32) -> f64 {
        round3(frame as f64 / f64::from(rate))
    }
}
