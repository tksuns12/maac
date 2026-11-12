//! A voicing (§8.2): a named list of pitches that chords play.
//!
//! A chord names a voicing, or a use or a placement supplies one to the
//! chords under it. Each pitch is written in any form a note's `pitch`
//! takes, and lowers like one. The model holds how many pitches there are,
//! not the pitches.

use super::fields::{required, SourceField};
use super::Validator;
use crate::diagnostic::DiagnosticCode;
use crate::syntax::{Field, Object, ValueKind};

pub(crate) const FIELDS: &[SourceField] = &[required("pitches")];

/// A voicing as its object declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Voicing {
    /// How many pitches it has. There is at least one.
    pub(crate) pitches: usize,
}

impl Validator<'_> {
    /// Check a `voicing` object and keep its model.
    pub(super) fn validate_voicing(&mut self, object: &Object, path: &[String]) {
        if let Some(voicing) = self.read_voicing(object, path) {
            self.models.voicings.insert(object.id.clone(), voicing);
        }
    }

    fn read_voicing(&mut self, object: &Object, path: &[String]) -> Option<Voicing> {
        let reported = self.reported();
        self.check_declared(object, path, FIELDS);
        let pitches = object
            .field("pitches")
            .and_then(|field| self.validate_pitch_list(field, path));
        self.reject_children(object, path);
        (self.reported() == reported)
            .then_some(pitches)
            .flatten()
            .map(|pitches| Voicing { pitches })
    }

    /// A nonempty list of pitches, as a chord or voicing `pitches`; returns
    /// its length. Each pitch is checked like a note's `pitch`, and no model
    /// holds it: a wrong pitch does not hide how many there are.
    pub(super) fn validate_pitch_list(&mut self, field: &Field, path: &[String]) -> Option<usize> {
        match &field.value.kind {
            ValueKind::List(items) if !items.is_empty() => {
                let before = self.diagnostics.len();
                for item in items {
                    self.validate_pitch_value_at(item, path, "pitches");
                }
                self.outside_models += self.diagnostics.len() - before;
                Some(items.len())
            }
            _ => {
                self.push(
                    DiagnosticCode::Range,
                    "pitches must be a nonempty list of pitches",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["pitches".into()],
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    /// The model of `voicing v { ...fields }`, if it has one.
    fn voicing(fields: &str) -> Option<Voicing> {
        let document = crate::parse(&format!("maac 1; voicing v {{ {fields} }}")).unwrap();
        models_of(&document).voicings.remove("v")
    }

    #[test]
    fn a_voicing_reads_how_many_pitches_it_has() {
        assert_eq!(
            voicing(r#"pitches = [G2, 220Hz, D3, D3]; label = "open";"#),
            Some(Voicing { pitches: 4 })
        );
    }

    /// Editing counts the members of a chord in a tree whose imported
    /// tunings are not resolved.
    #[test]
    fn a_wrong_pitch_does_not_hide_how_many_there_are() {
        for pitches in ["[C4, 1q]", "[degree(1, &elsewhere.just), C4]"] {
            assert_eq!(
                voicing(&format!("pitches = {pitches};")),
                Some(Voicing { pitches: 2 }),
                "{pitches}"
            );
        }
    }

    #[test]
    fn a_voicing_with_a_wrong_field_has_no_model() {
        for fields in [
            "pitches = [];",
            "pitches = C4;",
            "pitches = [C4]; top = 1;",
            "pitches = [C4]; note n { at = 0q; dur = 1q; pitch = C4; }",
        ] {
            assert_eq!(voicing(fields), None, "{fields}");
        }
    }
}
