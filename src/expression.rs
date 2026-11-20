use crate::plan::{
    GainExpression, Interpolation, PitchExpression, PressureExpression, TimbreExpression,
};
use crate::Rational;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use std::sync::Arc;

/// Trait unifying expression curves for frame-indexed pre-compilation.
pub(crate) trait ExpressionCurve {
    fn len(&self) -> usize;
    fn point_fields(&self, index: usize) -> (&Rational, &Rational, Interpolation);
    fn eval_rational(&self, coordinate: &Rational) -> Option<f64>;
}

impl ExpressionCurve for PitchExpression {
    fn len(&self) -> usize {
        self.points.len()
    }

    fn point_fields(&self, index: usize) -> (&Rational, &Rational, Interpolation) {
        let point = &self.points[index];
        (&point.position, &point.cents, point.shape)
    }

    fn eval_rational(&self, coordinate: &Rational) -> Option<f64> {
        self.cents_at(coordinate).to_f64()
    }
}

impl ExpressionCurve for GainExpression {
    fn len(&self) -> usize {
        self.points.len()
    }

    fn point_fields(&self, index: usize) -> (&Rational, &Rational, Interpolation) {
        let point = &self.points[index];
        (&point.position, &point.gain, point.shape)
    }

    fn eval_rational(&self, coordinate: &Rational) -> Option<f64> {
        Some(self.gain_at(coordinate))
    }
}

impl ExpressionCurve for TimbreExpression {
    fn len(&self) -> usize {
        self.points.len()
    }

    fn point_fields(&self, index: usize) -> (&Rational, &Rational, Interpolation) {
        let point = &self.points[index];
        (&point.position, &point.value, point.shape)
    }

    fn eval_rational(&self, coordinate: &Rational) -> Option<f64> {
        Some(self.value_at(coordinate))
    }
}

impl ExpressionCurve for PressureExpression {
    fn len(&self) -> usize {
        self.points.len()
    }

    fn point_fields(&self, index: usize) -> (&Rational, &Rational, Interpolation) {
        let point = &self.points[index];
        (&point.position, &point.value, point.shape)
    }

    fn eval_rational(&self, coordinate: &Rational) -> Option<f64> {
        Some(self.value_at(coordinate))
    }
}

#[derive(Clone, Debug)]
enum FastSegment {
    Constant(Option<f64>),
    LinearI128 {
        base_num: i128,
        slope_num: i128,
        denom: u128,
    },
    Fallback,
}

#[derive(Clone, Debug)]
struct CompiledSegment {
    end_delta: u64,
    eval: FastSegment,
}

// Keep the immutable curve shared across scheduled events and live voices. Exact
// rational coordinates preserve knot selection even below binary64 resolution.
#[derive(Clone, Debug)]
pub(crate) struct ExpressionRuntime<T> {
    pub(crate) curve: Arc<T>,
    pub(crate) coordinate_per_frame: Rational,
    pub(crate) gate: u64,
    /// Frames the note had already sounded when the voice using this
    /// expression began: nonzero only for a note a preview restarts midway.
    shift: u64,
    segments: Vec<CompiledSegment>,
    gate_value: Option<f64>,
}

/// Two runtimes are equal when they were built from the same curve, rate and
/// gate; `segments` is compiled from those and is not compared.
impl<T: PartialEq> PartialEq for ExpressionRuntime<T> {
    fn eq(&self, other: &Self) -> bool {
        self.curve == other.curve
            && self.coordinate_per_frame == other.coordinate_per_frame
            && self.gate == other.gate
            && self.shift == other.shift
            && self.gate_value == other.gate_value
    }
}

impl<T: ExpressionCurve> ExpressionRuntime<T> {
    pub(crate) fn new(curve: Arc<T>, coordinate_per_frame: Rational, gate: u64) -> Self {
        let (segments, gate_value) = compile_segments(curve.as_ref(), &coordinate_per_frame, gate);
        Self {
            curve,
            coordinate_per_frame,
            gate,
            shift: 0,
            segments,
            gate_value,
        }
    }

    /// The same expression for a voice that begins `frames` into its note,
    /// so it continues the curve instead of replaying it.
    pub(crate) fn shifted(mut self, frames: u64) -> Self {
        self.shift = frames;
        self
    }

