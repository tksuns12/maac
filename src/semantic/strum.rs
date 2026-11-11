//! A strum (§8.2): a chord struck one string after another.
//!
//! A downstroke strikes the chord's members in list order and an upstroke
//! in reverse. Each string is `stagger` later than the last, and the
//! velocity changes by `tilt` from the first string struck to the last.

use num_rational::BigRational;
use num_traits::Signed;

use super::fields::{beside, default_of, optional, DefaultValue, SourceField};
use super::Validator;
use crate::syntax::{Object, Unit, ValueKind};

/// The fields of a chord that say how it is strummed.
pub(crate) const FIELDS: &[SourceField] = &[
    beside("stagger", "strum", DefaultValue::Fraction(1, 125, Unit::S)),
    optional("strum"),
    beside("tilt", "strum", DefaultValue::Number(0)),
];

/// A strum as its chord declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Strum {
    /// Whether the stroke runs from the last member to the first.
    pub(crate) up: bool,
    /// Seconds from one string to the next.
    pub(crate) stagger: BigRational,
    /// The change of velocity from the first string struck to the last.
    pub(crate) tilt: BigRational,
}

impl Validator<'_> {
    /// Check a chord's `strum`, `stagger` and `tilt`, and keep the model of
    /// a strummed chord under the chord's path.
    pub(super) fn validate_strum(&mut self, object: &Object, path: &[String]) {
        if let Some(strum) = self.read_strum(object, path) {
            self.models.strums.insert(path.to_vec(), strum);
        }
    }

    /// `stagger` and `tilt` belong to a strum, and `tilt` to a single
    /// velocity.
    fn read_strum(&mut self, object: &Object, path: &[String]) -> Option<Strum> {
        let strum = object.field("strum");
        let up = strum.and_then(|field| match field.value.as_symbol() {
            Some("down") => Some(false),
            Some("up") => Some(true),
            _ => {
                self.within(false, "chord.strum must be `down` or `up`", field, path);
                None
            }
        });
        let stagger = match object.field("stagger") {
            None => Some(default_of(FIELDS, "stagger")),
            Some(field) => {
                let strummed = self.within(
                    strum.is_some(),
                    "chord.stagger requires `strum = down` or `strum = up`",
                    field,
                    path,
                );
                self.quantity_value(&field.value, &[Unit::S, Unit::Ms], path, "stagger")
                    .filter(|stagger| {
                        self.within(
                            !stagger.is_negative(),
                            "chord.stagger must be nonnegative",
                            field,
                            path,
                        )
                    })
                    .filter(|_| strummed)
            }
        };
        let tilt = match object.field("tilt") {
            None => Some(default_of(FIELDS, "tilt")),
            Some(field) => {
                let strummed = self.within(
                    strum.is_some(),
                    "chord.tilt requires `strum = down` or `strum = up`",
                    field,
                    path,
                );
                let tilt = self.number_value(&field.value, path, "tilt");
                let listed = matches!(
                    object
                        .field("velocity")
                        .map(|velocity| &velocity.value.kind),
                    Some(ValueKind::List(_))
                );
                let single = self.within(
                    !listed,
                    "chord.tilt requires a single velocity, not a list",
                    field,
                    path,
                );
                tilt.filter(|_| strummed && single)
            }
        };
        Some(Strum {
            up: up?,
            stagger: stagger?,
            tilt: tilt?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    /// The strum of `chord a { ...fields }`, if it has one.
    fn strum(fields: &str) -> Option<Strum> {
        let document = crate::parse(&format!(
            "maac 1; pattern bar {{ length = 4q; chord a {{ at = 0q; dur = 1q; pitches = [C4, E4]; {fields} }} }}"
        ))
        .unwrap();
        models_of(&document)
            .strums
            .remove(["bar".to_owned(), "a".to_owned()].as_slice())
    }

    #[test]
    fn a_strum_reads_its_stroke_its_stagger_and_its_tilt() {
        assert_eq!(
            strum("strum = up; stagger = 30ms; tilt = -1/10;"),
            Some(Strum {
                up: true,
                stagger: BigRational::new(3.into(), 100.into()),
                tilt: BigRational::new((-1).into(), 10.into()),
            })
        );
    }

    #[test]
    fn a_field_left_out_beside_a_strum_reads_as_its_declared_default() {
        let left_out = strum("strum = down;");
        assert!(left_out.is_some());
        assert_eq!(strum("strum = down; stagger = 1/125s;"), left_out);
        assert_eq!(strum("strum = down; stagger = 8ms; tilt = 0;"), left_out);
    }

    #[test]
    fn a_chord_without_a_strum_or_with_a_wrong_one_has_no_model() {
        for fields in [
            "",
            "stagger = 8ms;",
            "tilt = 0;",
            "strum = sideways;",
            "strum = down; stagger = -1ms;",
            "strum = down; stagger = 1q;",
            "strum = down; tilt = 1ct;",
            "strum = down; velocity = [1, 1]; tilt = 0;",
        ] {
            assert_eq!(strum(fields), None, "{fields}");
        }
    }
}
