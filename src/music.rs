//! Exact musical coordinate helpers.
//!
//! This module contains the small, deterministic part of ScoreIR's musical
//! model that is useful to both the semantic compiler and the renderer:
//! tempo/meter coordinates and pitch/tuning resolution.  Source parsing and
//! diagnostics are deliberately kept outside of this module.  In particular,
//! the arithmetic type is the public `num_rational::BigRational` type so that
//! callers can share the exact-value implementation used by the parser.

use std::fmt;
use std::sync::Arc;

use num_bigint::BigInt;
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

/// Maximum numerator/denominator bit length accepted by the musical helpers.
///
/// ScoreIR implementations must publish a hard rational resource limit.  A
/// caller that needs a different limit can apply the same checks before
/// constructing these values; the foundation implementation uses 4096 bits.
pub const MAX_RATIONAL_BITS: u64 = 4096;

/// Shared name used by semantic callers that already import the exact module
/// under its shorter alias.
pub type Rational = BigRational;

/// Stable diagnostic codes emitted by this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicErrorCode {
    /// A value is outside a declared range.
    Range,
    /// Tempo points or values are malformed.
    Tempo,
    /// A meter change is not on a bar boundary.
    MeterBoundary,
    /// The requested feature is recognized but outside the foundation
    /// capability profile (currently linear tempo integration).
    Capability,
    /// A pitch or tuning declaration is malformed.
    Pitch,
    /// A frequency/result is non-finite.
    Nonfinite,
    /// A rational or collection exceeded the published host bound.
    ResourceLimit,
    /// A value could not be converted at the requested numerical boundary.
    TimePrecision,
}

impl MusicErrorCode {
    /// Return the wire-level ScoreIR code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Range => "E_RANGE",
            Self::Tempo => "E_TEMPO",
            Self::MeterBoundary => "E_METER_BOUNDARY",
            Self::Capability => "E_CAPABILITY",
            // ScoreIR/1 has no separate pitch diagnostic; malformed or out of
            // range pitch values use the general declared-range code.
            Self::Pitch => "E_RANGE",
            Self::Nonfinite => "E_NONFINITE",
            Self::ResourceLimit => "E_RESOURCE_LIMIT",
            Self::TimePrecision => "E_TIME_PRECISION",
        }
    }
}

impl fmt::Display for MusicErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned by exact clock and pitch operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicError {
    pub code: MusicErrorCode,
    pub message: String,
}

impl MusicError {
    fn new(code: MusicErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn range(message: impl Into<String>) -> Self {
        Self::new(MusicErrorCode::Range, message)
    }

    fn tempo(message: impl Into<String>) -> Self {
        Self::new(MusicErrorCode::Tempo, message)
    }

    fn meter_boundary(message: impl Into<String>) -> Self {
        Self::new(MusicErrorCode::MeterBoundary, message)
    }

    fn capability(message: impl Into<String>) -> Self {
        Self::new(MusicErrorCode::Capability, message)
    }

    fn pitch(message: impl Into<String>) -> Self {
        Self::new(MusicErrorCode::Pitch, message)
    }

    fn nonfinite(message: impl Into<String>) -> Self {
        Self::new(MusicErrorCode::Nonfinite, message)
    }

    fn resource(message: impl Into<String>) -> Self {
        Self::new(MusicErrorCode::ResourceLimit, message)
    }

    fn precision(message: impl Into<String>) -> Self {
        Self::new(MusicErrorCode::TimePrecision, message)
    }
}

impl fmt::Display for MusicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MusicError {}

/// A checked exact rational operation.
///
/// `BigRational` itself has no bit-size limit.  Keeping construction and
/// arithmetic here makes it difficult for a semantic caller to accidentally
/// bypass the published 4096-bit bound.
fn ensure_rational(value: &BigRational) -> Result<(), MusicError> {
    let numerator_bits = value.numer().magnitude().bits();
    let denominator_bits = value.denom().magnitude().bits();
    if numerator_bits > MAX_RATIONAL_BITS || denominator_bits > MAX_RATIONAL_BITS {
        return Err(MusicError::resource(format!(
            "rational exceeds {MAX_RATIONAL_BITS}-bit numerator/denominator bound"
        )));
    }
    Ok(())
}

fn ensure_operands(a: &BigRational, b: &BigRational) -> Result<(), MusicError> {
    ensure_rational(a)?;
    ensure_rational(b)
}

fn checked_add(a: &BigRational, b: &BigRational) -> Result<BigRational, MusicError> {
    ensure_operands(a, b)?;
    // The unreduced cross products are intermediates too.  Reject a request
    // that would exceed the host bound even when a later gcd could cancel it.
    if a.numer().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || b.numer().magnitude().bits() + a.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || a.denom().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(MusicError::resource(
            "rational addition intermediate exceeds bit bound",
        ));
    }
    let result = a + b;
    ensure_rational(&result)?;
    Ok(result)
}

