//! A track (§10): what groups a placement's occurrences, and the event
//! port that their notes, hits and messages go to.
//!
//! A track needs no `target` while every placement on it expands audio
//! leaves alone (§9.1). One that places a note, a hit or a message has
//! nowhere to send it.

use std::collections::BTreeSet;

use super::{ref_object_id, PortDirection, PortKind, Validator};
use crate::diagnostic::DiagnosticCode;
use crate::syntax::Object;

impl Validator<'_> {
    /// Check a `track` object.
    pub(super) fn validate_track(&mut self, object: &Object, path: &[String]) {
        self.check_schema(object, path, &[], &["target", "label"], &[]);
        if let Some(field) = object.field("target") {
            self.expect_port_ref(
                field,
                "target",
                Some(PortDirection::Input),
                Some(PortKind::Events),
                path,
            );
        }
        self.reject_children(object, path);
    }

    /// Whether pattern `id` holds a note, a chord, a hit or a message,
    /// itself or through its uses. A use repeats at least once, so each
    /// such leaf expands to an event wherever the pattern is placed.
    fn expands_events(&self, id: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut waiting = vec![id.to_owned()];
        while let Some(id) = waiting.pop() {
            let Some(pattern) = self
                .document
                .object(&id)
                .filter(|object| object.kind == "pattern")
            else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            for child in pattern.children.values() {
                match child.kind.as_str() {
                    "note" | "chord" | "hit" | "message" => return true,
                    "use" => waiting.extend(
                        child
                            .field("pattern")
                            .and_then(|field| ref_object_id(&field.value)),
                    ),
                    _ => {}
                }
            }
        }
        false
    }

    /// The track of placement `object` has a `target` when the placement
    /// expands a note, a hit or a message, from its pattern or an insert.
    pub(super) fn validate_place_target(&mut self, object: &Object, path: &[String]) {
        let Some(field) = object.field("track") else {
            return;
        };
        let bare = ref_object_id(&field.value)
            .and_then(|id| self.document.object(&id))
            .is_some_and(|track| track.kind == "track" && track.field("target").is_none());
        if !bare {
            return;
        }
        let inserted = object
            .children
            .values()
            .filter(|child| child.kind == "insert")
            .flat_map(|insert| insert.children.values())
            .any(|leaf| matches!(leaf.kind.as_str(), "note" | "hit" | "message"));
        let placed = object
            .field("pattern")
            .and_then(|field| ref_object_id(&field.value))
            .is_some_and(|pattern| self.expands_events(&pattern));
        if inserted || placed {
            self.push(
                DiagnosticCode::Reference,
                "note, hit, and message leaves require a track with an event target",
                Some(field.value.span),
                path.to_vec(),
                vec!["track".into()],
            );
        }
    }
}
