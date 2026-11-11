//! A phrase (§9.3): a pattern child that shapes the events under a span of
//! its pattern.
//!
//! Its score-clock curves read from the phrase's `at`: `dynamics` scales
//! the velocity of each event, and `pressure` and `timbre` give a note the
//! expression it does not write itself.

use num_rational::BigRational;
use num_traits::{One, Signed, Zero};

use super::fields::{optional, required, SourceField};
use super::{ReferenceTarget, Validator};
use crate::diagnostic::DiagnosticCode;
use crate::syntax::{Field, Object, Unit, Value, ValueKind};

pub(crate) const FIELDS: &[SourceField] = &[
    required("at"),
    required("length"),
    optional("dynamics"),
    optional("pressure"),
    optional("timbre"),
];

/// A phrase as its object declares it. It names at least one curve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Phrase {
    /// Where the span starts, in quarter notes of the pattern.
    pub(crate) at: BigRational,
    /// The length of the span, in quarter notes.
    pub(crate) length: BigRational,
    /// The ID of the curve that scales velocities.
    pub(crate) dynamics: Option<String>,
    /// The ID of the curve that gives notes their pressure.
    pub(crate) pressure: Option<String>,
    /// The ID of the curve that gives notes their timbre.
    pub(crate) timbre: Option<String>,
}

impl Validator<'_> {
    /// Check a `phrase` object and keep its model under its path.
    pub(super) fn validate_phrase(&mut self, object: &Object, path: &[String]) {
        if let Some(phrase) = self.read_phrase(object, path) {
            self.models.phrases.insert(path.to_vec(), phrase);
        }
    }

    fn read_phrase(&mut self, object: &Object, path: &[String]) -> Option<Phrase> {
        self.check_declared(object, path, FIELDS);
        self.reject_children(object, path);
        let at = object.field("at").and_then(|field| {
            let at = self.quantity(field, &[Unit::Q], path, "at")?;
            self.within(
                !at.is_negative(),
                "phrase.at must be nonnegative",
                field,
                path,
            )
            .then_some(at)
        });
        let length = object.field("length").and_then(|field| {
            let length = self.quantity(field, &[Unit::Q], path, "length")?;
            self.within(
                length.is_positive(),
                "phrase.length must be positive",
                field,
                path,
            )
            .then_some(length)
        });
        let mut sound = true;
        if ["dynamics", "pressure", "timbre"]
            .iter()
            .all(|name| object.field(name).is_none())
        {
            self.push(
                DiagnosticCode::Range,
                "phrase requires `dynamics`, `pressure` or `timbre`",
                Some(object.span),
                path.to_vec(),
                Vec::new(),
            );
            sound = false;
        }
        let mut curve = |name: &str| {
            let field = object.field(name)?;
            let curve = self.phrase_curve(field, path);
            sound &= curve.is_some();
            curve
        };
        let (dynamics, pressure, timbre) = (curve("dynamics"), curve("pressure"), curve("timbre"));
        sound.then_some(Phrase {
            at: at?,
            length: length?,
            dynamics,
            pressure,
            timbre,
        })
    }

    /// The ID of the curve that `field` of a phrase names, if it is one a
    /// phrase may use: on the score clock, with dimensionless values in the
    /// field's range and `step` or `linear` segments.
    fn phrase_curve(&mut self, field: &Field, path: &[String]) -> Option<String> {
        let name = field.name.as_str();
        let ReferenceTarget::Object { id, .. } =
            self.expect_object_ref(field, name, &["curve"], path)?
        else {
            return None;
        };
        let curve = self.document.object(&id)?;
        let mut problems = Vec::new();
        if curve.field("clock").and_then(|f| f.value.as_symbol()) != Some("score") {
            problems.push((
                DiagnosticCode::Unit,
                "phrase curves use the score clock",
                field.value.span,
            ));
        }
        let points = match curve.field("points").map(|f| &f.value.kind) {
            Some(ValueKind::List(points)) => points.as_slice(),
            _ => &[],
        };
        for point in points {
            let ValueKind::Tuple(items) = &point.kind else {
                continue;
            };
            match items.get(1).map(|value| &value.kind) {
                Some(ValueKind::Number(value)) => {
                    let (inside, message) = if name == "dynamics" {
                        (
                            !value.is_negative(),
                            "phrase dynamics values must be nonnegative",
                        )
                    } else {
                        (
                            value >= &BigRational::zero() && value <= &BigRational::one(),
                            "phrase pressure and timbre values must be within 0..=1",
                        )
                    };
                    if !inside {
                        problems.push((DiagnosticCode::Range, message, point.span));
                    }
                }
                Some(_) => problems.push((
                    DiagnosticCode::Unit,
                    "phrase curve values must be dimensionless",
                    point.span,
                )),
                None => {}
            }
            if items.get(2).and_then(Value::as_symbol) == Some("exponential") {
                problems.push((
                    DiagnosticCode::Range,
                    "phrase curves use only step and linear segments",
                    point.span,
                ));
            }
        }
        let usable = problems.is_empty();
        for (code, message, span) in problems {
            self.push(code, message, Some(span), path.to_vec(), vec![name.into()]);
        }
        usable.then_some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    /// The model of `phrase p { ...fields }` in pattern `bar`, if it reads.
    fn phrase(fields: &str) -> Option<Phrase> {
        let document = crate::parse(&format!(
            "maac 1; curve arc {{ clock = score; points = [(0q, 1/2, linear), (4q, 3/2, step)]; }} \
             curve breath {{ clock = score; points = [(0q, 0, linear), (4q, 4/5, step)]; }} \
             curve held {{ clock = seconds; points = [(0s, 1/2, step)]; }} \
             pattern bar {{ length = 8q; phrase p {{ {fields} }} }}"
        ))
        .unwrap();
        models_of(&document)
            .phrases
            .remove(["bar".to_owned(), "p".to_owned()].as_slice())
    }

    #[test]
    fn a_phrase_reads_its_span_and_the_curves_it_names() {
        assert_eq!(
            phrase("at = 1/2q; length = 6q; dynamics = &arc; pressure = &breath;"),
            Some(Phrase {
                at: BigRational::new(1.into(), 2.into()),
                length: BigRational::from_integer(6.into()),
                dynamics: Some("arc".into()),
                pressure: Some("breath".into()),
                timbre: None,
            })
        );
    }

    #[test]
    fn dynamics_may_pass_one_and_pressure_may_not() {
        assert!(phrase("at = 0q; length = 8q; dynamics = &arc;").is_some());
        assert_eq!(phrase("at = 0q; length = 8q; pressure = &arc;"), None);
    }

    #[test]
    fn a_wrong_phrase_has_no_model() {
        for fields in [
            "at = 0q; length = 8q;",
            "at = -1q; length = 8q; dynamics = &arc;",
            "at = 0q; length = 0q; dynamics = &arc;",
            "length = 8q; dynamics = &arc;",
            "at = 0q; length = 8q; dynamics = &arc; timbre = &held;",
            "at = 0q; length = 8q; dynamics = &nothing;",
        ] {
            assert_eq!(phrase(fields), None, "{fields}");
        }
    }
}
