//! What a placement's destination plays of what the placement sends it
//! (§6.2, §8.3, §10).
//!
//! A track's `target` is an event port of a node, and each kind of node
//! plays its own events: a synth or an instrument plays notes, with the
//! pressure, the timbre and the connected notes that its voice provides,
//! and a kit plays hits on its keys. A leaf is right or wrong only where a
//! placement sends it, so this is checked for each placement, over the
//! leaves its pattern reaches through uses and the leaves it inserts.
//!
//! An override may delete an occurrence or replace a hit's key, and only an
//! expansion says which occurrence it names. A leaf that such an override
//! names by ID is left to the compiler.

use std::collections::BTreeSet;

use super::{child_path, ref_object_id, ProcessorKind, Validator};
use crate::diagnostic::DiagnosticCode;
use crate::plan_v7::ProcessorV7;
use crate::syntax::{Object, ValueKind};

/// What the node behind an event port plays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Receives {
    /// Notes, and of what a note may carry:
    Notes {
        /// a pressure expression;
        pressure: bool,
        /// a timbre expression;
        timbre: bool,
        /// a `slur` or a `slide`, as it joins notes that touch.
        connected: bool,
    },
    /// Hits on these keys, and no notes.
    Hits(BTreeSet<String>),
}

/// What is wrong with what a placement sends its destination: the code, the
/// message, and the field of the leaf at fault when there is one.
type Unplayed = (DiagnosticCode, &'static str, Option<&'static str>);

impl Receives {
    /// What stops this destination from playing the note or the chord
    /// `leaf`.
    fn refuses_note(&self, leaf: &Object) -> Vec<Unplayed> {
        let marked = if matches!(
            leaf.field("slur").map(|field| &field.value.kind),
            Some(ValueKind::Boolean(true))
        ) {
            Some("slur")
        } else {
            leaf.field("slide")
                .filter(|field| match &field.value.kind {
                    ValueKind::Quantity { value, .. } => value > &num_traits::Zero::zero(),
                    _ => false,
                })
                .map(|_| "slide")
        };
        let unconnected = |field| {
            (
                DiagnosticCode::Capability,
                "`slur` and `slide` need a destination that plays connected notes",
                Some(field),
            )
        };
        let (pressure, timbre, connected) = match self {
            Self::Hits(_) => {
                return vec![marked.map_or(
                    (
                        DiagnosticCode::Capability,
                        "target does not accept this event transport",
                        None,
                    ),
                    unconnected,
                )]
            }
            Self::Notes {
                pressure,
                timbre,
                connected,
            } => (*pressure, *timbre, *connected),
        };
        let carries = |kind: &str| {
            leaf.children.values().any(|child| {
                child.kind == "expression"
                    && child
                        .field("kind")
                        .is_some_and(|field| field.value.as_symbol() == Some(kind))
            })
        };
        let mut refused = Vec::new();
        if let Some(field) = marked.filter(|_| !connected) {
            refused.push(unconnected(field));
        }
        if !timbre && carries("timbre") {
            refused.push((
                DiagnosticCode::Capability,
                "timbre expression requires an instrument with synth.timbre/1",
                None,
            ));
        }
        if !pressure && carries("pressure") {
            refused.push((
                DiagnosticCode::Capability,
                "pressure expression requires an instrument with synth.pressure/1",
                None,
            ));
        }
        refused
    }

    /// What stops this destination from playing a hit on `key`, when the
    /// key is known.
    fn refuses_hit(&self, key: Option<&str>) -> Option<Unplayed> {
        match self {
            Self::Notes { .. } => Some((
                DiagnosticCode::Capability,
                "hit requires a kit receiver",
                None,
            )),
            Self::Hits(keys) => key.filter(|key| !keys.contains(*key)).map(|_| {
                (
                    DiagnosticCode::Reference,
                    "kit has no such key",
                    Some("key"),
                )
            }),
        }
    }
}

/// The ID of the leaf that the event address `event` ends in: its last
/// segment, without a chord member's index.
fn named_leaf(event: &str) -> &str {
    let leaf = event.rsplit('/').next().unwrap_or_default();
    leaf.split_once('.').map_or(leaf, |(chord, _)| chord)
}

