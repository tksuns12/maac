//! Certified preparation of native rate-mode transport coordinates and fades.

use crate::{
    music::MusicError,
    plan::{OutputSettings, PlanError, Rational},
    plan_v3::{AutomationAnchor, TimingContext},
    plan_v5::{AudioClip, AudioFadeShape},
    tempo::TimeValue,
};
use num_traits::{One, ToPrimitive, Zero};
use std::cmp::Ordering;

fn error(code: &str, message: &str) -> PlanError {
    PlanError {
        code: code.into(),
        path: "audio".into(),
        message: message.into(),
        span: None,
    }
}
fn timing_error(error: MusicError) -> PlanError {
    self::error(error.code.as_str(), &error.message)
}
fn integer(value: u64) -> Rational {
    Rational::from_integer(value.into())
}
fn time(value: Rational) -> Result<TimeValue, PlanError> {
    // Derived ratios may exceed the source-input bit limit but remain bounded.
    TimeValue::from_rational(Rational::one())
        .and_then(|t| t.scaled(&value))
        .map_err(timing_error)
}
fn scaled(value: &TimeValue, factor: &Rational) -> Result<TimeValue, PlanError> {
    value.scaled(factor).map_err(timing_error)
}
fn add(a: &TimeValue, b: &TimeValue) -> Result<TimeValue, PlanError> {
    a.sum(b).map_err(timing_error)
}
fn subtract(a: &TimeValue, b: &TimeValue) -> Result<TimeValue, PlanError> {
    a.difference(b).map_err(timing_error)
}
fn minimum(context: &TimingContext, a: TimeValue, b: &TimeValue) -> Result<TimeValue, PlanError> {
    Ok(if context.compare_times(&a, b)? == Ordering::Greater {
        b.clone()
    } else {
        a
    })
}
fn frame(context: &TimingContext, value: &TimeValue, rate: u64) -> Result<u64, PlanError> {
    context
        .ceil_time(value, rate)?
        .to_u64()
        .ok_or_else(|| error("E_RESOURCE_LIMIT", "clip frame bound exceeds u64"))
}

struct Recipe {
    start: TimeValue,
    finish: TimeValue,
    clipped_finish: TimeValue,
    source_rate: Rational,
    start_frame: u64,
    end_frame: u64,
}
fn recipe(
    clip: &AudioClip,
    context: &TimingContext,
    output: &OutputSettings,
    asset_rate: u32,
    asset_frames: u64,
) -> Result<Recipe, PlanError> {
    if asset_rate == 0 || output.sample_rate_hz == 0 {
        return Err(error("E_RANGE", "sample rates must be positive"));
    }
    if clip.speed <= Rational::zero()
        || clip.gain < Rational::zero()
        || clip.fade_in_seconds < Rational::zero()
        || clip.fade_out_seconds < Rational::zero()
    {
        return Err(error(
            "E_RANGE",
            "speed must be positive and gain/fades nonnegative",
        ));
    }
    if clip.source_start_frame >= clip.source_end_frame || clip.source_end_frame > asset_frames {
        return Err(error(
            "E_RANGE",
            "source slice must be nonempty and contained in the asset",
        ));
    }
    // Bound inputs before forming derived products, including unused parameters.
    for value in [
        &clip.speed,
        &clip.gain,
        &clip.fade_in_seconds,
        &clip.fade_out_seconds,
    ] {
        if value.numer().bits() > 4096 || value.denom().bits() > 4096 {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "clip rational input exceeds bit allowance",
            ));
        }
    }
    let start = match &clip.at {
        AutomationAnchor::Score { q } => context.relative_at_score(q, &Rational::zero())?,
        AutomationAnchor::Seconds { seconds } => context.relative_at_seconds(seconds)?,
    };
    if context.compare_times(&start, &time(Rational::zero())?)? == Ordering::Less
        || context.compare_times(&start, &context.end)? != Ordering::Less
    {
        return Err(error(
            "E_INTERVAL",
            "clip must start within the physical score interval",
        ));
    }
    let source_rate = integer(u64::from(asset_rate)) * &clip.speed;
    let duration = time(integer(clip.source_end_frame - clip.source_start_frame) / &source_rate)?;
    let finish = add(&start, &duration)?;
    let clipped_finish = minimum(context, finish.clone(), &context.duration)?;
    let rate = u64::from(output.sample_rate_hz);
    let start_frame = frame(context, &start, rate)?;
    let end_frame = frame(context, &clipped_finish, rate)?;
    Ok(Recipe {
        start,
        finish,
        clipped_finish,
        source_rate,
        start_frame,
        end_frame,
    })
}

