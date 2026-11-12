//! A placement's destination plays what the placement sends it (§6.2, §8.3,
//! §10): a synth or an instrument plays notes, with the pressure, the timbre
//! and the connected notes its voice provides for, and a kit plays hits on
//! its keys. A compile and an edit in either context refuse the same
//! documents, at the same place and in the same words.

use maac::bundle::sha256_digest;
use maac::compiler::compile_bundle_artifact;
use maac::editing::{
    apply_transaction, AuthoredDocument, BundleEditContext, FoundationEditContext, Operation,
    Transaction,
};
use maac::{parse, Diagnostic, DiagnosticCode, SourceBundle};
use serde_json::{json, Value};

fn kick() -> Vec<u8> {
    [1f32, 0.5].iter().flat_map(|v| v.to_le_bytes()).collect()
}

const HEAD: &str = r#"maac 1;
project p { score = [0q, 16q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sine:out; tail = 100ms; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
curve rise { clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }
node sine { type = "core.sine/1"; params = { attack = 5ms; release = 50ms; level = 0.05; }; }
track to_sine { target = &sine:events; }
"#;

/// A source whose only destinations are native: `sine` on track `to_sine`
/// and the kit `kit`, with the key `kick`, on track `to_kit`. The
/// foundation context edits it too.
fn native(body: &str) -> String {
    format!(
        r#"{HEAD}asset sample {{ kind = audio; path = "kick.pcm"; hash = "{}"; format = "pcm_f32le_interleaved/1"; rate = 24000Hz; channels = 1; frames = 2; }}
node kit {{ type = "core.kit/1"; config = {{ channels = 1; samples = [{{ key = "kick"; asset = &sample; }}]; }}; }}
track to_kit {{ target = &kit:events; }}
{body}
"#,
        sha256_digest(&kick()),
    )
}

/// An instrument `name` whose voice has `fields` and `nodes`.
fn voice(name: &str, fields: &str, nodes: &str) -> String {
    format!(
        r#"instrument {name}_voice {{
  channels = 1;
  voice voice {{
    channels = 1; amplitude = &amp; output = &tone:out; {fields}
    node amp {{ type = "synth.adsr/1"; params = {{ attack = 10ms; sustain = 1; release = 5ms; }}; }}
    node tone {{ type = "synth.saw/1"; params = {{ level = 1/16; }}; }}
{nodes}  }}
}}
node {name} {{ instrument = &{name}_voice; config = {{ voices = 2; }}; }}
track to_{name} {{ target = &{name}:events; }}
"#
    )
}

/// A source with `sine`, and the instruments `plain`, `touch` (pressure),
/// `colour` (timbre) and `reed` (connected notes), and the kit instrument
/// `drums` with the key `kick`, each on the track `to_<node>`.
fn instruments(body: &str) -> String {
    let sensing = |name: &str, processor: &str| {
        voice(
            name,
            "",
            &format!(
                "    node sense {{ type = \"{processor}\"; }}\n    modulate m {{ from = &sense:out; to = &tone.params.level; depth = 1/16; }}\n"
            ),
        )
    };
    format!(
        r#"{HEAD}{}{}{}{}import basic {{ builtin = "std/basic/1.0.0"; }}
instrument boom_bap {{ channels = 2; piece kick {{ instrument = &basic.kick; key = "kick"; gate = 100ms; }} }}
node drums {{ instrument = &boom_bap; }}
track to_drums {{ target = &drums:events; }}
{body}
"#,
        voice("plain", "", ""),
        sensing("touch", "synth.pressure/1"),
        sensing("colour", "synth.timbre/1"),
        voice("reed", "connected = true;", ""),
    )
}

fn bundle(text: &str) -> SourceBundle {
    let mut bundle = SourceBundle::new("main.maac", text);
    if text.contains("kick.pcm") {
        bundle.assets.insert("kick.pcm".into(), kick());
    }
    bundle
}

/// What a diagnostic or an edit refusal says: its code, its message, its
/// object path and its field path.
type Said = (String, String, Vec<String>, Vec<String>);

fn compile(text: &str) -> Vec<Said> {
    match compile_bundle_artifact(&bundle(text)) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics
            .into_vec()
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.code_str().to_owned(),
                    diagnostic.message.clone(),
                    diagnostic.object_path.clone(),
                    diagnostic.field_path.clone(),
                )
            })
            .collect(),
    }
}

