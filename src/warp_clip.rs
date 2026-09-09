//! Certified musical warp recipe admission and physical scheduling.

use crate::{
    audio_clip::EnvelopeSettings,
    plan::{OutputSettings, PlanError, Rational},
    plan_v3::TimingContext,
    plan_v6::WarpClip,
    tempo::TimeValue,
};
use num_traits::{Signed, ToPrimitive, Zero};
use std::cmp::Ordering;

fn error(code: &str, message: &str) -> PlanError {
    PlanError {
        code: code.into(),
        path: "warp".into(),
        message: message.into(),
        span: None,
    }
}

struct Recipe {
    start: TimeValue,
    finish: TimeValue,
    clipped_finish: TimeValue,
    start_frame: u64,
    end_frame: u64,
}

fn recipe(
    clip: &WarpClip,
    context: &TimingContext,
    output: &OutputSettings,
    asset_frames: u64,
) -> Result<Recipe, PlanError> {
    if output.sample_rate_hz == 0
        || clip.source_start_frame >= clip.source_end_frame
        || clip.source_end_frame > asset_frames
    {
        return Err(error(
            "E_RANGE",
            "sample rate and source slice must be valid",
        ));
    }
    if !(2..=4096).contains(&clip.warp.len()) {
        return Err(error(
            if clip.warp.len() > 4096 {
                "E_RESOURCE_LIMIT"
            } else {
                "E_RANGE"
            },
            "warp requires two to 4096 anchors",
        ));
    }
    for value in [
        &clip.at_q,
        &clip.gain,
        &clip.fade_in_seconds,
        &clip.fade_out_seconds,
    ]
    .into_iter()
    .chain(clip.warp.iter().map(|anchor| &anchor.q))
    {
        if value.numer().bits() > 4096 || value.denom().bits() > 4096 {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "warp rational input exceeds bit allowance",
            ));
        }
    }
    EnvelopeSettings::new(
        &clip.gain,
        [&clip.fade_in_seconds, &clip.fade_out_seconds],
        clip.fade_shape,
    )?;
    let first = &clip.warp[0];
    let last = &clip.warp[clip.warp.len() - 1];
    if !first.q.is_zero()
        || first.source_frame != clip.source_start_frame
        || last.q <= Rational::zero()
        || last.source_frame != clip.source_end_frame
        || clip
            .warp
            .windows(2)
            .any(|pair| pair[0].q >= pair[1].q || pair[0].source_frame >= pair[1].source_frame)
    {
        return Err(error("E_RANGE", "warp anchors must strictly increase from (0, source start) to (positive length, source end)"));
    }
    let zero = Rational::zero();
    let start = context.relative_at_score(&clip.at_q, &zero)?;
    let zero_time =
        TimeValue::from_rational(zero.clone()).map_err(|e| error(e.code.as_str(), &e.message))?;
    if context.compare_times(&start, &zero_time)? == Ordering::Less
        || context.compare_times(&start, &context.end)? != Ordering::Less
    {
        return Err(error(
            "E_INTERVAL",
            "warp clip must start within the physical score interval",
        ));
    }
    // Two bounded source rationals form this bounded derived score position.
    // The clock independently enforces its supported exact-position allowance.
    let finish_q = &clip.at_q + &last.q;
    let finish = context.relative_at_score(&finish_q, &zero)?;
    let clipped_finish = if context.compare_times(&finish, &context.duration)? == Ordering::Greater
    {
        context.duration.clone()
    } else {
        finish.clone()
    };
    let rate = u64::from(output.sample_rate_hz);
    let start_frame = context
        .ceil_time(&start, rate)?
        .to_u64()
        .ok_or_else(|| error("E_RESOURCE_LIMIT", "warp frame bound exceeds u64"))?;
    let end_frame = context
        .ceil_time(&clipped_finish, rate)?
        .to_u64()
        .ok_or_else(|| error("E_RESOURCE_LIMIT", "warp frame bound exceeds u64"))?;
    Ok(Recipe {
        start,
        finish,
        clipped_finish,
        start_frame,
        end_frame,
    })
}

/// Validate the musical recipe and schedule it without trusting stored frames.
/// Timing comparisons and rounding use the caller's shared clock budget.
pub(crate) fn schedule_clip(
    clip: &WarpClip,
    context: &TimingContext,
    output: &OutputSettings,
    asset_frames: u64,
) -> Result<(u64, u64), PlanError> {
    let recipe = recipe(clip, context, output, asset_frames)?;
    Ok((recipe.start_frame, recipe.end_frame))
}

