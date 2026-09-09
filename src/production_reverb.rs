//! The bounded native feedback network specified by `docs/production.md`.

use crate::dsp::{RenderError, Result};
use crate::plan::PlanError;

const LENGTHS: [usize; 8] = [1493, 1601, 1747, 1867, 1999, 2137, 2281, 2437];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Ring {
    start: usize,
    length: usize,
    index: usize,
}

impl Ring {
    fn position(self) -> usize {
        self.start + self.index
    }

    fn advance(&mut self) {
        if self.length != 0 {
            self.index = (self.index + 1) % self.length;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reverb {
    channels: usize,
    damping: f64,
    history: Vec<f64>,
    preprocessing: [[Ring; 3]; 2],
    feedback: [Ring; 8],
    previous: [f64; 8],
    hadamard: [[f64; 8]; 8],
}

impl Reverb {
    pub(crate) fn new(channels: u8, predelay_frames: usize, damping: f64) -> Result<Self> {
        if !(1..=2).contains(&channels)
            || predelay_frames > 12000
            || !damping.is_finite()
            || !(0.0..=1.0).contains(&damping)
        {
            return Err(plan_error(
                "E_RANGE",
                "invalid reverb channels, predelay, or damping",
            ));
        }
        let channels = usize::from(channels);
        let cells = predelay_frames
            .checked_add(360)
            .and_then(|n| n.checked_mul(channels))
            .and_then(|n| n.checked_add(15562))
            .ok_or_else(|| plan_error("E_RESOURCE_LIMIT", "reverb storage count overflow"))?;
        // Account for the eight previous-read histories and byte conversion too.
        cells
            .checked_add(8)
            .and_then(|n| n.checked_mul(size_of::<f64>()))
            .ok_or_else(|| plan_error("E_RESOURCE_LIMIT", "reverb storage byte count overflow"))?;
        let mut history = Vec::new();
        history
            .try_reserve_exact(cells)
            .map_err(|_| plan_error("E_RESOURCE_LIMIT", "cannot allocate reverb histories"))?;
        history.resize(cells, 0.0);
        let mut cursor = 0;
        let mut make_ring = |length| {
            let ring = Ring {
                start: cursor,
                length,
                index: 0,
            };
            cursor += length;
            ring
        };
        let mut preprocessing = [[Ring::default(); 3]; 2];
        for channel in preprocessing.iter_mut().take(channels) {
            *channel = [make_ring(predelay_frames), make_ring(149), make_ring(211)];
        }
        let feedback = LENGTHS.map(&mut make_ring);
        debug_assert_eq!(cursor, cells);
        let scale = 1.0 / 8.0_f64.sqrt();
        let mut hadamard = [[0.0; 8]; 8];
        for (i, row) in hadamard.iter_mut().enumerate() {
            for (j, value) in row.iter_mut().enumerate() {
                *value = if (i & j).count_ones() % 2 == 0 {
                    scale
                } else {
                    -scale
                };
            }
        }
        Ok(Self {
            channels,
            damping,
            history,
            preprocessing,
            feedback,
            previous: [0.0; 8],
            hadamard,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.history.fill(0.0);
        self.previous.fill(0.0);
        for channel in &mut self.preprocessing {
            for ring in channel {
                ring.index = 0;
            }
        }
        for ring in &mut self.feedback {
            ring.index = 0;
        }
    }

    pub(crate) fn process(&mut self, input: &[f64], decay: f64, mix: f64) -> Result<[f64; 2]> {
        if input.len() != self.channels {
            return Err(RenderError::RenderState(
                "reverb input width does not match channels".into(),
            ));
        }
        if !decay.is_finite()
            || !(0.1..=30.0).contains(&decay)
            || !mix.is_finite()
            || !(0.0..=1.0).contains(&mix)
            || input.iter().any(|x| !x.is_finite())
        {
            return Err(RenderError::Nonfinite(
                "reverb input or parameter is outside its finite range".into(),
            ));
        }
        // Fixed candidate scratch: never copy rings or mutate any history until
        // every preprocessing, feedback, and output operation has succeeded.
        let mut prewrites = [[0.0; 3]; 2];
        let mut u = [0.0; 2];
        for c in 0..self.channels {
            let predelay = self.preprocessing[c][0];
            let mut x = if predelay.length == 0 {
                input[c]
            } else {
                finite(self.history[predelay.position()])?
            };
            prewrites[c][0] = input[c];
            for (stage, write) in prewrites[c].iter_mut().enumerate().skip(1) {
                let old = finite(self.history[self.preprocessing[c][stage].position()])?;
                let y = finite(old - finite(0.5 * x)?)?;
                *write = finite(x + finite(0.5 * y)?)?;
                x = y;
            }
            u[c] = x;
        }
        let mut reads = [0.0; 8];
        let mut filtered = [0.0; 8];
        let half_damping = self.damping / 2.0;
        for i in 0..8 {
            reads[i] = finite(self.history[self.feedback[i].position()])?;
            filtered[i] = finite(
                finite((1.0 - half_damping) * reads[i])?
                    + finite(half_damping * finite(self.previous[i])?)?,
            )?;
        }
        let mut writes = [0.0; 8];
        for i in 0..8 {
            let mut projected = 0.0;
            for (c, &value) in u.iter().enumerate().take(self.channels) {
                projected = finite(projected + finite(self.hadamard[i][c] * value)?)?;
            }
            let mut feedback = 0.0;
            for (j, &value) in filtered.iter().enumerate() {
                feedback = finite(feedback + finite(self.hadamard[i][j] * value)?)?;
            }
            let seconds = LENGTHS[i] as f64 / 48000.0;
            let gain = finite(10.0_f64.powf(-3.0 * seconds / decay))?;
            writes[i] = finite(projected + finite(gain * feedback)?)?;
        }
        let mut output = [0.0; 2];
        for c in 0..self.channels {
            let mut wet = 0.0;
            for (i, &value) in reads.iter().enumerate() {
                wet = finite(wet + finite(self.hadamard[i][c] * value)?)?;
            }
            output[c] = finite(finite((1.0 - mix) * input[c])? + finite(mix * wet)?)?;
        }
        for (c, channel) in self
            .preprocessing
            .iter_mut()
            .enumerate()
            .take(self.channels)
        {
            for (stage, ring) in channel.iter_mut().enumerate() {
                if ring.length != 0 {
                    self.history[ring.position()] = prewrites[c][stage];
                    ring.advance();
                }
            }
        }
        for (i, ring) in self.feedback.iter_mut().enumerate() {
            self.history[ring.position()] = writes[i];
            ring.advance();
        }
        self.previous = reads;
        Ok(output)
    }
}

fn finite(value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(RenderError::Nonfinite(
            "reverb intermediate or state is nonfinite".into(),
        ))
    }
}

fn plan_error(code: &str, message: &str) -> RenderError {
    RenderError::Plan(PlanError {
        code: code.into(),
        path: "reverb".into(),
        message: message.into(),
        span: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_wet_impulse_includes_predelay() {
        for predelay in [0, 1, 37, 12000] {
            let mut reverb = Reverb::new(1, predelay, 0.5).unwrap();
            for frame in 0..=1493 + predelay {
                let output = reverb
                    .process(&[if frame == 0 { 1.0 } else { 0.0 }], 1.5, 1.0)
                    .unwrap();
                if frame < 1493 + predelay {
                    assert_eq!(output, [0.0, 0.0]);
                } else {
                    assert!(
                        (output[0] - 1.0 / 32.0).abs() < 1e-16,
                        "frame {frame}: {output:?}"
                    );
                    assert_eq!(output[1], 0.0);
                }
            }
        }
    }

    #[test]
    fn stereo_projection_uses_first_two_sylvester_columns() {
        for excited_channel in 0..2 {
            let mut reverb = Reverb::new(2, 0, 0.5).unwrap();
            for frame in 0..=1601 {
                let mut input = [0.0; 2];
                if frame == 0 {
                    input[excited_channel] = 1.0;
                }
                let output = reverb.process(&input, 1.5, 1.0).unwrap();
                let expected = match frame {
                    1493 => [1.0 / 32.0; 2],
                    1601 if excited_channel == 0 => [1.0 / 32.0, -1.0 / 32.0],
                    1601 => [-1.0 / 32.0, 1.0 / 32.0],
                    _ => [0.0; 2],
                };
                for c in 0..2 {
                    assert!(
                        (output[c] - expected[c]).abs() < 1e-16,
                        "frame {frame}: {output:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn feedback_uses_old_reads_damping_and_destination_gain() {
        let mut reverb = Reverb::new(1, 0, 0.5).unwrap();
        reverb.history[reverb.feedback[1].position()] = 1.0;
        reverb.previous[1] = 0.5;
        let positions = reverb.feedback.map(Ring::position);
        let output = reverb.process(&[0.0], 2.0, 1.0).unwrap();
        assert_eq!(output[0], 1.0 / 8.0_f64.sqrt());
        for i in 0..8 {
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            let expected = 10.0_f64.powf(-3.0 * (LENGTHS[i] as f64 / 48000.0) / 2.0)
                * (sign * (1.0 / 8.0_f64.sqrt()) * 0.875);
            assert_eq!(reverb.history[positions[i]], expected);
        }
        assert_eq!(reverb.previous, [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn reset_replays_history_indices_and_parameter_changes() {
        let mut reverb = Reverb::new(2, 23, 0.8).unwrap();
        let run = |reverb: &mut Reverb| {
            (0..6000)
                .map(|n| {
                    let input = if n < 9 { [0.3, -0.2] } else { [0.0; 2] };
                    let decay = if n < 1800 { 0.7 } else { 4.0 };
                    reverb.process(&input, decay, 0.6).unwrap()
                })
                .collect::<Vec<_>>()
        };
        let first = run(&mut reverb);
        assert!(first[1800..]
            .iter()
            .any(|frame| frame.iter().any(|x| *x != 0.0)));
        reverb.reset();
        assert_eq!(reverb, Reverb::new(2, 23, 0.8).unwrap());
        assert_eq!(run(&mut reverb), first);
    }

    #[test]
    fn muted_network_receives_input_and_keeps_tail_across_automation() {
        let mut muted = Reverb::new(1, 0, 0.5).unwrap();
        let mut wet = muted.clone();
        for n in 0..1493 {
            let input = [if n == 0 { 1.0 } else { 0.0 }];
            let decay = if n < 700 { 0.1 } else { 30.0 };
            assert_eq!(muted.process(&input, decay, 0.0).unwrap()[0], input[0]);
            wet.process(&input, decay, 1.0).unwrap();
        }
        assert_eq!(muted, wet);
        let actual = muted.process(&[0.0], 3.0, 1.0).unwrap();
        assert!((actual[0] - 1.0 / 32.0).abs() < 1e-16);
        assert_eq!(actual, wet.process(&[0.0], 3.0, 1.0).unwrap());
    }

    #[test]
    fn finite_feedback_overflow_does_not_commit_any_candidate_state() {
        let mut reverb = Reverb::new(2, 7, 0.0).unwrap();
        for ring in reverb.feedback {
            reverb.history[ring.position()] = f64::MAX;
        }
        let before = reverb.clone();
        let error = reverb.process(&[0.1, 0.2], 1.5, 0.0).unwrap_err();
        assert_eq!(error.code(), "E_NONFINITE");
        assert_eq!(reverb, before);
    }

    #[test]
    fn allpass_overflow_and_bad_parameters_leave_state_unchanged() {
        let mut reverb = Reverb::new(1, 1, 0.5).unwrap();
        reverb.history[reverb.preprocessing[0][0].position()] = f64::MAX;
        reverb.history[reverb.preprocessing[0][1].position()] = -f64::MAX;
        let before = reverb.clone();
        assert!(reverb.process(&[0.2], 1.5, 0.0).is_err());
        assert_eq!(reverb, before);
        for (input, decay, mix) in [
            (f64::NAN, 1.5, 0.2),
            (0.0, f64::INFINITY, 0.2),
            (0.0, 0.09, 0.2),
            (0.0, 30.1, 0.2),
            (0.0, 1.5, -0.01),
            (0.0, 1.5, 1.01),
            (0.0, 1.5, f64::NAN),
        ] {
            assert!(reverb.process(&[input], decay, mix).is_err());
            assert_eq!(reverb, before);
        }
        assert!(reverb.process(&[0.0, 0.0], 1.5, 0.2).is_err());
        assert_eq!(reverb, before);
    }

    #[test]
    fn configuration_and_history_storage_are_bounded() {
        for channels in [0, 3, u8::MAX] {
            assert!(Reverb::new(channels, 0, 0.5).is_err());
        }
        for predelay in [12001, usize::MAX] {
            assert!(Reverb::new(1, predelay, 0.5).is_err());
        }
        for damping in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            assert!(Reverb::new(1, 0, damping).is_err());
        }
        for channels in [1, 2] {
            for predelay in [0, 12000] {
                let mut reverb = Reverb::new(channels, predelay, 1.0).unwrap();
                let expected = 15562 + usize::from(channels) * (predelay + 360) + 8;
                assert_eq!(reverb.history.len() + reverb.previous.len(), expected);
                let allocation = (reverb.history.as_ptr(), reverb.history.capacity());
                let input = vec![0.0; usize::from(channels)];
                reverb.process(&input, 0.1, 1.0).unwrap();
                reverb.reset();
                assert_eq!(
                    (reverb.history.as_ptr(), reverb.history.capacity()),
                    allocation
                );
            }
        }
    }
}
