//! `synth.delay/1`: a delay line whose length is a parameter, counted in
//! seconds or in quarter notes of score time, with feedback inside the node.

use std::sync::Arc;

use crate::dsp::{RenderError, Result};
use crate::plan::{Interpolation, OutputSettings, PlanError, Rational, TempoMap};
use num_traits::ToPrimitive;

/// The tempo map in binary64, for a delay that counts in quarter notes.
///
/// `T(q)` is the physical time of score position `q` with `T(0) = 0`, and
/// `T⁻¹` its inverse (§5.1). Before the first point and after the last, the
/// nearest point's tempo holds.
#[derive(Clone, Debug)]
pub struct TempoCurve {
    /// Each point's position, its time measured from the first point, its
    /// tempo, and the slope of its outgoing segment in bpm per quarter note
    /// (zero for a step and for the last point).
    points: Vec<(f64, f64, f64, f64)>,
    /// The time of `q = 0` measured from the first point.
    shift: f64,
    /// The engine's reset origin `O = T(score.start)` and the score end
    /// `E = T(score.end)`, in seconds.
    origin: f64,
    end: f64,
    rate: f64,
}

impl TempoCurve {
    pub fn new(tempo: &TempoMap, output: &OutputSettings) -> Result<Self> {
        let raw = tempo
            .points
            .iter()
            .map(|point| {
                Ok((
                    finite(&point.q, "tempo position")?,
                    finite(&point.bpm, "tempo")?,
                    point.shape,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        if raw.is_empty() {
            return Err(RenderError::RenderState("tempo map has no points".into()));
        }
        let mut points = Vec::with_capacity(raw.len());
        let mut time = 0.0;
        for (index, &(q, bpm, shape)) in raw.iter().enumerate() {
            let slope = match (shape, raw.get(index + 1)) {
                (Interpolation::Linear, Some(&(next_q, next_bpm, _))) => {
                    (next_bpm - bpm) / (next_q - q)
                }
                _ => 0.0,
            };
            points.push((q, time, bpm, slope));
            if let Some(&(next_q, _, _)) = raw.get(index + 1) {
                time += elapsed(next_q - q, bpm, slope);
            }
        }
        let mut curve = Self {
            points,
            shift: 0.0,
            origin: 0.0,
            end: 0.0,
            rate: f64::from(output.sample_rate_hz),
        };
        curve.shift = curve.relative_time(0.0);
        curve.origin = curve.time_at(finite(&output.score_start_q, "score start")?);
        curve.end = curve.time_at(finite(&output.score_end_q, "score end")?);
        Ok(curve)
    }

    /// The time of `q` measured from the first point.
    fn relative_time(&self, q: f64) -> f64 {
        let first = self.points[0];
        if q < first.0 {
            return first.1 + 60.0 * (q - first.0) / first.2;
        }
        let index = self.points.partition_point(|point| point.0 <= q) - 1;
        let (start, time, bpm, slope) = self.points[index];
        time + elapsed(q - start, bpm, slope)
    }

    /// `T(q)` in seconds.
    pub fn time_at(&self, q: f64) -> f64 {
        self.relative_time(q) - self.shift
    }

    /// `T⁻¹(t)` in quarter notes.
    pub fn score_at(&self, t: f64) -> f64 {
        let relative = t + self.shift;
        let first = self.points[0];
        if relative < first.1 {
            return first.0 + (relative - first.1) * first.2 / 60.0;
        }
        let index = self.points.partition_point(|point| point.1 <= relative) - 1;
        let (start, time, bpm, slope) = self.points[index];
        let seconds = relative - time;
        if slope == 0.0 {
            start + seconds * bpm / 60.0
        } else {
            start + bpm * ((slope * seconds / 60.0).exp() - 1.0) / slope
        }
    }

    /// The delay in frames at engine frame `frame` for a length of `beats`
    /// quarter notes: the input is read from the instant `beats` quarter
    /// notes earlier in score time. In the tail the length holds its value
    /// at the score end.
    pub fn delay_frames(&self, frame: u64, beats: f64) -> f64 {
        let now = (self.origin + frame as f64 / self.rate).min(self.end);
        let earlier = self.time_at(self.score_at(now) - beats);
        (now - earlier) * self.rate
    }
}

fn finite(value: &Rational, what: &str) -> Result<f64> {
    value
        .to_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| RenderError::RenderState(format!("{what} is not finite")))
}

/// Seconds elapsed over `distance` quarter notes from a tempo of `bpm`
/// changing by `slope` bpm per quarter note.
fn elapsed(distance: f64, bpm: f64, slope: f64) -> f64 {
    if slope == 0.0 {
        60.0 * distance / bpm
    } else {
        60.0 / slope * ((bpm + slope * distance) / bpm).ln()
    }
}

/// The state of one `synth.delay/1` node: the values written to its line,
/// and its last delayed value, for damping.
#[derive(Clone, Debug)]
pub struct DelayLine {
    /// Written values; frame `n` lives at `n % len`.
    buffer: Vec<[f64; 2]>,
    previous: [f64; 2],
    tempo: Option<Arc<TempoCurve>>,
}

impl DelayLine {
    /// A line that holds `max_frames` frames of history, the most a length
    /// may reach.
    pub fn new(max_frames: usize) -> Self {
        Self {
            buffer: vec![[0.0; 2]; max_frames + 2],
            previous: [0.0; 2],
            tempo: None,
        }
    }

    pub fn attach_tempo(&mut self, tempo: &Arc<TempoCurve>) {
        self.tempo = Some(Arc::clone(tempo));
    }

    /// Process engine frame `frame`: read the line `time` (seconds, or
    /// quarter notes with `score`) earlier, interpolating linearly between
    /// frames, and write `input + feedback * damped`. Returns the delayed
    /// value. Without `commit` the line is left as it was.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        frame: u64,
        rate: f64,
        input: [f64; 2],
        channels: usize,
        score: bool,
        params: [f64; 3],
        commit: bool,
    ) -> Result<[f64; 2]> {
        let [time, feedback, damping] = params;
        let max_frames = (self.buffer.len() - 2) as f64;
        let frames = if score {
            self.tempo
                .as_ref()
                .ok_or_else(|| {
                    RenderError::RenderState("a delay in quarter notes has no tempo map".into())
                })?
                .delay_frames(frame, time)
        } else {
            time * rate
        };
        if !frames.is_finite() || frames < 1.0 || frames > max_frames {
            return Err(RenderError::Plan(PlanError {
                code: "E_RANGE".into(),
                path: "synth.delay/1.time".into(),
                message: format!(
                    "delay length of {frames} frames at frame {frame} is outside one frame through its max of {max_frames}"
                ),
                span: None,
            }));
        }
        let length = self.buffer.len() as u64;
        let position = frame as f64 - frames;
        let base = position.floor();
        let fraction = position - base;
        let read = |at: f64| -> [f64; 2] {
            if at < 0.0 {
                [0.0; 2]
            } else {
                self.buffer[(at as u64 % length) as usize]
            }
        };
        let earlier = read(base);
        let later = if fraction > 0.0 {
            read(base + 1.0)
        } else {
            [0.0; 2]
        };
        let mut output = [0.0; 2];
        let mut written = [0.0; 2];
        for channel in 0..channels {
            let delayed = earlier[channel] * (1.0 - fraction) + later[channel] * fraction;
            let damped = (1.0 - damping / 2.0) * delayed + (damping / 2.0) * self.previous[channel];
            let write = input[channel] + feedback * damped;
            if !delayed.is_finite() || !write.is_finite() {
                return Err(RenderError::Nonfinite(format!(
                    "delay at frame {frame} produced a nonfinite value"
                )));
            }
            output[channel] = delayed;
            written[channel] = write;
        }
        if commit {
            self.buffer[(frame % length) as usize] = written;
            self.previous = output;
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PortRef, TempoPoint};
    use num_bigint::BigInt;

    fn rational(numerator: i64, denominator: i64) -> Rational {
        Rational::new(BigInt::from(numerator), BigInt::from(denominator))
    }

    fn output(start: i64, end: i64) -> OutputSettings {
        OutputSettings {
            score_start_q: rational(start, 1),
            score_end_q: rational(end, 1),
            tail_seconds: rational(1, 1),
            sample_rate_hz: 48_000,
            channels: 1,
            total_frames: 48_000,
            output: PortRef::new("out", "out").unwrap(),
        }
    }

    fn map(points: &[(i64, i64, Interpolation)]) -> TempoMap {
        TempoMap {
            points: points
                .iter()
                .map(|&(q, bpm, shape)| TempoPoint {
                    q: rational(q, 1),
                    bpm: rational(bpm, 1),
                    shape,
                })
                .collect(),
        }
    }

    #[test]
    fn a_steady_tempo_counts_beats_as_seconds() {
        let curve = TempoCurve::new(&map(&[(0, 120, Interpolation::Step)]), &output(0, 8)).unwrap();
        assert!((curve.time_at(4.0) - 2.0).abs() < 1e-12);
        assert!((curve.score_at(2.0) - 4.0).abs() < 1e-12);
        // A dotted eighth at 120 bpm is 0.375 s, 18000 frames.
        assert!((curve.delay_frames(96_000, 0.75) - 18_000.0).abs() < 1e-6);
    }

    #[test]
    fn a_ramp_inverts_and_an_echo_lands_a_fixed_number_of_beats_later() {
        let curve = TempoCurve::new(
            &map(&[
                (0, 60, Interpolation::Linear),
                (8, 180, Interpolation::Step),
            ]),
            &output(0, 8),
        )
        .unwrap();
        for q in [0.5, 1.0, 3.25, 7.9, 9.0, -1.0] {
            let t = curve.time_at(q);
            assert!((curve.score_at(t) - q).abs() < 1e-9, "{q}");
        }
        // Read at score position 5 for an echo of one beat: the input was
        // written at score position 4.
        let now = curve.time_at(5.0);
        let frame = (now * 48_000.0).round() as u64;
        let frames = curve.delay_frames(frame, 1.0);
        let source = curve.score_at(frame as f64 / 48_000.0 - frames / 48_000.0);
        assert!((source - (curve.score_at(frame as f64 / 48_000.0) - 1.0)).abs() < 1e-9);
    }

    #[test]
    fn a_line_delays_interpolates_and_feeds_back() {
        let mut line = DelayLine::new(16);
        let mut out = Vec::new();
        for frame in 0..8 {
            let input = if frame == 0 { [1.0, 0.0] } else { [0.0; 2] };
            let delayed = line
                .process(
                    frame,
                    48_000.0,
                    input,
                    1,
                    false,
                    [2.5 / 48_000.0, 0.5, 0.0],
                    true,
                )
                .unwrap();
            out.push(delayed[0]);
        }
        // An impulse 2.5 frames late lands half at frame 2 and half at 3. Fed
        // back at half strength, it returns 2.5 frames later, interpolated a
        // second time: 0.5 × [0.25, 0.5, 0.25] from frame 4, plus the start
        // of its own next return at frame 6.
        assert_eq!(&out[..4], &[0.0, 0.0, 0.5, 0.5]);
        assert_eq!(&out[4..7], &[0.125, 0.25, 0.15625]);
        let error = DelayLine::new(4)
            .process(
                0,
                48_000.0,
                [0.0; 2],
                1,
                false,
                [5.0 / 48_000.0, 0.0, 0.0],
                true,
            )
            .unwrap_err();
        assert!(matches!(error, RenderError::Plan(ref plan) if plan.code == "E_RANGE"));
    }
}
