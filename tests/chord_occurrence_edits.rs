//! An edit is refused when it leaves a chord that does not play where a use
//! or a placement puts it (§8.2), with the code and the words the compiler
//! gives the same document.

use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, BundleEditContext, EditError, FoundationEditContext,
    Operation, Transaction,
};
use maac::{parse, SourceBundle};
use serde_json::{json, Value};

/// Voicings `g` (six pitches) and `d` (four), a harmony `turn` of G then D
/// that voicings `strings` voice, pattern `bar` holding `chord`, and
/// placement `main` with `place`.
fn source(chord: &str, place: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 32q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.05; }}; }}
voicing g {{ pitches = [G2, B2, D3, G3, B3, G4]; }}
voicing d {{ pitches = [D3, A3, D4, F#4]; }}
harmony turn {{ length = 8q; chords = [(0q, "G"), (4q, "D")]; }}
voicings strings {{ chords = [("G", &g), ("D", &d)]; }}
pattern bar {{ length = 4q;
  chord c {{ at = 0q; dur = 1q; {chord} }}
}}
pattern song {{ length = 8q;
  use first {{ pattern = &bar; at = 0q; }}
  use second {{ pattern = &bar; at = 4q; }}
}}
track t {{ target = &synth:events; }}
place main {{ pattern = &song; track = &t; at = 0q; {place} }}
"#
    )
}

fn number(numerator: i64, denominator: i64) -> Value {
    json!({"t":"number","n":numerator.to_string(),"d":denominator.to_string()})
}

fn reference(id: &str) -> Value {
    json!({"t":"ref","path":[id],"port":null})
}

fn set(object: &[&str], field: &str, value: Value) -> Operation {
    Operation::Set {
        object: object.iter().map(|id| (*id).to_owned()).collect(),
        field: vec![field.into()],
        value,
        expect: None,
        expect_absent: false,
    }
}

fn unset(object: &[&str], field: &str) -> Operation {
    Operation::Unset {
        object: object.iter().map(|id| (*id).to_owned()).collect(),
        field: vec![field.into()],
        expect: None,
    }
}

/// Apply `operations` to `text` under the foundation context and under
/// the bundle context. Returns the error of each that refuses.
fn edit(text: &str, operations: &[Operation]) -> [Option<EditError>; 2] {
    assert!(
        compile_bundle_artifact(&SourceBundle::new("main.maac", text)).is_ok(),
        "the document compiles before the edit"
    );
    [false, true].map(|bundled| {
        let mut document = AuthoredDocument::from_document(&parse(text).unwrap()).unwrap();
        let transaction =
            Transaction::new(document.revision().to_owned(), operations.to_vec()).unwrap();
        if bundled {
            let context = BundleEditContext::new(&SourceBundle::new("main.maac", text)).unwrap();
            apply_transaction(&mut document, &transaction, &context).err()
        } else {
            apply_transaction(&mut document, &transaction, &FoundationEditContext).err()
        }
    })
}

/// The edit of `before` by `operations` is refused at chord `c`'s `field`,
/// with the code and message the compiler gives `after`, the document the
/// edit would leave.
fn refused_like_the_compiler(
    what: &str,
    before: &str,
    operations: &[Operation],
    after: &str,
    field: Option<&str>,
) {
    let compiled = compile_bundle_artifact(&SourceBundle::new("main.maac", after))
        .expect_err("the document the edit would leave does not compile")
        .into_vec();
    let [diagnostic] = compiled.as_slice() else {
        panic!("{what}: expected one diagnostic, got {compiled:#?}");
    };
    for (context, result) in ["foundation", "bundle"]
        .into_iter()
        .zip(edit(before, operations))
    {
        let error =
            result.unwrap_or_else(|| panic!("{what}: the {context} context applies the edit"));
        assert_eq!(error.code, diagnostic.code_str(), "{what}, {context}");
        assert_eq!(error.message, diagnostic.message, "{what}, {context}");
        assert_eq!(error.object_path, ["bar", "c"], "{what}, {context}");
        let field: Vec<&str> = field.into_iter().collect();
        assert_eq!(error.field_path, field, "{what}, {context}");
    }
}

#[test]
fn a_chord_left_without_a_voicing_is_refused() {
    refused_like_the_compiler(
        "no voicing in scope",
        &source("", "voicing = &g;"),
        &[unset(&["main"], "voicing")],
        &source("", ""),
        None,
    );
}

#[test]
fn a_harmony_chord_that_the_voicings_do_not_map_is_refused() {
    let chords = |second: &str| {
        json!({"t":"list","items":[
            {"t":"tuple","items":[{"t":"quantity","n":"0","d":"1","u":"q"},{"t":"string","v":"G"}]},
            {"t":"tuple","items":[{"t":"quantity","n":"4","d":"1","u":"q"},{"t":"string","v":second}]},
        ]})
    };
    let place = "harmony = &turn; voicings = &strings;";
    refused_like_the_compiler(
        "unmapped name",
        &source("", place),
        &[set(&["turn"], "chords", chords("E"))],
        &source("", place).replace("(4q, \"D\")", "(4q, \"E\")"),
        None,
    );
    // A name that no chord reaches is fine: the song ends at 8q, and "E"
    // sounds from 12q.
    let late = source("", place).replace(
        "length = 8q; chords = [(0q, \"G\"), (4q, \"D\")]",
        "length = 16q; chords = [(0q, \"G\"), (12q, \"E\")]",
    );
    let longer = json!({"t":"quantity","n":"20","d":"1","u":"q"});
    for result in edit(&late, &[set(&["turn"], "length", longer)]) {
        assert_eq!(result.map(|error| error.message), None);
    }
}

#[test]
fn a_chord_that_does_not_fit_the_voicing_it_is_given_is_refused() {
    // `top` past the four pitches of `d`.
    refused_like_the_compiler(
        "top past a supplied voicing",
        &source("top = 5;", "voicing = &g;"),
        &[set(&["main"], "voicing", reference("d"))],
        &source("top = 5;", "voicing = &d;"),
        Some("top"),
    );
    // `pick` past the second chord of the harmony, which `d` voices.
    refused_like_the_compiler(
        "pick past a harmony's voicing",
        &source("pick = 4;", "harmony = &turn; voicings = &strings;"),
        &[set(&["bar", "c"], "pick", number(6, 1))],
        &source("pick = 6;", "harmony = &turn; voicings = &strings;"),
        Some("pick"),
    );
    // The same, where only the placement's second repetition reaches `d`.
    let twice = |chord: &str| {
        source(chord, "count = 2; harmony = &turn; voicings = &strings;").replace(
            "place main { pattern = &song;",
            "place main { pattern = &bar;",
        )
    };
    refused_like_the_compiler(
        "pick past the voicing of a later repetition",
        &twice("pick = 4;"),
        &[set(&["bar", "c"], "pick", number(6, 1))],
        &twice("pick = 6;"),
        Some("pick"),
    );
    // A list with an entry too many for the pitches played.
    refused_like_the_compiler(
        "a list against a supplied voicing",
        &source("top = 3; velocity = [1/2, 1/2, 1/2];", "voicing = &g;"),
        &[set(&["bar", "c"], "top", number(2, 1))],
        &source("top = 2; velocity = [1/2, 1/2, 1/2];", "voicing = &g;"),
        Some("velocity"),
    );
    // A tilt that takes the third of six strings past 1.
    refused_like_the_compiler(
        "tilt under a supplied voicing",
        &source(
            "strum = down; velocity = 9/10; tilt = 1/10;",
            "voicing = &g;",
        ),
        &[set(&["bar", "c"], "tilt", number(1, 2))],
        &source(
            "strum = down; velocity = 9/10; tilt = 1/2;",
            "voicing = &g;",
        ),
        Some("tilt"),
    );
}

/// The number of members is known without an occurrence here, and both
/// contexts refuse the tilt as semantic analysis reports it.
#[test]
fn a_tilt_out_of_range_on_a_chord_with_its_own_pitches_is_refused() {
    refused_like_the_compiler(
        "tilt with own pitches",
        &source(
            "pitches = [C4, E4, G4]; strum = down; velocity = 9/10; tilt = 1/10;",
            "",
        ),
        &[set(&["bar", "c"], "tilt", number(1, 2))],
        &source(
            "pitches = [C4, E4, G4]; strum = down; velocity = 9/10; tilt = 1/2;",
            "",
        ),
        Some("tilt"),
    );
}

/// What one operation breaks, another of the same transaction may mend.
#[test]
fn an_edit_that_changes_the_voicing_and_the_chord_together_applies() {
    for result in edit(
        &source("top = 5;", "voicing = &g;"),
        &[
            set(&["main"], "voicing", reference("d")),
            set(&["bar", "c"], "top", number(4, 1)),
        ],
    ) {
        assert_eq!(result.map(|error| error.message), None);
    }
    // The inner use supplies its own voicing, so the outer one may go.
    let nested = source("", "voicing = &g;")
        .replace(
            "use first { pattern = &bar; at = 0q; }",
            "use first { pattern = &bar; at = 0q; voicing = &d; }",
        )
        .replace(
            "use second { pattern = &bar; at = 4q; }",
            "use second { pattern = &bar; at = 4q; voicing = &d; }",
        );
    for result in edit(&nested, &[unset(&["main"], "voicing")]) {
        assert_eq!(result.map(|error| error.message), None);
    }
}

/// A harmony comes round again, so the check stops at the repetition that
/// sits where the first did. A placement repeated past what the compiler
/// expands is the compiler's to refuse.
#[test]
fn a_placement_repeated_a_million_times_does_not_stall_an_edit() {
    let text = source("", "count = 1000000; harmony = &turn; voicings = &strings;");
    let mut document = AuthoredDocument::from_document(&parse(&text).unwrap()).unwrap();
    let label = json!({"t":"string","v":"long"});
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![set(&["main"], "label", label)],
    )
    .unwrap();
    apply_transaction(&mut document, &transaction, &FoundationEditContext).unwrap();
}