fn music_error(e: crate::music::MusicError) -> PlanError {
    error(e.code.as_str(), &e.message)
}
fn bounded(value: Rational) -> Result<Rational, PlanError> {
    if value.numer().bits() > 32768 || value.denom().bits() > 32768 {
        Err(error(
            "E_RESOURCE_LIMIT",
            "warp derived rational exceeds bit allowance",
        ))
    } else {
        Ok(value)
    }
}
fn integer(n: u64) -> Rational {
    Rational::from_integer(n.into())
}
fn time(value: Rational) -> Result<TimeValue, PlanError> {
    TimeValue::from_rational(integer(1))
        .and_then(|t| t.scaled(&value))
        .map_err(music_error)
}
fn scale(t: &TimeValue, r: &Rational) -> Result<TimeValue, PlanError> {
    t.scaled(r).map_err(music_error)
}
fn difference(a: &TimeValue, b: &TimeValue) -> Result<TimeValue, PlanError> {
    a.difference(b).map_err(music_error)
}
fn finite(value: f64) -> Result<f64, PlanError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(error(
            "E_NONFINITE",
            "warp preparation produced a nonfinite value",
        ))
    }
}
fn positive(value: f64, required: bool) -> Result<f64, PlanError> {
    finite(value)?;
    if value < 0. || (required && value == 0.) {
        Err(error(
            "E_TIME_PRECISION",
            "warp positive value lost precision",
        ))
    } else {
        Ok(value)
    }
}
fn approximate(context: &TimingContext, value: &TimeValue) -> Result<f64, PlanError> {
    let sign = context.compare_times(value, &time(Rational::zero())?)?;
    if sign == Ordering::Equal {
        return Ok(0.);
    }
    let value = finite(context.approximate_time(value)?)?;
    if value == 0. || (value > 0.) != (sign == Ordering::Greater) {
        return Err(error(
            "E_TIME_PRECISION",
            "warp nonzero value lost sign or underflowed",
        ));
    }
    Ok(value)
}
fn exprel(z: f64) -> f64 {
    if z == 0. {
        1.
    } else {
        z.exp_m1() / z
    }
}

struct FractionRecipe<'a> {
    b: &'a Rational,
    s: &'a Rational,
    dq: &'a Rational,
    duration: &'a TimeValue,
}
struct Fractions {
    first: f64,
    advance: f64,
    dz: f64,
}
fn fractions(
    context: &TimingContext,
    r: FractionRecipe<'_>,
    elapsed: &TimeValue,
    advance_time: &Rational,
) -> Result<Fractions, PlanError> {
    let sixty = integer(60);
    let zfactor = bounded(r.s / &sixty)?;
    let zfirst = approximate(context, &scale(elapsed, &zfactor)?)?;
    let dz = approximate(context, &time(bounded(advance_time * &zfactor)?)?)?;
    let last_time = elapsed
        .sum(&time(advance_time.clone())?)
        .map_err(music_error)?;
    let zlast = approximate(context, &scale(&last_time, &zfactor)?)?;
    let zspan = approximate(context, &scale(r.duration, &zfactor)?)?;
    let elapsed_positive =
        context.compare_times(elapsed, &time(Rational::zero())?)? == Ordering::Greater;
    let last_before_end = context.compare_times(&last_time, r.duration)? == Ordering::Less;
    if !r.s.is_zero() {
        let valid = if r.s > &Rational::zero() {
            zfirst >= 0.
                && zlast >= zfirst
                && zlast <= zspan
                && (!last_before_end || zlast < zspan)
                && (!elapsed_positive || zfirst > 0.)
        } else {
            zfirst <= 0.
                && zlast <= zfirst
                && zlast >= zspan
                && (!last_before_end || zlast > zspan)
                && (!elapsed_positive || zfirst < 0.)
        };
        if !valid {
            return Err(error(
                "E_TIME_PRECISION",
                "warp exponential endpoints lost strict ordering",
            ));
        }
    }
    let delta = bounded(bounded(r.s * r.dq)? / r.b)?;
    let (first, advance) = if delta.abs() <= Rational::new(1.into(), 2.into()) {
        let factor = bounded(r.b / bounded(&sixty * r.dq)?)?;
        let pfirst = approximate(context, &scale(elapsed, &factor)?)?;
        let dp = approximate(context, &time(bounded(advance_time * &factor)?)?)?;
        (pfirst * exprel(zfirst), dp * zfirst.exp() * exprel(dz))
    } else if r.s > &Rational::zero() {
        let denominator = -(-zspan).exp_m1();
        (
            (zfirst - zspan).exp() * (-(-zfirst).exp_m1()) / denominator,
            (zlast - zspan).exp() * (-(-dz).exp_m1()) / denominator,
        )
    } else {
        let denominator = zspan.exp_m1();
        (
            zfirst.exp_m1() / denominator,
            zfirst.exp() * dz.exp_m1() / denominator,
        )
    };
    Ok(Fractions {
        first: positive(first, elapsed_positive)?,
        advance: positive(advance, !advance_time.is_zero())?,
        dz,
    })
}