fn checked_sub(a: &BigRational, b: &BigRational) -> Result<BigRational, MusicError> {
    ensure_operands(a, b)?;
    if a.numer().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || b.numer().magnitude().bits() + a.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || a.denom().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(MusicError::resource(
            "rational subtraction intermediate exceeds bit bound",
        ));
    }
    let result = a - b;
    ensure_rational(&result)?;
    Ok(result)
}

fn checked_mul(a: &BigRational, b: &BigRational) -> Result<BigRational, MusicError> {
    ensure_operands(a, b)?;
    if a.numer().magnitude().bits() + b.numer().magnitude().bits() > MAX_RATIONAL_BITS
        || a.denom().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(MusicError::resource(
            "rational multiplication intermediate exceeds bit bound",
        ));
    }
    let result = a * b;
    ensure_rational(&result)?;
    Ok(result)
}

fn checked_div(a: &BigRational, b: &BigRational) -> Result<BigRational, MusicError> {
    ensure_operands(a, b)?;
    if b.is_zero() {
        return Err(MusicError::range("division by zero"));
    }
    if a.numer().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || a.denom().magnitude().bits() + b.numer().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(MusicError::resource(
            "rational division intermediate exceeds bit bound",
        ));
    }
    let result = a / b;
    ensure_rational(&result)?;
    Ok(result)
}

fn checked_mul_integer(a: &BigRational, integer: i64) -> Result<BigRational, MusicError> {
    let b = BigRational::from_integer(BigInt::from(integer));
    checked_mul(a, &b)
}

fn rational_i64(value: i64) -> BigRational {
    BigRational::from_integer(BigInt::from(value))
}

fn rational_u32(value: u32) -> BigRational {
    BigRational::from_integer(BigInt::from(value))
}

/// The shape of the outgoing tempo segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempoShape {
    /// Constant BPM until the next point.
    Step,
    /// Linear BPM in score-time.  The foundation recognizes the shape but
    /// does not approximate its logarithmic integral.
    Linear,
}

/// One tempo-map point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempoPoint {
    pub position_q: BigRational,
    pub bpm: BigRational,
    pub shape: TempoShape,
}

impl TempoPoint {
    pub fn new(position_q: BigRational, bpm: BigRational, shape: TempoShape) -> Self {
        Self {
            position_q,
            bpm,
            shape,
        }
    }
}

/// A validated exact tempo map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempoMap {
    points: Vec<TempoPoint>,
    has_linear: bool,
}

impl TempoMap {
    /// Validate and construct a tempo map.
    ///
    /// Linear points are recognized but rejected with `E_CAPABILITY` because
    /// their logarithmic integral is not an exact rational.  No floating
    /// approximation is silently substituted.
    pub fn new(points: Vec<TempoPoint>) -> Result<Self, MusicError> {
        if points.is_empty() {
            return Err(MusicError::tempo("tempo.points must be nonempty"));
        }
        let mut previous: Option<&BigRational> = None;
        let mut has_linear = false;
        for (index, point) in points.iter().enumerate() {
            ensure_rational(&point.position_q)?;
            ensure_rational(&point.bpm)?;
            if point.bpm <= BigRational::zero() {
                return Err(MusicError::tempo(format!(
                    "tempo point {index} has nonpositive BPM"
                )));
            }
            if let Some(previous) = previous {
                if point.position_q <= *previous {
                    return Err(MusicError::tempo(
                        "tempo point positions must be strictly increasing",
                    ));
                }
            }
            if point.shape == TempoShape::Linear {
                has_linear = true;
            }
            previous = Some(&point.position_q);
        }
        if points
            .last()
            .is_some_and(|point| point.shape != TempoShape::Step)
        {
            return Err(MusicError::tempo(
                "the final tempo point must have step shape",
            ));
        }
        if has_linear {
            return Err(MusicError::capability(
                "linear tempo integration is outside the exact rational foundation profile",
            ));
        }
        Ok(Self { points, has_linear })
    }

