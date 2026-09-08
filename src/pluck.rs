//! The bounded, deterministic string recurrence specified in docs/plucked-string.md.

use crate::dsp::{RenderError, Result};
use crate::plan::PlanError;

pub(crate) const DELAY_CELLS: usize = 2402;

// Runtime graph cloning is used only for shared reset state, where Pluck is
// forbidden. Voice construction uses the fallible constructor below.
#[derive(Clone, Debug)]
pub(crate) struct Pluck {
    ring: Vec<f64>,
    write: usize,
    previous: f64,
    cached: Option<([u64; 3], Coefficients)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Coefficients {
    delay: usize,
    fraction: f64,
    current_weight: f64,
    previous_weight: f64,
    gain: f64,
}

impl Pluck {
    pub(crate) fn new(seed: u32) -> Result<Self> {
        if seed == 0 {
            return Err(RenderError::Plan(PlanError {
                code: "E_RANGE".into(),
                path: "pluck.seed".into(),
                message: "pluck seed must be nonzero".into(),
                span: None,
            }));
        }
        let mut ring = allocate_ring(DELAY_CELLS)?;
        let mut state = seed;
        for _ in 0..DELAY_CELLS {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            ring.push(state as f64 / 2147483648.0 - 1.0);
        }
        Ok(Self {
            ring,
            write: 0,
            previous: 0.0,
            cached: None,
        })
    }

    pub(crate) fn sample(
        &mut self,
        frequency_hz: f64,
        decay: f64,
        damping: f64,
        level: f64,
    ) -> Result<f64> {
        if !frequency_hz.is_finite()
            || !(20.0..=4000.0).contains(&frequency_hz)
            || !decay.is_finite()
            || !(0.05..=30.0).contains(&decay)
            || !damping.is_finite()
            || !(0.0..=1.0).contains(&damping)
            || !level.is_finite()
            || !(0.0..=16.0).contains(&level)
        {
            return Err(RenderError::Nonfinite(
                "pluck frequency or parameter is outside its finite range".into(),
            ));
        }
        let key = [frequency_hz.to_bits(), decay.to_bits(), damping.to_bits()];
        let coefficients = match self.cached {
            Some((cached_key, coefficients)) if key == cached_key => coefficients,
            _ => Coefficients::new(frequency_hz, decay, damping)?,
        };
        let output = self.advance(coefficients, level)?;
        self.cached = Some((key, coefficients));
        Ok(output)
    }