    pub(crate) fn coordinate_at(&self, on_frame: u64, frame: u64) -> Rational {
        &self.coordinate_per_frame
            * Rational::from_integer(
                frame
                    .saturating_sub(on_frame)
                    .saturating_add(self.shift)
                    .min(self.gate)
                    .into(),
            )
    }

    /// Evaluate the expression at `frame` with bit-identical rounding to
    /// `curve.eval_rational(&self.coordinate_at(on_frame, frame))`, avoiding
    /// per-frame `BigInt` allocations and `gcd` reductions on the fast path.
    pub(crate) fn eval_at(&self, on_frame: u64, frame: u64) -> Option<f64> {
        let delta = frame.saturating_sub(on_frame).saturating_add(self.shift);
        if delta >= self.gate {
            return self.gate_value;
        }
        let seg = self
            .segments
            .iter()
            .find(|segment| delta < segment.end_delta)
            .unwrap_or_else(|| self.segments.last().expect("nonempty segments"));
        match &seg.eval {
            FastSegment::Constant(value) => *value,
            FastSegment::LinearI128 {
                base_num,
                slope_num,
                denom,
            } => {
                let num = base_num + slope_num * (delta as i128);
                Some(ratio_i128_to_f64(num, *denom))
            }
            FastSegment::Fallback => self
                .curve
                .eval_rational(&self.coordinate_at(on_frame, frame)),
        }
    }
}

fn ceil_rational_u64(value: &Rational) -> u64 {
    if !value.is_positive() {
        return 0;
    }
    let (numer, denom) = (value.numer(), value.denom());
    let ceiled = (numer + denom - BigInt::from(1u8)) / denom;
    ceiled.to_u64().unwrap_or(u64::MAX)
}

fn compile_segments<T: ExpressionCurve>(
    curve: &T,
    coordinate_per_frame: &Rational,
    gate: u64,
) -> (Vec<CompiledSegment>, Option<f64>) {
    let gate_coord = coordinate_per_frame * Rational::from_integer(BigInt::from(gate));
    let gate_value = curve.eval_rational(&gate_coord);
    let len = curve.len();
    if len == 0 {
        return (Vec::new(), gate_value);
    }
    if coordinate_per_frame.is_zero() {
        let (_, left_val, _) = curve.point_fields(0);
        return (
            vec![CompiledSegment {
                end_delta: u64::MAX,
                eval: FastSegment::Constant(left_val.to_f64()),
            }],
            gate_value,
        );
    }

    let mut segments = Vec::with_capacity(len);
    let mut prev_end = 0u64;
    for index in 0..len {
        let end_delta = if index + 1 < len {
            let (next_pos, _, _) = curve.point_fields(index + 1);
            let ratio = next_pos / coordinate_per_frame;
            ceil_rational_u64(&ratio)
        } else {
            u64::MAX
        };
        if end_delta <= prev_end {
            // Zero frames fall in this segment for integer frame deltas.
            continue;
        }
        let (left_pos, left_val, shape) = curve.point_fields(index);
        let eval = if index + 1 >= len || shape == Interpolation::Step {
            FastSegment::Constant(left_val.to_f64())
        } else if shape == Interpolation::Linear {
            let (next_pos, next_val, _) = curve.point_fields(index + 1);
            let k = (next_val - left_val) / (next_pos - left_pos);
            let a = left_val - &k * left_pos;
            let b = &k * coordinate_per_frame;
            let lcm_denom = a.denom().lcm(b.denom());
            let base_num = a.numer() * (&lcm_denom / a.denom());
            let slope_num = b.numer() * (&lcm_denom / b.denom());
            let max_delta = end_delta.min(gate).saturating_sub(1);
            if let (Some(base_i128), Some(slope_i128), Some(denom_u128)) =
                (base_num.to_i128(), slope_num.to_i128(), lcm_denom.to_u128())
            {
                let max_ok = slope_i128
                    .checked_mul(max_delta as i128)
                    .and_then(|prod| base_i128.checked_add(prod))
                    .is_some();
                if max_ok && denom_u128 > 0 {
                    FastSegment::LinearI128 {
                        base_num: base_i128,
                        slope_num: slope_i128,
                        denom: denom_u128,
                    }
                } else {
                    FastSegment::Fallback
                }
            } else {
                FastSegment::Fallback
            }
        } else {
            FastSegment::Fallback
        };
        segments.push(CompiledSegment { end_delta, eval });
        prev_end = end_delta;
        if prev_end >= gate {
            break;
        }
    }
    if segments.is_empty() {
        let (_, left_val, _) = curve.point_fields(len - 1);
        segments.push(CompiledSegment {
            end_delta: u64::MAX,
            eval: FastSegment::Constant(left_val.to_f64()),
        });
    }
    (segments, gate_value)
}

