//! The modal resonator bank specified in docs/instruments.md#modal-bank.

use crate::dsp::{RenderError, Result};

pub const MAX_MODES: u8 = 64;
pub const MAX_STRINGS: u8 = 3;
const RATE_HZ: f64 = 48_000.0;
/// Modes at or above this frequency contribute nothing.
pub const MODE_CEILING_HZ: f64 = 20_000.0;

/// One mode of one string: the complex state `s` and its coefficients.
#[derive(Clone, Copy, Debug, Default)]
struct Resonator {
    re: f64,
    im: f64,
    pole_re: f64,
    pole_im: f64,
    weight: f64,
}

/// The parameters that set a bank's coefficients, after `ratio` is applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ModalShape {
    pub frequency_hz: f64,
    pub stretch: f64,
    pub decay: f64,
    pub damping: f64,
    pub position: f64,
    pub detune_cents: f64,
}

impl ModalShape {
    fn key(&self) -> [u64; 6] {
        [
            self.frequency_hz.to_bits(),
            self.stretch.to_bits(),
            self.decay.to_bits(),
            self.damping.to_bits(),
            self.position.to_bits(),
            self.detune_cents.to_bits(),
        ]
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Modal {
    modes: usize,
    strings: usize,
    /// Mode-major: mode `n` of string `k` is at `n * strings + k`.
    resonators: Vec<Resonator>,
    /// Resonators below the ceiling; the rest have zero state and weight.
    active: Vec<bool>,
    cached: Option<[u64; 6]>,
}

impl Modal {
    pub(crate) fn new(modes: u8, strings: u8) -> Result<Self> {
        if !(1..=MAX_MODES).contains(&modes) || !(1..=MAX_STRINGS).contains(&strings) {
            return Err(RenderError::RenderState(format!(
                "modal bank needs 1..={MAX_MODES} modes and 1..={MAX_STRINGS} strings"
            )));
        }
        let count = usize::from(modes) * usize::from(strings);
        let mut resonators = Vec::new();
        resonators
            .try_reserve_exact(count)
            .map_err(|_| RenderError::RenderState("modal bank state allocation failed".into()))?;
        resonators.resize(count, Resonator::default());
        let mut active = Vec::new();
        active
            .try_reserve_exact(count)
            .map_err(|_| RenderError::RenderState("modal bank state allocation failed".into()))?;
        active.resize(count, false);
        Ok(Self {
            modes: usize::from(modes),
            strings: usize::from(strings),
            resonators,
            active,
            cached: None,
        })
    }

    /// Recompute the poles and weights when the shape changed. A resonator
    /// that crosses the ceiling loses its state.
    fn prepare(&mut self, shape: &ModalShape) -> Result<()> {
        let key = shape.key();
        if self.cached == Some(key) {
            return Ok(());
        }
        if !shape.frequency_hz.is_finite() || shape.frequency_hz <= 0.0 {
            return Err(RenderError::Nonfinite(format!(
                "modal frequency {} Hz must be positive and finite",
                shape.frequency_hz
            )));
        }
        let centre = (self.strings as f64 - 1.0) / 2.0;
        for mode in 0..self.modes {
            let n = (mode + 1) as f64;
            let base = n * shape.frequency_hz * (1.0 + shape.stretch * n * n).sqrt();
            let time = shape.decay / (1.0 + shape.damping * (n - 1.0));
            let radius = 10f64.powf(-3.0 / (time * RATE_HZ));
            let weight = if shape.position == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * n * shape.position).sin()
            };
            for string in 0..self.strings {
                let cents = (string as f64 - centre) * shape.detune_cents;
                let frequency = base * 2f64.powf(cents / 1200.0);
                let index = mode * self.strings + string;
                let resonator = &mut self.resonators[index];
                if !frequency.is_finite() || !radius.is_finite() || !weight.is_finite() {
                    return Err(RenderError::Nonfinite(
                        "modal coefficients are nonfinite".into(),
                    ));
                }
                if frequency >= MODE_CEILING_HZ {
                    *resonator = Resonator::default();
                    self.active[index] = false;
                    continue;
                }
                let omega = std::f64::consts::TAU * frequency / RATE_HZ;
                resonator.pole_re = radius * omega.cos();
                resonator.pole_im = radius * omega.sin();
                resonator.weight = weight;
                self.active[index] = true;
            }
        }
        self.cached = Some(key);
        Ok(())
    }

