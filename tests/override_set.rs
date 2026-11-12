//! What an override may set on the event it names (§11), an event's `order`
//! (§6), and the track of a placement that expands events (§10). A compile
//! and an edit in either context refuse the same documents, at the same
//! place and in the same words.

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

fn tone() -> Vec<u8> {
    (0..480)
        .flat_map(|j| ((j as f32 + 1.) / 480.).to_le_bytes())
        .collect()
}

/// A 16q score with a synth on track `t`, a kit on track `k`, track `bare`
/// without a target, and a sum `mix` that the project plays.
fn source(body: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 16q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &mix:out; tail = 100ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node synth {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 50ms; level = 0.05; }}; }}
asset sample {{ kind = audio; path = "kick.pcm"; hash = "{}"; format = "pcm_f32le_interleaved/1"; rate = 24000Hz; channels = 1; frames = 2; }}
node kit {{ type = "core.kit/1"; config = {{ channels = 1; samples = [{{ key = "kick"; asset = &sample; }}]; }}; }}
asset tone {{ kind = audio; path = "tone.pcm"; hash = "{}"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = 480; }}
node mix {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
connect synth_up {{ from = &synth:out; to = &mix:in; }}
connect kit_up {{ from = &kit:out; to = &mix:in; }}
track t {{ target = &synth:events; }}
track k {{ target = &kit:events; }}
track bare {{}}
{body}
"#,
        sha256_digest(&kick()),
        sha256_digest(&tone()),
    )
}

fn bundle(text: &str) -> SourceBundle {
    let mut bundle = SourceBundle::new("main.maac", text);
    bundle.assets.insert("kick.pcm".into(), kick());
    bundle.assets.insert("tone.pcm".into(), tone());
    bundle
}

/// What a diagnostic or an edit refusal says: its code, its message, its
/// object path and its field path.
type Said = (String, String, Vec<String>, Vec<String>);

fn said(diagnostic: &Diagnostic) -> Said {
    (
        diagnostic.code_str().to_owned(),
        diagnostic.message.clone(),
        diagnostic.object_path.clone(),
        diagnostic.field_path.clone(),
    )
}

fn compile(text: &str) -> Vec<Said> {
    match compile_bundle_artifact(&bundle(text)) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics.into_vec().iter().map(said).collect(),
    }
}

fn refusal(code: DiagnosticCode, message: &str, object: &[&str], field: &[&str]) -> Said {
    let owned = |path: &[&str]| path.iter().map(|part| (*part).to_owned()).collect();
    (
        Diagnostic::error(code, "", None).code_str().to_owned(),
        message.to_owned(),
        owned(object),
        owned(field),
    )
}

