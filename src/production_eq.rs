//! Native production EQ: the pinned W3C Cookbook biquads, direct form I.

use crate::dsp::{RenderError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqMode {
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

#[derive(Clone, Debug)]
pub(crate) struct Eq {
    channels: u8,
    mode: EqMode,
    histories: [[f64; 4]; 2],
}

impl Eq {
    pub(crate) fn new(channels: u8, mode: EqMode) -> Result<Self> {
        if !(1..=2).contains(&channels) {
            return Err(RenderError::RenderState(
                "fx.eq/1 requires one or two channels".into(),
            ));
        }
        Ok(Self {
            channels,
            mode,
            histories: [[0.0; 4]; 2],
        })
    }

    pub(crate) fn reset(&mut self) {
        self.histories = [[0.0; 4]; 2];
    }

    /// Process one frame. Inapplicable q/gain arguments are finite placeholders;
    /// source and plan validation reject explicitly supplied inapplicable fields.
    pub(crate) fn process(
        &mut self,
        input: &[f64],
        frequency: f64,
        q: f64,
        gain_db: f64,
    ) -> Result<[f64; 2]> {
        if input.len() != usize::from(self.channels) {
            return Err(RenderError::RenderState(
                "fx.eq/1 input width mismatch".into(),
            ));
        }
        let [b0, b1, b2, a1, a2] = self.coefficients(frequency, q, gain_db)?;
        let mut output = [0.0; 2];
        let mut next = self.histories;
        for (channel, &sample) in input.iter().enumerate() {
            finite(sample)?;
            let [x1, x2, y1, y2] = self.histories[channel];
            // Check each product and partial sum, not just the eventual output.
            // Deliberately no mul_add or reassociation: this is the DFI order.
            let mut y = finite(b0 * sample)?;
            y = finite(y + finite(b1 * x1)?)?;
            y = finite(y + finite(b2 * x2)?)?;
            y = finite(y - finite(a1 * y1)?)?;
            y = finite(y - finite(a2 * y2)?)?;
            output[channel] = y;
            next[channel] = [sample, x1, y, y1];
        }
        // A failure in either channel leaves the entire node at its old frame.
        self.histories = next;
        Ok(output)
    }

    fn coefficients(&self, frequency: f64, q: f64, gain_db: f64) -> Result<[f64; 5]> {
        if !(frequency.is_finite()
            && 0.0 < frequency
            && frequency < 24_000.0
            && q.is_finite()
            && gain_db.is_finite())
        {
            return Err(RenderError::Nonfinite(
                "fx.eq/1 invalid frequency or nonfinite parameter".into(),
            ));
        }
        let shelf = matches!(self.mode, EqMode::LowShelf | EqMode::HighShelf);
        let pass = matches!(self.mode, EqMode::LowPass | EqMode::HighPass);
        if !shelf && !(0.1..=18.0).contains(&q) {
            return Err(RenderError::Nonfinite(
                "fx.eq/1 q is outside [0.1,18]".into(),
            ));
        }
        if !pass && !(-24.0..=24.0).contains(&gain_db) {
            return Err(RenderError::Nonfinite(
                "fx.eq/1 gain is outside [-24,24] dB".into(),
            ));
        }
        let w0 = finite(2.0 * std::f64::consts::PI * frequency / 48_000.0)?;
        let sin = finite(w0.sin())?;
        let cos = finite(w0.cos())?;
        let a = if pass {
            1.0
        } else {
            finite(10.0_f64.powf(gain_db / 40.0))?
        };
        // The shelf Cookbook expression with S=1 simplifies exactly to sqrt(2).
        let alpha = finite(if shelf {
            sin / 2.0 * 2.0_f64.sqrt()
        } else {
            sin / (2.0 * q)
        })?;
        // Every coefficient intermediate below is bounded by the validated
        // frequency/Q/gain ranges (A is in [10^-0.6,10^0.6]).
        let [b0, b1, b2, a0, a1, a2] = match self.mode {
            EqMode::Peak => [
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ],
            EqMode::LowPass => [
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ],
            EqMode::HighPass => [
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ],
            EqMode::LowShelf => {
                let t = finite(2.0 * a.sqrt() * alpha)?;
                [
                    a * ((a + 1.0) - (a - 1.0) * cos + t),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - t),
                    (a + 1.0) + (a - 1.0) * cos + t,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - t,
                ]
            }
            EqMode::HighShelf => {
                let t = finite(2.0 * a.sqrt() * alpha)?;
                [
                    a * ((a + 1.0) + (a - 1.0) * cos + t),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - t),
                    (a + 1.0) - (a - 1.0) * cos + t,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - t,
                ]
            }
        };
        for coefficient in [b0, b1, b2, a0, a1, a2] {
            finite(coefficient)?;
        }
        if a0 == 0.0 {
            return Err(RenderError::Nonfinite(
                "fx.eq/1 zero normalization denominator".into(),
            ));
        }
        Ok([
            finite(b0 / a0)?,
            finite(b1 / a0)?,
            finite(b2 / a0)?,
            finite(a1 / a0)?,
            finite(a2 / a0)?,
        ])
    }
}

