//! What an override's `set` may replace on the event it names (§11, §9.1).
//!
//! `set` replaces fields of the leaf's kind, and a note has fields that a
//! hit, a message or an audio leaf has not. Which kind an override names
//! shows only where its event address leads: the compiler knows it of the
//! event it expanded, and editing of the leaf it walked to. Both ask here,
//! and refuse in these words.

use crate::diagnostic::DiagnosticCode;

/// The kind of event an override names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Replaced {
    /// A note, or a member of a chord.
    Note,
    Hit,
    Message,
    Audio,
}

impl Replaced {
    /// The code of a `set` field that its event's kind does not take.
    pub(crate) const REFUSED: DiagnosticCode = DiagnosticCode::UnknownField;

    /// The kind of event a leaf of object kind `kind` expands to.
    pub(crate) fn of_leaf(kind: &str) -> Option<Self> {
        match kind {
            "note" | "chord" => Some(Self::Note),
            "hit" => Some(Self::Hit),
            "message" => Some(Self::Message),
            "audio" => Some(Self::Audio),
            _ => None,
        }
    }

    /// The fields that `set` may replace on an event of this kind.
    pub(crate) fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Note => &[
                "at",
                "dur",
                "pitch",
                "velocity",
                "release_velocity",
                "onset_offset",
                "release_offset",
                "order",
                "label",
            ],
            Self::Hit => &["at", "key", "velocity", "onset_offset", "order", "label"],
            Self::Message => &["at", "protocol", "bytes", "onset_offset", "order", "label"],
            Self::Audio => &["at", "gain", "fade_in", "fade_out", "fade_shape", "label"],
        }
    }

    /// Why `set` may not replace `field` on an event of this kind, if it
    /// may not.
    pub(crate) fn refusal(self, field: &str) -> Option<String> {
        let event = match self {
            Self::Note => "a note",
            Self::Hit => "a hit",
            Self::Message => "a message",
            Self::Audio => "an audio leaf",
        };
        (!self.fields().contains(&field))
            .then(|| format!("override.set cannot replace `{field}` of {event}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_member_is_a_note() {
        assert_eq!(Replaced::of_leaf("chord"), Some(Replaced::Note));
        assert_eq!(Replaced::of_leaf("note"), Some(Replaced::Note));
        assert_eq!(Replaced::of_leaf("audio"), Some(Replaced::Audio));
        assert_eq!(Replaced::of_leaf("use"), None);
    }

    #[test]
    fn each_kind_takes_its_own_fields_and_says_which_it_does_not() {
        assert_eq!(Replaced::Note.refusal("pitch"), None);
        assert_eq!(Replaced::Hit.refusal("key"), None);
        assert_eq!(Replaced::Message.refusal("bytes"), None);
        assert_eq!(Replaced::Audio.refusal("fade_shape"), None);
        assert_eq!(
            Replaced::Note.refusal("key").as_deref(),
            Some("override.set cannot replace `key` of a note")
        );
        assert_eq!(
            Replaced::Hit.refusal("dur").as_deref(),
            Some("override.set cannot replace `dur` of a hit")
        );
        assert_eq!(
            Replaced::Message.refusal("velocity").as_deref(),
            Some("override.set cannot replace `velocity` of a message")
        );
        assert_eq!(
            Replaced::Audio.refusal("order").as_deref(),
            Some("override.set cannot replace `order` of an audio leaf")
        );
    }

    /// Every kind may move its event and name it.
    #[test]
    fn every_kind_takes_at_and_label() {
        for kind in [
            Replaced::Note,
            Replaced::Hit,
            Replaced::Message,
            Replaced::Audio,
        ] {
            assert_eq!(kind.refusal("at"), None, "{kind:?}");
            assert_eq!(kind.refusal("label"), None, "{kind:?}");
        }
    }
}