    /// Advance every resonator by one sample of `input` and return
    /// `level × Σ Im(s) / strings`.
    pub(crate) fn sample(&mut self, shape: &ModalShape, level: f64, input: f64) -> Result<f64> {
        self.prepare(shape)?;
        let mut sum = 0.0;
        for (resonator, &active) in self.resonators.iter_mut().zip(&self.active) {
            if !active {
                continue;
            }
            let (re, im) = step(resonator, input);
            resonator.re = re;
            resonator.im = im;
            sum += im;
        }
        self.finish(sum, level)
    }

    /// The output `sample` would return, without advancing the state.
    pub(crate) fn preview(&self, shape: &ModalShape, level: f64, input: f64) -> Result<f64> {
        let mut bank = self.clone();
        bank.sample(shape, level, input)
    }

    fn finish(&self, sum: f64, level: f64) -> Result<f64> {
        let output = level * sum / self.strings as f64;
        if !output.is_finite() {
            return Err(RenderError::Nonfinite("modal output is nonfinite".into()));
        }
        Ok(output)
    }

    /// Modes and strings below the ceiling, for tests.
    #[cfg(test)]
    fn active_count(&self) -> usize {
        self.active.iter().filter(|&&active| active).count()
    }
}

/// `s ← R e^{iω} s + w x`.
fn step(resonator: &Resonator, input: f64) -> (f64, f64) {
    (
        resonator.pole_re * resonator.re - resonator.pole_im * resonator.im
            + resonator.weight * input,
        resonator.pole_re * resonator.im + resonator.pole_im * resonator.re,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(frequency_hz: f64) -> ModalShape {
        ModalShape {
            frequency_hz,
            stretch: 0.0,
            decay: 2.0,
            damping: 0.0,
            position: 0.0,
            detune_cents: 0.0,
        }
    }

    /// The impulse response of one isolated resonator.
    fn impulse(modes: u8, strings: u8, shape: &ModalShape, frames: usize) -> Vec<f64> {
        let mut bank = Modal::new(modes, strings).unwrap();
        (0..frames)
            .map(|frame| {
                bank.sample(shape, 1.0, if frame == 0 { 1.0 } else { 0.0 })
                    .unwrap()
            })
            .collect()
    }

    /// The frequency of a decaying sinusoid from its zero crossings.
    fn crossing_frequency(samples: &[f64]) -> f64 {
        let crossings: Vec<f64> = samples
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[0] <= 0.0 && pair[1] > 0.0)
            .map(|(index, pair)| index as f64 + pair[0] / (pair[0] - pair[1]))
            .collect();
        let periods = (crossings.len() - 1) as f64;
        RATE_HZ * periods / (crossings[crossings.len() - 1] - crossings[0])
    }

    #[test]
    fn one_mode_rings_at_its_frequency_and_decays_60_db_in_its_time() {
        let samples = impulse(1, 1, &shape(441.0), 96_000);
        assert_eq!(samples[0], 0.0);
        let cents = 1200.0 * (crossing_frequency(&samples[..48_000]) / 441.0).log2();
        assert!(cents.abs() < 0.01, "{cents}");
        // The envelope |s| after T seconds is 10^-3 exactly.
        let mut bank = Modal::new(1, 1).unwrap();
        bank.sample(&shape(441.0), 1.0, 1.0).unwrap();
        for _ in 1..96_000 {
            bank.sample(&shape(441.0), 1.0, 0.0).unwrap();
        }
        let resonator = bank.resonators[0];
        let magnitude = resonator.re.hypot(resonator.im);
        let expected = 10f64.powf(-3.0 * 95_999.0 / 96_000.0);
        assert!((magnitude / expected - 1.0).abs() < 1e-9, "{magnitude}");
    }

    #[test]
    fn stretched_modes_sit_at_n_f_root_one_plus_b_n_squared() {
        let mut stretched = shape(110.0);
        stretched.stretch = 1.0 / 400.0;
        let bank = {
            let mut bank = Modal::new(8, 1).unwrap();
            bank.prepare(&stretched).unwrap();
            bank
        };
        for (mode, resonator) in bank.resonators.iter().enumerate() {
            let n = (mode + 1) as f64;
            let expected = n * 110.0 * (1.0 + n * n / 400.0).sqrt();
            let measured =
                resonator.pole_im.atan2(resonator.pole_re) * RATE_HZ / std::f64::consts::TAU;
            let cents = 1200.0 * (measured / expected).log2();
            assert!(cents.abs() < 0.1, "mode {n}: {cents} cents");
        }
    }

    #[test]
    fn damping_shortens_higher_modes() {
        let mut damped = shape(200.0);
        damped.damping = 1.0;
        let mut bank = Modal::new(4, 1).unwrap();
        bank.prepare(&damped).unwrap();
        for (mode, resonator) in bank.resonators.iter().enumerate() {
            let time = 2.0 / (1.0 + mode as f64);
            let radius = resonator.pole_re.hypot(resonator.pole_im);
            let measured = -3.0 / (RATE_HZ * radius.log10());
            assert!((measured / time - 1.0).abs() < 1e-9, "{measured} {time}");
        }
    }

    #[test]
    fn modes_at_or_above_20_khz_are_absent() {
        let mut bank = Modal::new(64, 1).unwrap();
        bank.prepare(&shape(1_000.0)).unwrap();
        assert_eq!(bank.active_count(), 19);
        bank.prepare(&shape(5_000.0)).unwrap();
        assert_eq!(bank.active_count(), 3);
        // A note above the ceiling is silent, not an error.
        let silent = impulse(4, 2, &shape(21_000.0), 64);
        assert!(silent.iter().all(|&sample| sample == 0.0));
    }

    #[test]
    fn position_removes_the_modes_at_its_nodes() {
        let mut struck = shape(100.0);
        struck.position = 1.0 / 4.0;
        let mut bank = Modal::new(8, 1).unwrap();
        bank.prepare(&struck).unwrap();
        for (mode, resonator) in bank.resonators.iter().enumerate() {
            let n = mode + 1;
            if n % 4 == 0 {
                assert!(resonator.weight.abs() < 1e-15, "mode {n}");
            } else {
                assert!(resonator.weight.abs() > 0.38, "mode {n}");
            }
        }
    }

    #[test]
    fn detuned_strings_spread_symmetrically_in_cents() {
        let mut detuned = shape(300.0);
        detuned.detune_cents = 4.0;
        let mut bank = Modal::new(1, 3).unwrap();
        bank.prepare(&detuned).unwrap();
        for (string, offset) in [-4.0, 0.0, 4.0].iter().enumerate() {
            let pole = bank.resonators[string];
            let measured = pole.pole_im.atan2(pole.pole_re) * RATE_HZ / std::f64::consts::TAU;
            let cents = 1200.0 * (measured / 300.0).log2();
            assert!((cents - offset).abs() < 1e-6, "{cents}");
        }
    }

    #[test]
    fn a_pitch_sweep_every_sample_stays_finite_and_bounded() {
        let mut bank = Modal::new(64, 3).unwrap();
        let mut sweep = shape(50.0);
        sweep.stretch = 1.0 / 100.0;
        sweep.detune_cents = 3.0;
        sweep.decay = 30.0;
        let mut peak: f64 = 0.0;
        for frame in 0..48_000 {
            sweep.frequency_hz = 50.0 * 2f64.powf(8.0 * frame as f64 / 48_000.0);
            let input = if frame < 96 { 1.0 } else { 0.0 };
            let output = bank.sample(&sweep, 1.0, input).unwrap();
            peak = peak.max(output.abs());
        }
        // The bound: every resonator's magnitude is at most the sum of its
        // weighted inputs, 96 here.
        assert!(peak.is_finite() && peak <= 96.0 * 64.0, "{peak}");
    }

    #[test]
    fn preview_matches_sample_without_advancing() {
        let mut bank = Modal::new(6, 2).unwrap();
        let mut ringing = shape(220.0);
        ringing.detune_cents = 2.0;
        bank.sample(&ringing, 1.0, 1.0).unwrap();
        let before = bank.resonators.clone();
        let preview = bank.preview(&ringing, 0.5, 0.25).unwrap();
        assert_eq!(bank.resonators.len(), before.len());
        for (after, before) in bank.resonators.iter().zip(&before) {
            assert_eq!((after.re, after.im), (before.re, before.im));
        }
        assert_eq!(preview, bank.sample(&ringing, 0.5, 0.25).unwrap());
    }
}
