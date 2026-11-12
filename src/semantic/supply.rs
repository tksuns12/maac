//! What a use or a placement supplies to the chords under it that name no
//! pitches (§8.2): one voicing, or a harmony with a part's voicings.
//!
//! The nearest use or placement that supplies anything is a chord's
//! supplier. A chord's own `pitches` or `voicing` ignores it.

use super::{ReferenceTarget, Validator};
use crate::diagnostic::DiagnosticCode;
use crate::syntax::Object;

/// What a use or a placement supplies, as its fields declare it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Supply {
    /// The voicing with this ID.
    Voicing(String),
    /// For each chord, the voicing that the voicings with ID `voicings` map
    /// to the chord name of the harmony with ID `harmony` at the chord's
    /// position.
    Harmony { harmony: String, voicings: String },
}

impl Validator<'_> {
    /// Check the `voicing`, `harmony` and `voicings` of `object`, a use or
    /// a placement, and keep what it supplies under its path.
    pub(super) fn validate_supply(&mut self, object: &Object, path: &[String]) {
        if let Some(supply) = self.read_supply(object, path) {
            self.models.supplies.insert(path.to_vec(), supply);
        }
    }

    fn read_supply(&mut self, object: &Object, path: &[String]) -> Option<Supply> {
        let reported = self.reported();
        let [voicing, harmony, voicings] = ["voicing", "harmony", "voicings"].map(|name| {
            let field = object.field(name)?;
            match self.expect_object_ref(field, name, &[name], path)? {
                ReferenceTarget::Object { id, .. } => Some(id),
                _ => None,
            }
        });
        let kind = &object.kind;
        let problem = match (
            object.field("voicing"),
            object.field("harmony"),
            object.field("voicings"),
        ) {
            (Some(_), Some(field), _) => {
                Some((field, format!("{kind}.harmony cannot sit beside `voicing`")))
            }
            (_, Some(field), None) => Some((
                field,
                format!("{kind}.harmony needs `voicings`, the part's voicing for each chord"),
            )),
            (_, None, Some(field)) => Some((field, format!("{kind}.voicings needs a `harmony`"))),
            _ => None,
        };
        if let Some((field, message)) = problem {
            self.push(
                DiagnosticCode::Range,
                message,
                Some(field.value.span),
                path.to_vec(),
                vec![field.name.clone()],
            );
        }
        if self.reported() != reported {
            return None;
        }
        match (voicing, harmony, voicings) {
            (Some(voicing), None, None) => Some(Supply::Voicing(voicing)),
            (None, Some(harmony), Some(voicings)) => Some(Supply::Harmony { harmony, voicings }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    /// What `use u` of a pattern and `place p` supply when each has
    /// `fields`.
    fn supplies(fields: &str) -> [Option<Supply>; 2] {
        let document = crate::parse(&format!(
            "maac 1; voicing g {{ pitches = [G2, D3]; }} \
             harmony h {{ length = 4q; chords = [(0q, \"G\")]; }} \
             voicings v {{ chords = [(\"G\", &g)]; }} \
             pattern bar {{ length = 4q; }} \
             pattern song {{ length = 4q; use u {{ pattern = &bar; at = 0q; {fields} }} }} \
             place p {{ pattern = &bar; track = &t; at = 0q; {fields} }}"
        ))
        .unwrap();
        let mut models = models_of(&document);
        [
            models
                .supplies
                .remove(["song".to_owned(), "u".to_owned()].as_slice()),
            models.supplies.remove(["p".to_owned()].as_slice()),
        ]
    }

    #[test]
    fn a_use_and_a_placement_supply_a_voicing_or_a_harmony() {
        let voicing = Supply::Voicing("g".into());
        assert_eq!(
            supplies("voicing = &g;"),
            [Some(voicing.clone()), Some(voicing)]
        );
        let harmony = Supply::Harmony {
            harmony: "h".into(),
            voicings: "v".into(),
        };
        assert_eq!(
            supplies("harmony = &h; voicings = &v;"),
            [Some(harmony.clone()), Some(harmony)]
        );
        assert_eq!(supplies(""), [None, None]);
    }

    #[test]
    fn a_wrong_supply_supplies_nothing() {
        for fields in [
            "voicing = &g; harmony = &h; voicings = &v;",
            "voicing = &g; voicings = &v;",
            "harmony = &h;",
            "voicings = &v;",
            "voicing = &h;",
            "voicing = &nowhere;",
            "harmony = &v; voicings = &h;",
            "harmony = &h; voicings = &g;",
        ] {
            assert_eq!(supplies(fields), [None, None], "{fields}");
        }
    }
}