fn finite(value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(RenderError::Nonfinite(
            "fx.eq/1 nonfinite arithmetic or input".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual={actual:?}, expected={expected:?}, tolerance={tolerance:?}"
        );
    }

    #[test]
    fn quarter_rate_pass_impulses_match_algebraic_fixtures() {
        // At w0=pi/2 and Q=1/2, a=[2,0,0] before normalization.
        for (mode, expected) in [
            (EqMode::LowPass, [0.25, 0.5, 0.25, 0.0]),
            (EqMode::HighPass, [0.25, -0.5, 0.25, 0.0]),
        ] {
            let mut eq = Eq::new(1, mode).unwrap();
            for (n, expected) in expected.into_iter().enumerate() {
                let out = eq
                    .process(&[if n == 0 { 1.0 } else { 0.0 }], 12000.0, 0.5, 0.0)
                    .unwrap();
                close(out[0], expected, 2e-16);
                assert_eq!(out[1], 0.0);
            }
        }
    }

    #[test]
    fn boosted_peak_impulse_matches_independent_recurrence() {
        // A=2, w0=pi/2, Q=1/2 gives b=[2,0,-2/3], a=[1,0,1/3].
        let gain = 40.0 * 2.0_f64.log10();
        let mut eq = Eq::new(1, EqMode::Peak).unwrap();
        for n in 0..18 {
            let expected = if n == 0 {
                2.0
            } else if n % 2 == 1 {
                0.0
            } else {
                (-4.0 / 3.0) * (-1.0_f64 / 3.0).powi(n / 2 - 1)
            };
            let actual = eq
                .process(&[if n == 0 { 1.0 } else { 0.0 }], 12000.0, 0.5, gain)
                .unwrap();
            close(actual[0], expected, 8e-16);
        }
    }

    #[test]
    fn all_modes_match_analytical_dc_midpoint_and_nyquist_gains() {
        // Sum impulse Fourier components at 0, pi/2, pi. Independent analog
        // endpoint/corner gains detect swapped shelves, wrong Q, and gain /20.
        for gain_sign in [-1.0, 1.0] {
            let gain = gain_sign * 40.0 * 2.0_f64.log10();
            let a = if gain_sign > 0.0 { 2.0 } else { 0.5 };
            for (mode, expected) in [
                (EqMode::Peak, [1.0, a * a, 1.0]),
                (EqMode::LowShelf, [a * a, a, 1.0]),
                (EqMode::HighShelf, [1.0, a, a * a]),
                (EqMode::LowPass, [1.0, 0.7, 0.0]),
                (EqMode::HighPass, [0.0, 0.7, 1.0]),
            ] {
                let mut eq = Eq::new(1, mode).unwrap();
                let mut dc = 0.0;
                let mut real = 0.0;
                let mut imag = 0.0;
                let mut nyquist = 0.0;
                for n in 0..512 {
                    let y = eq
                        .process(&[if n == 0 { 1.0 } else { 0.0 }], 12000.0, 0.7, gain)
                        .unwrap()[0];
                    dc += y;
                    nyquist += if n % 2 == 0 { y } else { -y };
                    match n % 4 {
                        0 => real += y,
                        1 => imag -= y,
                        2 => real -= y,
                        _ => imag += y,
                    }
                }
                for (actual, expected) in [dc.abs(), real.hypot(imag), nyquist.abs()]
                    .into_iter()
                    .zip(expected)
                {
                    close(actual, expected, 8e-14);
                }
            }
        }
    }

    #[test]
    fn zero_gain_retains_state_during_automation() {
        let mut eq = Eq::new(1, EqMode::Peak).unwrap();
        let gain = 40.0 * 2.0_f64.log10();
        // Q=1: boosted b0=1.6, b2=0, a2=.6 at pi/2.
        close(
            eq.process(&[1.0], 12000.0, 1.0, gain).unwrap()[0],
            1.6,
            1e-15,
        );
        eq.process(&[0.0], 12000.0, 1.0, 0.0).unwrap();
        // Now unity b2=a2=1/3 still sees x[n-2]=1, y[n-2]=1.6.
        close(
            eq.process(&[0.0], 12000.0, 1.0, 0.0).unwrap()[0],
            -0.2,
            1e-15,
        );
    }

    #[test]
    fn frequency_automation_uses_new_coefficients_with_old_histories() {
        let mut eq = Eq::new(1, EqMode::LowPass).unwrap();
        close(
            eq.process(&[1.0], 12000.0, 0.5, 0.0).unwrap()[0],
            0.25,
            1e-16,
        );
        let s = 0.5_f64.sqrt();
        let expected = (1.0 - s / 2.0) / (1.0 + s);
        close(
            eq.process(&[0.0], 6000.0, 0.5, 0.0).unwrap()[0],
            expected,
            3e-16,
        );
    }

    #[test]
    fn stereo_channels_are_independent_and_reset_replays() {
        for mode in [
            EqMode::Peak,
            EqMode::LowShelf,
            EqMode::HighShelf,
            EqMode::LowPass,
            EqMode::HighPass,
        ] {
            let mut stereo = Eq::new(2, mode).unwrap();
            let mut left = Eq::new(1, mode).unwrap();
            let mut right = Eq::new(1, mode).unwrap();
            let mut replay = Vec::new();
            for n in 0..64 {
                let input = [
                    if n == 0 { 1.0 } else { 0.0 },
                    if n == 7 { -0.5 } else { 0.0 },
                ];
                let frequency = 500.0 + n as f64 * 100.0;
                let frame = stereo.process(&input, frequency, 0.8, 6.0).unwrap();
                assert_eq!(
                    frame[0],
                    left.process(&input[..1], frequency, 0.8, 6.0).unwrap()[0]
                );
                assert_eq!(
                    frame[1],
                    right.process(&input[1..], frequency, 0.8, 6.0).unwrap()[0]
                );
                replay.push((input, frequency, frame));
            }
            stereo.reset();
            for (input, frequency, expected) in replay {
                assert_eq!(
                    stereo.process(&input, frequency, 0.8, 6.0).unwrap(),
                    expected
                );
            }
        }
    }

    #[test]
    fn invalid_parameters_and_frame_shapes_fail_without_state_changes() {
        for channels in [0, 3, 255] {
            assert!(Eq::new(channels, EqMode::Peak).is_err());
        }
        let mut eq = Eq::new(2, EqMode::Peak).unwrap();
        eq.process(&[0.3, -0.2], 1000.0, 1.0, 6.0).unwrap();
        let history = eq.histories;
        for input in [
            vec![],
            vec![0.0],
            vec![0.0; 3],
            vec![0.0, f64::NAN],
            vec![0.0, f64::INFINITY],
        ] {
            assert!(eq.process(&input, 1000.0, 1.0, 6.0).is_err());
            assert_eq!(eq.histories, history);
        }
        for (f, q, gain) in [
            (0.0, 1.0, 0.0),
            (-1.0, 1.0, 0.0),
            (24000.0, 1.0, 0.0),
            (f64::NAN, 1.0, 0.0),
            (1000.0, f64::INFINITY, 0.0),
            (1000.0, 0.099, 0.0),
            (1000.0, 18.001, 0.0),
            (1000.0, 1.0, -24.001),
            (1000.0, 1.0, 24.001),
            (1000.0, 1.0, f64::NAN),
        ] {
            assert!(eq.process(&[0.0, 0.0], f, q, gain).is_err());
            assert_eq!(eq.histories, history);
        }
    }

    #[test]
    fn overflow_in_second_channel_does_not_commit_first_channel() {
        let mut eq = Eq::new(2, EqMode::Peak).unwrap();
        eq.process(&[0.3, 0.2], 12000.0, 0.5, 12.0).unwrap();
        let mut control = eq.clone();
        assert!(eq.process(&[0.0, f64::MAX], 12000.0, 0.5, 24.0).is_err());
        assert_eq!(eq.histories, control.histories);
        assert_eq!(
            eq.process(&[0.0, 0.0], 1000.0, 1.0, 0.0).unwrap(),
            control.process(&[0.0, 0.0], 1000.0, 1.0, 0.0).unwrap()
        );
    }

    #[test]
    fn intermediate_overflow_is_rejected_even_when_terms_could_cancel() {
        let mut eq = Eq::new(1, EqMode::HighShelf).unwrap();
        // Seed a finite state to make the b1*x1 product overflow. The node must
        // reject that partial result even though a later feedback term offsets it.
        eq.histories[0] = [f64::MAX, 0.0, f64::MAX, 0.0];
        let old = eq.histories;
        assert!(eq.process(&[0.0], 1000.0, 1.0, 24.0).is_err());
        assert_eq!(eq.histories, old);
    }
}
