//! What stops a chord from playing where a use or a placement puts it
//! (§8.2).
//!
//! Whether a chord without pitches has a voicing, and whether its `top`,
//! `bottom`, `pick`, per-member lists and tilt fit that voicing, shows only
//! at each occurrence. The compiler finds these while it expands a
//! placement, and editing while it walks the occurrences of a tree. Both
//! report them with these codes and in these words.

use num_rational::BigRational;

use super::{Misfit, Models, Strum, Supply, Voicings};
use crate::diagnostic::DiagnosticCode;

/// Why a chord does not play at one occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Unplayable {
    pub(crate) code: DiagnosticCode,
    pub(crate) message: String,
    /// The chord field at fault, when there is one.
    pub(crate) field: Option<&'static str>,
}

impl Unplayable {
    /// Chord `chord` names no pitches, and nothing supplies a voicing.
    pub(crate) fn no_voicing(chord: &str) -> Self {
        Self {
            code: DiagnosticCode::Reference,
            message: format!(
                "chord `{chord}` has no `pitches` or `voicing`, and no use or placement that reaches it names a voicing"
            ),
            field: None,
        }
    }

    /// Chord `chord` plays the chord `name` of harmony `harmony`, which
    /// the voicings with ID `voicings` that supplier `by` names do not
    /// map.
    pub(crate) fn unmapped(
        chord: &str,
        name: &str,
        harmony: &str,
        voicings: &str,
        mapped: &Voicings,
        by: &str,
    ) -> Self {
        let mapped: Vec<String> = mapped
            .chords
            .keys()
            .map(|name| format!("\"{name}\""))
            .collect();
        Self {
            code: DiagnosticCode::Reference,
            message: format!(
                "chord `{chord}` plays \"{name}\" from harmony `{harmony}`, which voicings `{voicings}` does not map; it maps {} (from {by})",
                mapped.join(", ")
            ),
            field: None,
        }
    }

    /// The chord's `top`, `bottom`, `pick` or a per-member list does not
    /// fit its pitches.
    pub(crate) fn misfit(misfit: &Misfit) -> Self {
        Self {
            code: DiagnosticCode::Range,
            message: misfit.message(),
            field: Some(misfit.field()),
        }
    }

    /// The tilt takes member `member` to `velocity`, outside 0 to 1.
    pub(crate) fn overshoot(member: usize, velocity: &BigRational) -> Self {
        Self {
            code: DiagnosticCode::Range,
            message: Strum::overshoot_message(member, velocity),
            field: Some("tilt"),
        }
    }

    /// The same, naming the supplied voicing the chord does not fit, in the
    /// words of [`origin`].
    pub(crate) fn under(mut self, origin: &str) -> Self {
        self.message = format!("{} ({origin})", self.message);
        self
    }
}

/// The words for the use or placement with ID `id` that supplies a voicing,
/// where `kind` is its object kind.
pub(crate) fn supplier(kind: &str, id: &str) -> String {
    match kind {
        "place" => format!("placement `{id}`"),
        _ => format!("{kind} `{id}`"),
    }
}

/// The words for voicing `voicing` that `supply` gives a chord through
/// supplier `by`. `name` is the chord of the harmony it voices, when the
/// supply is a harmony.
pub(crate) fn origin(supply: &Supply, voicing: &str, name: Option<&str>, by: &str) -> String {
    match (supply, name) {
        (Supply::Harmony { harmony, .. }, Some(name)) => {
            format!("voicing `{voicing}` for \"{name}\" in harmony `{harmony}` from {by}")
        }
        _ => format!("voicing `{voicing}` from {by}"),
    }
}

impl Models {
    /// How many members the chord at object path `chord` has where it is
    /// supplied a voicing of `pitches` pitches, unless it does not fit
    /// them: its `top`, `bottom` or `pick`, then its per-member lists, then
    /// its tilt. `None` is a chord that did not read.
    pub(crate) fn members_given(
        &self,
        chord: &[String],
        pitches: usize,
    ) -> Option<Result<usize, Unplayable>> {
        let played = self.played.get(chord)?;
        let members = match played.of(&vec![(); pitches]) {
            Ok(members) => members.len(),
            Err(misfit) => return Some(Err(Unplayable::misfit(&misfit))),
        };
        let overshoot = self
            .strums
            .get(chord)
            .and_then(|strum| strum.overshoot(members));
        Some(match overshoot {
            Some((member, velocity)) => Err(Unplayable::overshoot(member, &velocity)),
            None => Ok(members),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::models_of;
    use super::*;

    /// The models of pattern `bar` with chord `c { ...fields }`.
    fn models(fields: &str) -> Models {
        models_of(
            &crate::parse(&format!(
                "maac 1; pattern bar {{ length = 4q; chord c {{ at = 0q; dur = 1q; {fields} }} }}"
            ))
            .unwrap(),
        )
    }

    fn given(fields: &str, pitches: usize) -> Result<usize, String> {
        models(fields)
            .members_given(&["bar".to_owned(), "c".to_owned()], pitches)
            .expect("the chord reads")
            .map_err(|unplayable| {
                assert_eq!(unplayable.code, DiagnosticCode::Range, "{fields}");
                format!("{}: {}", unplayable.field.unwrap(), unplayable.message)
            })
    }

    #[test]
    fn a_chord_has_the_members_its_fields_keep_of_a_supplied_voicing() {
        assert_eq!(given("", 6), Ok(6));
        assert_eq!(given("top = 4;", 6), Ok(4));
        assert_eq!(given("pick = [1, 3];", 4), Ok(2));
        assert_eq!(
            given("bottom = 2; velocity = [1/2, 1/4]; strum = up;", 4),
            Ok(2)
        );
    }

    /// The first rule a chord breaks is the one reported: what it keeps,
    /// then its lists, then its tilt.
    #[test]
    fn a_chord_that_does_not_fit_a_supplied_voicing_says_how() {
        assert_eq!(
            given("top = 5; velocity = [1/2]; strum = down; tilt = 1/2;", 4),
            Err("top: chord.top must be an integer from 1 through 4, the number of pitches".into())
        );
        assert_eq!(
            given("pick = [1, 5];", 4),
            Err(
                "pick: chord.pick positions must be integers from 1 through 4, the number of pitches"
                    .into()
            )
        );
        assert_eq!(
            given("top = 3; release_offset = [1ms, 2ms];", 4),
            Err(
                "release_offset: chord.release_offset must have one entry per pitch played (3)"
                    .into()
            )
        );
        assert_eq!(
            given("strum = down; velocity = 9/10; tilt = 3/10;", 4),
            Err("tilt: chord velocity with tilt is 11/10 at member 2, outside [0,1]".into())
        );
        // One pitch has no last string to tilt toward.
        assert_eq!(
            given("strum = down; velocity = 9/10; tilt = 3/10;", 1),
            Ok(1)
        );
    }

    #[test]
    fn the_words_for_a_supplier_and_its_voicing() {
        assert_eq!(supplier("place", "main"), "placement `main`");
        assert_eq!(supplier("use", "b2"), "use `b2`");
        let voicing = Supply::Voicing("g".into());
        assert_eq!(
            origin(&voicing, "g", None, "use `b2`"),
            "voicing `g` from use `b2`"
        );
        let harmony = Supply::Harmony {
            harmony: "turn".into(),
            voicings: "strings".into(),
        };
        assert_eq!(
            origin(&harmony, "d", Some("D"), "placement `main`"),
            "voicing `d` for \"D\" in harmony `turn` from placement `main`"
        );
    }
}
