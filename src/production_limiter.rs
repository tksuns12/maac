//! Linked lookahead true-peak limiting for the 48 kHz production profile.
//!
//! The input is driven by `gain`, delayed by the lookahead `L` (the declared
//! technical latency), and multiplied by a gain that never exceeds 1. Each
//! frame's detected level combines its sample magnitudes with the four-times
//! Annex 2 interpolation used by the delivery analyzer, so inter-sample peaks
//! are seen before they reach the output.

use std::collections::VecDeque;

use crate::dsp::{RenderError, Result};
use crate::production_analysis::Interpolator;

/// The interpolated values pushed with input frame `n` are computed from input
/// frames `n - 11` through `n`. Holding the required gain over all of them
/// keeps the output's interpolation at or under the ceiling.
const SUPPORT: u64 = 11;
pub(crate) const MIN_LOOKAHEAD_FRAMES: u32 = 12;
pub(crate) const MAX_LOOKAHEAD_FRAMES: u32 = 480;

#[derive(Clone, Copy, Debug)]
pub(crate) struct LimiterParams {
    pub gain: f64,
    pub ceiling: f64,
    pub release: f64,
}

/// A pending detection: the required gain and the input frame that found it.
#[derive(Clone, Copy, Debug)]
struct Detection {
    frame: u64,
    required: f64,
}

#[derive(Debug)]
pub(crate) struct Limiter {
    channels: usize,
    lookahead: u64,
    interpolator: Interpolator,
    delay: Vec<VecDeque<f64>>,
    detections: VecDeque<Detection>,
    gain: f64,
    frame: u64,
}

impl Limiter {
    pub(crate) fn new(channels: u8, lookahead_frames: u32) -> Result<Self> {
        if !(1..=2).contains(&channels)
            || !(MIN_LOOKAHEAD_FRAMES..=MAX_LOOKAHEAD_FRAMES).contains(&lookahead_frames)
        {
            return Err(RenderError::RenderState(
                "limiter layout must be mono or stereo with a 12 to 480 frame lookahead".into(),
            ));
        }
        let lookahead = u64::from(lookahead_frames);
        Ok(Self {
            channels: usize::from(channels),
            lookahead,
            interpolator: Interpolator::default(),
            delay: (0..channels)
                .map(|_| VecDeque::from(vec![0.0; lookahead as usize]))
                .collect(),
            detections: VecDeque::new(),
            gain: 1.0,
            frame: 0,
        })
    }

    /// The bound a detection places on the gain at output frame `k`: a line
    /// from 1 at detection down to `required` when the earliest input frame of
    /// its interpolation support (`frame - 11`) reaches the output, held until
    /// `frame` does.
    fn bound(&self, detection: Detection, k: u64) -> f64 {
        let ramp = self.lookahead - SUPPORT;
        let reached = detection.frame + ramp;
        if k >= reached {
            detection.required
        } else {
            let remaining = (reached - k) as f64 / ramp as f64;
            detection.required + (1.0 - detection.required) * remaining
        }
    }

