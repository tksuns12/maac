//! A voicing (§8.2): a named list of pitches that chords play.
//!
//! A chord names a voicing, or a use or a placement supplies one to the
//! chords under it. Each pitch is written in any form a note's `pitch`
//! takes, and lowers like one.

use super::fields::{required, SourceField};
use super::{Pitch, Validator};
use crate::syntax::Object;

pub(crate) const FIELDS: &[SourceField] = &[required("pitches")];

/// A voicing as its object declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Voicing {
    /// Its pitches, in list order. There is at least one.
    pub(crate) pitches: Vec<Pitch>,
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
            .and_then(|field| self.read_pitch_list(field, path).0);
        self.reject_children(object, path);
        (self.reported() == reported)
            .then_some(pitches)
            .flatten()
            .map(|pitches| Voicing { pitches })
    }
}

#[cfg(test)]
mod tests {
    use num_rational::BigRational;

    use super::super::models_of;
    use super::*;
    use crate::music::Spelling;

    /// The model of `voicing v { ...fields }`, if it has one.
    fn voicing(fields: &str) -> Option<Voicing> {
        let document = crate::parse(&format!("maac 1; voicing v {{ {fields} }}")).unwrap();
        models_of(&document).voicings.remove("v")
    }

    fn spelled(letter: char, accidental: i8, octave: i64) -> Pitch {
        Pitch::Spelled(Spelling::new(letter, accidental, octave).unwrap())
    }

    #[test]
    fn a_voicing_reads_its_pitches() {
        assert_eq!(
            voicing(r#"pitches = [G2, 220Hz, D3, D3]; label = "open";"#),
            Some(Voicing {
                pitches: vec![
                    spelled('G', 0, 2),
                    Pitch::Frequency(BigRational::from_integer(220.into())),
                    spelled('D', 0, 3),
                    spelled('D', 0, 3),
                ],
            })
        );
    }

    /// Editing reads the pitches of a voicing in an unlinked authored tree
    /// whose imported tunings are not resolved yet.
    #[test]
    fn an_imported_tuning_does_not_hide_a_voicing() {
        assert_eq!(
            voicing("pitches = [degree(1, &elsewhere.just), C4];"),
            Some(Voicing {
                pitches: vec![
                    Pitch::Degree {
                        index: 1,
                        tuning: vec!["elsewhere".into(), "just".into()],
                    },
                    spelled('C', 0, 4),
                ],
            })
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