/// What refuses `operation` on `text`, which compiles, under the foundation
/// context and under the bundle context.
fn edit(text: &str, operation: Operation) -> [Option<Said>; 2] {
    assert_eq!(compile(text), [], "the document compiles before the edit");
    [false, true].map(|bundled| {
        let mut document = AuthoredDocument::from_document(&parse(text).unwrap()).unwrap();
        let transaction =
            Transaction::new(document.revision().to_owned(), vec![operation.clone()]).unwrap();
        let error = if bundled {
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

fn number(numerator: i64, denominator: i64) -> Value {
    json!({"t":"number","n":numerator.to_string(),"d":denominator.to_string()})
}

/// A pattern `bar` holding one leaf of `kind`, placed as `main` with
/// `children`, and the address of the event it expands.
fn placed(kind: &str, children: &str) -> (String, &'static str) {
    let (leaf, track, event, more) = match kind {
        "note" => ("note a { at = 0q; dur = 1q; pitch = C4; }", "t", "0/a", ""),
        "member" => (
            "chord a { at = 0q; dur = 1q; pitches = [C4, E4, G4]; }",
            "t",
            "0/a.2",
            "",
        ),
        "hit" => (r#"hit a { at = 0q; key = "kick"; }"#, "k", "0/a", ""),
        "message" => (
            r#"message a { at = 0q; protocol = "midi1"; bytes = [176, 1, 64]; }"#,
            "t",
            "0/a",
            "",
        ),
        "audio" => (
            "audio a { asset = &tone; at = 0q; source = [0frame, 480frame]; mode = rate; }",
            "bare",
            "0/a",
            "connect clip_up { from = &main:out; to = &mix:in; }",
        ),
        _ => unreachable!(),
    };
    let text = source(&format!(
        "pattern bar {{ length = 4q; {leaf} }}\nplace main {{ pattern = &bar; track = &{track}; at = 0q; {children} }}\n{more}"
    ));
    (text, event)
}

/// Each `set` field, written as a value that is right for it.
const FIELDS: [(&str, &str); 16] = [
    ("at", "1q"),
    ("dur", "1/2q"),
    ("pitch", "D4"),
    ("velocity", "1/2"),
    ("release_velocity", "1/2"),
    ("onset_offset", "1ms"),
    ("release_offset", "1ms"),
    ("order", "3"),
    ("label", r#""x""#),
    ("key", r#""kick""#),
    ("protocol", r#""midi1""#),
    ("bytes", "[176, 1, 0]"),
    ("gain", "1/2"),
    ("fade_in", "1ms"),
    ("fade_out", "1ms"),
    ("fade_shape", "equal_power"),
];

#[test]
fn an_override_sets_only_the_fields_of_the_event_it_names() {
    let note = "at dur pitch velocity release_velocity onset_offset release_offset order label";
    for (kind, event_kind, takes) in [
        ("note", "a note", note),
        ("member", "a note", note),
        ("hit", "a hit", "at key velocity onset_offset order label"),
        (
            "message",
            "a message",
            "at protocol bytes onset_offset order label",
        ),
        (
            "audio",
            "an audio leaf",
            "at gain fade_in fade_out fade_shape label",
        ),
    ] {
        for (field, value) in FIELDS {
            let event = placed(kind, "").1;
            let (text, _) = placed(
                kind,
                &format!(r#"override o {{ event = "{event}"; set = {{ {field} = {value}; }}; }}"#),
            );
            let expected = if takes.split(' ').any(|taken| taken == field) {
                Vec::new()
            } else {
                vec![refusal(
                    DiagnosticCode::UnknownField,
                    &format!("override.set cannot replace `{field}` of {event_kind}"),
                    &["main", "o"],
                    &["set", field],
                )]
            };
            assert_eq!(compile(&text), expected, "{kind} {field}");
        }
    }
}

/// The first field in name order is the one reported.
#[test]
fn an_edit_that_sets_a_field_the_event_does_not_take_is_refused_as_a_compile_refuses_it() {
    for (kind, field, value, event_kind) in [
        ("note", "key", json!({"t":"string","v":"kick"}), "a note"),
        ("member", "gain", number(1, 2), "a note"),
        ("hit", "release_velocity", number(1, 2), "a hit"),
        ("message", "velocity", number(1, 2), "a message"),
        ("audio", "order", number(3, 1), "an audio leaf"),
    ] {
        let event = placed(kind, "").1;
        let (text, _) = placed(
            kind,
            &format!(r#"override o {{ event = "{event}"; set = {{ at = 1q; }}; }}"#),
        );
        let refused = refusal(
            DiagnosticCode::UnknownField,
            &format!("override.set cannot replace `{field}` of {event_kind}"),
            &["main", "o"],
            &["set", field],
        );
        assert_eq!(
            edit(&text, set(&["main", "o"], &["set", field], value)),
            [Some(refused.clone()), Some(refused.clone())],
            "{kind} {field}"
        );
        // The document that the edit would leave does not compile, for the
        // same reason.
        let written = FIELDS.iter().find(|(name, _)| *name == field).unwrap().1;
        let left = text.replace("at = 1q; }", &format!("at = 1q; {field} = {written}; }}"));
        assert_eq!(compile(&left), [refused], "{kind} {field}");
    }
    // A field the event takes applies.
    let (text, _) = placed(
        "hit",
        r#"override o { event = "0/a"; set = { at = 1q; }; }"#,
    );
    assert_eq!(
        edit(
            &text,
            set(&["main", "o"], &["set", "velocity"], number(1, 2))
        ),
        [None, None]
    );
}

const PAST_32_BITS: &str = "event order exceeds signed 32-bit range";
const NOT_WHOLE: &str = "order must be an integer";

#[test]
fn an_order_is_an_integer_that_fits_32_bits() {
    let leaf = |kind: &str, order: &str| {
        let (text, _) = placed(kind, "");
        text.replace("{ at = 0q;", &format!("{{ order = {order}; at = 0q;"))
    };
    let set_order = |kind: &str, order: &str| {
        let event = placed(kind, "").1;
        placed(
            kind,
            &format!(r#"override o {{ event = "{event}"; set = {{ order = {order}; }}; }}"#),
        )
        .0
    };
    let inserted = |order: &str| {
        placed(
            "note",
            &format!("insert i {{ note n {{ at = 1q; dur = 1q; pitch = G4; order = {order}; }} }}"),
        )
        .0
    };
    for fits in ["0", "-2", "2147483647", "-2147483648"] {
        for kind in ["note", "member", "hit", "message"] {
            assert_eq!(compile(&leaf(kind, fits)), [], "{kind} {fits}");
            assert_eq!(compile(&set_order(kind, fits)), [], "{kind} {fits}");
        }
        assert_eq!(compile(&inserted(fits)), [], "{fits}");
    }
    for (order, code, message) in [
        ("2147483648", DiagnosticCode::ResourceLimit, PAST_32_BITS),
        ("-2147483649", DiagnosticCode::ResourceLimit, PAST_32_BITS),
        ("1099511627776", DiagnosticCode::ResourceLimit, PAST_32_BITS),
        ("1/2", DiagnosticCode::Range, NOT_WHOLE),
    ] {
        for kind in ["note", "member", "hit", "message"] {
            assert_eq!(
                compile(&leaf(kind, order)),
                [refusal(code, message, &["bar", "a"], &["order"])],
                "{kind} {order}"
            );
            assert_eq!(
                compile(&set_order(kind, order)),
                [refusal(code, message, &["main", "o"], &["set", "order"])],
                "{kind} {order}"
            );
        }
        assert_eq!(
            compile(&inserted(order)),
            [refusal(code, message, &["main", "i", "n"], &["order"])],
            "{order}"
        );
    }
}

#[test]
fn an_edit_that_leaves_a_wrong_order_is_refused_in_both_contexts() {
    let (text, _) = placed(
        "note",
        r#"override o { event = "0/a"; set = { order = 1; }; } insert i { note n { at = 1q; dur = 1q; pitch = G4; } }"#,
    );
    for (object, field, at) in [
        (&["bar", "a"][..], &["order"][..], &["order"][..]),
        (&["main", "i", "n"], &["order"], &["order"]),
        (&["main", "o"], &["set", "order"], &["set", "order"]),
    ] {
        let past = refusal(DiagnosticCode::ResourceLimit, PAST_32_BITS, object, at);
        assert_eq!(
            edit(&text, set(object, field, number(2_147_483_648, 1))),
            [Some(past.clone()), Some(past)],
            "{object:?}"
        );
        let half = refusal(DiagnosticCode::Range, NOT_WHOLE, object, at);
        assert_eq!(
            edit(&text, set(object, field, number(1, 2))),
            [Some(half.clone()), Some(half)],
            "{object:?}"
        );
        assert_eq!(
            edit(&text, set(object, field, number(2_147_483_647, 1))),
            [None, None],
            "{object:?}"
        );
    }
}

const NO_TARGET: &str = "note, hit, and message leaves require a track with an event target";

/// Pattern `bar` holding `children` and placed as `main` on track `bare`,
/// with `more` beside them.
fn on_bare(children: &str, more: &str) -> String {
    source(&format!(
        "pattern bar {{ length = 4q; {children} }}\nplace main {{ pattern = &bar; track = &bare; at = 0q; }}\n{more}"
    ))
}

const CLIP: &str = "audio x { asset = &tone; at = 0q; source = [0frame, 480frame]; mode = rate; }";
const CLIP_UP: &str = "connect clip_up { from = &main:out; to = &mix:in; }";
const NOTE: &str = "note n { at = 1q; dur = 1q; pitch = C4; }";

#[test]
fn a_placement_that_expands_an_event_needs_a_track_with_a_target() {
    let missing = [refusal(
        DiagnosticCode::Reference,
        NO_TARGET,
        &["main"],
        &["track"],
    )];
    for (children, more) in [
        (NOTE, ""),
        ("chord c { at = 0q; dur = 1q; pitches = [C4, E4]; }", ""),
        (r#"hit h { at = 0q; key = "kick"; }"#, ""),
        (
            r#"message m { at = 0q; protocol = "midi1"; bytes = [176, 1, 64]; }"#,
            "",
        ),
        // Beside an audio leaf, and through uses.
        (&format!("{CLIP} {NOTE}")[..], CLIP_UP),
        (
            "use u { pattern = &mid; at = 0q; }",
            &format!("pattern mid {{ length = 4q; use v {{ pattern = &leaf; at = 0q; }} }}\npattern leaf {{ length = 4q; {NOTE} }}")[..],
        ),
    ] {
        assert_eq!(compile(&on_bare(children, more)), missing, "{children}");
    }
    // An insert, and a note that an override deletes, are events all the same.
    let inserted =
        on_bare("", "").replace("at = 0q; }", &format!("at = 0q; insert i {{ {NOTE} }} }}"));
    assert_eq!(compile(&inserted), missing);
    let deleted = on_bare(NOTE, "").replace(
        "track = &bare; at = 0q; }",
        r#"track = &bare; at = 0q; override gone { event = "0/n"; delete = true; } }"#,
    );
    assert_eq!(compile(&deleted), missing);
}

#[test]
fn a_placement_that_expands_no_event_needs_no_target() {
    for (children, more) in [
        ("", ""),
        (CLIP, CLIP_UP),
        (
            "use u { pattern = &rest; at = 0q; }",
            "pattern rest { length = 4q; }",
        ),
        (
            "use u { pattern = &loop; at = 0q; }",
            &format!("pattern loop {{ length = 4q; {CLIP} }}\n{CLIP_UP}")[..],
        ),
    ] {
        assert_eq!(compile(&on_bare(children, more)), [], "{children}");
    }
    // A pattern of notes that nothing places on the bare track.
    let elsewhere = source(&format!(
        "pattern bar {{ length = 4q; {NOTE} }}\npattern quiet {{ length = 4q; }}\nplace main {{ pattern = &bar; track = &t; at = 0q; }}\nplace second {{ pattern = &quiet; track = &bare; at = 0q; }}"
    ));
    assert_eq!(compile(&elsewhere), []);
}

#[test]
fn every_placement_without_a_target_is_reported() {
    let text = source(&format!(
        "pattern bar {{ length = 4q; {NOTE} }}\nplace main {{ pattern = &bar; track = &bare; at = 0q; }}\nplace second {{ pattern = &bar; track = &bare; at = 4q; }}\nplace third {{ pattern = &bar; track = &t; at = 8q; }}"
    ));
    assert_eq!(
        compile(&text),
        [
            refusal(DiagnosticCode::Reference, NO_TARGET, &["main"], &["track"]),
            refusal(
                DiagnosticCode::Reference,
                NO_TARGET,
                &["second"],
                &["track"]
            ),
        ]
    );
}

#[test]
fn an_edit_that_leaves_notes_without_a_target_is_refused_in_both_contexts() {
    let text = source(&format!(
        "pattern bar {{ length = 4q; {NOTE} }}\npattern quiet {{ length = 4q; }}\nplace main {{ pattern = &bar; track = &t; at = 0q; }}\nplace second {{ pattern = &quiet; track = &bare; at = 0q; }}"
    ));
    let missing = |place: &str| {
        Some(refusal(
            DiagnosticCode::Reference,
            NO_TARGET,
            &[place],
            &["track"],
        ))
    };
    // Taking the target from the track that the notes play on.
    let untarget = Operation::Unset {
        object: vec!["t".into()],
        field: vec!["target".into()],
        expect: None,
    };
    assert_eq!(edit(&text, untarget), [missing("main"), missing("main")]);
    // Moving the notes to the track without one.
    let bare = json!({"t":"ref","path":["bare"],"port":null});
    assert_eq!(
        edit(&text, set(&["main"], &["track"], bare)),
        [missing("main"), missing("main")]
    );
    // Placing a pattern of notes where the empty one was.
    let bar = json!({"t":"ref","path":["bar"],"port":null});
    assert_eq!(
        edit(&text, set(&["second"], &["pattern"], bar)),
        [missing("second"), missing("second")]
    );
    // An edit that leaves the notes their target applies.
    let label = json!({"t":"string","v":"x"});
    assert_eq!(edit(&text, set(&["bare"], &["label"], label)), [None, None]);
}