const PRESSURE: &str = "pressure expression requires an instrument with synth.pressure/1";
const TIMBRE: &str = "timbre expression requires an instrument with synth.timbre/1";
const CONNECTED: &str = "`slur` and `slide` need a destination that plays connected notes";
const NO_NOTES: &str = "target does not accept this event transport";
const NO_HITS: &str = "hit requires a kit receiver";
const NO_KEY: &str = "kit has no such key";

/// What is said of `message` at `object` and `field` when placement `place`
/// plays it on `node`.
fn refusal(message: &str, object: &[&str], field: &[&str], place: &str, node: &str) -> Said {
    let code = if message == NO_KEY {
        DiagnosticCode::Reference
    } else {
        DiagnosticCode::Capability
    };
    let owned = |path: &[&str]| path.iter().map(|part| (*part).to_owned()).collect();
    (
        Diagnostic::error(code, "", None).code_str().to_owned(),
        format!("{message} (placement `{place}` plays it on `{node}`)"),
        owned(object),
        owned(field),
    )
}

/// Pattern `bar` holding `children`, placed as `main` on the track of
/// `node` with `fields`.
fn placed(children: &str, node: &str, fields: &str) -> String {
    format!(
        "pattern bar {{ length = 4q; {children} }}\nplace main {{ pattern = &bar; track = &to_{node}; at = 0q; {fields} }}"
    )
}

const NOTE: &str = "note a { at = 0q; dur = 1q; pitch = C4; }";
const PRESSED: &str =
    "note a { at = 0q; dur = 1q; pitch = C4; expression e { kind = pressure; curve = &rise; } }";
const COLOURED: &str =
    "note a { at = 0q; dur = 1q; pitch = C4; expression e { kind = timbre; curve = &rise; } }";
const PRESSED_CHORD: &str = "chord a { at = 0q; dur = 1q; pitches = [C4, E4]; expression e { kind = pressure; curve = &rise; } }";
const SLURRED: &str =
    "note z { at = 0q; dur = 1q; pitch = C4; } note a { at = 1q; dur = 1q; pitch = D4; slur = true; }";
const SLID: &str =
    "note z { at = 0q; dur = 1q; pitch = C4; } note a { at = 1q; dur = 1q; pitch = D4; slide = 100ms; }";
const HIT: &str = r#"hit a { at = 0q; key = "kick"; }"#;
const COWBELL: &str = r#"hit a { at = 0q; key = "cowbell"; }"#;

#[test]
fn a_destination_plays_what_its_voice_provides_for() {
    // (what the pattern holds, the message when a node does not play it,
    // the field at fault, the nodes that play it)
    let sends: [(&str, &str, &[&str], &[&str]); 6] = [
        (NOTE, "", &[], &["sine", "plain", "touch", "colour", "reed"]),
        (PRESSED, PRESSURE, &[], &["touch"]),
        (PRESSED_CHORD, PRESSURE, &[], &["touch"]),
        (COLOURED, TIMBRE, &[], &["colour"]),
        (SLURRED, CONNECTED, &["slur"], &["reed"]),
        (SLID, CONNECTED, &["slide"], &["reed"]),
    ];
    for (children, message, field, plays) in sends {
        for node in ["sine", "plain", "touch", "colour", "reed"] {
            let expected = if plays.contains(&node) {
                Vec::new()
            } else {
                vec![refusal(message, &["bar", "a"], field, "main", node)]
            };
            assert_eq!(
                compile(&instruments(&placed(children, node, ""))),
                expected,
                "{children} on {node}"
            );
        }
    }
}

#[test]
fn a_kit_plays_hits_on_its_keys_and_other_destinations_play_none() {
    for (text, node) in [
        (native(&placed(HIT, "kit", "")), "kit"),
        (instruments(&placed(HIT, "drums", "")), "drums"),
    ] {
        assert_eq!(compile(&text), [], "{node}");
        assert_eq!(
            compile(&text.replace("\"kick\"; }", "\"cowbell\"; }")),
            [refusal(NO_KEY, &["bar", "a"], &["key"], "main", node)],
            "{node}"
        );
        let notes = text.replace(HIT, NOTE);
        assert_eq!(
            compile(&notes),
            [refusal(NO_NOTES, &["bar", "a"], &[], "main", node)],
            "{node}"
        );
    }
    for node in ["sine", "plain", "reed"] {
        assert_eq!(
            compile(&instruments(&placed(HIT, node, ""))),
            [refusal(NO_HITS, &["bar", "a"], &[], "main", node)],
            "{node}"
        );
    }
}

