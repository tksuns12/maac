//! Certified half-cell LFO preparation; runtime uses only bounded frame lookup and f64 arithmetic.
use crate::{
    plan::{OutputSettings, PlanError, PlanLimits, Rational},
    plan_v3::TimingContext,
    plan_v7::{ControlClock, LfoConfig, LfoWave},
    tempo::TimeValue,
};
use num_traits::{Signed, ToPrimitive, Zero};
use std::cmp::Ordering;
fn error(code: &str, message: &str) -> PlanError {
    PlanError {
        code: code.into(),
        path: "lfo".into(),
        message: message.into(),
        span: None,
    }
}
fn music_error(e: crate::music::MusicError) -> PlanError {
    error(e.code.as_str(), &e.message)
}
fn bounded(value: Rational) -> Result<Rational, PlanError> {
    if value.numer().bits() > 32768 || value.denom().bits() > 32768 {
        Err(error(
            "E_RESOURCE_LIMIT",
            "LFO derived rational exceeds bit allowance",
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
            "E_TIME_PRECISION",
            "LFO preparation produced an unrepresentable value",
        ))
    }
}
fn positive(value: f64, required: bool) -> Result<f64, PlanError> {
    finite(value)?;
    if value < 0. || (required && value == 0.) {
        Err(error(
            "E_TIME_PRECISION",
            "LFO positive value lost precision",
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
    let value = context.approximate_time(value)?;
    if !value.is_finite() {
        return Err(error(
            "E_TIME_PRECISION",
            "LFO coefficient is not representable",
        ));
    }
    if value == 0. || (value > 0.) != (sign == Ordering::Greater) {
        return Err(error(
            "E_TIME_PRECISION",
            "LFO nonzero value lost sign or underflowed",
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
                "LFO exponential endpoints lost strict ordering",
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
    odd: bool,
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
/// Reject oversized fields before derived arithmetic or candidate allocation.
pub(crate) fn candidate_bound(
    config: &LfoConfig,
    output: &OutputSettings,
    tempo_point_count: usize,
    limits: &PlanLimits,
) -> Result<u64, PlanError> {
    for r in [
        &config.phase,
        &config.period,
        &output.score_start_q,
        &output.score_end_q,
    ] {
        if r.numer().bits() > limits.max_rational_bits.min(4096)
            || r.denom().bits() > limits.max_rational_bits.min(4096)
        {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "LFO rational input exceeds bit allowance",
            ));
        }
    }
    if config.phase < Rational::zero()
        || config.phase >= integer(1)
        || config.period <= Rational::zero()
        || output.sample_rate_hz == 0
        || output.score_end_q < output.score_start_q
    {
        return Err(error(
            "E_RANGE",
            "LFO requires canonical phase, positive period and a valid output clock",
        ));
    }
    let end = end_cycle(config, output)?;
    let cells = (bounded(end * integer(2))?).floor().to_integer()
        - (bounded(&config.phase * integer(2))?).floor().to_integer()
        + 1;
    let count: num_bigint::BigInt = cells
        + if config.clock == ControlClock::Score {
            num_bigint::BigInt::from(tempo_point_count)
        } else {
            0.into()
        };
    if count > (limits.max_work / 128).into() {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "LFO half-cell candidates exceed preparation allowance",
        ));
    }
    count
        .to_u64()
        .ok_or_else(|| error("E_RESOURCE_LIMIT", "LFO candidate count exceeds u64"))
}
fn end_cycle(config: &LfoConfig, output: &OutputSettings) -> Result<Rational, PlanError> {
    let distance = if config.clock == ControlClock::Score {
        bounded(&output.score_end_q - &output.score_start_q)?
    } else {
        bounded(
            integer(output.total_frames.saturating_sub(1))
                / integer(u64::from(output.sample_rate_hz)),
        )?
    };
    bounded(&config.phase + bounded(distance / &config.period)?)
}
#[derive(Debug)]
pub(crate) struct PreparedLfo {
    wave: LfoWave,
    frames: u64,
    spans: Vec<PreparedSpan>,
    held: Option<(u64, bool, f64)>,
}
impl PreparedLfo {
    pub(crate) fn value_at(&self, frame: u64) -> Result<f64, PlanError> {
        if frame >= self.frames {
            return Err(error("E_RANGE", "LFO frame lies outside prepared render"));
        }
        let (odd, f) = if let Some((_, odd, f)) = self.held.filter(|(end, _, _)| frame >= *end) {
            (odd, f)
        } else {
            let span = self
                .spans
                .get(self.spans.partition_point(|span| span.end <= frame))
                .ok_or_else(|| {
                    error(
                        "E_TIME_PRECISION",
                        "LFO prepared clock has an uncovered frame",
                    )
                })?;
            (span.odd, span.value(frame))
        };
        let value = match self.wave {
            LfoWave::Sine => (std::f64::consts::TAU * f).sin(),
            LfoWave::Triangle => 1. - 4. * (f - 0.5).abs(),
            LfoWave::Saw => 2. * f - 1.,
            LfoWave::Square => {
                if odd {
                    -1.
                } else {
                    1.
                }
            }
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(error("E_NONFINITE", "LFO wave produced a nonfinite result"))
        }
    }
}
fn ceil_frame(
    context: &TimingContext,
    t: &TimeValue,
    output: &OutputSettings,
) -> Result<u64, PlanError> {
    let frame = context.ceil_time(t, u64::from(output.sample_rate_hz))?;
    if frame < 0.into() {
        return Err(error("E_TIME_PRECISION", "negative LFO cut frame"));
    }
    Ok(frame.min(output.total_frames.into()).to_u64().unwrap())
}
fn phase_value(context: &TimingContext, cycle: &Rational) -> Result<(bool, f64), PlanError> {
    let whole = cycle.floor();
    let f = bounded(cycle - whole)?;
    let odd = f >= Rational::new(1.into(), 2.into());
    let left = if odd {
        Rational::new(1.into(), 2.into())
    } else {
        Rational::zero()
    };
    let right = &left + Rational::new(1.into(), 2.into());
    let value = approximate(context, &time(f.clone())?)?;
    let progress = approximate(context, &time(bounded(&f - &left)?)?)?;
    let gap = approximate(context, &time(bounded(&right - &f)?)?)?;
    let lo = if odd { 0.5 } else { 0. };
    let hi = lo + 0.5;
    if value < lo
        || value >= hi
        || (progress > 0. && value <= lo)
        || hi - gap >= hi
        || hi - gap < lo
    {
        return Err(error(
            "E_TIME_PRECISION",
            "LFO held phase lost half-cell containment",
        ));
    }
    Ok((odd, value))
}

pub(crate) fn prepare_lfo(
    config: &LfoConfig,
    output: &OutputSettings,
    context: &TimingContext,
    limits: &PlanLimits,
) -> Result<PreparedLfo, PlanError> {
    let candidates = candidate_bound(config, output, context.map.points().len(), limits)?;
    let capacity = usize::try_from(candidates)
        .map_err(|_| error("E_RESOURCE_LIMIT", "LFO candidates exceed address space"))?;
    let final_cycle = end_cycle(config, output)?;
    let first_half = bounded(&config.phase * integer(2))?.floor().to_integer();
    let last_half = bounded(&final_cycle * integer(2))?.floor().to_integer();
    let half_count = (&last_half - &first_half + num_bigint::BigInt::from(1))
        .to_u64()
        .ok_or_else(|| error("E_RESOURCE_LIMIT", "LFO half-cell count exceeds u64"))?;
    let origin = if config.clock == ControlClock::Score {
        output.score_start_q.clone()
    } else {
        Rational::zero()
    };
    let mut cuts = Vec::new();
    cuts.try_reserve(
        capacity
            .checked_add(1)
            .ok_or_else(|| error("E_RESOURCE_LIMIT", "LFO capacity overflow"))?,
    )
    .map_err(|_| error("E_RESOURCE_LIMIT", "LFO allocation failed"))?;
    cuts.push(origin.clone());
    for i in 1..=half_count {
        let cycle = Rational::new(&first_half + num_bigint::BigInt::from(i), 2.into());
        let position =
            bounded(&origin + bounded(&config.period * bounded(cycle - &config.phase)?)?)?;
        if config.clock == ControlClock::Score && position >= output.score_end_q {
            break;
        }
        cuts.push(position);
    }
    if config.clock == ControlClock::Score {
        for p in context.map.points() {
            if p.position_q > origin && p.position_q < output.score_end_q {
                cuts.push(p.position_q.clone());
            }
        }
        if output.score_end_q > origin {
            cuts.push(output.score_end_q.clone());
        }
    }
    cuts.sort();
    cuts.dedup();
    let held = if config.clock == ControlClock::Score {
        let (odd, f) = phase_value(context, &final_cycle)?;
        Some((ceil_frame(context, &context.end, output)?, odd, f))
    } else {
        None
    };
    let mut spans = Vec::new();
    spans
        .try_reserve(capacity)
        .map_err(|_| error("E_RESOURCE_LIMIT", "LFO allocation failed"))?;
    let rate = integer(u64::from(output.sample_rate_hz));
    // Each exact cut uses the clock's canonical actual-tempo prefix. Half cuts
    // never become prefixes, which would split logarithms into distinct recipes.
    let mut base_q = origin.clone();
    let mut base_time = time(Rational::zero())?;
    for pair in cuts.windows(2) {
        let (x, y) = (&pair[0], &pair[1]);
        let (start_time, finish_time, duration, b, s, next) = if config.clock == ControlClock::Score
        {
            let (b, s, next) = context.map.segment_at(x).map_err(music_error)?;
            let start = base_time
                .sum(
                    &context
                        .map
                        .seconds_between(&base_q, x)
                        .map_err(music_error)?,
                )
                .map_err(music_error)?;
            let finish = base_time
                .sum(
                    &context
                        .map
                        .seconds_between(&base_q, y)
                        .map_err(music_error)?,
                )
                .map_err(music_error)?;
            let duration = context.map.seconds_between(x, y).map_err(music_error)?;
            (start, finish, duration, b, s, next)
        } else {
            (
                time(x.clone())?,
                time(y.clone())?,
                time(bounded(y - x)?)?,
                integer(60),
                Rational::zero(),
                None,
            )
        };
        let start = ceil_frame(context, &start_time, output)?;
        let end = ceil_frame(context, &finish_time, output)?;
        let dq = bounded(y - x)?;
        let cycle = bounded(&config.phase + bounded(bounded(x - &origin)? / &config.period)?)?;
        let whole = cycle.floor();
        let a = bounded(cycle - whole)?;
        let h = bounded(&dq / &config.period)?;
        let odd = a >= Rational::new(1.into(), 2.into());
        // Coefficients are checked even for cells with coincident ceil frames.
        let af = approximate(context, &time(a.clone())?)?;
        let hf = approximate(context, &time(h.clone())?)?;
        let upper = approximate(context, &time(bounded(&a + &h)?)?)?;
        fractions(
            context,
            FractionRecipe {
                b: &b,
                s: &s,
                dq: &dq,
                duration: &duration,
            },
            &time(Rational::zero())?,
            &Rational::zero(),
        )?;

        if hf <= 0. || upper <= af {
            return Err(error(
                "E_TIME_PRECISION",
                "LFO cell coefficients lost positive width",
            ));
        }
        if start < end {
            let elapsed = difference(&time(integer(start) / &rate)?, &start_time)?;
            let residual = difference(&finish_time, &time(integer(end - 1) / &rate)?)?;
            if context.compare_times(&elapsed, &time(Rational::zero())?)? == Ordering::Less
                || context.compare_times(&residual, &time(Rational::zero())?)? != Ordering::Greater
            {
                return Err(error("E_TIME_PRECISION", "LFO samples escaped exact cell"));
            }
            let f = fractions(
                context,
                FractionRecipe {
                    b: &b,
                    s: &s,
                    dq: &dq,
                    duration: &duration,
                },
                &elapsed,
                &(integer(end - start - 1) / &rate),
            )?;
            let by = bounded(&b + bounded(&s * &dq)?)?;
            let reverse_s = -&s;
            let backwards = fractions(
                context,
                FractionRecipe {
                    b: &by,
                    s: &reverse_s,
                    dq: &dq,
                    duration: &duration,
                },
                &residual,
                &Rational::zero(),
            )?;
            let progress = positive(hf * f.first, f.first > 0.)?;
            let first = finite(af + progress)?;
            let advance = positive(hf * f.advance, start + 1 < end)?;
            let last = finite(first + advance)?;
            let gap = positive(hf * backwards.first, true)?;
            let backward_last = finite(upper - gap)?;
            let half_right = if odd { 1. } else { 0.5 };
            if first < af
                || first >= upper
                || last >= upper
                || last >= half_right
                || backward_last >= upper
                || backward_last < af
                || (progress > 0. && first <= af)
                || (start + 1 < end && last <= first)
            {
                return Err(error(
                    "E_TIME_PRECISION",
                    "LFO half-cell endpoint lost representable containment",
                ));
            }
            spans.push(PreparedSpan {
                start,
                end,
                first,
                advance,
                dz: f.dz,
                odd,
            });
        }
        if next.as_ref() == Some(y) {
            base_q = y.clone();
            base_time = finish_time;
        }
    }
    let expected = held.map_or(output.total_frames, |(end, _, _)| end);
    let mut covered = 0;
    for span in &spans {
        if span.start != covered {
            return Err(error("E_TIME_PRECISION", "LFO spans leave a frame gap"));
        }
        covered = span.end;
    }
    if covered != expected {
        return Err(error(
            "E_TIME_PRECISION",
            "LFO spans do not cover the render",
        ));
    }
    Ok(PreparedLfo {
        wave: config.wave,
        frames: output.total_frames,
        spans,
        held,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Interpolation, TempoMap, TempoPoint};
    fn r(n: i64, d: i64) -> Rational {
        Rational::new(n.into(), d.into())
    }
    fn output() -> OutputSettings {
        serde_json::from_value(serde_json::json!({"score_start_q":"0/1","score_end_q":"1/1","tail_seconds":"1/1","sample_rate_hz":8,"channels":1,"total_frames":16,"output":{"node":"a","port":"out"}})).unwrap()
    }
    fn config(wave: LfoWave) -> LfoConfig {
        LfoConfig {
            clock: ControlClock::Seconds,
            period: r(1, 1),
            phase: r(0, 1),
            wave,
        }
    }
    fn map() -> TempoMap {
        TempoMap {
            points: vec![TempoPoint {
                q: r(0, 1),
                bpm: r(60, 1),
                shape: Interpolation::Step,
            }],
        }
    }
    #[test]
    fn four_wave_quarter_grid() {
        let out = output();
        let limits = PlanLimits::default();
        let ctx = TimingContext::new_with_limits(&map(), &out, &limits).unwrap();
        for (wave, expected) in [
            (LfoWave::Sine, [0., 1., 0., -1., 0.]),
            (LfoWave::Triangle, [-1., 0., 1., 0., -1.]),
            (LfoWave::Saw, [-1., -0.5, 0., 0.5, -1.]),
            (LfoWave::Square, [1., 1., -1., -1., 1.]),
        ] {
            let p = prepare_lfo(&config(wave), &out, &ctx, &limits).unwrap();
            for (i, e) in expected.into_iter().enumerate() {
                assert!(
                    (p.value_at(i as u64 * 2).unwrap() - e).abs() < 1e-12,
                    "wave {wave:?} frame {}",
                    i * 2
                );
            }
        }
    }
    fn prepared(
        c: &LfoConfig,
        out: &OutputSettings,
        tempo: &TempoMap,
    ) -> Result<PreparedLfo, PlanError> {
        let limits = PlanLimits::default();
        let ctx = TimingContext::new_with_limits(tempo, out, &limits)?;
        prepare_lfo(c, out, &ctx, &limits)
    }
    #[test]
    fn reset_phase_negative_origin_and_tail_clocks() {
        let mut out = output();
        out.score_start_q = r(-3, 1);
        out.score_end_q = r(-2, 1);
        let mut c = config(LfoWave::Saw);
        c.phase = r(1, 4);
        let seconds = prepared(&c, &out, &map()).unwrap();
        c.clock = ControlClock::Score;
        let score = prepared(&c, &out, &map()).unwrap();
        for n in [0, 2, 4, 6] {
            assert!((seconds.value_at(n).unwrap() - score.value_at(n).unwrap()).abs() < 1e-12);
        }
        assert_eq!(score.value_at(0).unwrap(), -0.5);
        assert_eq!(score.value_at(10).unwrap(), -0.5);
        assert_eq!(seconds.value_at(10).unwrap(), 0.);
        let reset = prepared(&c, &out, &map()).unwrap();
        assert_eq!(reset.value_at(0).unwrap(), score.value_at(0).unwrap());
    }
    #[test]
    fn score_step_has_exact_right_owned_half_cuts() {
        let out = output();
        let mut c = config(LfoWave::Square);
        c.clock = ControlClock::Score;
        let tempo = TempoMap {
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
        let p = prepared(&c, &out, &tempo).unwrap();
        for (n, e) in [(0, 1.), (2, 1.), (3, -1.), (4, -1.), (5, 1.), (15, 1.)] {
            assert_eq!(p.value_at(n).unwrap(), e, "frame {n}");
        }
    }
    #[test]
    fn score_ramps_match_independent_analytical_inverse() {
        for (b, end) in [(60, 120000000), (120, 60000000), (60, 60000001)] {
            let mut out = output();
            out.sample_rate_hz = 32;
            out.total_frames = 64;
            let mut c = config(LfoWave::Saw);
            c.clock = ControlClock::Score;
            c.period = r(1, 2);
            c.phase = r(1, 10);
            let tempo = TempoMap {
                points: vec![
                    TempoPoint {
                        q: r(0, 1),
                        bpm: r(b, 1),
                        shape: Interpolation::Linear,
                    },
                    TempoPoint {
                        q: r(1, 1),
                        bpm: r(end, 1000000),
                        shape: Interpolation::Step,
                    },
                ],
            };
            let p = prepared(&c, &out, &tempo).unwrap();
            let slope = end as f64 / 1e6 - b as f64;
            let end_seconds = 60. / slope * ((end as f64 / 1e6) / (b as f64)).ln();
            for n in 0..64 {
                let q = if n as f64 / 32. >= end_seconds {
                    1.
                } else {
                    b as f64 / slope * (slope * n as f64 / (60. * 32.)).exp_m1()
                };
                let expected = 2. * (0.1 + 2. * q).fract() - 1.;
                assert!(
                    (p.value_at(n).unwrap() - expected).abs() < 2e-8,
                    "b={b} end={end} frame={n}"
                );
            }
        }
    }
    #[test]
    fn fractional_score_end_holds_exact_phase() {
        let mut out = output();
        out.score_end_q = r(1, 10);
        let mut c = config(LfoWave::Triangle);
        c.clock = ControlClock::Score;
        c.phase = r(1, 8);
        let p = prepared(&c, &out, &map()).unwrap();
        assert_eq!(p.value_at(0).unwrap(), -0.5);
        for n in 1..16 {
            assert!((p.value_at(n).unwrap() + 0.1).abs() < 1e-12);
        }
    }
    #[test]
    fn hostile_fields_and_candidate_budget_are_rejected_before_math() {
        let out = output();
        let limits = PlanLimits::default();
        let mut c = config(LfoWave::Sine);
        for phase in [r(-1, 2), r(1, 1), r(3, 2)] {
            c.phase = phase;
            assert_eq!(
                candidate_bound(&c, &out, 1, &limits).unwrap_err().code,
                "E_RANGE"
            );
        }
        c.phase = r(0, 1);
        for period in [r(0, 1), r(-1, 1)] {
            c.period = period;
            assert_eq!(
                candidate_bound(&c, &out, 1, &limits).unwrap_err().code,
                "E_RANGE"
            );
        }
        c.period = Rational::new(1.into(), num_bigint::BigInt::from(1) << 4096);
        assert_eq!(
            candidate_bound(&c, &out, 1, &limits).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
        c.period = r(1, 1000000000);
        assert_eq!(
            candidate_bound(&c, &out, 1, &limits).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
        c.period = r(1, 1);
        assert_eq!(
            candidate_bound(
                &c,
                &out,
                1,
                &PlanLimits {
                    max_work: 127,
                    ..limits
                }
            )
            .unwrap_err()
            .code,
            "E_RESOURCE_LIMIT"
        );
    }
    #[test]
    fn coincident_cut_frames_are_certified_and_omitted() {
        let mut out = output();
        out.sample_rate_hz = 1;
        out.total_frames = 2;
        let mut c = config(LfoWave::Square);
        c.period = r(1, 10);
        let p = prepared(&c, &out, &map()).unwrap();
        assert_eq!(p.value_at(0).unwrap(), 1.);
        assert_eq!(p.value_at(1).unwrap(), 1.);
        out.total_frames = 0;
        let p = prepared(&c, &out, &map()).unwrap();
        assert_eq!(p.value_at(0).unwrap_err().code, "E_RANGE");
    }
    #[test]
    fn shared_timing_budget_is_not_replaced() {
        let out = output();
        let mut c = config(LfoWave::Saw);
        c.clock = ControlClock::Score;
        let limits = PlanLimits::default();
        let mut tempo = map();
        tempo.points[0].shape = Interpolation::Linear;
        tempo.points.push(TempoPoint {
            q: r(1, 1),
            bpm: r(120, 1),
            shape: Interpolation::Step,
        });
        let ctx = TimingContext::new_with_limits(
            &tempo,
            &out,
            &PlanLimits {
                max_work: 0,
                ..limits
            },
        )
        .unwrap();
        assert_eq!(
            prepare_lfo(&c, &out, &ctx, &limits).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
    }
    #[test]
    fn positive_endpoint_residual_cannot_round_to_half_cell_right() {
        let out = output();
        let mut c = config(LfoWave::Saw);
        let tiny = Rational::new(1.into(), num_bigint::BigInt::from(1) << 1200);
        c.period = r(1, 4) + tiny;
        assert_eq!(
            prepared(&c, &out, &map()).unwrap_err().code,
            "E_TIME_PRECISION"
        );
    }
    #[test]
    fn candidate_counts_include_held_boundary_and_all_tempo_points() {
        let mut out = output();
        let mut c = config(LfoWave::Sine);
        let limits = PlanLimits::default();
        assert_eq!(candidate_bound(&c, &out, 99, &limits).unwrap(), 4);
        c.clock = ControlClock::Score;
        assert_eq!(candidate_bound(&c, &out, 99, &limits).unwrap(), 102);
        c.phase = r(3, 4);
        assert_eq!(candidate_bound(&c, &out, 99, &limits).unwrap(), 102);
        c.clock = ControlClock::Seconds;
        out.total_frames = 0;
        assert_eq!(candidate_bound(&c, &out, 99, &limits).unwrap(), 1);
    }
    #[test]
    fn fractional_ramp_origin_uses_canonical_tempo_prefix() {
        let mut out = output();
        out.score_start_q = r(1, 10);
        out.score_end_q = r(3, 5);
        out.total_frames = 16;
        let mut c = config(LfoWave::Saw);
        c.clock = ControlClock::Score;
        c.period = r(1, 5);
        c.phase = r(1, 7);
        let tempo = TempoMap {
            points: vec![
                TempoPoint {
                    q: r(0, 1),
                    bpm: r(60, 1),
                    shape: Interpolation::Linear,
                },
                TempoPoint {
                    q: r(1, 1),
                    bpm: r(120, 1),
                    shape: Interpolation::Step,
                },
            ],
        };
        let p = prepared(&c, &out, &tempo).unwrap();
        for n in 0..16 {
            let delta = (1.1 * (n as f64 / 8.).exp() - 1.1).min(0.5);
            let expected = 2. * (1. / 7. + delta / 0.2).fract() - 1.;
            assert!(
                (p.value_at(n).unwrap() - expected).abs() < 1e-12,
                "frame {n}"
            );
        }
    }
    #[test]
    fn huge_score_origin_is_subtracted_exactly() {
        let origin = Rational::from_integer(num_bigint::BigInt::from(1) << 1000);
        let mut out = output();
        out.score_start_q = origin.clone();
        out.score_end_q = &origin + r(1, 1);
        let mut c = config(LfoWave::Saw);
        c.clock = ControlClock::Score;
        let tempo = TempoMap {
            points: vec![
                TempoPoint {
                    q: origin.clone(),
                    bpm: r(60, 1),
                    shape: Interpolation::Linear,
                },
                TempoPoint {
                    q: origin + r(1, 1),
                    bpm: r(120, 1),
                    shape: Interpolation::Step,
                },
            ],
        };
        let p = prepared(&c, &out, &tempo).unwrap();
        assert!((p.value_at(1).unwrap() - (2. * 0.125f64.exp_m1() - 1.)).abs() < 1e-12);
    }
    #[test]
    fn ramp_backward_residual_cannot_round_back_to_cut() {
        let mut out = output();
        out.sample_rate_hz = 48000;
        out.total_frames = 2;
        out.tail_seconds = r(0, 1);
        let mut c = config(LfoWave::Saw);
        c.clock = ControlClock::Score;
        c.period = r(2, 1);
        let b = Rational::new(
            "9930385589350642984058755095404800790126870706742824492072954375"
                .parse()
                .unwrap(),
            "6277101735386680763835789423207666416102355444464034512896"
                .parse()
                .unwrap(),
        );
        let tempo = TempoMap {
            points: vec![
                TempoPoint {
                    q: r(0, 1),
                    bpm: b.clone(),
                    shape: Interpolation::Linear,
                },
                TempoPoint {
                    q: r(1, 1),
                    bpm: b * r(3, 1),
                    shape: Interpolation::Step,
                },
            ],
        };
        assert_eq!(
            prepared(&c, &out, &tempo).unwrap_err().code,
            "E_TIME_PRECISION"
        );
    }
}