/// Convert `num / denom` (`denom > 0`) to `f64` with exact IEEE-754
/// round-to-nearest, ties-to-even, matching `Ratio<BigInt>::to_f64()`.
#[inline]
fn ratio_i128_to_f64(num: i128, denom: u128) -> f64 {
    if num == 0 {
        return 0.0;
    }
    let neg = num < 0;
    let a = num.unsigned_abs();
    let b = denom;
    let a_bits = 128 - a.leading_zeros() as i32;
    let b_bits = 128 - b.leading_zeros() as i32;
    let exp_est = a_bits - b_bits;
    let mut exp = if exp_est >= 0 {
        if a < (b << exp_est) {
            exp_est - 1
        } else {
            exp_est
        }
    } else if (a << (-exp_est)) < b {
        exp_est - 1
    } else {
        exp_est
    };

    let shift = 52 - exp;
    let (num_shifted, den_shifted) = if shift >= 0 {
        if a_bits + shift <= 128 {
            (a << shift, b)
        } else {
            return num_rational::Ratio::new_raw(BigInt::from(num), BigInt::from(denom))
                .to_f64()
                .expect("i128/u128 ratio fits in finite f64");
        }
    } else {
        let k = -shift;
        if b_bits + k <= 128 {
            (a, b << k)
        } else {
            return num_rational::Ratio::new_raw(BigInt::from(num), BigInt::from(denom))
                .to_f64()
                .expect("i128/u128 ratio fits in finite f64");
        }
    };

    let mut q = num_shifted / den_shifted;
    let rem = num_shifted % den_shifted;
    let half = den_shifted >> 1;
    let round_up = if rem > half {
        true
    } else if rem == half && (den_shifted & 1) == 0 {
        (q & 1) == 1
    } else {
        false
    };
    if round_up {
        q += 1;
        if q == (1u128 << 53) {
            q >>= 1;
            exp += 1;
        }
    }

    let sign_bit = if neg { 1u64 << 63 } else { 0 };
    let exp_bits = ((exp + 1023) as u64) << 52;
    let mant_bits = (q as u64) & ((1u64 << 52) - 1);
    f64::from_bits(sign_bit | exp_bits | mant_bits)
}

#[cfg(test)]
mod tests {
    use super::ratio_i128_to_f64;
    use num_bigint::BigInt;
    use num_rational::Ratio;
    use num_traits::ToPrimitive;

    #[test]
    fn ratio_i128_to_f64_matches_bigint_bit_for_bit() {
        let nums = [
            0i128,
            1,
            -1,
            2,
            -2,
            3,
            -3,
            7,
            -7,
            1024,
            -1024,
            (1i128 << 53) - 1,
            1i128 << 53,
            (1i128 << 53) + 1,
            (1i128 << 53) + 2,
            (1i128 << 53) + 3,
            123_456_789_012_345_678,
            -123_456_789_012_345_678,
            i128::MAX / 4,
            i128::MIN / 4,
        ];
        let denoms = [
            1u128,
            2,
            3,
            4,
            5,
            7,
            8,
            10,
            48_000,
            96_000,
            1u128 << 53,
            (1u128 << 53) + 1,
            (1u128 << 80) + 12345,
        ];
        for &n in &nums {
            for &d in &denoms {
                let fast = ratio_i128_to_f64(n, d);
                let exact = Ratio::new_raw(BigInt::from(n), BigInt::from(d))
                    .to_f64()
                    .unwrap();
                assert_eq!(
                    fast.to_bits(),
                    exact.to_bits(),
                    "mismatch for {n}/{d}: fast={fast} exact={exact}"
                );
            }
        }
    }
}