    pub fn points(&self) -> &[TempoPoint] {
        &self.points
    }

    pub fn has_linear(&self) -> bool {
        self.has_linear
    }

    fn ensure_resolvable(&self) -> Result<(), MusicError> {
        if self.has_linear {
            Err(MusicError::capability(
                "linear tempo integration is outside the exact rational foundation profile",
            ))
        } else {
            Ok(())
        }
    }

    /// Find the step tempo active at a score position.
    fn bpm_at(&self, q: &BigRational) -> &BigRational {
        let index = self.points.partition_point(|point| point.position_q <= *q);
        if index == 0 {
            &self.points[0].bpm
        } else {
            &self.points[index - 1].bpm
        }
    }

    /// Integrate one positive step-tempo interval `[start, end]`.
    fn forward_seconds(
        &self,
        start: &BigRational,
        end: &BigRational,
    ) -> Result<BigRational, MusicError> {
        debug_assert!(start <= end);
        let mut cursor = start.clone();
        let mut total = BigRational::zero();
        let mut index = self
            .points
            .partition_point(|point| point.position_q <= *start);
        let mut bpm = self.bpm_at(start).clone();
        while index < self.points.len() && self.points[index].position_q < *end {
            let boundary = &self.points[index].position_q;
            let distance = checked_sub(boundary, &cursor)?;
            let seconds = checked_div(&checked_mul_integer(&distance, 60)?, &bpm)?;
            total = checked_add(&total, &seconds)?;
            cursor = boundary.clone();
            bpm = self.points[index].bpm.clone();
            index += 1;
        }
        let distance = checked_sub(end, &cursor)?;
        let seconds = checked_div(&checked_mul_integer(&distance, 60)?, &bpm)?;
        checked_add(&total, &seconds)
    }

    /// Convert a score position in quarter notes to exact seconds, with
    /// `T(0) = 0` and endpoint tempos held beyond the first/last point.
    pub fn seconds_at(&self, q: &BigRational) -> Result<BigRational, MusicError> {
        self.ensure_resolvable()?;
        ensure_rational(q)?;
        if q.is_zero() {
            return Ok(BigRational::zero());
        }
        if q.is_positive() {
            self.forward_seconds(&BigRational::zero(), q)
        } else {
            Ok(-self.forward_seconds(q, &BigRational::zero())?)
        }
    }

    /// Convert exact seconds back to score quarter notes.
    pub fn q_at(&self, seconds: &BigRational) -> Result<BigRational, MusicError> {
        self.ensure_resolvable()?;
        ensure_rational(seconds)?;
        if seconds.is_zero() {
            return Ok(BigRational::zero());
        }
        if seconds.is_positive() {
            self.inverse_forward(seconds)
        } else {
            self.inverse_backward(&(-seconds))
        }
    }

