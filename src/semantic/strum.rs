//! A strum (§8.2): a chord struck one string after another.
//!
//! A downstroke strikes the chord's members in list order and an upstroke
//! in reverse. Each string is `stagger` later than the last, and the
//! velocity changes by `tilt` from the first string struck to the last.

use num_rational::BigRational;
use num_traits::{One, Signed, Zero};

use super::fields::{beside, default_of, optional, DefaultValue, SourceField};
use super::Validator;
use crate::diagnostic::DiagnosticCode;
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
    /// The velocity the tilt starts from: the chord's `velocity`, or 1 when
    /// it writes none. `None` when it is not one number; the tilt is then 0.
    pub(crate) velocity: Option<BigRational>,
}

impl Strum {
    /// The first of a chord's `members` whose velocity the tilt takes
    /// outside 0 to 1, counted from zero, with that velocity. Member `k` of
    /// `n` is struck at place `i`, `k` going down and `n - 1 - k` going up,
    /// and plays `velocity + tilt * i / (n - 1)`.
    pub(crate) fn overshoot(&self, members: usize) -> Option<(usize, BigRational)> {
        let velocity = self.velocity.as_ref()?;
        if members < 2 || self.tilt.is_zero() {
            return None;
        }
        let last = BigRational::from_integer((members - 1).into());
        (0..members).find_map(|member| {
            let place = if self.up {
                members - 1 - member
            } else {
                member
            };
            let struck = velocity + &self.tilt * BigRational::from_integer(place.into()) / &last;
            (struck.is_negative() || struck > BigRational::one()).then_some((member, struck))
        })
    }

    /// What is reported of member `member` struck at `velocity`, outside 0
    /// to 1.
    pub(crate) fn overshoot_message(member: usize, velocity: &BigRational) -> String {
        format!("chord velocity with tilt is {velocity} at member {member}, outside [0,1]")
    }
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
        // The velocity is checked with the chord's other per-note fields.
        let velocity = match object.field("velocity").map(|field| &field.value.kind) {
            None => Some(BigRational::one()),
            Some(ValueKind::Number(velocity)) => Some(velocity.clone()),
            Some(_) => None,
        };
        Some(Strum {
            up: up?,
            stagger: stagger?,
            tilt: tilt?,
            velocity,
        })
    }

    /// A strummed chord of `members` keeps every velocity within 0 to 1.
    /// The number of members is known here when the chord has its own
    /// pitches, names a voicing or picks positions.
    pub(super) fn validate_tilt(&mut self, object: &Object, path: &[String], members: usize) {
        // A velocity outside 0 to 1 is reported as the velocity's own
        // mistake.
        let within =
            |velocity: &BigRational| !velocity.is_negative() && velocity <= &BigRational::one();
        let overshoot = self
            .models
            .strums
            .get(path)
            .filter(|strum| strum.velocity.as_ref().is_some_and(within))
            .and_then(|strum| strum.overshoot(members));
        if let (Some((member, velocity)), Some(field)) = (overshoot, object.field("tilt")) {
            self.push(
                DiagnosticCode::Range,
                Strum::overshoot_message(member, &velocity),
                Some(field.value.span),
                path.to_vec(),
                vec!["tilt".into()],
            );
        }
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

    fn ratio(numerator: i64, denominator: i64) -> BigRational {
        BigRational::new(numerator.into(), denominator.into())
    }

    #[test]
    fn a_strum_reads_its_stroke_its_stagger_and_its_tilt() {
        assert_eq!(
            strum("strum = up; stagger = 30ms; tilt = -1/10;"),
            Some(Strum {
                up: true,
                stagger: ratio(3, 100),
                tilt: ratio(-1, 10),
                velocity: Some(ratio(1, 1)),
            })
        );
    }

    #[test]
    fn a_strum_reads_the_velocity_its_tilt_starts_from() {
        let velocity = |fields: &str| strum(fields).unwrap().velocity;
        assert_eq!(velocity("strum = down; velocity = 3/5;"), Some(ratio(3, 5)));
        assert_eq!(velocity("strum = down;"), Some(ratio(1, 1)));
        assert_eq!(velocity("strum = down; velocity = [1/2, 1/2];"), None);
    }

    /// A downstroke gets louder or softer toward the last member, and an
    /// upstroke toward the first.
    #[test]
    fn a_tilt_overshoots_at_the_first_member_it_takes_out_of_range() {
        let strummed = |up: bool, velocity: BigRational, tilt: BigRational| Strum {
            up,
            stagger: ratio(0, 1),
            tilt,
            velocity: Some(velocity),
        };
        // 9/10 rising by 3/10 over four members: 9/10, 1, 11/10, 6/5.
        let rising = strummed(false, ratio(9, 10), ratio(3, 10));
        assert_eq!(rising.overshoot(4), Some((2, ratio(11, 10))));
        // Going up, the last member is struck first: 6/5, 11/10, 1, 9/10.
        assert_eq!(
            strummed(true, ratio(9, 10), ratio(3, 10)).overshoot(4),
            Some((0, ratio(6, 5)))
        );
        // Two members take the whole tilt at once.
        assert_eq!(rising.overshoot(2), Some((1, ratio(6, 5))));
        assert_eq!(
            strummed(false, ratio(1, 10), ratio(-1, 5)).overshoot(3),
            Some((2, ratio(-1, 10)))
        );
        // One member has no last string to tilt toward, and a tilt that
        // ends on 0 or 1 fits.
        assert_eq!(rising.overshoot(1), None);
        assert_eq!(
            strummed(false, ratio(9, 10), ratio(1, 10)).overshoot(6),
            None
        );
        assert_eq!(strummed(true, ratio(1, 5), ratio(-1, 5)).overshoot(6), None);
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
