//! Certified linear-in-score tempo integration, additive to `music::TempoMap`.
//!
//! Inputs have at most 4096 bits; this isolated helper allows 32768-bit reduced
//! intermediates (arithmetic temporaries at most 65538 bits), 4096 points
//! and log terms. Certification refines at 64,128,256,512,1024 fractional bits.
//! Each logarithm uses at most 1024 series terms per refinement. DSP conversion
//! is deliberately separate from certified comparisons and signed frame ceilings.
use crate::{
    music::{MusicError, MusicErrorCode, TempoPoint, TempoShape},
    Rational,
};
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};
use std::{cmp::Ordering, collections::BTreeMap};
const WORK_BITS: u64 = 32768;
const MAX_TERMS: usize = 4096;
const MAX_TIMING_WORK: u64 = 20_000_000;
fn error(code: MusicErrorCode, message: &str) -> MusicError {
    MusicError {
        code,
        message: message.into(),
    }
}
fn check(x: &Rational, bits: u64) -> Result<(), MusicError> {
    if x.denom() <= &BigInt::zero() {
        return Err(error(
            MusicErrorCode::Range,
            "rational denominator must be positive",
        ));
    }
    if x.numer().bits() > bits || x.denom().bits() > bits {
        return Err(error(
            MusicErrorCode::ResourceLimit,
            "tempo rational bit allowance exceeded",
        ));
    }
    Ok(())
}
fn bounded(x: Rational) -> Result<Rational, MusicError> {
    check(&x, WORK_BITS)?;
    Ok(x)
}
fn integer(x: i64) -> Rational {
    Rational::from_integer(x.into())
}

/// Per-operation allowance for certified logarithm-series work.
///
/// The allowance is measured in fractional-bit terms and is capped at the
/// implementation maximum. Exact rational timing paths do not consume it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimingBudget {
    remaining: u64,
}

impl TimingBudget {
    pub fn new(terms: u64) -> Self {
        Self {
            remaining: terms.min(MAX_TIMING_WORK),
        }
    }

    pub fn remaining_terms(&self) -> u64 {
        self.remaining
    }

    fn reserve(&mut self, terms: u64) -> Result<(), MusicError> {
        if terms > self.remaining {
            return Err(error(
                MusicErrorCode::ResourceLimit,
                "tempo numerical work allowance exceeded",
            ));
        }
        self.remaining -= terms;
        Ok(())
    }
}