#[derive(Debug)]
struct PreparedSpan {
    start: u64,
    end: u64,
    first: f64,
    advance: f64,
    dz: f64,
}
impl PreparedSpan {
    fn value(&self, frame: u64) -> f64 {
        if frame == self.start || self.end - self.start == 1 {
            return self.first;
        }
        if frame == self.end - 1 {
            return self.first + self.advance;
        }
        let f = (frame - self.start) as f64 / (self.end - self.start - 1) as f64;
        let g = if self.dz.abs() <= 0.5 {
            f * exprel(self.dz * f) / exprel(self.dz)
        } else if self.dz > 0. {
            (self.dz * (f - 1.)).exp() * (-(-self.dz * f).exp_m1()) / (-(-self.dz).exp_m1())
        } else {
            (self.dz * f).exp_m1() / self.dz.exp_m1()
        };
        self.first + self.advance * g
    }
}
#[derive(Debug)]
pub(crate) struct PreparedWarpClip {
    pub(crate) start_frame: u64,
    pub(crate) end_frame: u64,
    spans: Vec<PreparedSpan>,
    envelope: crate::audio_clip::PreparedEnvelope,
}
impl PreparedWarpClip {
    pub(crate) fn coordinate(&self, frame: u64) -> Option<(usize, f64)> {
        if frame < self.start_frame || frame >= self.end_frame {
            return None;
        }
        let span = &self.spans[self.spans.partition_point(|span| span.end <= frame)];
        let u = span.value(frame);
        let index = u.floor() as usize;
        Some((index, u - index as f64))
    }
    pub(crate) fn gain_at(&self, frame: u64) -> f64 {
        self.envelope.gain_at(frame)
    }
}

