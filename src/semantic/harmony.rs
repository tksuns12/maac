//! A harmony (§8.2): a timeline of chord names that repeats every `length`.
//!
//! A use or a placement supplies a harmony with a part's voicings. A chord
//! under it that names no pitches then plays the voicing of the chord name
//! sounding at its position.

use num_rational::BigRational;
use num_traits::{Signed, Zero};

use super::fields::{required, SourceField};
use super::Validator;
use crate::diagnostic::DiagnosticCode;
use crate::syntax::{Object, Unit, ValueKind};

pub(crate) const FIELDS: &[SourceField] = &[required("length"), required("chords")];

/// A harmony as its object declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Harmony {
    /// The length of one pass, in quarter notes. It is positive.
    pub(crate) length: BigRational,
    /// Each chord name with the position it sounds from. The first is at
    /// 0q, and the positions strictly increase and stay under `length`.
    pub(crate) chords: Vec<(BigRational, String)>,
}

impl Harmony {
    /// The name of the chord sounding at `position` of one pass, from 0q
    /// and under `length`: the last entry at or before it.
    pub(crate) fn chord_within(&self, position: &BigRational) -> &str {
        self.chords
            .iter()
            .rev()
            .find(|(start, _)| start <= position)
            .map(|(_, name)| name.as_str())
            .expect("a harmony starts at 0q")
    }
}

/// The code for a `chords` that is not a nonempty list: an empty list is
/// out of range, and any other value has the wrong type.
pub(super) fn wrong_list(chords: &ValueKind) -> DiagnosticCode {
    match chords {
        ValueKind::List(_) => DiagnosticCode::Range,
        _ => DiagnosticCode::Unit,
    }
}

/// The code for a chord name that is not a nonempty string: an empty
/// string or a bare symbol is out of range, and any other value has the
/// wrong type.
pub(super) fn wrong_name(name: &ValueKind) -> DiagnosticCode {
    match name {
        ValueKind::String(_) | ValueKind::Symbol(_) => DiagnosticCode::Range,
        _ => DiagnosticCode::Unit,
    }
}

impl Validator<'_> {
    /// Check a `harmony` object and keep its model.
    pub(super) fn validate_harmony(&mut self, object: &Object, path: &[String]) {
        if let Some(harmony) = self.read_harmony(object, path) {
            self.models.harmonies.insert(object.id.clone(), harmony);
        }
    }

    fn read_harmony(&mut self, object: &Object, path: &[String]) -> Option<Harmony> {
        let reported = self.diagnostics.len();
        self.check_declared(object, path, FIELDS);
        let length = object.field("length").and_then(|field| {
            let length = self.quantity(field, &[Unit::Q], path, "length")?;
            self.within(
                length.is_positive(),
                "harmony.length must be positive",
                field,
                path,
            )
            .then_some(length)
        });
        let field = object.field("chords")?;
        let items = match &field.value.kind {
            ValueKind::List(items) if !items.is_empty() => items,
            kind => {
                self.push(
                    wrong_list(kind),
                    "chords must be a nonempty list of (position, \"name\") pairs",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["chords".into()],
                );
                return None;
            }
        };
        let mut chords: Vec<(BigRational, String)> = Vec::new();
        let mut previous: Option<BigRational> = None;
        for (index, item) in items.iter().enumerate() {
            let at = ["chords".to_owned(), index.to_string()];
            let ValueKind::Tuple(pair) = &item.kind else {
                self.push_type_value(item, path, "(position, \"name\") pair");
                continue;
            };
            if pair.len() != 2 {
                self.push(
                    DiagnosticCode::Range,
                    "harmony chords are (position, \"name\") pairs",
                    Some(item.span),
                    path.to_vec(),
                    at.to_vec(),
                );
                continue;
            }
            let name = pair[1].as_string().filter(|name| !name.is_empty());
            if name.is_none() {
                self.push(
                    wrong_name(&pair[1].kind),
                    "a harmony chord name must be a nonempty string",
                    Some(pair[1].span),
                    path.to_vec(),
                    at.to_vec(),
                );
            }
            let Some(position) =
                self.quantity_at(&pair[0], &[Unit::Q], path, &at, "harmony position")
            else {
                continue;
            };
            let problem = if index == 0 && !position.is_zero() {
                Some("the first harmony position must be 0q")
            } else if previous
                .as_ref()
                .is_some_and(|previous| &position <= previous)
            {
                Some("harmony positions must strictly increase")
            } else if length.as_ref().is_some_and(|length| &position >= length) {
                Some("harmony positions must be under its length")
            } else {
                None
            };
            if let Some(message) = problem {
                self.push(
                    DiagnosticCode::Range,
                    message,
                    Some(pair[0].span),
                    path.to_vec(),
                    at.to_vec(),
                );
            }
            if let Some(name) = name {
                chords.push((position.clone(), name.to_owned()));
            }
            previous = Some(position);
        }
        (self.diagnostics.len() == reported)
            .then_some(length)
            .flatten()
            .map(|length| Harmony { length, chords })
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    fn q(numerator: i64, denominator: i64) -> BigRational {
        BigRational::new(numerator.into(), denominator.into())
    }

    /// The model of `harmony h { ...fields }`, if it has one.
    fn harmony(fields: &str) -> Option<Harmony> {
        let document = crate::parse(&format!("maac 1; harmony h {{ {fields} }}")).unwrap();
        models_of(&document).harmonies.remove("h")
    }

    #[test]
    fn a_harmony_reads_its_length_and_its_chords() {
        assert_eq!(
            harmony(r#"length = 8q; chords = [(0q, "G"), (5/2q, "D")]; label = "verse";"#),
            Some(Harmony {
                length: q(8, 1),
                chords: vec![(q(0, 1), "G".into()), (q(5, 2), "D".into())],
            })
        );
    }

    #[test]
    fn a_chord_sounds_from_its_position_to_the_next() {
        let harmony =
            harmony(r#"length = 8q; chords = [(0q, "G"), (3q, "D"), (7q, "A")];"#).unwrap();
        for (position, name) in [
            (q(0, 1), "G"),
            (q(5, 2), "G"),
            (q(3, 1), "D"),
            (q(13, 2), "D"),
            (q(7, 1), "A"),
            (q(31, 4), "A"),
        ] {
            assert_eq!(harmony.chord_within(&position), name, "at {position}q");
        }
    }

    #[test]
    fn a_harmony_with_a_wrong_field_has_no_model() {
        for fields in [
            r#"length = 0q; chords = [(0q, "G")];"#,
            r#"length = 4s; chords = [(0q, "G")];"#,
            "length = 4q; chords = [];",
            r#"length = 4q; chords = [(1q, "G")];"#,
            r#"length = 4q; chords = [(0q, "G"), (0q, "D")];"#,
            r#"length = 4q; chords = [(0q, "G"), (4q, "D")];"#,
            r#"length = 4q; chords = [(0q, "")];"#,
            r#"length = 4q; chords = [(0q, "G"), "D"];"#,
            r#"length = 4q; chords = [(0q, "G", 1)];"#,
            r#"length = 4q; chords = [(0q, "G")]; key = C4;"#,
        ] {
            assert_eq!(harmony(fields), None, "{fields}");
        }
    }
}