/// Exact rational plus a bounded finite sum of rational multiples of logarithms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeValue {
    rational: Rational,
    logs: BTreeMap<Rational, Rational>,
}
impl TimeValue {
    pub fn from_rational(value: Rational) -> Result<Self, MusicError> {
        check(&value, 4096)?;
        Ok(Self {
            rational: Rational::new(value.numer().clone(), value.denom().clone()),
            logs: BTreeMap::new(),
        })
    }
    pub fn as_rational(&self) -> Option<&Rational> {
        self.logs.is_empty().then_some(&self.rational)
    }
    pub fn negated(&self) -> Self {
        Self {
            rational: -&self.rational,
            logs: self.logs.iter().map(|(a, c)| (a.clone(), -c)).collect(),
        }
    }
    pub fn add_offset(&self, offset: &Rational) -> Result<Self, MusicError> {
        check(offset, 4096)?;
        let mut t = self.clone();
        t.rational = bounded(&t.rational + offset)?;
        Ok(t)
    }
    pub fn sum(&self, other: &Self) -> Result<Self, MusicError> {
        let mut t = self.clone();
        t.rational = bounded(&t.rational + &other.rational)?;
        for (a, c) in &other.logs {
            t.add_log(a.clone(), c.clone())?;
        }
        Ok(t)
    }
    pub fn difference(&self, other: &Self) -> Result<Self, MusicError> {
        self.sum(&other.negated())
    }
    fn add_log(&mut self, mut arg: Rational, mut coefficient: Rational) -> Result<(), MusicError> {
        check(&arg, WORK_BITS)?;
        check(&coefficient, WORK_BITS)?;
        if arg <= Rational::zero() {
            return Err(error(
                MusicErrorCode::Tempo,
                "nonpositive logarithm argument",
            ));
        }
        if arg < Rational::one() {
            arg = arg.recip();
            coefficient = -coefficient;
        }
        if arg.is_one() || coefficient.is_zero() {
            return Ok(());
        }
        let c = bounded(self.logs.get(&arg).cloned().unwrap_or_else(Rational::zero) + coefficient)?;
        if c.is_zero() {
            self.logs.remove(&arg);
        } else {
            if !self.logs.contains_key(&arg) && self.logs.len() >= MAX_TERMS {
                return Err(error(
                    MusicErrorCode::ResourceLimit,
                    "tempo logarithm term allowance exceeded",
                ));
            }
            self.logs.insert(arg, c);
        }
        Ok(())
    }
    fn bounds(
        &self,
        p: usize,
        budget: &mut TimingBudget,
    ) -> Result<(Rational, Rational), MusicError> {
        if self.logs.is_empty() {
            return Ok((self.rational.clone(), self.rational.clone()));
        }
        let work = (self.logs.len() as u64 + 1)
            .checked_mul(p as u64)
            .ok_or_else(|| {
                error(
                    MusicErrorCode::ResourceLimit,
                    "tempo numerical work allowance exceeded",
                )
            })?;
        budget.reserve(work)?;
        let scale = BigInt::one() << p;
        let mut lo = self.rational.clone();
        let mut hi = lo.clone();
        let log_two = unit_log(&integer(2), p);
        for (arg, c) in &self.logs {
            let (l, h) = log_bounds(arg, p, &log_two);
            let (l, h) = if c.is_negative() { (h, l) } else { (l, h) };
            lo = bounded(lo + bounded(c * Rational::new(l, scale.clone()))?)?;
            hi = bounded(hi + bounded(c * Rational::new(h, scale.clone()))?)?;
        }
        Ok((lo, hi))
    }
    pub fn compare(&self, other: &Self) -> Result<Ordering, MusicError> {
        self.compare_with_budget(other, &mut TimingBudget::new(MAX_TIMING_WORK))
    }
    pub fn compare_with_budget(
        &self,
        other: &Self,
        budget: &mut TimingBudget,
    ) -> Result<Ordering, MusicError> {
        let d = self.difference(other)?;
        if let Some(r) = d.as_rational() {
            return Ok(r.cmp(&Rational::zero()));
        }
        for p in [64, 128, 256, 512, 1024] {
            let (l, h) = d.bounds(p, budget)?;
            if l.is_positive() {
                return Ok(Ordering::Greater);
            }
            if h.is_negative() {
                return Ok(Ordering::Less);
            }
        }
        Err(error(
            MusicErrorCode::TimePrecision,
            "tempo ordering unresolved at 1024 bits",
        ))
    }
    pub fn ceil_frames(&self, rate: u64) -> Result<BigInt, MusicError> {
        self.ceil_frames_with_budget(rate, &mut TimingBudget::new(MAX_TIMING_WORK))
    }
    pub fn ceil_frames_with_budget(
        &self,
        rate: u64,
        budget: &mut TimingBudget,
    ) -> Result<BigInt, MusicError> {
        self.ceil_frames_with_precision_and_budget(rate, 1024, budget)
    }
    /// Lower the certification ceiling for callers with smaller work allowances.
    /// Allowed caps are 64,128,256,512,1024 bits; never guesses on exhaustion.
    pub fn ceil_frames_with_precision(
        &self,
        rate: u64,
        max_bits: usize,
    ) -> Result<BigInt, MusicError> {
        self.ceil_frames_with_precision_and_budget(
            rate,
            max_bits,
            &mut TimingBudget::new(MAX_TIMING_WORK),
        )
    }
    fn ceil_frames_with_precision_and_budget(
        &self,
        rate: u64,
        max_bits: usize,
        budget: &mut TimingBudget,
    ) -> Result<BigInt, MusicError> {
        if ![64, 128, 256, 512, 1024].contains(&max_bits) {
            return Err(error(MusicErrorCode::Range, "invalid tempo precision cap"));
        }
        if rate == 0 {
            return Err(error(MusicErrorCode::Range, "sample rate must be positive"));
        }
        let rate = BigInt::from(rate);
        let ceil = |r: Rational| (r.numer() * &rate).div_ceil(r.denom());
        if let Some(r) = self.as_rational() {
            return Ok(ceil(r.clone()));
        }
        for p in [64, 128, 256, 512, 1024]
            .into_iter()
            .filter(|p| *p <= max_bits)
        {
            let (l, h) = self.bounds(p, budget)?;
            let l = ceil(l);
            let h = ceil(h);
            if l == h {
                return Ok(l);
            }
        }
        Err(error(
            MusicErrorCode::TimePrecision,
            "frame boundary unresolved at precision allowance",
        ))
    }
    /// DSP-only midpoint approximation with interval width at most
    /// `2^-48 * max(1, |lower|, |upper|)`, plus the final f64 rounding.
    /// Never use this result for scheduling. Unresolved or nonfinite values fail.
    pub fn approximate_seconds(&self) -> Result<f64, MusicError> {
        self.approximate_seconds_with_budget(&mut TimingBudget::new(MAX_TIMING_WORK))
    }
    pub fn approximate_seconds_with_budget(
        &self,
        budget: &mut TimingBudget,
    ) -> Result<f64, MusicError> {
        for p in [64, 128, 256, 512, 1024] {
            let (l, h) = self.bounds(p, budget)?;
            let magnitude = Rational::one().max(l.abs()).max(h.abs());
            if bounded(&h - &l)?
                <= bounded(magnitude / Rational::from_integer(BigInt::one() << 48))?
            {
                let mid = bounded(bounded(l + h)? / integer(2))?;
                return mid.to_f64().filter(|v| v.is_finite()).ok_or_else(|| {
                    error(
                        MusicErrorCode::TimePrecision,
                        "tempo DSP value is not finite",
                    )
                });
            }
        }
        Err(error(
            MusicErrorCode::TimePrecision,
            "tempo DSP approximation unresolved at 1024 bits",
        ))
    }
}
// For 1<=x<=2, z=(x-1)/(x+1)<=1/3. All fixed-point operations
// round outward. After N terms, 2*sum z^(2j+1)/(2j+1) has tail
// <= 2*z^(2N+1)/(1-z²) <= 3/9^N. N=p is finite and conservative.
fn unit_log(x: &Rational, p: usize) -> (BigInt, BigInt) {
    let s = BigInt::one() << p;
    let z = Rational::new(x.numer() - x.denom(), x.numer() + x.denom());
    let zl = (z.numer() * &s).div_floor(z.denom());
    let zh = (z.numer() * &s).div_ceil(z.denom());
    let z2l = (&zl * &zl).div_floor(&s);
    let z2h = (&zh * &zh).div_ceil(&s);
    let (mut pl, mut ph) = (zl, zh);
    let (mut lo, mut hi) = (BigInt::zero(), BigInt::zero());
    for j in 0..p {
        let d = BigInt::from(2 * j + 1);
        lo += (&pl * 2u8).div_floor(&d);
        hi += (&ph * 2u8).div_ceil(&d);
        pl = (pl * &z2l).div_floor(&s);
        ph = (ph * &z2h).div_ceil(&s);
    }
    hi += (&s * 3u8).div_ceil(&BigInt::from(9u8).pow(p as u32));
    (lo, hi)
}
fn log_bounds(x: &Rational, p: usize, log_two: &(BigInt, BigInt)) -> (BigInt, BigInt) {
    // TimeValue canonicalizes all log arguments to >=1.
    let mut k = x.numer().bits() as usize - x.denom().bits() as usize;
    if x.numer() < &(x.denom() << k) {
        k -= 1;
    }
    let y = Rational::new(x.numer().clone(), x.denom() << k);
    let (l, h) = unit_log(&y, p);
    let (a, b) = log_two;
    (l + a * k, h + b * k)
}
/// Tempo clock accepting both step and linear-in-score segments.
#[derive(Clone, Debug)]
pub struct RampTempoMap {
    points: Vec<TempoPoint>,
}
impl RampTempoMap {
    pub fn new(mut points: Vec<TempoPoint>) -> Result<Self, MusicError> {
        if points.is_empty() {
            return Err(error(
                MusicErrorCode::Tempo,
                "tempo points must be nonempty",
            ));
        }
        if points.len() > MAX_TERMS {
            return Err(error(
                MusicErrorCode::ResourceLimit,
                "tempo point allowance exceeded",
            ));
        }
        for point in &points {
            check(&point.position_q, 4096)?;
            check(&point.bpm, 4096)?;
        }
        for point in &mut points {
            point.position_q = Rational::new(
                point.position_q.numer().clone(),
                point.position_q.denom().clone(),
            );
            point.bpm = Rational::new(point.bpm.numer().clone(), point.bpm.denom().clone());
        }
        if points.iter().any(|p| p.bpm <= Rational::zero())
            || points
                .windows(2)
                .any(|p| p[0].position_q >= p[1].position_q)
            || points.last().unwrap().shape != TempoShape::Step
        {
            return Err(error(
                MusicErrorCode::Tempo,
                "tempo requires positive BPM, ordered points, and final step",
            ));
        }
        Ok(Self { points })
    }
    pub fn points(&self) -> &[TempoPoint] {
        &self.points
    }
    pub fn has_linear(&self) -> bool {
        self.points.iter().any(|p| p.shape == TempoShape::Linear)
    }
    pub fn seconds_at(&self, q: &Rational) -> Result<TimeValue, MusicError> {
        self.seconds_between(&Rational::zero(), q)
    }
    /// Integrate locally, avoiding subtraction of large absolute clock values.
    pub fn seconds_between(
        &self,
        start: &Rational,
        end: &Rational,
    ) -> Result<TimeValue, MusicError> {
        check(start, 4096)?;
        check(end, 4096)?;
        if start > end {
            return Ok(self.seconds_between(end, start)?.negated());
        }
        let mut result = TimeValue::from_rational(Rational::zero())?;
        let mut cursor = start.clone();
        while cursor < *end {
            let next = self.points.partition_point(|p| p.position_q <= cursor);
            let stop = if next < self.points.len() {
                end.min(&self.points[next].position_q)
            } else {
                end
            };
            let point = &self.points[next.saturating_sub(1)];
            let distance = bounded(stop - &cursor)?;
            if next > 0
                && next < self.points.len()
                && point.shape == TempoShape::Linear
                && point.bpm != self.points[next].bpm
            {
                let slope = bounded(
                    bounded(&self.points[next].bpm - &point.bpm)?
                        / bounded(&self.points[next].position_q - &point.position_q)?,
                )?;
                let begin =
                    bounded(&point.bpm + bounded(&slope * bounded(&cursor - &point.position_q)?)?)?;
                let finish =
                    bounded(&point.bpm + bounded(&slope * bounded(stop - &point.position_q)?)?)?;
                result.add_log(bounded(finish / begin)?, bounded(integer(60) / slope)?)?;
            } else {
                result.rational = bounded(
                    &result.rational + bounded(bounded(integer(60) * distance)? / &point.bpm)?,
                )?;
            }
            cursor = stop.clone();
        }
        Ok(result)
    }
}
