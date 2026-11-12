//! The repetitions of a use or a placement (§9, §10): its pattern,
//! stretched, played `count` times end to end from `at`.
//!
//! They fit inside what holds them: a use's inside its parent pattern, and
//! a placement's inside the project's score.

use num_rational::BigRational;
use num_traits::{One, Signed};

use super::Validator;
use crate::diagnostic::DiagnosticCode;
use crate::syntax::{Object, Unit, ValueKind};

impl Validator<'_> {
    /// The length in quarter notes of pattern `id`, if it is positive.
    fn pattern_length(&self, id: &str) -> Option<BigRational> {
        let pattern = self.document.object(id)?;
        match &pattern.field("length")?.value.kind {
            ValueKind::Quantity {
                value,
                unit: Unit::Q,
            } if pattern.kind == "pattern" && value.is_positive() => Some(value.clone()),
            _ => None,
        }
    }

    /// The quarter notes that the repetitions of `object`, a use or a
    /// placement, take in all. `None` unless its `count`, its `stretch` and
    /// its pattern's length read: a wrong one is reported as its own
    /// mistake, and a count past what the compiler expands as that limit.
    fn repeated_span(&self, object: &Object) -> Option<BigRational> {
        let number = |name: &str| match object.field(name).map(|field| &field.value.kind) {
            None => Some(BigRational::one()),
            Some(ValueKind::Number(number)) if number.is_positive() => Some(number.clone()),
            Some(_) => None,
        };
        let limit = BigRational::from_integer(crate::compiler::MAX_EXPANDED_NOTES.into());
        let count = number("count").filter(|count| count.is_integer() && count <= &limit)?;
        let stretch = number("stretch")?;
        let reference = object.field("pattern")?.value.reference()?;
        let [pattern] = reference.path.as_slice() else {
            return None;
        };
        Some(stretch * count * self.pattern_length(pattern)?)
    }

    /// The repetitions of use `object`, from `at`, end inside its parent
    /// pattern.
    pub(super) fn validate_use_fits(
        &mut self,
        object: &Object,
        path: &[String],
        at: Option<&BigRational>,
    ) {
        let [parent, _] = path else {
            return;
        };
        let (Some(at), Some(length), Some(span)) =
            (at, self.pattern_length(parent), self.repeated_span(object))
        else {
            return;
        };
        // A use that starts at or past the end is reported as a child out
        // of place.
        if at < &length && at + span > length {
            self.push(
                DiagnosticCode::Interval,
                "repeated use does not fit in its parent pattern",
                Some(object.span),
                path.to_vec(),
                Vec::new(),
            );
        }
    }

    /// The repetitions of placement `object`, from `at` inside the score,
    /// end inside the score.
    pub(super) fn validate_place_fits(
        &mut self,
        object: &Object,
        path: &[String],
        at: Option<&BigRational>,
    ) {
        let (Some(at), Some((_, end)), Some(span)) =
            (at, self.project_score.clone(), self.repeated_span(object))
        else {
            return;
        };
        if at + span > end {
            self.push(
                DiagnosticCode::Interval,
                "placement repetition span must fit in the project score",
                Some(object.span),
                path.to_vec(),
                Vec::new(),
            );
        }
    }
}