impl Validator<'_> {
    /// What the node with ID `node` plays, when it is known: a library
    /// instance by its descriptor, a native kit by its samples, and
    /// `core.sine/1`.
    fn receives(&self, node: &str) -> Option<Receives> {
        if let Some(instance) = self.instrument_nodes.get(node) {
            return instance.receives.clone();
        }
        if let Some(kit) = self.kit_nodes.get(node) {
            let ProcessorV7::Kit { samples, .. } = &kit.processor else {
                return None;
            };
            return Some(Receives::Hits(
                samples.iter().map(|sample| sample.key.clone()).collect(),
            ));
        }
        matches!(self.nodes.get(node)?.processor, ProcessorKind::Sine).then_some(Receives::Notes {
            pressure: false,
            timbre: false,
            connected: false,
        })
    }

    /// The patterns that pattern `id` reaches through uses, itself included.
    fn reached_patterns(&self, id: &str) -> Vec<&Object> {
        let mut seen = BTreeSet::new();
        let mut waiting = vec![id.to_owned()];
        let mut reached = Vec::new();
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
            waiting.extend(
                pattern
                    .children
                    .values()
                    .filter(|child| child.kind == "use")
                    .filter_map(|child| ref_object_id(&child.field("pattern")?.value)),
            );
            reached.push(pattern);
        }
        reached
    }

    /// The destination of placement `object` plays what the placement
    /// sends it.
    pub(super) fn validate_place_destination(&mut self, object: &Object, path: &[String]) {
        let Some(node) = object
            .field("track")
            .and_then(|field| ref_object_id(&field.value))
            .and_then(|track| self.document.object(&track))
            .filter(|track| track.kind == "track")
            .and_then(|track| track.field("target")?.value.reference())
            .filter(|target| target.path.len() == 1)
            .map(|target| target.path[0].clone())
        else {
            return;
        };
        let Some(receives) = self.receives(&node) else {
            return;
        };
        let by = format!(" (placement `{}` plays it on `{node}`)", object.id);
        let overrides = || {
            object
                .children
                .values()
                .filter(|child| child.kind == "override")
        };
        // The leaves whose occurrences an override may delete, and the hits
        // whose key one may replace.
        let mut deleted = BTreeSet::new();
        let mut rekeyed = BTreeSet::new();
        let mut found = Vec::new();
        for child in overrides() {
            let Some(event) = child.field("event").and_then(|f| f.value.as_string()) else {
                continue;
            };
            if child.field("delete").is_some() {
                deleted.insert(named_leaf(event).to_owned());
            }
            let Some(ValueKind::Record(set)) = child.field("set").map(|f| &f.value.kind) else {
                continue;
            };
            if let Some(key) = set.get("key") {
                rekeyed.insert(named_leaf(event).to_owned());
                if let (Receives::Hits(_), Some(text)) = (&receives, key.value.as_string()) {
                    if let Some((code, message, _)) = receives.refuses_hit(Some(text)) {
                        found.push((
                            code,
                            message,
                            key.value.span,
                            child_path(path, &child.id),
                            vec!["set".to_owned(), "key".to_owned()],
                        ));
                    }
                }
            }
        }
        let placed = object
            .field("pattern")
            .and_then(|field| ref_object_id(&field.value))
            .map(|pattern| self.reached_patterns(&pattern))
            .unwrap_or_default();
        let leaves = placed
            .into_iter()
            .flat_map(|pattern| {
                pattern
                    .children
                    .values()
                    .filter(|leaf| !deleted.contains(&leaf.id))
                    .map(|leaf| (vec![pattern.id.clone(), leaf.id.clone()], leaf, true))
            })
            .chain(
                object
                    .children
                    .values()
                    .filter(|child| child.kind == "insert")
                    .flat_map(|insert| {
                        insert.children.values().map(|leaf| {
                            let path = child_path(&child_path(path, &insert.id), &leaf.id);
                            (path, leaf, false)
                        })
                    }),
            );
        for (leaf_path, leaf, placed) in leaves {
            let refused = match leaf.kind.as_str() {
                "note" | "chord" => receives.refuses_note(leaf),
                "hit" => {
                    let key = leaf
                        .field("key")
                        .and_then(|field| field.value.as_string())
                        .filter(|_| !(placed && rekeyed.contains(&leaf.id)));
                    receives.refuses_hit(key).into_iter().collect()
                }
                _ => Vec::new(),
            };
            for (code, message, field) in refused {
                let span = field
                    .and_then(|name| leaf.field(name))
                    .map_or(leaf.span, |field| field.value.span);
                found.push((
                    code,
                    message,
                    span,
                    leaf_path.clone(),
                    field.map(str::to_owned).into_iter().collect(),
                ));
            }
        }
        for (code, message, span, at, field) in found {
            self.push(code, format!("{message}{by}"), Some(span), at, field);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(text: &str) -> Object {
        let document =
            crate::parse(&format!("maac 1; pattern bar {{ length = 4q; {text} }}")).unwrap();
        document.object("bar").unwrap().children["a"].clone()
    }

    fn messages(refused: Vec<Unplayed>) -> Vec<(&'static str, Option<&'static str>)> {
        refused
            .into_iter()
            .map(|(_, message, field)| (message, field))
            .collect()
    }

    const PLAIN: Receives = Receives::Notes {
        pressure: false,
        timbre: false,
        connected: false,
    };
    const RICH: Receives = Receives::Notes {
        pressure: true,
        timbre: true,
        connected: true,
    };
    const NOTE: &str = "note a { at = 0q; dur = 1q; pitch = C4; EXTRA }";
    const UNCONNECTED: &str = "`slur` and `slide` need a destination that plays connected notes";

    #[test]
    fn a_destination_plays_the_notes_its_voice_provides_for() {
        let touched = leaf(&NOTE.replace(
            "EXTRA",
            "slide = 50ms; expression p { kind = pressure; curve = &c; } expression t { kind = timbre; curve = &c; } expression g { kind = gain; curve = &c; }",
        ));
        assert_eq!(messages(RICH.refuses_note(&touched)), []);
        assert_eq!(
            messages(PLAIN.refuses_note(&touched)),
            [
                (UNCONNECTED, Some("slide")),
                (
                    "timbre expression requires an instrument with synth.timbre/1",
                    None
                ),
                (
                    "pressure expression requires an instrument with synth.pressure/1",
                    None
                ),
            ]
        );
        // A gain or a pitch expression, and a slide of no time, need nothing.
        let plain = leaf(&NOTE.replace(
            "EXTRA",
            "slur = false; slide = 0s; expression g { kind = gain; curve = &c; } expression b { kind = pitch; curve = &c; }",
        ));
        assert_eq!(messages(PLAIN.refuses_note(&plain)), []);
    }

    #[test]
    fn a_kit_plays_hits_on_its_keys_and_no_notes() {
        let kit = Receives::Hits(BTreeSet::from(["kick".to_owned()]));
        assert_eq!(kit.refuses_hit(Some("kick")), None);
        assert_eq!(kit.refuses_hit(None), None);
        assert_eq!(
            kit.refuses_hit(Some("cowbell")),
            Some((
                DiagnosticCode::Reference,
                "kit has no such key",
                Some("key")
            ))
        );
        assert_eq!(
            messages(kit.refuses_note(&leaf(&NOTE.replace("EXTRA", "")))),
            [("target does not accept this event transport", None)]
        );
        // A slur is what the compiler reports of a slurred note on a kit.
        assert_eq!(
            messages(kit.refuses_note(&leaf(&NOTE.replace("EXTRA", "slur = true;")))),
            [(UNCONNECTED, Some("slur"))]
        );
        assert_eq!(
            PLAIN.refuses_hit(Some("kick")),
            Some((
                DiagnosticCode::Capability,
                "hit requires a kit receiver",
                None
            ))
        );
    }

    #[test]
    fn an_event_address_ends_in_its_leaf() {
        assert_eq!(named_leaf("0/a"), "a");
        assert_eq!(named_leaf("1/u/0/down.2"), "down");
        assert_eq!(named_leaf(""), "");
    }
}