    fn advance(&mut self, coefficients: Coefficients, level: f64) -> Result<f64> {
        let i0 = (self.write + DELAY_CELLS - coefficients.delay) % DELAY_CELLS;
        let i1 = (self.write + DELAY_CELLS - coefficients.delay - 1) % DELAY_CELLS;
        let value =
            (1.0 - coefficients.fraction) * self.ring[i0] + coefficients.fraction * self.ring[i1];
        let filtered =
            coefficients.current_weight * value + coefficients.previous_weight * self.previous;
        let next = coefficients.gain * filtered;
        let output = value * level;
        if !value.is_finite() || !filtered.is_finite() || !next.is_finite() || !output.is_finite() {
            return Err(RenderError::Nonfinite(
                "pluck string state or output is nonfinite".into(),
            ));
        }
        self.ring[self.write] = next;
        self.previous = value;
        self.write = (self.write + 1) % DELAY_CELLS;
        Ok(output)
    }
}

impl Coefficients {
    fn new(frequency_hz: f64, decay: f64, damping: f64) -> Result<Self> {
        let omega = (2.0 * std::f64::consts::PI * frequency_hz) / 48000.0;
        let previous_weight = damping / 2.0;
        let current_weight = 1.0 - previous_weight;
        let phi =
            (previous_weight * omega.sin()).atan2(current_weight + previous_weight * omega.cos());
        let delay = 48000.0 / frequency_hz - phi / omega;
        let integer = delay.floor();
        let alpha = delay - integer;
        let theta = alpha * omega;
        let fraction = if alpha == 0.0 {
            0.0
        } else {
            theta.sin() / ((omega - theta).sin() + theta.sin())
        };
        let gain = 10.0_f64.powf(-3.0 / (frequency_hz * decay));
        if [
            omega,
            previous_weight,
            current_weight,
            phi,
            delay,
            integer,
            alpha,
            theta,
            fraction,
            gain,
        ]
        .iter()
        .any(|value| !value.is_finite())
            || !(11.0..=2400.0).contains(&integer)
            || !(0.0..=1.0).contains(&fraction)
            || gain <= 0.0
            || gain >= 1.0
        {
            return Err(RenderError::Nonfinite(
                "pluck coefficient is outside its finite range".into(),
            ));
        }
        Ok(Self {
            delay: integer as usize,
            fraction,
            current_weight,
            previous_weight,
            gain,
        })
    }
}

fn allocate_ring(cells: usize) -> Result<Vec<f64>> {
    let mut ring = Vec::new();
    ring.try_reserve_exact(cells).map_err(|_| {
        RenderError::Plan(PlanError {
            code: "E_RESOURCE_LIMIT".into(),
            path: "pluck.ring".into(),
            message: "could not allocate bounded pluck delay storage".into(),
            span: None,
        })
    })?;
    Ok(ring)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_maximum_seed_vectors_are_fixed() {
        for (seed, expected) in [
            (
                1831565813,
                [
                    0.5940741715021431,
                    -0.2197297173552215,
                    -0.10352158639580011,
                    0.8362207105383277,
                    0.4697931264527142,
                    0.5322304172441363,
                ],
            ),
            (
                u32::MAX,
                [
                    0.5725153642706573,
                    -0.26355752535164356,
                    -0.7724201465025544,
                    0.5895145367830992,
                    0.7064992054365575,
                    -0.9517689226195216,
                ],
            ),
        ] {
            let mut pluck = Pluck::new(seed).unwrap();
            for value in expected {
                assert_eq!(pluck.sample(480.0, 3.0, 0.0, 1.0).unwrap(), value);
            }
        }
    }

    #[test]
    fn synthetic_ring_proves_fractional_indices_and_read_before_write() {
        let mut pluck = Pluck::new(1).unwrap();
        pluck.ring.fill(0.0);
        pluck.write = DELAY_CELLS - 1;
        pluck.ring[DELAY_CELLS - 12] = 0.75;
        pluck.ring[DELAY_CELLS - 13] = -0.25;
        pluck.ring[DELAY_CELLS - 1] = 0.9;
        pluck.previous = -0.5;
        let coefficients = Coefficients {
            delay: 11,
            fraction: 0.25,
            current_weight: 0.75,
            previous_weight: 0.25,
            gain: 0.5,
        };
        // Read = .75*.75 + .25*(-.25) = .5; filtered = .75*.5-.25*.5 = .25.
        assert_eq!(pluck.advance(coefficients, 2.0).unwrap(), 1.0);
        assert_eq!(pluck.ring[DELAY_CELLS - 1], 0.125);
        assert_eq!(pluck.previous, 0.5);
        assert_eq!(pluck.write, 0);
        assert_eq!(pluck.ring[DELAY_CELLS - 12], 0.75);
    }

    #[test]
    fn phase_oracle_covers_guitar_semitones_bends_and_frequency_endpoints() {
        let frequencies = (40..=88)
            .flat_map(|midi| {
                [-2.0, 0.0, 2.0]
                    .map(move |bend| 440.0 * 2.0_f64.powf((midi as f64 + bend - 69.0) / 12.0))
            })
            .chain([20.0, 4000.0]);
        for frequency in frequencies {
            for damping in [0.0, 0.5, 1.0] {
                let c = Coefficients::new(frequency, 3.0, damping).unwrap();
                let omega = std::f64::consts::TAU * frequency / 48000.0;
                // Independently evaluate phases of the actual two FIR transfer functions.
                let loss_phase = (c.previous_weight * omega.sin())
                    .atan2(c.current_weight + c.previous_weight * omega.cos());
                let interpolation_phase =
                    (c.fraction * omega.sin()).atan2(1.0 - c.fraction + c.fraction * omega.cos());
                let total = c.delay as f64 * omega + loss_phase + interpolation_phase;
                assert!(
                    (total - std::f64::consts::TAU).abs() < 1e-12,
                    "f={frequency} damping={damping} phase={total}"
                );
            }
        }
    }

    #[test]
    fn independently_derived_recurrence_matches_through_live_delay_crossings() {
        let mut pluck = Pluck::new(1).unwrap();
        // Initialization and tuning oracle intentionally do not call production helpers.
        let mut seed: u64 = 1;
        let mut history: Vec<_> = (0..DELAY_CELLS)
            .map(|_| {
                seed ^= (seed << 13) & 0xffff_ffff;
                seed ^= seed >> 17;
                seed ^= (seed << 5) & 0xffff_ffff;
                seed as f64 / 2147483648.0 - 1.0
            })
            .collect();
        let mut previous = 0.0;
        for frame in 0..6000 {
            let frequency = 220.0 * 2.0_f64.powf((frame as f64 / 3000.0 - 1.0) / 6.0);
            let damping = if frame < 2000 {
                0.0
            } else if frame < 4000 {
                0.5
            } else {
                1.0
            };
            let decay = if frame < 3000 { 3.0 } else { 0.8 };
            let omega = std::f64::consts::TAU * frequency / 48000.0;
            let q = damping * 0.5;
            let loss_phase = (q * omega.sin() / (1.0 - q + q * omega.cos())).atan();
            let real_delay = (std::f64::consts::TAU - loss_phase) / omega;
            let integer = real_delay.floor() as usize;
            let tangent = ((real_delay - integer as f64) * omega).tan();
            let fraction = tangent / (omega.sin() + tangent * (1.0 - omega.cos()));
            let write = frame % DELAY_CELLS;
            let read = (1.0 - fraction) * history[(write + DELAY_CELLS - integer) % DELAY_CELLS]
                + fraction * history[(write + DELAY_CELLS - integer - 1) % DELAY_CELLS];
            history[write] =
                10.0_f64.powf(-3.0 / (frequency * decay)) * ((1.0 - q) * read + q * previous);
            previous = read;
            assert!(
                (pluck.sample(frequency, decay, damping, 1.0).unwrap() - read).abs() < 2e-12,
                "frame {frame}"
            );
        }
    }

    #[test]
    fn invalid_values_do_not_update_ring_scalar_state_or_cache() {
        assert_eq!(Pluck::new(0).unwrap_err().code(), "E_RANGE");
        let mut pluck = Pluck::new(1).unwrap();
        pluck.sample(440.0, 3.0, 0.5, 1.0).unwrap();
        let snapshot = pluck.clone();
        for args in [
            [19.999, 3.0, 0.5, 1.0],
            [4000.001, 3.0, 0.5, 1.0],
            [f64::NAN, 3.0, 0.5, 0.0],
            [440.0, 0.049, 0.5, 1.0],
            [440.0, 30.001, 0.5, 1.0],
            [440.0, f64::INFINITY, 0.5, 1.0],
            [440.0, 3.0, -0.01, 1.0],
            [440.0, 3.0, 1.01, 1.0],
            [440.0, 3.0, f64::NAN, 1.0],
            [440.0, 3.0, 0.5, -0.1],
            [440.0, 3.0, 0.5, 16.001],
            [440.0, 3.0, 0.5, f64::INFINITY],
        ] {
            assert_eq!(
                pluck
                    .sample(args[0], args[1], args[2], args[3])
                    .unwrap_err()
                    .code(),
                "E_NONFINITE"
            );
            assert_eq!(pluck.ring, snapshot.ring);
            assert_eq!(pluck.write, snapshot.write);
            assert_eq!(pluck.previous, snapshot.previous);
            assert_eq!(pluck.cached, snapshot.cached);
        }
        assert_eq!(
            allocate_ring(usize::MAX).unwrap_err().code(),
            "E_RESOURCE_LIMIT"
        );
    }

    #[test]
    fn coefficient_cache_and_recomputation_have_identical_samples() {
        let mut cached = Pluck::new(1).unwrap();
        let mut uncached = Pluck::new(1).unwrap();
        for frame in 0..3000 {
            let f = if frame < 1000 {
                82.4068892282175
            } else {
                1318.5102276514797
            };
            let damping = if frame % 37 == 0 { 0.0 } else { 0.5 };
            uncached.cached = None;
            assert_eq!(
                cached.sample(f, 3.0, damping, 1.0).unwrap(),
                uncached.sample(f, 3.0, damping, 1.0).unwrap()
            );
        }
    }

    #[test]
    fn separate_loss_filter_attenuation_increases_with_damping() {
        for frequency in [100.0, 1000.0, 4000.0, 12000.0, 23000.0] {
            let omega = std::f64::consts::TAU * frequency / 48000.0;
            let mut last = 1.0;
            for step in 0..=100 {
                let b = step as f64 / 200.0;
                let power = (1.0 - b + b * omega.cos()).powi(2) + (b * omega.sin()).powi(2);
                assert!(power <= last + 1e-15);
                last = power;
            }
        }
    }

    #[test]
    fn worst_case_changing_coefficients_remain_bounded_without_reallocating() {
        let start = std::time::Instant::now();
        let mut pluck = Pluck::new(u32::MAX).unwrap();
        let pointer = pluck.ring.as_ptr();
        let capacity = pluck.ring.capacity();
        let mut random = 7u32;
        for frame in 0..96000 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let fraction = random as f64 / u32::MAX as f64;
            let frequency = if frame % 100 == 0 {
                20.0
            } else if frame % 100 == 1 {
                4000.0
            } else {
                20.0 + fraction * 3980.0
            };
            let damping = if frame % 2 == 0 { 0.0 } else { 1.0 };
            let decay = if frame % 3 == 0 { 0.05 } else { 30.0 };
            let value = pluck.sample(frequency, decay, damping, 16.0).unwrap();
            assert!(value.is_finite() && value.abs() <= 16.0 + 1e-12);
            if frame % 1000 == 0 {
                assert!(pluck
                    .ring
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() <= 1.0 + 1e-12));
            }
        }
        assert_eq!(pluck.ring.as_ptr(), pointer);
        assert_eq!(pluck.ring.capacity(), capacity);
        assert_eq!(pluck.ring.len(), DELAY_CELLS);
        eprintln!(
            "pluck worst-case coefficient benchmark: 96000 frames in {:?}",
            start.elapsed()
        );
    }

    fn spectral_power(samples: &[f64], frequency: f64) -> f64 {
        let step = std::f64::consts::TAU * frequency / 48000.0;
        let mut real = 0.0;
        let mut imaginary = 0.0;
        for (index, &sample) in samples.iter().enumerate() {
            let window = 0.5
                - 0.5 * (std::f64::consts::TAU * index as f64 / (samples.len() - 1) as f64).cos();
            real += sample * window * (step * index as f64).cos();
            imaginary += sample * window * (step * index as f64).sin();
        }
        real * real + imaginary * imaginary
    }

    #[test]
    fn default_low_middle_and_high_string_pitches_are_measured_within_two_cents() {
        // Independent estimator: Hann-windowed direct Fourier peak scan, .25-cent
        // steps in a +/-10-cent band. Samples begin100ms after initial excitation.
        for midi in [40, 64, 88] {
            let expected = 440.0 * 2.0_f64.powf((midi as f64 - 69.0) / 12.0);
            let mut pluck = Pluck::new(1831565813).unwrap();
            for _ in 0..4800 {
                pluck.sample(expected, 3.0, 0.5, 1.0).unwrap();
            }
            let samples: Vec<_> = (0..16384)
                .map(|_| pluck.sample(expected, 3.0, 0.5, 1.0).unwrap())
                .collect();
            let (cents, _) = (-40..=40)
                .map(|step| {
                    let cents = step as f64 / 4.0;
                    (
                        cents,
                        spectral_power(&samples, expected * 2.0_f64.powf(cents / 1200.0)),
                    )
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            eprintln!("pluck measured MIDI{midi} ({expected:.6} Hz): {cents:+.2} cents");
            assert!(cents.abs() <= 2.0, "MIDI{midi}: {cents} cents");
        }
    }

    #[test]
    fn upper_partials_decay_faster_than_lower_partials() {
        let mut pluck = Pluck::new(1831565813).unwrap();
        let samples: Vec<_> = (0..32000)
            .map(|_| pluck.sample(110.0, 3.0, 0.5, 1.0).unwrap())
            .collect();
        let ratio = |window: &[f64]| {
            let low: f64 = (1..=3)
                .map(|partial| spectral_power(window, 110.0 * partial as f64))
                .sum();
            let high: f64 = (8..=12)
                .map(|partial| spectral_power(window, 110.0 * partial as f64))
                .sum();
            high / low
        };
        let early = ratio(&samples[1024..5120]);
        let late = ratio(&samples[24000..28096]);
        eprintln!("pluck upper/lower partial energy: early={early:.6}, late={late:.6}");
        assert!(late < early * 0.8, "upper partials did not decay faster");
    }

    #[test]
    fn seed_one_reference_samples_and_first_recirculation_are_fixed() {
        let mut pluck = Pluck::new(1).unwrap();
        let expected = [
            0.9930807766504586,
            0.26170715782791376,
            -0.9504082570783794,
            0.9612405328080058,
            -0.7509497245773673,
            -0.03659906983375549,
        ];
        for value in expected {
            assert_eq!(pluck.sample(480.0, 3.0, 0.0, 1.0).unwrap(), value);
        }
        for _ in 6..100 {
            pluck.sample(480.0, 3.0, 0.0, 1.0).unwrap();
        }
        let expected = expected[0] * 0.9952144352021834;
        assert!((pluck.sample(480.0, 3.0, 0.0, 1.0).unwrap() - expected).abs() < 1e-15);
    }

    #[test]
    fn independently_initialized_instances_replay_and_level_mute_does_not_pause() {
        let mut reference = Pluck::new(1831565813).unwrap();
        let mut muted = Pluck::new(1831565813).unwrap();
        let mut replay = Pluck::new(1831565813).unwrap();
        for frame in 0..800 {
            let expected = reference.sample(440.0, 3.0, 0.5, 1.0).unwrap();
            assert_eq!(replay.sample(440.0, 3.0, 0.5, 1.0).unwrap(), expected);
            let actual = muted
                .sample(440.0, 3.0, 0.5, if frame < 300 { 0.0 } else { 1.0 })
                .unwrap();
            if frame < 300 {
                assert_eq!(actual, 0.0);
            } else {
                assert_eq!(actual, expected);
            }
        }
    }
}