    fn inverse_forward(&self, target_seconds: &BigRational) -> Result<BigRational, MusicError> {
        debug_assert!(target_seconds.is_positive());
        let mut cursor = BigRational::zero();
        let mut elapsed = BigRational::zero();
        let mut bpm = self.bpm_at(&cursor).clone();
        let mut index = self
            .points
            .partition_point(|point| point.position_q <= cursor);

        while index < self.points.len() {
            let boundary = self.points[index].position_q.clone();
            let distance = checked_sub(&boundary, &cursor)?;
            let segment_seconds = checked_div(&checked_mul_integer(&distance, 60)?, &bpm)?;
            let candidate = checked_add(&elapsed, &segment_seconds)?;
            if target_seconds <= &candidate {
                let remaining = checked_sub(target_seconds, &elapsed)?;
                let q_delta = checked_div(&checked_mul(&remaining, &bpm)?, &rational_i64(60))?;
                return checked_add(&cursor, &q_delta);
            }
            elapsed = candidate;
            cursor = boundary;
            bpm = self.points[index].bpm.clone();
            index += 1;
        }

        // The final step extends indefinitely.  q_delta = seconds * bpm / 60.
        let remaining = checked_sub(target_seconds, &elapsed)?;
        let q_delta = checked_div(&checked_mul(&remaining, &bpm)?, &rational_i64(60))?;
        checked_add(&cursor, &q_delta)
    }

    fn inverse_backward(&self, target_seconds: &BigRational) -> Result<BigRational, MusicError> {
        debug_assert!(target_seconds.is_positive());
        let mut cursor = BigRational::zero();
        let mut elapsed = BigRational::zero();

        loop {
            // A point at `cursor` starts the interval after it.  For the
            // backward interval, therefore, choose the greatest point that
            // is strictly before the cursor.
            let next_index = self
                .points
                .partition_point(|point| point.position_q < cursor);
            if next_index == 0 {
                // The first point's tempo is held indefinitely before its
                // position (including when that position is positive).
                let bpm = self.points[0].bpm.clone();
                let remaining = checked_sub(target_seconds, &elapsed)?;
                let q_delta = checked_div(&checked_mul(&remaining, &bpm)?, &rational_i64(60))?;
                return checked_sub(&cursor, &q_delta);
            }

            let boundary = self.points[next_index - 1].position_q.clone();
            let bpm = self.points[next_index - 1].bpm.clone();
            let distance = checked_sub(&cursor, &boundary)?;
            let segment_seconds = checked_div(&checked_mul_integer(&distance, 60)?, &bpm)?;
            let candidate = checked_add(&elapsed, &segment_seconds)?;
            if target_seconds <= &candidate {
                let remaining = checked_sub(target_seconds, &elapsed)?;
                let q_delta = checked_div(&checked_mul(&remaining, &bpm)?, &rational_i64(60))?;
                return checked_sub(&cursor, &q_delta);
            }
            elapsed = candidate;
            cursor = boundary;
        }
    }

    /// Convert an event time to a frame relative to a score reset origin.
    /// This is exact for rational step-tempo maps and implements the required
    /// mathematical ceiling without an early floating-point conversion.
    pub fn frame_at(
        &self,
        q: &BigRational,
        score_start_q: &BigRational,
        rate_hz: u64,
    ) -> Result<BigInt, MusicError> {
        if rate_hz == 0 {
            return Err(MusicError::range("sample rate must be positive"));
        }
        let event_seconds = self.seconds_at(q)?;
        let start_seconds = self.seconds_at(score_start_q)?;
        let relative = checked_sub(&event_seconds, &start_seconds)?;
        let scaled = checked_mul(&relative, &BigRational::from_integer(BigInt::from(rate_hz)))?;
        // ceil(n / d) for positive and negative values, with d > 0.
        let (quotient, remainder) = scaled.numer().div_rem(scaled.denom());
        let frame = if remainder.is_zero() {
            quotient
        } else if scaled.is_positive() {
            quotient + 1
        } else {
            quotient
        };
        Ok(frame)
    }
}

/// A validated meter-map point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterPoint {
    pub position_q: BigRational,
    pub numerator: u32,
    pub denominator: u32,
}

impl MeterPoint {
    pub fn new(position_q: BigRational, numerator: u32, denominator: u32) -> Self {
        Self {
            position_q,
            numerator,
            denominator,
        }
    }