/// Compute bounds without trusting or consulting the stored frame fields.
pub(crate) fn schedule_clip(
    clip: &AudioClip,
    context: &TimingContext,
    output: &OutputSettings,
    asset_rate: u32,
    asset_frames: u64,
) -> Result<(u64, u64), PlanError> {
    let recipe = recipe(clip, context, output, asset_rate, asset_frames)?;
    Ok((recipe.start_frame, recipe.end_frame))
}

/// Samples are evaluated relative to the segment's first frame. Storing the
/// complete advance avoids premature underflow of a tiny per-frame increment.
#[derive(Debug)]
struct Segment {
    start: u64,
    end: u64,
    first: f64,
    advance: f64,
}
impl Segment {
    fn validate_domain(&self, maximum: f64) -> Result<(), PlanError> {
        if !self.first.is_finite() || !self.advance.is_finite() {
            return Err(error("E_NONFINITE", "prepared clip segment is nonfinite"));
        }
        // Timing approximation has relative error <= 2^-48 plus f64 rounding.
        // Permit that rounding envelope, while proving runtime sums are finite.
        let allowance = maximum.max(1.) * 2f64.powi(-45);
        if self.first < -allowance
            || self.first > maximum + allowance
            || self.advance.abs() > maximum + allowance
        {
            return Err(error(
                "E_TIME_PRECISION",
                "prepared clip segment exceeds its bounded domain",
            ));
        }
        Ok(())
    }
    fn value(&self, frame: u64) -> f64 {
        let count = self.end - self.start;
        if count <= 1 {
            self.first
        } else {
            self.first + ((frame - self.start) as f64 / (count - 1) as f64) * self.advance
        }
    }
}
fn approximate_preserving_sign(
    context: &TimingContext,
    value: &TimeValue,
) -> Result<f64, PlanError> {
    let sign = context.compare_times(value, &time(Rational::zero())?)?;
    if sign == Ordering::Equal {
        return Ok(0.);
    }
    let approximate = context.approximate_time(value)?;
    if !approximate.is_finite() {
        return Err(error("E_NONFINITE", "prepared clip value is nonfinite"));
    }
    if approximate == 0. || (approximate > 0.) != (sign == Ordering::Greater) {
        return Err(error(
            "E_TIME_PRECISION",
            "nonzero prepared clip value lost its sign or underflowed",
        ));
    }
    Ok(approximate)
}

fn segment(
    context: &TimingContext,
    start: u64,
    end: u64,
    first: TimeValue,
    per_frame: Rational,
) -> Result<Segment, PlanError> {
    if start >= end {
        return Ok(Segment {
            start,
            end,
            first: 0.,
            advance: 0.,
        });
    }
    let exact_advance = time(per_frame * integer(end - start - 1))?;
    let exact_last = add(&first, &exact_advance)?;
    let first = approximate_preserving_sign(context, &first)?;
    let advance = approximate_preserving_sign(context, &exact_advance)?;
    let last = approximate_preserving_sign(context, &exact_last)?;
    let result = Segment {
        start,
        end,
        first,
        advance,
    };
    let reconstructed_last = result.value(end - 1);
    if !reconstructed_last.is_finite() {
        return Err(error("E_NONFINITE", "prepared clip endpoint is nonfinite"));
    }
    if reconstructed_last.partial_cmp(&0.) != last.partial_cmp(&0.) {
        return Err(error(
            "E_TIME_PRECISION",
            "prepared clip endpoint lost its sign through cancellation",
        ));
    }
    Ok(result)
}