    /// Process one frame. The unused second output channel is zero for mono.
    pub(crate) fn process(&mut self, input: &[f64], params: LimiterParams) -> Result<[f64; 2]> {
        if input.len() != self.channels {
            return Err(RenderError::RenderState(
                "limiter input width differs from its immutable layout".into(),
            ));
        }
        for (name, value, minimum, maximum) in [
            ("gain", params.gain, 0.0, 24.0),
            ("ceiling", params.ceiling, -24.0, 0.0),
            ("release", params.release, 0.001, 5.0),
        ] {
            if !value.is_finite() || !(minimum..=maximum).contains(&value) {
                return Err(RenderError::Nonfinite(format!(
                    "limiter {name} is nonfinite or outside its declared range"
                )));
            }
        }
        let drive = finite(10.0_f64.powf(params.gain / 20.0), "drive")?;
        let ceiling = finite(10.0_f64.powf(params.ceiling / 20.0), "ceiling")?;
        let mut driven = [0.0; 2];
        for (destination, &sample) in driven.iter_mut().zip(input) {
            *destination = finite(finite(sample, "input")? * drive, "driven input")?;
        }

        // Detection: sample magnitudes and the interpolated inter-sample values.
        let mut level = driven
            .iter()
            .take(self.channels)
            .fold(0.0_f64, |peak, x| peak.max(x.abs()));
        let interpolated = self
            .interpolator
            .push(driven, self.channels)
            .map_err(|failure| RenderError::Nonfinite(failure.message))?;
        for phase in interpolated {
            for value in phase.iter().take(self.channels) {
                level = level.max(value.abs());
            }
        }
        if level > ceiling {
            self.detections.push_back(Detection {
                frame: self.frame,
                required: finite(ceiling / level, "required gain")?,
            });
        }

        // Output frame `k` carries input frame `k - L`; detections whose last
        // affected input frame has passed no longer constrain the gain.
        let k = self.frame;
        while self
            .detections
            .front()
            .is_some_and(|d| d.frame + self.lookahead < k)
        {
            self.detections.pop_front();
        }
        let required = self
            .detections
            .iter()
            .fold(1.0_f64, |gain, d| gain.min(self.bound(*d, k)));
        let coefficient = finite((-1.0 / (48000.0 * params.release)).exp(), "release")?;
        let released = finite(1.0 - coefficient * (1.0 - self.gain), "release envelope")?;
        let gain = required.min(released).min(1.0);

        let mut output = [0.0; 2];
        for (channel, &sample) in driven.iter().enumerate().take(self.channels) {
            self.delay[channel].push_back(sample);
            let delayed = self.delay[channel]
                .pop_front()
                .expect("the delay line holds L frames");
            // Clamping only absorbs rounding: the gain already meets the
            // ceiling for every detected sample.
            output[channel] = finite(delayed * gain, "output")?.clamp(-ceiling, ceiling);
        }
        self.gain = gain;
        self.frame += 1;
        Ok(output)
    }
}

fn finite(value: f64, context: &str) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(RenderError::Nonfinite(format!(
            "limiter {context} is nonfinite"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(gain: f64) -> LimiterParams {
        LimiterParams {
            gain,
            ceiling: -1.0,
            release: 0.1,
        }
    }

    #[test]
    fn an_impulse_is_delayed_by_the_lookahead_and_held_at_the_ceiling() {
        let mut limiter = Limiter::new(1, 72).unwrap();
        let mut out = Vec::new();
        for n in 0..200 {
            out.push(
                limiter
                    .process(&[if n == 10 { 1.0 } else { 0.0 }], params(0.0))
                    .unwrap()[0],
            );
        }
        let ceiling = 10.0_f64.powf(-1.0 / 20.0);
        let peak = out.iter().fold(0.0_f64, |m, x| m.max(x.abs()));
        let at = out.iter().position(|x| x.abs() == peak).unwrap();
        assert_eq!(at, 10 + 72);
        assert!(peak <= ceiling);
    }

    #[test]
    fn quiet_input_passes_unchanged_after_the_delay() {
        let mut limiter = Limiter::new(2, 12).unwrap();
        let input: Vec<[f64; 2]> = (0..64)
            .map(|n| [0.1 * (n as f64 * 0.3).sin(), -0.05])
            .collect();
        let out: Vec<[f64; 2]> = input
            .iter()
            .map(|frame| limiter.process(frame, params(0.0)).unwrap())
            .collect();
        for n in 12..64 {
            assert_eq!(out[n], input[n - 12]);
        }
    }

    #[test]
    fn layouts_and_parameters_are_checked() {
        assert!(Limiter::new(3, 72).is_err());
        assert!(Limiter::new(1, 11).is_err());
        assert!(Limiter::new(1, 481).is_err());
        let mut limiter = Limiter::new(1, 72).unwrap();
        assert!(limiter.process(&[0.0], params(25.0)).is_err());
        assert!(limiter.process(&[0.0, 0.0], params(0.0)).is_err());
    }
}