    pub fn bar_length_q(&self) -> Result<BigRational, MusicError> {
        if self.numerator == 0 || self.denominator == 0 {
            return Err(MusicError::range(
                "meter numerator and denominator must be positive",
            ));
        }
        if !self.denominator.is_power_of_two() || self.denominator > 1024 {
            return Err(MusicError::range(
                "meter denominator must be a power of two <= 1024",
            ));
        }
        let numerator = rational_u32(self.numerator);
        let four = rational_i64(4);
        checked_div(
            &checked_mul(&numerator, &four)?,
            &rational_u32(self.denominator),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeterSegment {
    point: MeterPoint,
    first_bar: i64,
}

/// Exact global bar-numbering map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterMap {
    segments: Vec<MeterSegment>,
}

/// A bar coordinate returned by [`MeterMap::q_to_bar`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarCoordinate {
    pub bar: i64,
    pub unit: BigRational,
}

impl MeterMap {
    /// Validate meter points and precompute the bar number at every change.
    pub fn new(points: Vec<MeterPoint>) -> Result<Self, MusicError> {
        if points.is_empty() {
            return Err(MusicError::range("meter.points must be nonempty"));
        }
        if !points[0].position_q.is_zero() {
            return Err(MusicError::meter_boundary(
                "the first meter point must be at 0q",
            ));
        }
        let mut segments = Vec::with_capacity(points.len());
        let mut first_bar = 1i64;
        let mut previous: Option<MeterPoint> = None;
        for (index, point) in points.into_iter().enumerate() {
            ensure_rational(&point.position_q)?;
            if point.numerator == 0 || point.denominator == 0 {
                return Err(MusicError::range(format!(
                    "meter point {index} has a nonpositive numerator/denominator"
                )));
            }
            if !point.denominator.is_power_of_two() || point.denominator > 1024 {
                return Err(MusicError::range(format!(
                    "meter point {index} denominator must be a power of two <= 1024"
                )));
            }
            point.bar_length_q()?;
            if let Some(previous_point) = &previous {
                if point.position_q <= previous_point.position_q {
                    return Err(MusicError::meter_boundary(
                        "meter point positions must be strictly increasing",
                    ));
                }
                let distance = checked_sub(&point.position_q, &previous_point.position_q)?;
                let bars = checked_div(&distance, &bar_length_for(previous_point)?)?;
                if !bars.denom().is_one() || bars.is_negative() {
                    return Err(MusicError::meter_boundary(
                        "meter changes must occur on existing bar boundaries",
                    ));
                }
                let count = bars.to_integer().to_i64().ok_or_else(|| {
                    MusicError::resource("meter bar index exceeds signed 64-bit range")
                })?;
                first_bar = first_bar.checked_add(count).ok_or_else(|| {
                    MusicError::resource("meter bar index exceeds signed 64-bit range")
                })?;
            }
            segments.push(MeterSegment {
                point: point.clone(),
                first_bar,
            });
            previous = Some(point);
        }
        Ok(Self { segments })
    }

    pub fn points(&self) -> impl Iterator<Item = &MeterPoint> {
        self.segments.iter().map(|segment| &segment.point)
    }

    fn segment_for_bar(&self, bar: i64) -> &MeterSegment {
        let index = self
            .segments
            .partition_point(|segment| segment.first_bar <= bar);
        if index == 0 {
            &self.segments[0]
        } else {
            &self.segments[index - 1]
        }
    }

    fn segment_for_q(&self, q: &BigRational) -> &MeterSegment {
        let index = self
            .segments
            .partition_point(|segment| segment.point.position_q <= *q);
        if index == 0 {
            &self.segments[0]
        } else {
            &self.segments[index - 1]
        }
    }

    /// Resolve a one-based bar and one-based denominator-note position to q.
    /// Negative and zero bar numbers extend the first meter backward.
    pub fn bar_to_q(&self, bar: i64, unit: &BigRational) -> Result<BigRational, MusicError> {
        ensure_rational(unit)?;
        let segment = self.segment_for_bar(bar);
        let numerator = rational_u32(segment.point.numerator);
        let one = BigRational::from_integer(BigInt::from(1));
        if unit < &one || unit >= &checked_add(&numerator, &one)? {
            let upper_bound = u64::from(segment.point.numerator) + 1;
            return Err(MusicError::range(format!(
                "bar unit must satisfy 1 <= u < {}",
                upper_bound
            )));
        }
        let bar_delta = bar
            .checked_sub(segment.first_bar)
            .ok_or_else(|| MusicError::resource("bar index exceeds signed 64-bit range"))?;
        let bar_offset = checked_mul_integer(&segment.point.bar_length_q()?, bar_delta)?;
        let within = checked_mul(
            &checked_sub(unit, &one)?,
            &checked_div(&rational_i64(4), &rational_u32(segment.point.denominator))?,
        )?;
        checked_add(
            &checked_add(&segment.point.position_q, &bar_offset)?,
            &within,
        )
    }

