//! Exact source arithmetic.

use std::fmt;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::sign::Signed;

/// The hard parser/normalizer bound required by the foundation release.
pub const MAX_RATIONAL_BITS: u64 = 4096;

pub type Rational = BigRational;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RationalError {
    InvalidSyntax,
    ZeroDenominator,
    ResourceLimit,
}

impl fmt::Display for RationalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSyntax => f.write_str("invalid rational literal"),
            Self::ZeroDenominator => f.write_str("rational denominator must be positive"),
            Self::ResourceLimit => {
                f.write_str("rational numerator or denominator exceeds the bit limit")
            }
        }
    }
}

impl std::error::Error for RationalError {}

/// Parse an integer, finite decimal, or positive-denominator fraction into a
/// reduced exact rational. Exponents and surrounding whitespace are rejected.
pub fn parse_rational(input: &str) -> Result<Rational, RationalError> {
    parse_rational_with_limit(input, MAX_RATIONAL_BITS)
}

pub fn parse_rational_with_limit(input: &str, max_bits: u64) -> Result<Rational, RationalError> {
    if input.is_empty() || input.trim() != input || input.contains(['e', 'E']) {
        return Err(RationalError::InvalidSyntax);
    }

    let (negative, unsigned) = match input.as_bytes().first() {
        Some(b'+') => (false, &input[1..]),
        Some(b'-') => (true, &input[1..]),
        _ => (false, input),
    };
    if unsigned.is_empty() {
        return Err(RationalError::InvalidSyntax);
    }

    let (numerator_digits, denominator_digits): (
        std::borrow::Cow<'_, str>,
        std::borrow::Cow<'_, str>,
    ) = if let Some((n, d)) = unsigned.split_once('/') {
        if n.is_empty() || d.is_empty() || !n.bytes().all(|c| c.is_ascii_digit()) {
            return Err(RationalError::InvalidSyntax);
        }
        if !d.bytes().all(|c| c.is_ascii_digit()) {
            return Err(RationalError::InvalidSyntax);
        }
        if d == "0" {
            return Err(RationalError::ZeroDenominator);
        }
        // The grammar's positive_unsigned production does not allow a
        // leading zero in a denominator (1/01 is therefore not a fraction
        // literal). Numerator leading zeroes remain harmless source syntax.
        if d.starts_with('0') {
            return Err(RationalError::InvalidSyntax);
        }
        (std::borrow::Cow::Borrowed(n), std::borrow::Cow::Borrowed(d))
    } else if let Some((whole, fraction)) = unsigned.split_once('.') {
        if whole.is_empty()
            || fraction.is_empty()
            || !whole.bytes().all(|c| c.is_ascii_digit())
            || !fraction.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(RationalError::InvalidSyntax);
        }
        if fraction.len() > max_bits.saturating_add(1) as usize {
            return Err(RationalError::ResourceLimit);
        }
        let whole_digits = whole.trim_start_matches('0');
        let fraction_digits = fraction.trim_start_matches('0');
        let significant_len = if whole_digits.is_empty() {
            if fraction_digits.is_empty() {
                1
            } else {
                fraction_digits.len()
            }
        } else {
            whole_digits.len() + fraction.len()
        };
        if decimal_digit_count_exceeds_bits(significant_len, max_bits)
            || decimal_digit_count_exceeds_bits(fraction.len() + 1, max_bits)
        {
            return Err(RationalError::ResourceLimit);
        }
        let mut digits = if whole_digits.is_empty() {
            "0".to_owned()
        } else {
            whole_digits.to_owned()
        };
        digits.push_str(fraction);
        let denominator = format!("1{}", "0".repeat(fraction.len()));
        (
            std::borrow::Cow::Owned(digits),
            std::borrow::Cow::Owned(denominator),
        )
    } else {
        if !unsigned.bytes().all(|c| c.is_ascii_digit()) {
            return Err(RationalError::InvalidSyntax);
        }
        (
            std::borrow::Cow::Borrowed(unsigned),
            std::borrow::Cow::Borrowed("1"),
        )
    };

    // Remove source-level leading zeroes before converting. Apart from making
    // the allocation proportional to the value, this lets the bit-length
    // guard protect standalone callers that do not go through the source
    // parser's 4 MiB input limit.
    let numerator_digits = trim_leading_zeroes(&numerator_digits);
    let denominator_digits = trim_leading_zeroes(&denominator_digits);
    if decimal_digit_count_exceeds_bits(numerator_digits.len(), max_bits)
        || decimal_digit_count_exceeds_bits(denominator_digits.len(), max_bits)
    {
        return Err(RationalError::ResourceLimit);
    }
    let mut numerator =
        BigInt::parse_bytes(numerator_digits.as_bytes(), 10).ok_or(RationalError::InvalidSyntax)?;
    let denominator = BigInt::parse_bytes(denominator_digits.as_bytes(), 10)
        .ok_or(RationalError::InvalidSyntax)?;
    if denominator <= BigInt::from(0u8) {
        return Err(RationalError::ZeroDenominator);
    }
    if negative {
        numerator = -numerator;
    }
    // Check unreduced operands before constructing the reduced rational. A
    // huge numerator/denominator pair that happens to cancel still exceeds
    // the published source precision bound.
    if numerator.abs().bits() > max_bits || denominator.bits() > max_bits {
        return Err(RationalError::ResourceLimit);
    }
    let rational = BigRational::new(numerator, denominator);
    if rational.numer().abs().bits() > max_bits || rational.denom().bits() > max_bits {
        return Err(RationalError::ResourceLimit);
    }
    Ok(rational)
}

fn trim_leading_zeroes(digits: &str) -> &str {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        "0"
    } else {
        trimmed
    }
}

fn decimal_digit_count_exceeds_bits(digits: usize, max_bits: u64) -> bool {
    // An n-digit positive decimal integer has at least n-1 bits. This cheap
    // bound rejects pathological input before BigInt allocation; the exact
    // `bits()` checks below handle the remaining boundary cases.
    digits > max_bits.saturating_add(1) as usize
}

pub fn rational_parts(value: &Rational) -> (String, String) {
    (value.numer().to_string(), value.denom().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_integers_decimals_and_fractions_exactly() {
        assert_eq!(parse_rational("3/6").unwrap().to_string(), "1/2");
        assert_eq!(parse_rational("0.10").unwrap().to_string(), "1/10");
        assert_eq!(parse_rational("-4").unwrap().to_string(), "-4");
    }

    #[test]
    fn rejects_zero_denominator_and_exponents() {
        assert_eq!(parse_rational("1/0"), Err(RationalError::ZeroDenominator));
        assert_eq!(parse_rational("1/01"), Err(RationalError::InvalidSyntax));
        assert_eq!(parse_rational("1e3"), Err(RationalError::InvalidSyntax));
    }

    #[test]
    fn rejects_large_unreduced_operands_before_reduction() {
        let digits = "1".to_owned() + &"0".repeat(1300);
        let input = format!("{digits}/{digits}");
        assert_eq!(parse_rational(&input), Err(RationalError::ResourceLimit));
    }
}
