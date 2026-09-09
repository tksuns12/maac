//! Linked feedforward sample-peak compression for the 48 kHz production profile.

use crate::dsp::{RenderError, Result};

#[derive(Clone, Copy, Debug)]
pub(crate) struct CompressorParams {
    pub threshold: f64,
    pub ratio: f64,
    pub knee: f64,
    pub attack: f64,
    pub release: f64,
    pub makeup: f64,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self {
            threshold: -18.0,
            ratio: 4.0,
            knee: 6.0,
            attack: 0.01,
            release: 0.1,
            makeup: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Compressor {
    channels: u8,
    sidechain_channels: Option<u8>,
    reduction: f64,
}

impl Compressor {
    pub(crate) fn new(channels: u8, sidechain_channels: Option<u8>) -> Result<Self> {
        if !(1..=2).contains(&channels)
            || sidechain_channels.is_some_and(|count| !(1..=2).contains(&count))
        {
            return Err(RenderError::RenderState(
                "compressor main and sidechain layouts must be mono or stereo".into(),
            ));
        }
        Ok(Self {
            channels,
            sidechain_channels,
            reduction: 0.0,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.reduction = 0.0;
    }

    /// Process one frame, retaining reduction only after every result is finite.
    /// The unused second output channel is zero for mono nodes.
    pub(crate) fn process(
        &mut self,
        input: &[f64],
        sidechain: Option<&[f64]>,
        params: CompressorParams,
    ) -> Result<[f64; 2]> {
        if input.len() != usize::from(self.channels) {
            return Err(RenderError::RenderState(
                "compressor main input width differs from its immutable layout".into(),
            ));
        }
        let detector =
            match (self.sidechain_channels, sidechain) {
                (None, None) => input,
                (Some(count), Some(frame)) if frame.len() == usize::from(count) => frame,
                _ => return Err(RenderError::RenderState(
                    "compressor sidechain presence or width differs from its immutable detector"
                        .into(),
                )),
            };
        for (name, value, minimum, maximum) in [
            ("threshold", params.threshold, -120.0, 24.0),
            ("ratio", params.ratio, 1.0, 100.0),
            ("knee", params.knee, 0.0, 24.0),
            ("attack", params.attack, 0.0, 10.0),
            ("release", params.release, 0.0, 30.0),
            ("makeup", params.makeup, -24.0, 24.0),
        ] {
            if !value.is_finite() || !(minimum..=maximum).contains(&value) {
                return Err(RenderError::Nonfinite(format!(
                    "compressor {name} is nonfinite or outside its declared range"
                )));
            }
        }
        finite(self.reduction, "retained gain reduction")?;
        for &sample in input {
            finite(sample, "main input")?;
        }
        let mut peak = 0.0_f64;
        for &sample in detector {
            peak = peak.max(finite(sample, "detector input")?.abs());
        }
        let desired = if peak == 0.0 {
            // Digital silence is a zero target; never evaluate log10(0).
            0.0
        } else {
            let level = finite(20.0 * peak.log10(), "detector level")?;
            let delta = finite(level - params.threshold, "level above threshold")?;
            let slope = 1.0 - 1.0 / params.ratio;
            let half_knee = params.knee / 2.0;
            if params.knee == 0.0 {
                finite(slope * delta, "hard knee reduction")?.max(0.0)
            } else if delta <= -half_knee {
                0.0
            } else if delta >= half_knee {
                finite(slope * delta, "above knee reduction")?
            } else {
                let offset = finite(delta + half_knee, "knee offset")?;
                let square = finite(offset * offset, "squared knee offset")?;
                let numerator = finite(slope * square, "knee numerator")?;
                finite(numerator / (2.0 * params.knee), "soft knee reduction")?
            }
        };
        let time = if desired > self.reduction {
            params.attack
        } else {
            params.release
        };
        let next = if time == 0.0 {
            desired
        } else {
            let frames = finite(48000.0 * time, "time constant in frames")?;
            let exponent = finite(-1.0 / frames, "time constant exponent")?;
            let coefficient = finite(exponent.exp(), "smoothing coefficient")?;
            let retained = finite(coefficient * self.reduction, "retained smoothing term")?;
            let driven = finite((1.0 - coefficient) * desired, "driven smoothing term")?;
            finite(retained + driven, "smoothed reduction")?
        };
        let gain_db = finite(params.makeup - next, "output gain in decibels")?;
        let gain = finite(10.0_f64.powf(gain_db / 20.0), "linear output gain")?;
        let mut output = [0.0; 2];
        for (destination, &sample) in output.iter_mut().zip(input) {
            *destination = finite(sample * gain, "output")?;
        }
        self.reduction = next;
        Ok(output)
    }
}

fn finite(value: f64, name: &str) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(RenderError::Nonfinite(format!(
            "compressor {name} is nonfinite"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!(
            actual.is_finite() && (actual - expected).abs() < 2e-11,
            "actual={actual}, expected={expected}"
        );
    }

    fn instantaneous() -> CompressorParams {
        CompressorParams {
            attack: 0.0,
            release: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn independent_hard_and_soft_knee_reductions() {
        // Analytic reductions at threshold and the two knee boundaries.
        for (level, knee, expected) in [
            (-24.0, 0.0, 0.0),
            (-18.0, 0.0, 0.0),
            (-6.0, 0.0, 9.0),
            (-21.0, 6.0, 0.0),
            (-18.0, 6.0, 0.5625),
            (-15.0, 6.0, 2.25),
        ] {
            let mut compressor = Compressor::new(1, Some(1)).unwrap();
            let output = compressor
                .process(
                    &[1.0],
                    Some(&[10.0_f64.powf(level / 20.0)]),
                    CompressorParams {
                        knee,
                        ..instantaneous()
                    },
                )
                .unwrap();
            close(-20.0 * output[0].log10(), expected);
            assert_eq!(output[1], 0.0);
        }
    }

    #[test]
    fn attack_and_release_are_efolding_times() {
        let params = CompressorParams {
            threshold: -24.0,
            ratio: 2.0,
            knee: 0.0,
            ..Default::default()
        };
        let mut compressor = Compressor::new(1, Some(1)).unwrap();
        let mut output = [0.0; 2];
        for _ in 0..480 {
            output = compressor.process(&[1.0], Some(&[1.0]), params).unwrap();
        }
        // 12 * (1 - exp(-1)), independent of the sample recurrence.
        close(-20.0 * output[0].log10(), 7.585446705942692);
        // Set exactly 12 dB reduction, then release to zero over 4800 frames.
        compressor
            .process(
                &[1.0],
                Some(&[1.0]),
                CompressorParams {
                    attack: 0.0,
                    ..params
                },
            )
            .unwrap();
        for _ in 0..4800 {
            output = compressor.process(&[1.0], Some(&[0.0]), params).unwrap();
        }
        close(-20.0 * output[0].log10(), 4.414553294057308);
        let released = compressor
            .process(
                &[1.0],
                Some(&[0.0]),
                CompressorParams {
                    release: 0.0,
                    ..params
                },
            )
            .unwrap();
        assert_eq!(released, [1.0, 0.0]);
    }

    #[test]
    fn internal_peak_links_main_channels_and_sign() {
        let mut compressor = Compressor::new(2, None).unwrap();
        let output = compressor
            .process(
                &[-1.0, 0.25],
                None,
                CompressorParams {
                    threshold: -24.0,
                    ratio: 2.0,
                    knee: 0.0,
                    ..instantaneous()
                },
            )
            .unwrap();
        close(output[0], -0.251188643150958);
        close(output[1], 0.0627971607877395);
        close(output[0] / output[1], -4.0);
    }

    #[test]
    fn external_stereo_detector_is_linked_and_never_audible() {
        let params = CompressorParams {
            threshold: -24.0,
            ratio: 2.0,
            knee: 0.0,
            ..instantaneous()
        };
        let mut compressor = Compressor::new(2, Some(2)).unwrap();
        let output = compressor
            .process(&[1.0, -0.5], Some(&[0.1, -1.0]), params)
            .unwrap();
        close(output[0], 0.251188643150958);
        close(output[1], -0.125594321575479);
        assert_eq!(
            compressor
                .process(&[0.0, 0.0], Some(&[1.0, 1.0]), params)
                .unwrap(),
            [0.0, 0.0]
        );
        // A silent sidechain does not fall back to the loud main input.
        assert_eq!(
            compressor
                .process(&[1.0, -0.5], Some(&[0.0, 0.0]), params)
                .unwrap(),
            [1.0, -0.5]
        );
    }

    #[test]
    fn automation_retains_state_and_reset_replays() {
        let mut compressor = Compressor::new(1, Some(1)).unwrap();
        let params = CompressorParams {
            threshold: -24.0,
            ratio: 2.0,
            knee: 0.0,
            ..Default::default()
        };
        compressor
            .process(
                &[1.0],
                Some(&[1.0]),
                CompressorParams {
                    attack: 0.0,
                    ..params
                },
            )
            .unwrap();
        let automated = compressor
            .process(
                &[1.0],
                Some(&[1.0]),
                CompressorParams {
                    ratio: 1.0,
                    ..params
                },
            )
            .unwrap();
        close(
            -20.0 * automated[0].log10(),
            12.0 * (-1.0_f64 / 4800.0).exp(),
        );
        compressor.reset();
        let first = compressor.process(&[0.8], Some(&[0.75]), params).unwrap();
        compressor.process(&[0.2], Some(&[1.0]), params).unwrap();
        compressor.reset();
        assert_eq!(
            compressor.process(&[0.8], Some(&[0.75]), params).unwrap(),
            first
        );
    }

    #[test]
    fn unity_ratio_and_makeup_do_not_apply_automatic_gain() {
        let mut compressor = Compressor::new(1, None).unwrap();
        let output = compressor
            .process(
                &[0.5],
                None,
                CompressorParams {
                    ratio: 1.0,
                    makeup: 6.020599913279624,
                    ..instantaneous()
                },
            )
            .unwrap();
        close(output[0], 1.0);
        assert_eq!(
            compressor.process(&[0.0], None, instantaneous()).unwrap(),
            [0.0, 0.0]
        );
    }

    #[test]
    fn immutable_layout_and_sidechain_presence_are_enforced() {
        for channels in [0, 3, 255] {
            assert!(Compressor::new(channels, None).is_err());
        }
        for channels in [0, 3, 255] {
            assert!(Compressor::new(1, Some(channels)).is_err());
        }
        let params = instantaneous();
        let mut internal = Compressor::new(1, None).unwrap();
        assert!(internal.process(&[], None, params).is_err());
        assert!(internal.process(&[1.0, 1.0], None, params).is_err());
        assert!(internal.process(&[1.0], Some(&[1.0]), params).is_err());
        let mut external = Compressor::new(1, Some(2)).unwrap();
        assert!(external.process(&[1.0], None, params).is_err());
        assert!(external.process(&[1.0], Some(&[1.0]), params).is_err());
    }

    #[test]
    fn invalid_parameters_and_nonfinite_audio_do_not_commit_state() {
        let mut compressor = Compressor::new(1, Some(1)).unwrap();
        let valid = CompressorParams {
            threshold: -24.0,
            ratio: 2.0,
            knee: 0.0,
            ..Default::default()
        };
        compressor.process(&[1.0], Some(&[1.0]), valid).unwrap();
        let before = compressor.reduction;
        let invalid = [
            CompressorParams {
                threshold: -121.0,
                ..valid
            },
            CompressorParams {
                threshold: 25.0,
                ..valid
            },
            CompressorParams {
                ratio: 0.5,
                ..valid
            },
            CompressorParams {
                ratio: 101.0,
                ..valid
            },
            CompressorParams {
                knee: -1.0,
                ..valid
            },
            CompressorParams {
                knee: 25.0,
                ..valid
            },
            CompressorParams {
                attack: -1.0,
                ..valid
            },
            CompressorParams {
                attack: 11.0,
                ..valid
            },
            CompressorParams {
                release: -1.0,
                ..valid
            },
            CompressorParams {
                release: 31.0,
                ..valid
            },
            CompressorParams {
                makeup: -25.0,
                ..valid
            },
            CompressorParams {
                makeup: 25.0,
                ..valid
            },
            CompressorParams {
                threshold: f64::NAN,
                ..valid
            },
            CompressorParams {
                ratio: f64::INFINITY,
                ..valid
            },
            CompressorParams {
                knee: f64::NEG_INFINITY,
                ..valid
            },
            CompressorParams {
                attack: f64::NAN,
                ..valid
            },
            CompressorParams {
                release: f64::INFINITY,
                ..valid
            },
            CompressorParams {
                makeup: f64::NAN,
                ..valid
            },
        ];
        for params in invalid {
            assert!(compressor.process(&[1.0], Some(&[1.0]), params).is_err());
            assert_eq!(compressor.reduction, before);
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(compressor.process(&[bad], Some(&[1.0]), valid).is_err());
            assert!(compressor.process(&[1.0], Some(&[bad]), valid).is_err());
            assert_eq!(compressor.reduction, before);
        }
    }

    #[test]
    fn bounds_are_inclusive_and_nonfinite_intermediates_fail_transactionally() {
        let mut compressor = Compressor::new(1, Some(1)).unwrap();
        for params in [
            CompressorParams {
                threshold: -120.0,
                ratio: 1.0,
                knee: 0.0,
                attack: 0.0,
                release: 0.0,
                makeup: -24.0,
            },
            CompressorParams {
                threshold: 24.0,
                ratio: 100.0,
                knee: 24.0,
                attack: 10.0,
                release: 30.0,
                makeup: 24.0,
            },
        ] {
            assert!(compressor.process(&[0.01], Some(&[1.0]), params).is_ok());
        }
        let before = compressor.reduction;
        // Positive subnormal seconds are in range, but the prescribed reciprocal
        // overflows. The finite-intermediate contract forbids silently treating
        // an infinite exponent as the special zero-time case.
        let params = CompressorParams {
            attack: f64::from_bits(1),
            ..Default::default()
        };
        assert!(compressor.process(&[1.0], Some(&[1.0]), params).is_err());
        assert_eq!(compressor.reduction, before);
    }

    #[test]
    fn output_overflow_does_not_commit_reduction() {
        let mut compressor = Compressor::new(1, Some(1)).unwrap();
        let params = CompressorParams {
            threshold: -24.0,
            ratio: 2.0,
            knee: 0.0,
            ..instantaneous()
        };
        compressor.process(&[1.0], Some(&[1.0]), params).unwrap();
        let before = compressor.reduction;
        let overflowing = CompressorParams {
            makeup: 24.0,
            ..params
        };
        assert!(compressor
            .process(&[f64::MAX], Some(&[0.0]), overflowing)
            .is_err());
        assert_eq!(compressor.reduction, before);
    }
}