    /// Resolve q to a one-based bar and one-based denominator-note position.
    pub fn q_to_bar(&self, q: &BigRational) -> Result<BarCoordinate, MusicError> {
        ensure_rational(q)?;
        let segment = self.segment_for_q(q);
        let bar_length = segment.point.bar_length_q()?;
        let delta = checked_sub(q, &segment.point.position_q)?;
        let bar_ratio = checked_div(&delta, &bar_length)?;
        let quotient = bar_ratio.numer().div_floor(bar_ratio.denom());
        let remainder = bar_ratio.numer().mod_floor(bar_ratio.denom());
        let bar_delta = quotient
            .to_i64()
            .ok_or_else(|| MusicError::resource("bar index exceeds signed 64-bit range"))?;
        let bar = segment
            .first_bar
            .checked_add(bar_delta)
            .ok_or_else(|| MusicError::resource("bar index exceeds signed 64-bit range"))?;
        let offset = BigRational::new(remainder, bar_ratio.denom().clone());
        let denominator = rational_u32(segment.point.denominator);
        let offset_q = checked_mul(&offset, &bar_length)?;
        let unit = checked_add(
            &BigRational::from_integer(BigInt::from(1)),
            &checked_mul(&offset_q, &checked_div(&denominator, &rational_i64(4))?)?,
        )?;
        // `unit` is guaranteed below numerator+1 by the floor operation, but
        // retaining this check documents and protects the public invariant.
        if unit < BigRational::from_integer(BigInt::from(1))
            || unit >= checked_add(&rational_u32(segment.point.numerator), &rational_i64(1))?
        {
            return Err(MusicError::precision(
                "meter coordinate conversion overflow",
            ));
        }
        Ok(BarCoordinate { bar, unit })
    }
}

fn bar_length_for(point: &MeterPoint) -> Result<BigRational, MusicError> {
    point.bar_length_q()
}

/// A spelled pitch's letter and accidental data, retained for notation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spelling {
    pub letter: char,
    pub accidental: i8,
    pub octave: i64,
}

impl Spelling {
    pub fn new(letter: char, accidental: i8, octave: i64) -> Result<Self, MusicError> {
        let letter = letter.to_ascii_uppercase();
        if !matches!(letter, 'A' | 'B' | 'C' | 'D' | 'E' | 'F' | 'G') {
            return Err(MusicError::pitch("pitch letter must be A through G"));
        }
        if !(-2..=2).contains(&accidental) {
            return Err(MusicError::pitch(
                "pitch spelling permits zero through two identical accidentals",
            ));
        }
        Ok(Self {
            letter,
            accidental,
            octave,
        })
    }

    pub fn key(&self) -> Result<i64, MusicError> {
        let natural = match self.letter {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return Err(MusicError::pitch("invalid pitch letter")),
        };
        self.octave
            .checked_add(1)
            .and_then(|octave| octave.checked_mul(12))
            .and_then(|base| base.checked_add(natural + i64::from(self.accidental)))
            .ok_or_else(|| MusicError::resource("spelled pitch key exceeds signed 64-bit range"))
    }
}

/// An explicitly declared tuning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tuning {
    pub period_cents: BigRational,
    pub steps_cents: Vec<BigRational>,
    pub reference_index: i64,
    pub reference_frequency_hz: BigRational,
}

