//! A part's voicings (§8.2): the voicing the part plays for each chord name
//! of a harmony.
//!
//! Names are compared exactly, and are never read as chord symbols.

use std::collections::{BTreeMap, BTreeSet};

use super::fields::{required, SourceField};
use super::harmony::{wrong_list, wrong_name};
use super::Validator;
use crate::diagnostic::DiagnosticCode;
use crate::syntax::{Object, ValueKind};

pub(crate) const FIELDS: &[SourceField] = &[required("chords")];

/// A part's voicings as their object declares them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Voicings {
    /// The ID of the voicing for each chord name. There is at least one.
    pub(crate) chords: BTreeMap<String, String>,
}

impl Validator<'_> {
    /// Check a `voicings` object and keep its model.
    pub(super) fn validate_voicings(&mut self, object: &Object, path: &[String]) {
        if let Some(voicings) = self.read_voicings(object, path) {
            self.models.voicing_sets.insert(object.id.clone(), voicings);
        }
    }

    fn read_voicings(&mut self, object: &Object, path: &[String]) -> Option<Voicings> {
        let reported = self.diagnostics.len();
        self.check_declared(object, path, FIELDS);
        let field = object.field("chords")?;
        let items = match &field.value.kind {
            ValueKind::List(items) if !items.is_empty() => items,
            kind => {
                self.push(
                    wrong_list(kind),
                    "chords must be a nonempty list of (\"name\", &voicing) pairs",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["chords".into()],
                );
                return None;
            }
        };
        let mut names = BTreeSet::new();
        let mut chords = BTreeMap::new();
        for (index, item) in items.iter().enumerate() {
            let at = ["chords".to_owned(), index.to_string()];
            let ValueKind::Tuple(pair) = &item.kind else {
                self.push_type_value(item, path, "(\"name\", &voicing) pair");
                continue;
            };
            if pair.len() != 2 {
                self.push(
                    DiagnosticCode::Range,
                    "voicings chords are (\"name\", &voicing) pairs",
                    Some(item.span),
                    path.to_vec(),
                    at.to_vec(),
                );
                continue;
            }
            let name = pair[0].as_string().filter(|name| !name.is_empty());
            match name {
                Some(name) => {
                    if !names.insert(name) {
                        self.push(
                            DiagnosticCode::Range,
                            format!("voicings name \"{name}\" twice"),
                            Some(pair[0].span),
                            path.to_vec(),
                            at.to_vec(),
                        );
                    }
                }
                None => self.push(
                    wrong_name(&pair[0].kind),
                    "a voicings chord name must be a nonempty string",
                    Some(pair[0].span),
                    path.to_vec(),
                    at.to_vec(),
                ),
            }
            let voicing = pair[1]
                .reference()
                .filter(|reference| reference.port.is_none() && reference.path.len() == 1)
                .map(|reference| &reference.path[0])
                .filter(|id| {
                    self.document
                        .object(id)
                        .is_some_and(|object| object.kind == "voicing")
                });
            if voicing.is_none() {
                self.push(
                    DiagnosticCode::Reference,
                    "each voicings entry must reference a voicing",
                    Some(pair[1].span),
                    path.to_vec(),
                    at.to_vec(),
                );
            }
            if let (Some(name), Some(voicing)) = (name, voicing) {
                chords.insert(name.to_owned(), voicing.clone());
            }
        }
        (self.diagnostics.len() == reported).then_some(Voicings { chords })
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    /// The model of `voicings v { ...fields }`, if it has one.
    fn voicings(fields: &str) -> Option<Voicings> {
        let document = crate::parse(&format!(
            "maac 1; voicing g {{ pitches = [G2, D3]; }} voicing d {{ pitches = [D3, A3]; }} \
             harmony h {{ length = 4q; chords = [(0q, \"G\")]; }} voicings v {{ {fields} }}"
        ))
        .unwrap();
        models_of(&document).voicing_sets.remove("v")
    }

    #[test]
    fn voicings_read_the_voicing_of_each_name() {
        assert_eq!(
            voicings(r#"chords = [("G", &g), ("D", &d), ("G/B", &g)]; label = "guitar";"#),
            Some(Voicings {
                chords: [("D", "d"), ("G", "g"), ("G/B", "g")]
                    .into_iter()
                    .map(|(name, id)| (name.to_owned(), id.to_owned()))
                    .collect(),
            })
        );
    }

    #[test]
    fn voicings_with_a_wrong_field_have_no_model() {
        for fields in [
            "chords = [];",
            r#"chords = ("G", &g);"#,
            r#"chords = [("G", &g), ("G", &d)];"#,
            r#"chords = [("", &g)];"#,
            r#"chords = [("G", &h)];"#,
            r#"chords = [("G", &nowhere)];"#,
            r#"chords = [("G", &g, &d)];"#,
            r#"chords = [("G", &g), &d];"#,
            r#"chords = [("G", &g)]; length = 4q;"#,
        ] {
            assert_eq!(voicings(fields), None, "{fields}");
        }
    }
}