/// Physical fade settings; gain is checked before caller clock preparation.
pub(crate) struct EnvelopeSettings<'a> {
    gain: f64,
    fade_in_seconds: &'a Rational,
    fade_out_seconds: &'a Rational,
    shape: AudioFadeShape,
}
impl<'a> EnvelopeSettings<'a> {
    /// Fades are ordered [fade-in, fade-out], both in physical seconds.
    pub(crate) fn new(
        gain: &Rational,
        fades: [&'a Rational; 2],
        shape: AudioFadeShape,
    ) -> Result<Self, PlanError> {
        let value = gain
            .to_f64()
            .filter(|x| x.is_finite() && (*x != 0. || gain.is_zero()))
            .ok_or_else(|| {
                error(
                    "E_NONFINITE",
                    "clip gain cannot be represented by the engine",
                )
            })?;
        if gain < &Rational::zero() || fades.iter().any(|fade| *fade < &Rational::zero()) {
            return Err(error("E_RANGE", "gain and fades must be nonnegative"));
        }
        Ok(Self {
            gain: value,
            fade_in_seconds: fades[0],
            fade_out_seconds: fades[1],
            shape,
        })
    }
}

/// The caller supplies already certified active bounds and physical recipes.
pub(crate) struct EnvelopeSpan<'a> {
    pub start: &'a TimeValue,
    pub finish: &'a TimeValue,
    pub clipped_finish: &'a TimeValue,
    pub start_frame: u64,
    pub end_frame: u64,
    pub rate_hz: u32,
}

#[derive(Debug)]
pub(crate) struct PreparedEnvelope {
    start_frame: u64,
    end_frame: u64,
    fade_in: Option<Segment>,
    fade_out: Option<Segment>,
    gain: f64,
    shape: AudioFadeShape,
}
impl PreparedEnvelope {
    pub(crate) fn gain_at(&self, frame: u64) -> f64 {
        if frame < self.start_frame || frame >= self.end_frame {
            return 0.;
        }
        let incoming = self.fade_in.as_ref().map_or(1., |s| {
            if frame >= s.end {
                1.
            } else {
                s.value(frame).clamp(0., 1.)
            }
        });
        let outgoing = self.fade_out.as_ref().map_or(1., |s| {
            if frame < s.start {
                1.
            } else {
                s.value(frame).clamp(0., 1.)
            }
        });
        let shape = |x: f64| match self.shape {
            AudioFadeShape::Linear => x,
            AudioFadeShape::EqualPower => (std::f64::consts::FRAC_PI_2 * x).sin(),
        };
        self.gain * shape(incoming) * shape(outgoing)
    }
}

pub(crate) fn prepare_envelope(
    settings: EnvelopeSettings<'_>,
    span: EnvelopeSpan<'_>,
    context: &TimingContext,
) -> Result<PreparedEnvelope, PlanError> {
    let rate = integer(u64::from(span.rate_hz));
    let elapsed = subtract(&time(integer(span.start_frame) / &rate)?, span.start)?;
    let duration = subtract(span.finish, span.start)?;
    let fade_in = if settings.fade_in_seconds.is_zero() || span.start_frame == span.end_frame {
        None
    } else {
        let fade_end = minimum(
            context,
            add(span.start, &time(settings.fade_in_seconds.clone())?)?,
            span.clipped_finish,
        )?;
        let end = frame(context, &fade_end, u64::from(span.rate_hz))?;
        Some(segment(
            context,
            span.start_frame,
            end,
            scaled(&elapsed, &settings.fade_in_seconds.recip())?,
            Rational::one() / (&rate * settings.fade_in_seconds),
        )?)
    };
    let fade_out = if settings.fade_out_seconds.is_zero() || span.start_frame == span.end_frame {
        None
    } else {
        let local_start = subtract(&duration, &time(settings.fade_out_seconds.clone())?)?;
        let local_start =
            if context.compare_times(&local_start, &time(Rational::zero())?)? == Ordering::Less {
                time(Rational::zero())?
            } else {
                local_start
            };
        let fade_start = minimum(context, add(span.start, &local_start)?, span.clipped_finish)?;
        let start = frame(context, &fade_start, u64::from(span.rate_hz))?;
        let remaining = subtract(span.finish, &time(integer(start) / &rate)?)?;
        Some(segment(
            context,
            start,
            span.end_frame,
            scaled(&remaining, &settings.fade_out_seconds.recip())?,
            -Rational::one() / (&rate * settings.fade_out_seconds),
        )?)
    };
    if let Some(segment) = &fade_in {
        segment.validate_domain(1.)?;
    }
    if let Some(segment) = &fade_out {
        segment.validate_domain(1.)?;
    }
    Ok(PreparedEnvelope {
        start_frame: span.start_frame,
        end_frame: span.end_frame,
        fade_in,
        fade_out,
        gain: settings.gain,
        shape: settings.shape,
    })
}