impl Tuning {
    pub fn new(
        period_cents: BigRational,
        steps_cents: Vec<BigRational>,
        reference_index: i64,
        reference_frequency_hz: BigRational,
    ) -> Result<Self, MusicError> {
        ensure_rational(&period_cents)?;
        ensure_rational(&reference_frequency_hz)?;
        if period_cents <= BigRational::zero() {
            return Err(MusicError::pitch("tuning period must be positive cents"));
        }
        if steps_cents.is_empty() {
            return Err(MusicError::pitch("tuning steps must be nonempty"));
        }
        if reference_frequency_hz <= BigRational::zero() {
            return Err(MusicError::range(
                "tuning reference frequency must be positive",
            ));
        }
        let zero = BigRational::zero();
        let mut previous: Option<&BigRational> = None;
        for (index, step) in steps_cents.iter().enumerate() {
            ensure_rational(step)?;
            if index == 0 && step != &zero {
                return Err(MusicError::pitch("tuning steps must begin at 0ct"));
            }
            if step < &zero || step >= &period_cents {
                return Err(MusicError::pitch(
                    "tuning steps must be nonnegative and less than period",
                ));
            }
            if let Some(previous) = previous {
                if step <= previous {
                    return Err(MusicError::pitch(
                        "tuning steps must be strictly increasing",
                    ));
                }
            }
            previous = Some(step);
        }
        Ok(Self {
            period_cents,
            steps_cents,
            reference_index,
            reference_frequency_hz,
        })
    }

    /// Return c(k) using floor division, so negative degree indexes wrap to
    /// the preceding period (`degree(-1)` is the final step below period).
    pub fn cents_at(&self, index: i64) -> Result<BigRational, MusicError> {
        let step_count = i64::try_from(self.steps_cents.len())
            .map_err(|_| MusicError::resource("tuning has too many steps"))?;
        let (periods, remainder) = index.div_mod_floor(&step_count);
        let period_component = checked_mul_integer(&self.period_cents, periods)?;
        checked_add(&period_component, &self.steps_cents[remainder as usize])
    }

    pub fn frequency_hz(&self, index: i64) -> Result<f64, MusicError> {
        let cents = self.cents_at(index)?;
        let reference_cents = self.cents_at(self.reference_index)?;
        let delta = checked_sub(&cents, &reference_cents)?;
        let delta = delta.to_f64().ok_or_else(|| {
            MusicError::nonfinite("tuning cents cannot be represented as finite f64")
        })?;
        let reference = self.reference_frequency_hz.to_f64().ok_or_else(|| {
            MusicError::nonfinite("tuning reference frequency cannot be represented as finite f64")
        })?;
        let frequency = reference * (2.0_f64).powf(delta / 1200.0);
        validate_frequency(frequency, None)?;
        Ok(frequency)
    }
}

/// A source pitch before receiver-specific resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pitch {
    Key(i64),
    Spelled(Spelling),
    Frequency(BigRational),
    Ratio {
        ratio: BigRational,
        frequency_hz: BigRational,
    },
    Degree {
        index: i64,
        tuning: Arc<Tuning>,
    },
}

impl Pitch {
    pub fn key(key: i64) -> Self {
        Self::Key(key)
    }

    pub fn spelled(letter: char, accidental: i8, octave: i64) -> Result<Self, MusicError> {
        Ok(Self::Spelled(Spelling::new(letter, accidental, octave)?))
    }

    pub fn parse_spelled(source: &str) -> Result<Self, MusicError> {
        let mut chars = source.chars();
        let letter = chars
            .next()
            .ok_or_else(|| MusicError::pitch("empty pitch spelling"))?;
        if !matches!(
            letter.to_ascii_uppercase(),
            'A' | 'B' | 'C' | 'D' | 'E' | 'F' | 'G'
        ) {
            return Err(MusicError::pitch(
                "pitch spelling must begin with A through G",
            ));
        }
        let mut accidental: i8 = 0;
        let mut accidental_kind: Option<char> = None;
        while let Some(next) = chars.clone().next() {
            if next == '#' || next == 'b' {
                if accidental_kind.is_some_and(|kind| kind != next) {
                    return Err(MusicError::pitch("pitch accidentals must be identical"));
                }
                if accidental == 2 {
                    return Err(MusicError::pitch("at most two accidentals are permitted"));
                }
                accidental_kind = Some(next);
                accidental += if next == '#' { 1 } else { -1 };
                chars.next();
            } else {
                break;
            }
        }
        let octave_text: String = chars.collect();
        if octave_text.is_empty()
            || (octave_text.starts_with('+') && octave_text.len() == 1)
            || (octave_text.starts_with('-') && octave_text.len() == 1)
            || !octave_text
                .chars()
                .skip(
                    if octave_text.starts_with('+') || octave_text.starts_with('-') {
                        1
                    } else {
                        0
                    },
                )
                .all(|character| character.is_ascii_digit())
        {
            return Err(MusicError::pitch(
                "pitch spelling requires a signed or unsigned octave integer",
            ));
        }
        let octave = octave_text
            .parse::<i64>()
            .map_err(|_| MusicError::pitch("pitch octave is out of range"))?;
        Self::spelled(letter, accidental, octave)
    }