/// A leaf is wrong where a placement sends it, not where it is written.
#[test]
fn a_leaf_is_checked_for_each_placement_that_reaches_it() {
    let pattern = format!("pattern bar {{ length = 4q; {PRESSED} }}\n");
    let on = |name: &str, node: &str, at: &str| {
        format!("place {name} {{ pattern = &bar; track = &to_{node}; at = {at}; }}\n")
    };
    // A pattern that nothing places is on no destination.
    assert_eq!(compile(&instruments(&pattern)), []);
    assert_eq!(
        compile(&instruments(&format!(
            "{pattern}{}",
            on("main", "touch", "0q")
        ))),
        []
    );
    assert_eq!(
        compile(&instruments(&format!(
            "{pattern}{}{}{}",
            on("main", "touch", "0q"),
            on("second", "sine", "4q"),
            on("third", "plain", "8q")
        ))),
        [
            refusal(PRESSURE, &["bar", "a"], &[], "second", "sine"),
            refusal(PRESSURE, &["bar", "a"], &[], "third", "plain"),
        ]
    );
    // Through uses, and in an insert.
    let nested = format!(
        "pattern leaf {{ length = 4q; {PRESSED} }}\npattern mid {{ length = 4q; use u {{ pattern = &leaf; at = 0q; }} }}\npattern bar {{ length = 8q; use v {{ pattern = &mid; at = 4q; }} }}\n{}",
        on("main", "sine", "0q")
    );
    assert_eq!(
        compile(&instruments(&nested)),
        [refusal(PRESSURE, &["leaf", "a"], &[], "main", "sine")]
    );
    let inserted = format!(
        "pattern bar {{ length = 4q; }}\nplace main {{ pattern = &bar; track = &to_sine; at = 0q; insert i {{ {HIT} }} }}"
    );
    assert_eq!(
        compile(&instruments(&inserted)),
        [refusal(NO_HITS, &["main", "i", "a"], &[], "main", "sine")]
    );
}

/// Which occurrence an override names shows only in an expansion, so a leaf
/// that one may delete, or a hit that one may give another key, is left to
/// the compiler. The key an override sets is checked where it is written.
#[test]
fn an_override_that_deletes_or_rekeys_an_occurrence_is_the_compilers_to_judge() {
    let gone = r#"override o { event = "0/a"; delete = true; }"#;
    assert_eq!(compile(&native(&placed(PRESSED, "sine", gone))), []);
    assert_eq!(compile(&native(&placed(COWBELL, "kit", gone))), []);
    let kick = r#"override o { event = "0/a"; set = { key = "kick"; }; }"#;
    assert_eq!(compile(&native(&placed(COWBELL, "kit", kick))), []);
    let cowbell = r#"override o { event = "0/a"; set = { key = "cowbell"; }; }"#;
    assert_eq!(
        compile(&native(&placed(HIT, "kit", cowbell))),
        [refusal(
            NO_KEY,
            &["main", "o"],
            &["set", "key"],
            "main",
            "kit"
        )]
    );
    // One repetition deleted, one left: the compiler refuses the one left.
    let one_gone = compile(&native(&placed(
        PRESSED,
        "sine",
        &format!("count = 2; {gone}"),
    )));
    assert_eq!(one_gone.len(), 1, "{one_gone:?}");
    assert_eq!(one_gone[0].1, PRESSURE);
}

/// What refuses `operation` on `text`, which compiles, under the foundation
/// context when `foundation`, and under the bundle context.
fn edit(text: &str, operation: Operation, foundation: bool) -> Vec<Option<Said>> {
    assert_eq!(compile(text), [], "the document compiles before the edit");
    let contexts: &[bool] = if foundation { &[false, true] } else { &[true] };
    contexts
        .iter()
        .map(|bundled| {
            let mut document = AuthoredDocument::from_document(&parse(text).unwrap()).unwrap();
            let transaction =
                Transaction::new(document.revision().to_owned(), vec![operation.clone()]).unwrap();
            let error = if *bundled {
                let context = BundleEditContext::new(&bundle(text)).unwrap();
                apply_transaction(&mut document, &transaction, &context).err()
            } else {
                apply_transaction(&mut document, &transaction, &FoundationEditContext).err()
            };
            error.map(|error| {
                (
                    error.code,
                    error.message,
                    error.object_path,
                    error.field_path,
                )
            })
        })
        .collect()
}