pub(crate) fn prepare_clip(
    clip: &WarpClip,
    context: &TimingContext,
    output: &OutputSettings,
    asset_frames: u64,
) -> Result<PreparedWarpClip, PlanError> {
    let recipe = recipe(clip, context, output, asset_frames)?;
    if (clip.start_frame, clip.end_frame) != (recipe.start_frame, recipe.end_frame) {
        return Err(error(
            "E_INTERVAL",
            "stored warp frame bounds differ from certified schedule",
        ));
    }
    let settings = EnvelopeSettings::new(
        &clip.gain,
        [&clip.fade_in_seconds, &clip.fade_out_seconds],
        clip.fade_shape,
    )?;
    let envelope = crate::audio_clip::prepare_envelope(
        settings,
        crate::audio_clip::EnvelopeSpan {
            start: &recipe.start,
            finish: &recipe.finish,
            clipped_finish: &recipe.clipped_finish,
            start_frame: recipe.start_frame,
            end_frame: recipe.end_frame,
            rate_hz: output.sample_rate_hz,
        },
        context,
    )?;
    let absolute: Vec<_> = clip
        .warp
        .iter()
        .map(|anchor| bounded(&clip.at_q + &anchor.q))
        .collect::<Result<_, _>>()?;
    // Merge in score space, never deduplicate rounded frame boundaries.
    let mut cuts = Vec::with_capacity(absolute.len() + context.map.points().len());
    let (mut wi, mut ti) = (0, 0);
    let points = context.map.points();
    while wi < absolute.len() || ti < points.len() {
        let use_warp =
            ti == points.len() || (wi < absolute.len() && absolute[wi] <= points[ti].position_q);
        let q = if use_warp {
            let q = absolute[wi].clone();
            wi += 1;
            q
        } else {
            let q = points[ti].position_q.clone();
            ti += 1;
            q
        };
        if q >= absolute[0] && q <= absolute[absolute.len() - 1] && cuts.last() != Some(&q) {
            cuts.push(q);
        }
    }
    let rate = integer(u64::from(output.sample_rate_hz));
    let mut spans = Vec::new();
    // Keep one canonical prefix per actual tempo interval. Warp-only cuts
    // must not split logarithms into algebraically equal but distinct recipes.
    let active = points.partition_point(|point| point.position_q <= clip.at_q);
    let mut base_q = if active > 0 {
        context
            .origin
            .clone()
            .max(points[active - 1].position_q.clone())
    } else {
        context.origin.clone()
    };
    let mut base_time = context.relative_at_score(&base_q, &Rational::zero())?;
    let mut anchor = 0;
    for cut in cuts.windows(2) {
        let (x, y) = (&cut[0], &cut[1]);
        let start_time = base_time
            .sum(
                &context
                    .map
                    .seconds_between(&base_q, x)
                    .map_err(music_error)?,
            )
            .map_err(music_error)?;
        if context.compare_times(&start_time, &recipe.clipped_finish)? != Ordering::Less {
            break;
        }
        let (b, s, next_tempo_q) = context.map.segment_at(x).map_err(music_error)?;
        let dq = bounded(y - x)?;
        while anchor + 1 < absolute.len() - 1 && absolute[anchor + 1] <= *x {
            anchor += 1;
        }
        let source_slope = bounded(
            integer(clip.warp[anchor + 1].source_frame - clip.warp[anchor].source_frame)
                / bounded(&absolute[anchor + 1] - &absolute[anchor])?,
        )?;
        let a = bounded(
            integer(clip.warp[anchor].source_frame - clip.source_start_frame)
                + bounded(&source_slope * bounded(x - &absolute[anchor])?)?,
        )?;
        let h = bounded(&source_slope * &dq)?;
        let duration = context.map.seconds_between(x, y).map_err(music_error)?;
        let finish_time = base_time
            .sum(
                &context
                    .map
                    .seconds_between(&base_q, y)
                    .map_err(music_error)?,
            )
            .map_err(music_error)?;
        let clipped =
            if context.compare_times(&finish_time, &recipe.clipped_finish)? == Ordering::Greater {
                &recipe.clipped_finish
            } else {
                &finish_time
            };
        let start = context
            .ceil_time(&start_time, u64::from(output.sample_rate_hz))?
            .to_u64()
            .ok_or_else(|| error("E_RESOURCE_LIMIT", "warp frame bound exceeds u64"))?;
        let end = context
            .ceil_time(clipped, u64::from(output.sample_rate_hz))?
            .to_u64()
            .ok_or_else(|| error("E_RESOURCE_LIMIT", "warp frame bound exceeds u64"))?;
        if start < end {
            let elapsed = difference(&time(integer(start) / &rate)?, &start_time)?;
            let residual = difference(&finish_time, &time(integer(end - 1) / &rate)?)?;
            if context.compare_times(&elapsed, &time(Rational::zero())?)? == Ordering::Less
                || context.compare_times(&residual, &time(Rational::zero())?)? != Ordering::Greater
            {
                return Err(error(
                    "E_TIME_PRECISION",
                    "warp sample endpoint is outside its exact span",
                ));
            }
            let advance_time = integer(end - start - 1) / &rate;
            let f = fractions(
                context,
                FractionRecipe {
                    b: &b,
                    s: &s,
                    dq: &dq,
                    duration: &duration,
                },
                &elapsed,
                &advance_time,
            )?;
            let by = bounded(&b + bounded(&s * &dq)?)?;
            let backwards = fractions(
                context,
                FractionRecipe {
                    b: &by,
                    s: &(-&s),
                    dq: &dq,
                    duration: &duration,
                },
                &residual,
                &Rational::zero(),
            )?;
            let last_fraction = finite(f.first + f.advance)?;
            if f.first >= 1. || last_fraction >= 1. || last_fraction < f.first {
                return Err(error(
                    "E_TIME_PRECISION",
                    "warp fraction escaped its half-open domain",
                ));
            }
            let af = approximate(context, &time(a.clone())?)?;
            let hf = approximate(context, &time(h.clone())?)?;
            let right_gap = positive(hf * backwards.first, true)?;
            let progress = positive(hf * f.first, f.first > 0.)?;
            let first = finite(af + progress)?;
            let advance = positive(hf * f.advance, start + 1 < end)?;
            let last = finite(first + advance)?;
            let upper = approximate(context, &time(bounded(a + h)?)?)?;
            let backward_last = finite(upper - right_gap)?;
            let overall = (clip.source_end_frame - clip.source_start_frame) as f64;
            if backward_last < af
                || backward_last >= upper
                || first < af
                || first >= upper
                || last >= upper
                || last >= overall
                || (progress > 0. && first <= af)
                || (start + 1 < end && last <= first)
            {
                return Err(error(
                    "E_TIME_PRECISION",
                    "warp source endpoint lost representable progress",
                ));
            }
            spans.push(PreparedSpan {
                start,
                end,
                first,
                advance,
                dz: f.dz,
            });
        }
        if end == recipe.end_frame {
            break;
        }
        if next_tempo_q.as_ref() == Some(y) {
            base_q = y.clone();
            base_time = finish_time;
        }
    }
    let mut covered = recipe.start_frame;
    for span in &spans {
        if span.start != covered || span.start >= span.end {
            return Err(error(
                "E_TIME_PRECISION",
                "prepared warp spans do not cover the certified interval",
            ));
        }
        covered = span.end;
    }
    if covered != recipe.end_frame {
        return Err(error(
            "E_TIME_PRECISION",
            "prepared warp spans leave an uncovered frame",
        ));
    }
    Ok(PreparedWarpClip {
        start_frame: recipe.start_frame,
        end_frame: recipe.end_frame,
        spans,
        envelope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Interpolation, PlanLimits, TempoMap, TempoPoint};
    use serde_json::json;
    fn r(n: i64, d: i64) -> Rational {
        Rational::new(n.into(), d.into())
    }
    fn output() -> OutputSettings {
        serde_json::from_value(json!({"score_start_q":"0/1","score_end_q":"1/1","tail_seconds":"1/1","sample_rate_hz":48000,"channels":1,"total_frames":96000,"output":{"node":"clip","port":"out"}})).unwrap()
    }
    fn clip() -> WarpClip {
        serde_json::from_value(json!({"asset":"a","channels":1,"at_q":"1/192000","source_start_frame":0,"source_end_frame":3,"warp":[{"q":"0/1","source_frame":0},{"q":"1/16000","source_frame":3}],"gain":"1/2","fade_in_seconds":"0/1","fade_out_seconds":"0/1","fade_shape":"linear","source":{"object":"clip","path":["clip"]},"start_frame":999,"end_frame":999})).unwrap()
    }
    fn context(out: &OutputSettings, ramp: bool, work: u64) -> TimingContext {
        TimingContext::new_with_limits(
            &TempoMap {
                points: vec![
                    TempoPoint {
                        q: r(0, 1),
                        bpm: r(60, 1),
                        shape: if ramp {
                            Interpolation::Linear
                        } else {
                            Interpolation::Step
                        },
                    },
                    TempoPoint {
                        q: r(1, 1),
                        bpm: r(120, 1),
                        shape: Interpolation::Step,
                    },
                ],
            },
            out,
            &PlanLimits {
                max_work: work,
                ..PlanLimits::default()
            },
        )
        .unwrap()
    }
    fn prepare_with(
        mut c: WarpClip,
        out: &OutputSettings,
        map: TempoMap,
    ) -> Result<PreparedWarpClip, PlanError> {
        let ctx = TimingContext::new_with_limits(
            &map,
            out,
            &PlanLimits {
                max_work: 20_000_000,
                ..PlanLimits::default()
            },
        )
        .unwrap();
        (c.start_frame, c.end_frame) = schedule_clip(&c, &ctx, out, c.source_end_frame)?;
        prepare_clip(&c, &ctx, out, c.source_end_frame)
    }
    fn map_at(origin: Rational, b: Rational, end_b: Rational) -> TempoMap {
        TempoMap {
            points: vec![
                TempoPoint {
                    q: origin.clone(),
                    bpm: b,
                    shape: Interpolation::Linear,
                },
                TempoPoint {
                    q: origin + r(1, 1),
                    bpm: end_b,
                    shape: Interpolation::Step,
                },
            ],
        }
    }
    #[test]
    fn public_artifact_rejects_right_gap_that_rounds_back_to_source_end() {
        // The exact final gap is positive (~3.0464e-17 source frames), but
        // subtracting its approximation from 1 rounds to 1. A separately
        // rounded forward inverse still lands below 1 and must not admit it.
        let json = br#"{
          "version":6,
          "output":{"score_start_q":"0/1","score_end_q":"1/1","tail_seconds":"0/1","sample_rate_hz":48000,"channels":1,"total_frames":2,"output":{"node":"warp","port":"out"}},
          "tempo":{"points":[
            {"q":"0/1","bpm":"9930385589350642984058755095404800790126870706742824492072954375/6277101735386680763835789423207666416102355444464034512896","shape":"linear"},
            {"q":"1/1","bpm":"29791156768051928952176265286214402370380612120228473476218863125/6277101735386680763835789423207666416102355444464034512896","shape":"step"}
          ]},
          "nodes":[{"id":"warp","processor":{"kind":"warp_rate","clip":{
            "asset":"a","channels":1,"at_q":"0/1","source_start_frame":0,"source_end_frame":1,
            "warp":[{"q":"0/1","source_frame":0},{"q":"1/1","source_frame":1}],
            "gain":"1/1","fade_in_seconds":"0/1","fade_out_seconds":"0/1","fade_shape":"linear",
            "source":{"object":"warp","path":["warp"]},"start_frame":0,"end_frame":2
          }}}],
          "audio_assets":[{"id":"a","format":"pcm_f32le_interleaved/1","rate_hz":48000,"channels":1,"frames":1,"hash":"sha256:df3f619804a92fdb4057192dc43dd748ea778adc52bc498ce80524c014b81119","bytes":[0,0,0,0]}]
        }"#;
        let error = crate::PlanArtifact::from_json(json).unwrap_err();
        assert_eq!(error.code, "E_TIME_PRECISION");
    }

    #[test]
    fn zero_tail_score_end_at_subdivided_ramp_anchor() {
        let mut out = output();
        out.sample_rate_hz = 8;
        out.score_end_q = r(3, 5);
        out.tail_seconds = r(0, 1);
        let mut c = clip();
        c.at_q = r(1, 10);
        c.source_end_frame = 6;
        c.warp = vec![
            crate::plan_v6::WarpAnchor {
                q: r(0, 1),
                source_frame: 0,
            },
            crate::plan_v6::WarpAnchor {
                q: r(1, 5),
                source_frame: 1,
            },
            crate::plan_v6::WarpAnchor {
                q: r(1, 2),
                source_frame: 3,
            },
            crate::plan_v6::WarpAnchor {
                q: r(1, 1),
                source_frame: 6,
            },
        ];
        let p = prepare_with(c, &out, map_at(r(0, 1), r(60, 1), r(120, 1))).unwrap();
        assert_eq!((p.start_frame, p.end_frame), (1, 4));
        assert!(p.coordinate(3).is_some());
        assert_eq!(p.coordinate(4), None);
    }

    #[test]
    fn prepared_negative_origin_tail_and_fractional_ramp_start() {
        let mut out = output();
        out.sample_rate_hz = 8;
        out.score_start_q = r(-1, 1);
        let mut c = clip();
        c.at_q = r(3, 4);
        c.warp[1].q = r(5, 4);
        let map = TempoMap {
            points: vec![
                TempoPoint {
                    q: r(0, 1),
                    bpm: r(60, 1),
                    shape: Interpolation::Step,
                },
                TempoPoint {
                    q: r(1, 1),
                    bpm: r(120, 1),
                    shape: Interpolation::Step,
                },
            ],
        };
        let p = prepare_with(c, &out, map).unwrap();
        assert_eq!((p.start_frame, p.end_frame), (14, 20));
        for (n, u) in [(14, 0.), (16, 0.6), (18, 1.8)] {
            let (i, f) = p.coordinate(n).unwrap();
            assert!((i as f64 + f - u).abs() < 1e-12);
        }
        out.score_start_q = r(0, 1);
        let mut c = clip();
        c.at_q = r(1, 10);
        c.warp[1].q = r(1, 2);
        let p = prepare_with(c, &out, map_at(r(0, 1), r(60, 1), r(120, 1))).unwrap();
        for n in p.start_frame..p.end_frame {
            let (i, f) = p.coordinate(n).unwrap();
            let expected = 6. * ((n as f64 / 8.).exp_m1() - 0.1);
            assert!((i as f64 + f - expected).abs() < 1e-12);
        }
    }
    #[test]
    fn positive_progress_cannot_disappear_when_added_to_source_origin() {
        let mut out = output();
        out.sample_rate_hz = 8;
        let mut c = clip();
        c.at_q = r(0, 1);
        c.source_end_frame = 2;
        let epsilon = Rational::new(1.into(), num_bigint::BigInt::from(1u8) << 60);
        c.warp = vec![
            crate::plan_v6::WarpAnchor {
                q: r(0, 1),
                source_frame: 0,
            },
            crate::plan_v6::WarpAnchor {
                q: r(1, 8) - epsilon,
                source_frame: 1,
            },
            crate::plan_v6::WarpAnchor {
                q: r(1, 1),
                source_frame: 2,
            },
        ];
        assert_eq!(
            prepare_with(c, &out, map_at(r(0, 1), r(60, 1), r(60, 1)))
                .unwrap_err()
                .code,
            "E_TIME_PRECISION"
        );
    }
    #[test]
    fn preparation_uses_the_existing_timing_budget() {
        let out = output();
        let mut c = clip();
        c.at_q = r(1, 10);
        c.warp[1].q = r(1, 2);
        let generous = context(&out, true, 1_000_000);
        (c.start_frame, c.end_frame) = schedule_clip(&c, &generous, &out, 3).unwrap();
        let tight = context(&out, true, 0);
        assert_eq!(
            prepare_clip(&c, &tight, &out, 3).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
    }

    #[test]
    fn increasing_decreasing_and_nearflat_analytical_coordinates() {
        for (b, end_b) in [
            (60., 120.),
            (60., 240.),
            (120., 60.),
            (120., 30.),
            (60., 60.000001),
        ] {
            let mut out = output();
            out.sample_rate_hz = 8;
            let mut c = clip();
            c.at_q = r(0, 1);
            c.warp[1].q = r(1, 1);
            c.source_end_frame = 100;
            c.warp[1].source_frame = 100;
            let slope = end_b - b;
            let p = prepare_with(
                c,
                &out,
                map_at(
                    r(0, 1),
                    r(b as i64, 1),
                    r((end_b * 1_000_000.) as i64, 1_000_000),
                ),
            )
            .unwrap();
            for n in p.start_frame..p.end_frame {
                let expected = 100. * b / slope * ((slope * n as f64 / (60. * 8.)).exp_m1());
                let (i, f) = p.coordinate(n).unwrap();
                assert!(
                    (i as f64 + f - expected).abs() < 1e-6,
                    "b={b} end={end_b} n={n}"
                );
            }
        }
    }
    #[test]
    fn piecewise_warp_and_tempo_boundaries_have_right_ownership() {
        let mut out = output();
        out.sample_rate_hz = 8;
        let mut c = clip();
        c.at_q = r(0, 1);
        c.source_start_frame = 10;
        c.source_end_frame = 18;
        c.warp = vec![
            crate::plan_v6::WarpAnchor {
                q: r(0, 1),
                source_frame: 10,
            },
            crate::plan_v6::WarpAnchor {
                q: r(1, 2),
                source_frame: 12,
            },
            crate::plan_v6::WarpAnchor {
                q: r(1, 1),
                source_frame: 18,
            },
        ];
        let map = TempoMap {
            points: vec![
                TempoPoint {
                    q: r(0, 1),
                    bpm: r(60, 1),
                    shape: Interpolation::Step,
                },
                TempoPoint {
                    q: r(1, 4),
                    bpm: r(120, 1),
                    shape: Interpolation::Step,
                },
            ],
        };
        let p = prepare_with(c, &out, map).unwrap();
        for (n, u) in [(0, 0.), (1, 0.5), (2, 1.), (3, 2.), (4, 5.)] {
            let (i, f) = p.coordinate(n).unwrap();
            assert_eq!(i as f64 + f, u);
        }
        assert_eq!(p.coordinate(5), None);
    }
    #[test]
    fn huge_origin_and_bpm_ratio_are_combined_before_conversion() {
        let origin = Rational::from_integer(num_bigint::BigInt::from(1u8) << 1000);
        let mut out = output();
        out.score_start_q = origin.clone();
        out.score_end_q = &origin + r(1, 1);
        out.sample_rate_hz = 8;
        let mut c = clip();
        c.at_q = origin.clone();
        c.warp[1].q = r(1, 1);
        let p = prepare_with(c.clone(), &out, map_at(origin.clone(), r(60, 1), r(120, 1))).unwrap();
        assert_eq!(p.coordinate(0), Some((0, 0.)));
        let (i, f) = p.coordinate(1).unwrap();
        assert!((i as f64 + f - 3. * 0.125f64.exp_m1()).abs() < 1e-12);
        let huge = Rational::from_integer(num_bigint::BigInt::from(1u8) << 2000);
        let p = prepare_with(c, &out, map_at(origin, r(1, 1), huge)).unwrap();
        assert_eq!((p.start_frame, p.end_frame), (0, 1));
        assert_eq!(p.coordinate(0), Some((0, 0.)));
    }
    #[test]
    fn tiny_progress_and_endpoint_collapse_fail_explicitly() {
        let mut out = output();
        out.sample_rate_hz = 8;
        let mut c = clip();
        c.at_q = r(0, 1);
        c.warp[1].q = r(1, 1);
        let tiny = Rational::new(1.into(), num_bigint::BigInt::from(1u8) << 1200);
        out.score_end_q = &tiny / r(60, 1);
        assert_eq!(
            prepare_with(c.clone(), &out, map_at(r(0, 1), tiny.clone(), tiny.clone()))
                .unwrap_err()
                .code,
            "E_TIME_PRECISION"
        );
        out.sample_rate_hz = 48000;
        out.score_end_q = r(1, 1);
        c.source_end_frame = 1;
        c.warp[1].source_frame = 1;
        c.warp[1].q = (r(1, 1) + tiny) / r(48000, 1);
        assert_eq!(
            prepare_with(c, &out, map_at(r(0, 1), r(60, 1), r(60, 1)))
                .unwrap_err()
                .code,
            "E_TIME_PRECISION"
        );
    }
    #[test]
    fn empty_spans_fades_and_forged_bounds() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        c.warp[1].q = r(1, 192000);
        (c.start_frame, c.end_frame) = schedule_clip(&c, &ctx, &out, 3).unwrap();
        let p = prepare_clip(&c, &ctx, &out, 3).unwrap();
        assert_eq!(p.coordinate(1), None);
        assert_eq!(p.gain_at(1), 0.);
        c.end_frame += 1;
        assert_eq!(
            prepare_clip(&c, &ctx, &out, 3).unwrap_err().code,
            "E_INTERVAL"
        );
        c = clip();
        c.fade_in_seconds = r(1, 48000);
        c.fade_out_seconds = r(2, 48000);
        (c.start_frame, c.end_frame) = schedule_clip(&c, &ctx, &out, 3).unwrap();
        let p = prepare_clip(&c, &ctx, &out, 3).unwrap();
        for (n, gain) in [(1, 0.375), (2, 0.3125), (3, 0.0625)] {
            assert_eq!(p.gain_at(n), gain);
        }
    }

    #[test]
    fn prepared_constant_coordinates_and_fades() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        (c.start_frame, c.end_frame) = schedule_clip(&c, &ctx, &out, 3).unwrap();
        let p = prepare_clip(&c, &ctx, &out, 3).unwrap();
        for (n, u) in [(1, 0.75), (2, 1.75), (3, 2.75)] {
            let (i, f) = p.coordinate(n).unwrap();
            assert!((i as f64 + f - u).abs() < 1e-12);
        }
        assert_eq!(p.coordinate(0), None);
        assert_eq!(p.coordinate(4), None);
        assert_eq!(p.gain_at(1), 0.5);
    }

    #[test]
    fn constant_fractional_and_empty_intervals_ignore_stored_bounds() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        assert_eq!(schedule_clip(&c, &ctx, &out, 3).unwrap(), (1, 4));
        c.warp[1].q = r(1, 192000);
        assert_eq!(schedule_clip(&c, &ctx, &out, 3).unwrap(), (1, 1));
    }
    #[test]
    fn ramp_negative_origin_and_shared_budget() {
        let mut out = output();
        out.score_start_q = r(-1, 1);
        let mut c = clip();
        c.at_q = r(0, 1);
        c.warp[1].q = r(1, 1);
        let ctx = context(&out, true, 1_000_000);
        assert_eq!(schedule_clip(&c, &ctx, &out, 3).unwrap(), (48000, 81272));
        let tight = context(&out, true, 0);
        assert!(matches!(
            schedule_clip(&c, &tight, &out, 3)
                .unwrap_err()
                .code
                .as_str(),
            "E_RESOURCE_LIMIT" | "E_TIME_PRECISION"
        ));
    }
    #[test]
    fn future_tempo_in_tail_and_render_clipping() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        c.at_q = r(3, 4);
        c.warp[1].q = r(5, 4);
        assert_eq!(schedule_clip(&c, &ctx, &out, 3).unwrap(), (36000, 72000));
        c.warp[1].q = r(10, 1);
        assert_eq!(schedule_clip(&c, &ctx, &out, 3).unwrap(), (36000, 96000));
        c.at_q = r(1, 1);
        assert_eq!(
            schedule_clip(&c, &ctx, &out, 3).unwrap_err().code,
            "E_INTERVAL"
        );
        c.at_q = r(-1, 1);
        assert_eq!(
            schedule_clip(&c, &ctx, &out, 3).unwrap_err().code,
            "E_INTERVAL"
        );
    }
    #[test]
    fn prescore_end_start_can_first_sample_in_tail() {
        let mut out = output();
        out.score_end_q = r(1, 192000);
        out.tail_seconds = r(1, 1);
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        c.at_q = r(1, 384000);
        c.warp[1].q = r(1, 48000);
        assert_eq!(schedule_clip(&c, &ctx, &out, 3).unwrap(), (1, 2));
        out.sample_rate_hz = 0;
        assert_eq!(
            schedule_clip(&c, &ctx, &out, 3).unwrap_err().code,
            "E_RANGE"
        );
    }

    #[test]
    fn malformed_recipes_fail_before_clock_work() {
        let out = output();
        let ctx = context(&out, true, 0);
        for mutation in 0..9 {
            let mut c = clip();
            match mutation {
                0 => c.warp.clear(),
                1 => c.warp[0].q = r(1, 1),
                2 => c.warp[0].source_frame = 1,
                3 => c.warp[1].source_frame = 2,
                4 => c.warp[1].q = r(0, 1),
                5 => c.source_end_frame = 4,
                6 => c.gain = r(-1, 1),
                7 => c.fade_out_seconds = r(-1, 1),
                _ => c.warp.insert(1, c.warp[0].clone()),
            }
            assert_eq!(
                schedule_clip(&c, &ctx, &out, 3).unwrap_err().code,
                "E_RANGE",
                "mutation {mutation}"
            );
        }
        let mut c = clip();
        c.at_q = Rational::from_integer(num_bigint::BigInt::from(1u8) << 4096);
        assert_eq!(
            schedule_clip(&c, &ctx, &out, 3).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
        c = clip();
        c.warp = vec![c.warp[0].clone(); 4097];
        assert_eq!(
            schedule_clip(&c, &ctx, &out, 3).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
        c = clip();
        c.gain = Rational::from_integer(num_bigint::BigInt::from(1u8) << 2000);
        assert_eq!(
            schedule_clip(&c, &ctx, &out, 3).unwrap_err().code,
            "E_NONFINITE"
        );
    }
}