    pub fn hz(frequency_hz: BigRational) -> Result<Self, MusicError> {
        ensure_rational(&frequency_hz)?;
        if frequency_hz <= BigRational::zero() {
            return Err(MusicError::range("frequency must be positive"));
        }
        Ok(Self::Frequency(frequency_hz))
    }

    pub fn ratio(ratio: BigRational, frequency_hz: BigRational) -> Result<Self, MusicError> {
        ensure_rational(&ratio)?;
        ensure_rational(&frequency_hz)?;
        if ratio <= BigRational::zero() || frequency_hz <= BigRational::zero() {
            return Err(MusicError::range(
                "ratio and base frequency must be positive",
            ));
        }
        // Check the exact product now so a later renderer cannot trigger an
        // unbounded rational operation.
        checked_mul(&ratio, &frequency_hz)?;
        Ok(Self::Ratio {
            ratio,
            frequency_hz,
        })
    }

    pub fn degree(index: i64, tuning: Arc<Tuning>) -> Self {
        Self::Degree { index, tuning }
    }

    pub fn key_index(&self) -> Option<i64> {
        match self {
            Self::Key(key) => Some(*key),
            Self::Spelled(spelling) => spelling.key().ok(),
            _ => None,
        }
    }

    /// Resolve to finite Hz, enforcing an optional strict Nyquist boundary.
    pub fn resolve_hz(&self, nyquist_hz: Option<f64>) -> Result<f64, MusicError> {
        let frequency = match self {
            Self::Key(key) => {
                let exponent = (*key as f64 - 69.0) / 12.0;
                440.0 * (2.0_f64).powf(exponent)
            }
            Self::Spelled(spelling) => {
                let key = spelling.key()?;
                let exponent = (key as f64 - 69.0) / 12.0;
                440.0 * (2.0_f64).powf(exponent)
            }
            Self::Frequency(frequency) => frequency.to_f64().ok_or_else(|| {
                MusicError::nonfinite("frequency cannot be represented as finite f64")
            })?,
            Self::Ratio {
                ratio,
                frequency_hz,
            } => checked_mul(ratio, frequency_hz)?.to_f64().ok_or_else(|| {
                MusicError::nonfinite("ratio frequency cannot be represented as finite f64")
            })?,
            Self::Degree { index, tuning } => tuning.frequency_hz(*index)?,
        };
        validate_frequency(frequency, nyquist_hz)?;
        Ok(frequency)
    }

    pub fn frequency_hz(&self) -> Result<f64, MusicError> {
        self.resolve_hz(None)
    }
}

/// Validate a concrete frequency, optionally against a strict Nyquist limit.
pub fn validate_frequency(frequency_hz: f64, nyquist_hz: Option<f64>) -> Result<(), MusicError> {
    if !frequency_hz.is_finite() {
        return Err(MusicError::nonfinite("frequency must be finite"));
    }
    if frequency_hz <= 0.0 {
        return Err(MusicError::range("frequency must be positive"));
    }
    if let Some(nyquist_hz) = nyquist_hz {
        if !nyquist_hz.is_finite() || nyquist_hz <= 0.0 {
            return Err(MusicError::range(
                "Nyquist frequency must be positive and finite",
            ));
        }
        if frequency_hz >= nyquist_hz {
            return Err(MusicError::range(
                "frequency must be strictly below the engine Nyquist frequency",
            ));
        }
    }
    Ok(())
}
