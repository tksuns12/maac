//! Editing reads placements, uses, chords, voicings and harmonies from the
//! authored tree, apart from the compiler. These tests hold what it reads
//! to the compiled plan: the address of every event, and the pitch of
//! every chord member.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{enumerate_event_occurrences, names_insert, object_at, resolve_occurrence_pitches};
use crate::bundle::SourceBundle;
use crate::editing::AuthoredDocument;

/// Three voicings of different sizes, the voicings `strings` and the
/// harmony `turn` over them, and `legend`: one note of every pitch the
/// voicings and the tests' chords use, placed by `key`.
const HEAD: &str = r#"maac 1;
project p { score = [0q, 128q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node synth { type = "core.sine/1"; params = { attack = 5ms; release = 50ms; level = 0.1; }; }
track t { target = &synth:events; }
voicing g { pitches = [G2, B2, D3, G3, B3, G4]; }
voicing d { pitches = [D3, A3, D4, F#4]; }
voicing a { pitches = [A2, E3, A3, C#4, E4]; }
voicings strings { chords = [("G", &g), ("D", &d), ("A", &a)]; }
harmony turn { length = 12q; chords = [(0q, "G"), (3q, "D"), (7q, "A")]; }
pattern legend { length = 16q;
  note k00 { at = 0q; dur = 1/2q; pitch = G2; }
  note k01 { at = 1q; dur = 1/2q; pitch = B2; }
  note k02 { at = 2q; dur = 1/2q; pitch = D3; }
  note k03 { at = 3q; dur = 1/2q; pitch = G3; }
  note k04 { at = 4q; dur = 1/2q; pitch = B3; }
  note k05 { at = 5q; dur = 1/2q; pitch = G4; }
  note k06 { at = 6q; dur = 1/2q; pitch = A3; }
  note k07 { at = 7q; dur = 1/2q; pitch = D4; }
  note k08 { at = 8q; dur = 1/2q; pitch = F#4; }
  note k09 { at = 9q; dur = 1/2q; pitch = A2; }
  note k10 { at = 10q; dur = 1/2q; pitch = E3; }
  note k11 { at = 11q; dur = 1/2q; pitch = C#4; }
  note k12 { at = 12q; dur = 1/2q; pitch = E4; }
  note k13 { at = 13q; dur = 1/2q; pitch = C4; }
}
place key { pattern = &legend; track = &t; at = 112q; }
"#;

/// Compile `HEAD` with `body` and read the same source as editing does.
/// Editing must enumerate exactly the plan's event addresses, and resolve
/// each chord member to the pitch the plan gives it. Returns how many chord
/// members the plan holds.
fn agree(body: &str) -> usize {
    let source = format!("{HEAD}{body}");
    let plan =
        crate::compiler::compile_bundle_artifact(&SourceBundle::new("main.maac", source.as_str()))
            .unwrap_or_else(|diagnostics| panic!("compiles: {diagnostics}"));
    let wire: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    let compiled: Vec<(String, String)> = wire["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| {
            (
                event["address"].as_str().unwrap().to_owned(),
                event["kind"]["pitch_hz"].to_string(),
            )
        })
        .collect();

    let document = AuthoredDocument::from_document(&crate::parse(&source).unwrap()).unwrap();
    let tree = document.tree();
    let mut enumerated: Vec<String> = enumerate_event_occurrences(tree)
        .expect("editing enumerates the document")
        .into_iter()
        .map(|event| event.address)
        .collect();
    enumerated.sort();
    let mut addresses: Vec<String> = compiled
        .iter()
        .map(|(address, _)| address.clone())
        .collect();
    addresses.sort();
    assert_eq!(enumerated, addresses, "event addresses");

    // What the plan plays for each pitch a note writes, and what editing
    // says each chord member plays.
    let mut written: BTreeMap<String, String> = BTreeMap::new();
    let mut members = Vec::new();
    for (address, hertz) in &compiled {
        let (place_id, rest) = address.split_once('/').unwrap();
        let place = &tree["objects"][place_id];
        match resolve_occurrence_pitches(tree, place, rest) {
            Some((_, Some(pitches))) => {
                let (_, member) = address.rsplit_once('.').unwrap();
                let pitch = pitches[member.parse::<usize>().unwrap()];
                members.push((address, pitch.to_string(), hertz));
            }
            Some((mappings, None)) => {
                let (path, _) = mappings.last().unwrap();
                let leaf = object_at(tree, path).unwrap();
                if leaf["kind"] == "note" {
                    let pitch = leaf["fields"]["pitch"].to_string();
                    let earlier = written.insert(pitch.clone(), hertz.clone());
                    assert!(
                        earlier.as_ref().is_none_or(|earlier| earlier == hertz),
                        "{address}: {pitch} plays {hertz} and {earlier:?}"
                    );
                }
            }
            None => assert!(names_insert(place, rest), "{address} does not resolve"),
        }
    }
    for (address, pitch, hertz) in &members {
        assert_eq!(
            written.get(pitch),
            Some(*hertz),
            "{address}: editing reads {pitch}, the plan plays {hertz}"
        );
    }
    members.len()
}

#[test]
fn chords_with_their_own_pitches_or_a_named_voicing() {
    let members = agree(
        r#"pattern bar { length = 8q;
  chord own { at = 0q; dur = 1q; pitches = [C4, E4, G4]; }
  chord all { at = after(own); dur = 1q; voicing = &g; }
  note between { at = after(all, 1/2q); dur = 1/2q; pitch = A3; }
  chord high { at = 3q; dur = 1q; voicing = &g; top = 2; }
  chord low { at = 4q; dur = 1q; voicing = &a; bottom = 3; }
  chord one { at = 5q; dur = 1q; voicing = &d; pick = 2; }
  chord few { at = 6q; dur = 1q; voicing = &g; pick = [1, 3, 6]; strum = up; }
  chord part { at = 7q; dur = 1q; pitches = [C4, E4, G4, B3]; pick = [2, 4]; }
}
place main { pattern = &bar; track = &t; at = 0q; count = 2; }
"#,
    );
    assert_eq!(members, 2 * (3 + 6 + 2 + 3 + 1 + 3 + 2));
}

#[test]
fn chords_take_the_voicing_their_nearest_use_or_placement_supplies() {
    let members = agree(
        r#"pattern cell { length = 4q;
  chord whole { at = 0q; dur = 1q; }
  chord first { at = after(whole); dur = 1q; pick = 1; }
  chord upper { at = 2q; dur = 1q; top = 2; }
  chord named { at = 3q; dur = 1q; voicing = &a; bottom = 2; }
}
pattern song { length = 16q;
  use plain { pattern = &cell; at = 0q; }
  use other { pattern = &cell; at = 4q; voicing = &d; count = 2; }
  use slow { pattern = &cell; at = 12q; stretch = 1/2; count = 2; }
}
place main { pattern = &song; track = &t; at = 0q; voicing = &g; }
place bare { pattern = &cell; track = &t; at = 16q; voicing = &a; count = 2; }
"#,
    );
    // Under g (6), d (4) and a (5): whole, first, upper and named.
    let (g, d, a) = (6 + 1 + 2 + 2, 4 + 1 + 2 + 2, 5 + 1 + 2 + 2);
    assert_eq!(members, g + 2 * d + 2 * g + 2 * a);
}

#[test]
fn chords_follow_a_harmony_through_repeats_uses_and_stretch() {
    let members = agree(
        r#"pattern cell { length = 2q;
  chord whole { at = 0q; dur = 1q; }
  chord pair { at = after(whole); dur = 1q; pick = [1, 2]; }
}
pattern song { length = 8q;
  use twice { pattern = &cell; at = 0q; count = 2; }
  use slow { pattern = &cell; at = 4q; stretch = 2; }
  chord tail { at = 7q; dur = 1q; top = 1; }
}
pattern outer { length = 16q;
  use songs { pattern = &song; at = 0q; count = 2; }
  use own { pattern = &cell; at = 14q; voicing = &d; }
}
place main { pattern = &song; track = &t; at = 0q; count = 3; harmony = &turn; voicings = &strings; }
place late { pattern = &cell; track = &t; at = 33q; count = 7; harmony = &turn; voicings = &strings; }
place deep { pattern = &outer; track = &t; at = 48q; count = 2; harmony = &turn; voicings = &strings; }
place stretched { pattern = &song; track = &t; at = 80q; stretch = 3/2; count = 2; harmony = &turn; voicings = &strings; }
"#,
    );
    assert!(members > 100, "{members}");
}

#[test]
fn a_use_with_its_own_harmony_starts_it_again() {
    let members = agree(
        r#"harmony back { length = 4q; chords = [(0q, "A"), (2q, "G")]; }
pattern cell { length = 2q;
  chord whole { at = 0q; dur = 1q; }
  chord last { at = 1q; dur = 1q; top = 1; }
}
pattern song { length = 12q;
  use outer { pattern = &cell; at = 0q; count = 3; }
  use inner { pattern = &cell; at = 6q; count = 3; harmony = &back; voicings = &strings; }
}
place main { pattern = &song; track = &t; at = 0q; count = 2; harmony = &turn; voicings = &strings; }
"#,
    );
    assert!(members > 40, "{members}");
}

#[test]
fn inserts_and_notes_resolve_beside_chords() {
    let members = agree(
        r#"pattern bar { length = 4q;
  note lead { at = 0q; dur = 1q; pitch = E4; }
  chord under { at = 1q; dur = 1q; voicing = &d; strum = down; }
}
place main { pattern = &bar; track = &t; at = 0q; count = 2;
  insert extra { note pickup { at = 3q; dur = 1/2q; pitch = C4; } }
}
"#,
    );
    assert_eq!(members, 2 * 4);
}

/// Each chord here sits on one side of a change of the harmony only if its
/// position is read exactly: `turn` changes at 3q and at 7q.
#[test]
fn a_stretch_and_a_rest_move_a_chord_across_a_change_of_the_harmony() {
    let members = agree(
        r#"pattern cell { length = 2q;
  chord whole { at = 0q; dur = 1q; }
  chord pair { at = after(whole); dur = 1q; pick = [1, 2]; }
}
pattern wide { length = 8q;
  use slow { pattern = &cell; at = 1q; stretch = 2; }
}
pattern again { length = 8q;
  use slow { pattern = &cell; at = 0q; stretch = 2; count = 2; }
}
pattern pairs { length = 4q;
  use late { pattern = &cell; at = 1q; }
}
pattern nest { length = 8q;
  use slower { pattern = &pairs; at = 0q; stretch = 3/2; }
}
pattern rests { length = 8q;
  chord before { at = 2q; dur = 1/2q; }
  chord after { at = after(before, 1q); dur = 1q; }
  chord later { at = after(after, 2q); dur = 1/2q; top = 2; }
}
place stretched { pattern = &wide; track = &t; at = 0q; harmony = &turn; voicings = &strings; }
place repeated { pattern = &again; track = &t; at = 24q; harmony = &turn; voicings = &strings; }
place nested { pattern = &nest; track = &t; at = 8q; harmony = &turn; voicings = &strings; }
place rested { pattern = &rests; track = &t; at = 16q; harmony = &turn; voicings = &strings; }
"#,
    );
    // `wide`: whole at 1q (G, 6) and pair at 3q (D). `again`: whole at 0q
    // (G, 6) and at 4q (D, 4), each with its pair. `nest`: whole at 3/2q
    // (G, 6) and pair at 3q (D). `rests`: before at 2q (G, 6), after at 7/2q
    // (D, 4), later at 13/2q (D).
    assert_eq!(members, (6 + 2) + (6 + 2 + 4 + 2) + (6 + 2) + (6 + 4 + 2));
}
