//! A voicing (§8.2): a named list of pitches that chords play.
//!
//! A chord names a voicing, or a use or a placement supplies one to the
//! chords under it. Each pitch is written in any form a note's `pitch`
//! takes, and lowers like one.

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
        let reported = self.diagnostics.len();
        self.check_declared(object, path, FIELDS);
        let pitches = object
            .field("pitches")
            .and_then(|field| self.validate_pitch_list(field, path));
        self.reject_children(object, path);
        (self.diagnostics.len() == reported)
            .then_some(pitches)
            .flatten()
            .map(|pitches| Voicing { pitches })
    }

    /// A nonempty list of pitches, as a chord or voicing `pitches`; returns
    /// its length.
    pub(super) fn validate_pitch_list(&mut self, field: &Field, path: &[String]) -> Option<usize> {
        match &field.value.kind {
            ValueKind::List(items) if !items.is_empty() => {
                for item in items {
                    self.validate_pitch_value_at(item, path, "pitches");
                }
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

    #[test]
    fn a_voicing_with_a_wrong_field_has_no_model() {
        for fields in [
            "pitches = [];",
            "pitches = C4;",
            "pitches = [C4, 1q];",
            "pitches = [C4]; top = 1;",
            "pitches = [C4]; note n { at = 0q; dur = 1q; pitch = C4; }",
        ] {
            assert_eq!(voicing(fields), None, "{fields}");
        }
    }
}