#[derive(Debug)]
pub(crate) struct PreparedAudioClip {
    pub(crate) start_frame: u64,
    pub(crate) end_frame: u64,
    phase: Segment,
    envelope: PreparedEnvelope,
}
impl PreparedAudioClip {
    pub(crate) fn coordinate(&self, frame: u64) -> Option<(u64, f64)> {
        if frame < self.start_frame || frame >= self.end_frame {
            return None;
        }
        let u = self.phase.value(frame);
        // Preparation checks both endpoints of this monotone affine phase,
        // including positive source-end residuals. Every admitted sample has
        // a finite coordinate inside the slice; no active sample is dropped.
        let index = u.floor() as u64;
        Some((index, u - index as f64))
    }
    pub(crate) fn gain_at(&self, frame: u64) -> f64 {
        self.envelope.gain_at(frame)
    }
}

/// Prepare immutable floating-point values after independently checking bounds.
pub(crate) fn prepare_clip(
    clip: &AudioClip,
    context: &TimingContext,
    output: &OutputSettings,
    asset_rate: u32,
    asset_frames: u64,
) -> Result<PreparedAudioClip, PlanError> {
    let settings = EnvelopeSettings::new(
        &clip.gain,
        [&clip.fade_in_seconds, &clip.fade_out_seconds],
        clip.fade_shape,
    )?;
    let r = recipe(clip, context, output, asset_rate, asset_frames)?;
    if (clip.start_frame, clip.end_frame) != (r.start_frame, r.end_frame) {
        return Err(error(
            "E_INTERVAL",
            "stored clip frame bounds differ from certified recipe",
        ));
    }
    let rate = integer(u64::from(output.sample_rate_hz));
    let elapsed = subtract(&time(integer(r.start_frame) / &rate)?, &r.start)?;
    let first_phase = scaled(&elapsed, &r.source_rate)?;
    let phase = segment(
        context,
        r.start_frame,
        r.end_frame,
        first_phase.clone(),
        &r.source_rate / &rate,
    )?;
    let envelope = prepare_envelope(
        settings,
        EnvelopeSpan {
            start: &r.start,
            finish: &r.finish,
            clipped_finish: &r.clipped_finish,
            start_frame: r.start_frame,
            end_frame: r.end_frame,
            rate_hz: output.sample_rate_hz,
        },
        context,
    )?;
    phase.validate_domain((clip.source_end_frame - clip.source_start_frame) as f64)?;
    if r.start_frame < r.end_frame {
        let source_end = time(integer(clip.source_end_frame - clip.source_start_frame))?;
        let last_phase = add(
            &first_phase,
            &time((&r.source_rate / &rate) * integer(r.end_frame - r.start_frame - 1))?,
        )?;
        for (exact_phase, actual_phase) in [
            (&first_phase, phase.first),
            (&last_phase, phase.value(r.end_frame - 1)),
        ] {
            let residual = subtract(&source_end, exact_phase)?;
            let positive_residual = approximate_preserving_sign(context, &residual)?;
            if positive_residual <= 0.
                || actual_phase < 0.
                || actual_phase >= (clip.source_end_frame - clip.source_start_frame) as f64
            {
                return Err(error(
                    "E_TIME_PRECISION",
                    "active clip endpoint lost its positive source-end residual",
                ));
            }
        }
    }
    Ok(PreparedAudioClip {
        start_frame: r.start_frame,
        end_frame: r.end_frame,
        phase,
        envelope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Interpolation, PlanLimits, TempoMap, TempoPoint};
    use num_bigint::BigInt;
    use serde_json::json;
    fn r(n: i64, d: i64) -> Rational {
        Rational::new(n.into(), d.into())
    }
    fn output() -> OutputSettings {
        serde_json::from_value(json!({"score_start_q":"0/1","score_end_q":"1/1","tail_seconds":"1/1","sample_rate_hz":48000,"channels":1,"total_frames":96000,"output":{"node":"clip","port":"out"}})).unwrap()
    }
    fn context(out: &OutputSettings, ramp: bool, work: u64) -> TimingContext {
        let map = TempoMap {
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
        };
        let limits = PlanLimits {
            max_work: work,
            ..PlanLimits::default()
        };
        TimingContext::new_with_limits(&map, out, &limits).unwrap()
    }
    fn clip() -> AudioClip {
        serde_json::from_value(json!({"asset":"a","channels":1,"at":{"kind":"seconds","seconds":"1/192000"},"source_start_frame":0,"source_end_frame":3,"speed":"2/1","reverse":false,"gain":"1/2","fade_in_seconds":"1/48000","fade_out_seconds":"1/24000","fade_shape":"linear","source":{"object":"clip","path":["clip"]},"start_frame":1,"end_frame":4})).unwrap()
    }
    fn prepared(
        mut clip: AudioClip,
        out: &OutputSettings,
        ctx: &TimingContext,
        rate: u32,
        frames: u64,
    ) -> PreparedAudioClip {
        (clip.start_frame, clip.end_frame) = schedule_clip(&clip, ctx, out, rate, frames).unwrap();
        prepare_clip(&clip, ctx, out, rate, frames).unwrap()
    }
    #[test]
    fn fractional_start_retains_phase_and_exact_overlap_envelopes() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let c = clip();
        assert_eq!(schedule_clip(&c, &ctx, &out, 24000, 3).unwrap(), (1, 4));
        let p = prepare_clip(&c, &ctx, &out, 24000, 3).unwrap();
        for (frame, coord, gain) in [
            (1, (0, 0.75), 0.375),
            (2, (1, 0.75), 0.3125),
            (3, (2, 0.75), 0.0625),
        ] {
            assert_eq!(p.coordinate(frame), Some(coord));
            assert_eq!(p.gain_at(frame), gain);
        }
        assert_eq!(p.coordinate(0), None);
        assert_eq!(p.coordinate(4), None);
        assert_eq!(p.gain_at(4), 0.);
        let mut bad = c;
        bad.start_frame = 0;
        assert_eq!(
            prepare_clip(&bad, &ctx, &out, 24000, 3).unwrap_err().code,
            "E_INTERVAL"
        );
    }
    #[test]
    fn positive_subsample_interval_and_tail_first_sample() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        c.source_end_frame = 1;
        c.speed = r(100, 1);
        assert_eq!(schedule_clip(&c, &ctx, &out, 48000, 1).unwrap(), (1, 1));
        assert_eq!(
            prepared(c.clone(), &out, &ctx, 48000, 1).coordinate(1),
            None
        );
        c.at = AutomationAnchor::Seconds {
            seconds: r(191999, 192000),
        };
        c.speed = r(1, 1);
        assert_eq!(
            schedule_clip(&c, &ctx, &out, 48000, 1).unwrap(),
            (48000, 48001)
        );
    }
    #[test]
    fn negative_origin_score_ramp_and_budget_exhaustion() {
        let mut out = output();
        out.score_start_q = r(-2, 1);
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        c.at = AutomationAnchor::Seconds { seconds: r(-1, 1) };
        assert_eq!(
            schedule_clip(&c, &ctx, &out, 24000, 3).unwrap(),
            (48000, 48003)
        );
        let out = output();
        let mut c = clip();
        c.at = AutomationAnchor::Score { q: r(1, 2) };
        let ctx = context(&out, true, 1_000_000);
        let p = prepared(c.clone(), &out, &ctx, 24000, 3);
        assert_eq!(p.start_frame, 19463);
        assert!(p.coordinate(p.start_frame).is_some());
        let exhausted = context(&out, true, 0);
        assert_eq!(
            schedule_clip(&c, &exhausted, &out, 24000, 3)
                .unwrap_err()
                .code,
            "E_RESOURCE_LIMIT"
        );
    }
    #[test]
    fn equal_power_overlap_and_huge_tiny_rationals() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        c.fade_shape = AudioFadeShape::EqualPower;
        let p = prepared(c.clone(), &out, &ctx, 24000, 3);
        assert!((p.gain_at(2) - 0.5 * (std::f64::consts::FRAC_PI_2 * 0.625).sin()).abs() < 1e-15);
        let huge = Rational::from_integer(BigInt::from(1u8) << 4000);
        c.at = AutomationAnchor::Seconds { seconds: r(0, 1) };
        c.speed = huge.clone();
        c.fade_in_seconds = r(0, 1);
        c.fade_out_seconds = r(0, 1);
        let p = prepared(c.clone(), &out, &ctx, 48000, 3);
        assert_eq!((p.start_frame, p.end_frame), (0, 1));
        assert_eq!(p.coordinate(0), Some((0, 0.)));
        assert_eq!(p.gain_at(0), 0.5);
        c.speed = huge.recip();
        c.fade_in_seconds = huge.clone();
        c.fade_out_seconds = huge.clone();
        (c.start_frame, c.end_frame) = schedule_clip(&c, &ctx, &out, 48000, 3).unwrap();
        assert_eq!(
            prepare_clip(&c, &ctx, &out, 48000, 3).unwrap_err().code,
            "E_TIME_PRECISION"
        );
        let ratio = Rational::new(
            (BigInt::from(1u8) << 4000) + 1u8,
            (BigInt::from(1u8) << 4000) + 3u8,
        );
        assert_eq!(ratio.to_f64(), Some(1.));
        c.gain = huge;
        assert_eq!(
            prepare_clip(&c, &ctx, &out, 48000, 3).unwrap_err().code,
            "E_NONFINITE"
        );
    }
    #[test]
    fn ramp_start_just_before_sample_grid_fails_if_phase_sign_is_unresolved() {
        let out = output();
        let ctx = context(&out, true, 20_000_000);
        // For T(q)=ln(1+q), this strict truncated exp(1/R)-1 is
        // below the first frame by much less than an f64 clock subtraction.
        let x = r(1, 48000);
        let mut term = Rational::one();
        let mut q = Rational::zero();
        for n in 1..=16 {
            term = term * &x / r(n, 1);
            q += &term;
        }
        let mut c = clip();
        c.at = AutomationAnchor::Score { q };
        (c.start_frame, c.end_frame) = schedule_clip(&c, &ctx, &out, 24000, 3).unwrap();
        assert_eq!(c.start_frame, 1);
        assert_eq!(
            prepare_clip(&c, &ctx, &out, 24000, 3).unwrap_err().code,
            "E_TIME_PRECISION"
        );
    }

    #[test]
    fn total_advance_preserves_subnormal_phase_and_fade() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        c.at = AutomationAnchor::Seconds { seconds: r(0, 1) };
        c.speed = Rational::new(1.into(), BigInt::from(1u8) << 1080);
        c.fade_in_seconds = Rational::new(BigInt::from(1u8) << 1080, 48000.into());
        c.fade_out_seconds = r(0, 1);
        c.gain = r(1, 1);
        let p = prepared(c, &out, &ctx, 48000, 3);
        assert_eq!(p.coordinate(64), Some((0, f64::from_bits(1))));
        assert_eq!(p.gain_at(64), f64::from_bits(1));
    }

    #[test]
    fn nonzero_prepared_residuals_and_signed_advances_cannot_underflow() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let tiny = Rational::new(1.into(), BigInt::from(1u8) << 1200);
        assert_eq!(
            segment(&ctx, 0, 1, time(tiny.clone()).unwrap(), r(0, 1))
                .unwrap_err()
                .code,
            "E_TIME_PRECISION"
        );
        for advance in [tiny.clone(), -tiny] {
            assert_eq!(
                segment(&ctx, 0, 2, time(r(1, 1)).unwrap(), advance)
                    .unwrap_err()
                    .code,
                "E_TIME_PRECISION"
            );
        }
    }

    #[test]
    fn active_last_sample_cannot_round_to_source_end() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        for bits in [60, 1200] {
            let delta = Rational::new(1.into(), BigInt::from(1u8) << bits);
            let mut c = clip();
            c.at = AutomationAnchor::Seconds { seconds: r(0, 1) };
            c.source_end_frame = 1;
            c.speed = (Rational::one() + delta).recip();
            c.gain = Rational::from_integer(BigInt::from(1u8) << 1000);
            c.fade_in_seconds = r(0, 1);
            c.fade_out_seconds = r(0, 1);
            (c.start_frame, c.end_frame) = schedule_clip(&c, &ctx, &out, 48000, 1).unwrap();
            assert_eq!((c.start_frame, c.end_frame), (0, 2));
            assert_eq!(
                prepare_clip(&c, &ctx, &out, 48000, 1).unwrap_err().code,
                "E_TIME_PRECISION",
                "delta bits {bits}"
            );
        }
    }

    #[test]
    fn positive_last_fade_value_cannot_disappear_by_cancellation() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        for bits in [60, 1200] {
            let delta = Rational::new(1.into(), BigInt::from(1u8) << bits);
            let descending = -(Rational::one() + delta).recip();
            assert_eq!(
                segment(&ctx, 0, 2, time(r(1, 1)).unwrap(), descending)
                    .unwrap_err()
                    .code,
                "E_TIME_PRECISION",
                "delta bits {bits}"
            );
        }
    }

    #[test]
    fn endpoint_checks_preserve_zero_subnormal_and_truncated_segments() {
        let mut out = output();
        out.score_end_q = r(1, 48000);
        out.tail_seconds = r(0, 1);
        let ctx = context(&out, false, 1_000_000);
        let zero = segment(&ctx, 0, 1, time(r(0, 1)).unwrap(), r(1, 1)).unwrap();
        assert_eq!(zero.value(0), 0.);
        assert!(segment(&ctx, 1, 1, time(r(0, 1)).unwrap(), r(1, 1)).is_ok());
        let smallest = Rational::new(1.into(), BigInt::from(1u8) << 1074);
        let fade = segment(&ctx, 0, 2, time(&smallest * r(2, 1)).unwrap(), -&smallest).unwrap();
        assert_eq!(fade.value(1), f64::from_bits(1));
        let mut c = clip();
        c.at = AutomationAnchor::Seconds { seconds: r(0, 1) };
        c.source_end_frame = 1;
        c.speed = (Rational::one() + Rational::new(1.into(), BigInt::from(1u8) << 1200)).recip();
        c.fade_in_seconds = r(0, 1);
        c.fade_out_seconds = r(0, 1);
        let p = prepared(c, &out, &ctx, 48000, 1);
        assert_eq!((p.start_frame, p.end_frame), (0, 1));
        assert_eq!(p.coordinate(0), Some((0, 0.)));
    }

    #[test]
    fn ramp_preparation_budget_threshold_is_preserved() {
        let out = output();
        let mut c = clip();
        c.at = AutomationAnchor::Score { q: r(1, 2) };
        let generous = context(&out, true, 1_000_000);
        (c.start_frame, c.end_frame) = schedule_clip(&c, &generous, &out, 24000, 3).unwrap();
        let threshold = (0..=1_000_000)
            .step_by(64)
            .find(|work| {
                let ctx = context(&out, true, *work);
                prepare_clip(&c, &ctx, &out, 24000, 3).is_ok()
            })
            .expect("bounded preparation succeeds");
        println!("rate_envelope_budget_threshold={threshold}");
        assert_eq!(threshold, 5120);
    }

    #[test]
    fn derived_scaling_budget_and_unrepresentable_gain_are_explicit() {
        let excessive = Rational::from_integer(BigInt::from(1u8) << 32768);
        assert_eq!(time(excessive).unwrap_err().code, "E_RESOURCE_LIMIT");
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        let mut c = clip();
        c.gain = Rational::new(1.into(), BigInt::from(1u8) << 4000);
        assert_eq!(
            prepare_clip(&c, &ctx, &out, 24000, 3).unwrap_err().code,
            "E_NONFINITE"
        );
    }

    #[test]
    fn invalid_source_interval_and_parameters_fail() {
        let out = output();
        let ctx = context(&out, false, 1_000_000);
        for field in ["speed", "gain", "fade", "source", "start", "after"] {
            let mut c = clip();
            match field {
                "speed" => c.speed = r(0, 1),
                "gain" => c.gain = r(-1, 1),
                "fade" => c.fade_in_seconds = r(-1, 1),
                "source" => c.source_end_frame = 4,
                "start" => c.at = AutomationAnchor::Seconds { seconds: r(-1, 1) },
                "after" => c.at = AutomationAnchor::Seconds { seconds: r(1, 1) },
                _ => unreachable!(),
            }
            assert!(schedule_clip(&c, &ctx, &out, 24000, 3).is_err(), "{field}");
        }
    }
}