fn set(object: &[&str], field: &[&str], value: Value) -> Operation {
    Operation::Set {
        object: object.iter().map(|id| (*id).to_owned()).collect(),
        field: field.iter().map(|name| (*name).to_owned()).collect(),
        value,
        expect: None,
        expect_absent: false,
    }
}

fn track(node: &str) -> Value {
    json!({"t":"ref","path":[format!("to_{node}")],"port":null})
}

fn text(value: &str) -> Value {
    json!({"t":"string","v":value})
}

#[test]
fn an_edit_that_sends_a_native_destination_what_it_does_not_play_is_refused_in_both_contexts() {
    let both = |said: Said| vec![Some(said.clone()), Some(said)];
    // Notes moved to the kit, and hits to the synth.
    let notes = native(&placed(NOTE, "sine", ""));
    assert_eq!(
        edit(&notes, set(&["main"], &["track"], track("kit")), true),
        both(refusal(NO_NOTES, &["bar", "a"], &[], "main", "kit"))
    );
    let hits = native(&placed(HIT, "kit", ""));
    assert_eq!(
        edit(&hits, set(&["main"], &["track"], track("sine")), true),
        both(refusal(NO_HITS, &["bar", "a"], &[], "main", "sine"))
    );
    // A key the kit lacks, on the hit and in an override.
    assert_eq!(
        edit(&hits, set(&["bar", "a"], &["key"], text("cowbell")), true),
        both(refusal(NO_KEY, &["bar", "a"], &["key"], "main", "kit"))
    );
    let softer = native(&placed(
        HIT,
        "kit",
        r#"override o { event = "0/a"; set = { velocity = 1/2; }; }"#,
    ));
    assert_eq!(
        edit(
            &softer,
            set(&["main", "o"], &["set", "key"], text("cowbell")),
            true
        ),
        both(refusal(
            NO_KEY,
            &["main", "o"],
            &["set", "key"],
            "main",
            "kit"
        ))
    );
    // A slur on a synth that plays no connected notes.
    let two = native(&placed(&SLURRED.replace(" slur = true;", ""), "sine", ""));
    assert_eq!(
        edit(
            &two,
            set(&["bar", "a"], &["slur"], json!({"t":"boolean","v":true})),
            true
        ),
        both(refusal(CONNECTED, &["bar", "a"], &["slur"], "main", "sine"))
    );
    // An edit that keeps to what the destination plays applies.
    assert_eq!(
        edit(&hits, set(&["bar", "a"], &["key"], text("kick")), true),
        [None, None]
    );
}

#[test]
fn an_edit_that_sends_an_instrument_what_its_voice_does_not_provide_for_is_refused() {
    // Each edit leaves what a compile refuses in the same words.
    for (children, from, to, message, field) in [
        (PRESSED, "touch", "plain", PRESSURE, &[][..]),
        (PRESSED, "touch", "sine", PRESSURE, &[]),
        (COLOURED, "colour", "touch", TIMBRE, &[]),
        (SLURRED, "reed", "plain", CONNECTED, &["slur"]),
        (HIT, "drums", "reed", NO_HITS, &[]),
        (NOTE, "plain", "drums", NO_NOTES, &[]),
    ] {
        let before = instruments(&placed(children, from, ""));
        let refused = refusal(message, &["bar", "a"], field, "main", to);
        assert_eq!(
            edit(&before, set(&["main"], &["track"], track(to)), false),
            [Some(refused.clone())],
            "{children} from {from} to {to}"
        );
        assert_eq!(
            compile(&instruments(&placed(children, to, ""))),
            [refused],
            "{children} on {to}"
        );
    }
    // A move to a destination that plays it applies.
    let before = instruments(&placed(NOTE, "plain", ""));
    assert_eq!(
        edit(&before, set(&["main"], &["track"], track("reed")), false),
        [None]
    );
}
