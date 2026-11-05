//! Semantic normalization N(A) (§20.2): one module computes the execution
//! view of an authored document. Writing a specified default out and leaving
//! it implicit give the same N(A); the editing view keeps display labels and
//! the execution view drops them, with the fields added after the execution
//! hash was defined while they hold their defaults.

use std::fs;
use std::path::Path;

use maac::bundle::SourceBundle;
use maac::editing::{
    apply_transaction, AuthoredDocument, BundleEditContext, Operation, Transaction,
};
use maac::library::LibrarySet;
use maac::normalization::{
    execution_view, normalize_document, normalize_object, normalize_value, NormalizationContext,
};
use maac::production_identity::execution_identity;
use serde_json::{json, Value};

/// N(A) of a one-file composition, with its libraries resolved.
fn normalized(source: &str) -> Value {
    let bundle = SourceBundle::new("main.maac", source);
    let resolved = bundle
        .resolve()
        .unwrap_or_else(|e| panic!("resolves: {e:?}"));
    let libraries = LibrarySet::resolve(&resolved).unwrap_or_else(|e| panic!("libraries: {e:?}"));
    let authored = maac::parse(source)
        .unwrap_or_else(|e| panic!("parses: {e:?}"))
        .to_syntax_json_value();
    let context = NormalizationContext::new(&authored, "main.maac", Some(&libraries))
        .unwrap_or_else(|e| panic!("context: {e:?}"));
    normalize_document(&context).unwrap_or_else(|e| panic!("normalizes: {e:?}"))
}

/// N(A) of a document without library syntax.
fn normalized_plain(source: &str) -> Value {
    let authored = maac::parse(source)
        .unwrap_or_else(|e| panic!("parses: {e:?}"))
        .to_syntax_json_value();
    let context = NormalizationContext::new(&authored, "main.maac", None).unwrap();
    normalize_document(&context).unwrap_or_else(|e| panic!("normalizes: {e:?}"))
}

fn hash(source: &str) -> String {
    let document = maac::parse(source).unwrap();
    maac::compile(&document).unwrap_or_else(|e| panic!("compiles: {e:?}"));
    execution_identity(&document)
        .unwrap_or_else(|e| panic!("identity: {e:?}"))
        .execution_hash
}

// ---------------------------------------------------------------------------
// Instruments, effects and their graph nodes.

/// A song with a mono instrument of many synth nodes, a bowed section and a
/// stereo effect. `{name}` placeholders take each node's written defaults.
fn song(fields: &[(&str, &str)]) -> String {
    let mut text = r#"maac 1;
project p { score = [0q, 4q]; rate = 48000Hz; tail = 200ms; tempo = &clock; meter = &metre; output = &mix:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument tone {
 channels = 1;
 voice v {
  channels = 1; amplitude = &amp; output = &out_gain:out; {voice}
  node amp { type = "synth.adsr/1"; params = { release = 100ms; {amp} }; {amp_config} }
  node hiss { type = "synth.noise/1"; {hiss} }
  node pick { type = "synth.pluck/1"; {pick} }
  node body { type = "synth.modal/1"; config = { modes = 4; {body_config} }; {body} }
  node draw { type = "synth.random/1"; {draw} }
  node blend { type = "synth.mix/1"; config = { channels = 1; }; }
  node shape { type = "synth.svf/1"; config = { channels = 1; mode = lowpass; }; {shape} }
  node heat { type = "synth.drive/1"; config = { channels = 1; }; {heat} }
  node out_gain { type = "synth.gain/1"; config = { channels = 1; }; {out_gain} }
  connect hiss_body { from = &hiss:out; to = &body:in; }
  connect body_blend { from = &body:out; to = &blend:in; }
  connect pick_blend { from = &pick:out; to = &blend:in; }
  connect blend_shape { from = &blend:out; to = &shape:in; }
  connect shape_heat { from = &shape:out; to = &heat:in; }
  connect heat_gain { from = &heat:out; to = &out_gain:in; }
  modulate wobble { from = &draw:out; to = &shape.params.cutoff; depth = 100Hz; }
 }
}
instrument bowed {
 channels = 1;
 voice v {
  channels = 1; amplitude = &amp; output = &section:out;
  node amp { type = "synth.adsr/1"; params = { sustain = 1; release = 400ms; }; }
  node section { type = "synth.bow/1"; {section} }
 }
}
effect room {
 channels = 2; {room}
 shared main {
  channels = 2; output = &wide:out;
  node line { type = "synth.delay/1"; config = { channels = 2; max = 1s; {line_config} }; {line} }
  node lean { type = "synth.balance/1"; {lean} }
  node wide { type = "synth.width/1"; {wide} }
  connect in_line { from = &input:out; to = &line:in; }
  connect line_lean { from = &line:out; to = &lean:in; }
  connect lean_wide { from = &lean:out; to = &wide:in; }
 }
}
node src { instrument = &tone; {src} }
node strings { instrument = &bowed; {strings} }
node place_src { type = "core.pan/1"; }
node place_strings { type = "core.pan/1"; }
node fx { effect = &room; }
node mix { type = "core.sum/1"; config = { channels = 2; }; }
connect s1 { from = &src:out; to = &place_src:in; }
connect s2 { from = &place_src:out; to = &fx:in; }
connect s3 { from = &fx:out; to = &mix:in; }
connect s4 { from = &strings:out; to = &place_strings:in; }
connect s5 { from = &place_strings:out; to = &mix:in; }
track line_track { target = &src:events; }
track string_track { target = &strings:events; }
pattern body_pattern { length = 4q;
  note n { at = 0q; dur = 1q; pitch = A4; {note} }
}
place go { track = &line_track; pattern = &body_pattern; at = 0q; }
place bow_go { track = &string_track; pattern = &body_pattern; at = 0q; }
"#
    .to_owned();
    for (name, value) in fields {
        let marker = format!("{{{name}}}");
        assert!(text.contains(&marker), "fixture has {marker}");
        text = text.replace(&marker, value);
    }
    // Every placeholder not named is left empty.
    for name in PLACEHOLDERS {
        text = text.replace(&format!("{{{name}}}"), "");
    }
    text
}

const PLACEHOLDERS: &[&str] = &[
    "voice",
    "amp_config",
    "amp",
    "hiss",
    "pick",
    "body_config",
    "body",
    "draw",
    "shape",
    "heat",
    "out_gain",
    "section",
    "room",
    "line_config",
    "line",
    "lean",
    "wide",
    "src",
    "strings",
    "note",
];

/// Every default each node, instance, effect and note in [`song`] takes,
/// written out.
const WRITTEN: &[(&str, &str)] = &[
    ("voice", "connected = false;"),
    ("amp", "attack = 0s; decay = 0ms; sustain = 1; curve = 0;"),
    ("amp_config", "config = { on_join = hold; };"),
    (
        "hiss",
        "config = { seed = 1831565813; per_note = false; }; params = { level = 1; };",
    ),
    (
        "pick",
        "config = { seed = 1831565813; }; params = { ratio = 1; decay = 3s; damping = 1/2; level = 1; hardness = 1; position = 0; pickup = 0; };",
    ),
    ("body_config", "strings = 1;"),
    (
        "body",
        "params = { ratio = 1; stretch = 0; decay = 2s; damping = 0; position = 0; detune = 0; level = 1; };",
    ),
    ("draw", "config = { seed = 1; };"),
    ("shape", "params = { cutoff = 1kHz; ratio = 0; q = 707/1000; };"),
    ("heat", "params = { drive = 1; bias = 0; level = 1; reference = 0; };"),
    ("out_gain", "params = { level = 1; };"),
    (
        "section",
        "config = { players = 1; body = violin; channels = 1; seed = 1; }; params = { ratio = 1; dynamics = 1/2; position = 1/8; vibrato = 25/2; rate = 28/5Hz; attack = 80ms; timing = 18ms; spread = 1; width = 7/10; pan = 0; level = 1; };",
    ),
    ("room", "input = 2;"),
    ("line_config", "clock = seconds;"),
    ("line", "params = { time = 1/2s; feedback = 0; damping = 0; };"),
    ("lean", "params = { balance = 0; };"),
    ("wide", "params = { width = 1; };"),
    ("src", "config = { voices = 64; };"),
    ("strings", "config = { voices = 64; };"),
    ("note", "slur = false; slide = 0s;"),
];

#[test]
fn every_node_default_written_out_normalizes_like_the_default() {
    let omitted = normalized(&song(&[]));
    let written = normalized(&song(WRITTEN));
    assert_eq!(omitted, written);
}

#[test]
fn each_written_default_alone_normalizes_like_the_default() {
    let omitted = normalized(&song(&[]));
    for field in WRITTEN {
        assert_eq!(
            normalized(&song(&[*field])),
            omitted,
            "writing out {} changes N(A)",
            field.1
        );
    }
}

#[test]
fn a_value_away_from_its_default_is_kept() {
    let omitted = normalized(&song(&[]));
    for field in [
        ("shape", "params = { q = 2; };"),
        ("heat", "params = { bias = 1/2; };"),
        ("section", "config = { players = 2; };"),
        ("section", "params = { vibrato = 5; };"),
        ("body_config", "strings = 2;"),
        ("hiss", "config = { seed = 5; };"),
        ("hiss", "config = { per_note = true; };"),
        ("voice", "connected = true;"),
        ("line", "params = { feedback = 1/2; };"),
        ("lean", "params = { balance = 1/2; };"),
        ("note", "slur = true;"),
        ("amp_config", "config = { on_join = restart; };"),
    ] {
        assert_ne!(
            normalized(&song(&[field])),
            omitted,
            "{} must change N(A)",
            field.1
        );
    }
}

/// The fields added to the language after the execution hash was defined:
/// each object path, the field, and its value away from the default.
fn late_fields(view: &Value) -> [(&'static str, Option<&Value>); 5] {
    let note = &view["objects"]["body_pattern"]["children"]["n"]["fields"];
    let voice = &view["objects"]["tone"]["children"]["v"];
    let nodes = &voice["children"];
    [
        ("slur", note.get("slur")),
        ("slide", note.get("slide")),
        ("connected", voice["fields"].get("connected")),
        (
            "per_note",
            nodes["hiss"]["fields"]["config"]["fields"].get("per_note"),
        ),
        (
            "on_join",
            nodes["amp"]["fields"]
                .get("config")
                .and_then(|config| config["fields"].get("on_join")),
        ),
    ]
}

#[test]
fn late_fields_are_expanded_like_any_default() {
    let editing = normalized(&song(&[]));
    let defaults = [
        json!({"t":"boolean","v":false}),
        json!({"t":"quantity","n":"0","d":"1","u":"s"}),
        json!({"t":"boolean","v":false}),
        json!({"t":"boolean","v":false}),
        json!({"t":"symbol","v":"hold"}),
    ];
    for ((name, value), default) in late_fields(&editing).into_iter().zip(defaults) {
        assert_eq!(value, Some(&default), "N(A) expands {name}");
    }
}

#[test]
fn the_execution_view_leaves_out_late_fields_at_their_defaults() {
    let execution = execution_view(&normalized(&song(WRITTEN)));
    for (name, value) in late_fields(&execution) {
        assert_eq!(value, None, "the execution view keeps {name}");
    }
    // The holding envelope's config record goes with its only field.
    let amp = &execution["objects"]["tone"]["children"]["v"]["children"]["amp"];
    assert!(amp["fields"].get("config").is_none());

    let set = execution_view(&normalized(&song(&[
        ("note", "slur = true; slide = 30ms;"),
        ("voice", "connected = true;"),
        ("hiss", "config = { per_note = true; };"),
        ("amp_config", "config = { on_join = restart; };"),
    ])));
    for (name, value) in late_fields(&set) {
        assert!(value.is_some(), "{name} away from its default is kept");
    }
}

#[test]
fn graph_nodes_expand_every_descriptor_default() {
    let normalized = normalized(&song(&[]));
    let voice = &normalized["objects"]["tone"]["children"]["v"]["children"];
    let params = |node: &str| voice[node]["fields"]["params"]["fields"].clone();
    assert_eq!(
        params("shape")["q"],
        json!({"t":"number","n":"707","d":"1000"})
    );
    for name in ["drive", "bias", "level", "reference"] {
        assert!(params("heat").get(name).is_some(), "drive {name}");
    }
    for name in ["hardness", "pickup", "position"] {
        assert!(params("pick").get(name).is_some(), "pluck {name}");
    }
    for name in ["stretch", "detune"] {
        assert!(params("body").get(name).is_some(), "modal {name}");
    }
    assert_eq!(
        params("amp")["curve"],
        json!({"t":"number","n":"0","d":"1"})
    );
    let section = &normalized["objects"]["bowed"]["children"]["v"]["children"]["section"]["fields"];
    for name in ["dynamics", "vibrato", "rate", "timing", "spread"] {
        assert!(
            section["params"]["fields"].get(name).is_some(),
            "bow {name}"
        );
    }
    assert_eq!(
        section["config"]["fields"],
        json!({
            "players": {"t":"number","n":"1","d":"1"},
            "body": {"t":"symbol","v":"violin"},
            "channels": {"t":"number","n":"1","d":"1"},
            "seed": {"t":"number","n":"1","d":"1"},
        })
    );
    assert_eq!(
        voice["body"]["fields"]["config"]["fields"]["strings"],
        json!({"t":"number","n":"1","d":"1"})
    );
}

#[test]
fn effect_nodes_and_their_effect_normalize() {
    let normalized = normalized(&song(&[]));
    let room = &normalized["objects"]["room"];
    assert_eq!(
        room["fields"]["input"],
        json!({"t":"number","n":"2","d":"1"})
    );
    let graph = &room["children"]["main"]["children"];
    assert_eq!(
        graph["line"]["fields"]["config"]["fields"]["clock"],
        json!({"t":"symbol","v":"seconds"})
    );
    assert_eq!(
        graph["lean"]["fields"]["params"]["fields"]["balance"],
        json!({"t":"number","n":"0","d":"1"})
    );
    assert_eq!(
        graph["wide"]["fields"]["params"]["fields"]["width"],
        json!({"t":"number","n":"1","d":"1"})
    );
    // A node that takes no config gets none, written or not.
    assert!(graph["lean"]["fields"].get("config").is_none());
}

#[test]
fn a_node_that_takes_config_always_has_a_config_record() {
    let omitted = normalized(&song(&[]));
    let voice = &omitted["objects"]["tone"]["children"]["v"]["children"];
    assert_eq!(
        voice["draw"]["fields"]["config"]["fields"]["seed"],
        json!({"t":"number","n":"1","d":"1"})
    );
    assert_eq!(
        normalized(&song(&[("draw", "config = {};")])),
        omitted,
        "an empty config is the omitted config"
    );
    assert_eq!(
        voice["amp"]["fields"]["config"]["fields"]["on_join"],
        json!({"t":"symbol","v":"hold"})
    );
}

#[test]
fn a_sample_declaration_with_a_format_is_mono_by_default() {
    let header = "maac 1;\nproject p { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &mix:out; }\ntempo clock { points = [(0q, 120bpm, step)]; }\nmeter metre { points = [(0q, 4, 4)]; }\nnode mix { type = \"core.sum/1\"; config = { channels = 1; }; }\n";
    let sample = "sample tone { path = \"tone.pcm\"; hash = \"sha256:00\"; format = \"pcm_f32le_interleaved/1\"; rate = 24000Hz; frames = 4; root = A4; }";
    let omitted = normalized_plain(&format!("{header}{sample}"));
    let written = normalized_plain(&format!(
        "{header}{}",
        sample.replace("root = A4;", "root = A4; channels = 1;")
    ));
    assert_eq!(omitted, written);
}

// ---------------------------------------------------------------------------
// Editing: fragments in their base document's context.

#[test]
fn an_expectation_that_omits_a_config_value_means_its_default() {
    let source = song(&[("hiss", "config = { seed = 5; };")]);
    let bundle = SourceBundle::new("main.maac", source.clone());
    let libraries = LibrarySet::resolve(&bundle.resolve().unwrap()).unwrap();
    let base = maac::parse(&source).unwrap().to_syntax_json_value();
    let context = NormalizationContext::new(&base, "main.maac", Some(&libraries)).unwrap();
    let path: Vec<String> = ["tone", "v", "hiss"].map(String::from).to_vec();
    let actual = normalize_object(
        &context,
        &path,
        &base["objects"]["tone"]["children"]["v"]["children"]["hiss"],
    )
    .unwrap();
    assert_eq!(
        actual["fields"]["config"]["fields"]["seed"],
        json!({"t":"number","n":"5","d":"1"})
    );
    let expected = normalize_object(
        &context,
        &path,
        &json!({"kind":"node","fields":{"type":{"t":"string","v":"synth.noise/1"}},"children":{}}),
    )
    .unwrap();
    assert_eq!(
        expected["fields"]["config"]["fields"]["seed"],
        json!({"t":"number","n":"1831565813","d":"1"})
    );
}

#[test]
fn a_field_expectation_lowers_pitches_and_units_in_its_object() {
    let source = song(&[]);
    let base = maac::parse(&source).unwrap().to_syntax_json_value();
    let context = NormalizationContext::new(&base, "main.maac", None).unwrap();
    let note: Vec<String> = ["body_pattern", "n"].map(String::from).to_vec();
    assert_eq!(
        normalize_value(
            &context,
            &note,
            &["pitch".into()],
            &json!({"t":"symbol","v":"C4"})
        )
        .unwrap(),
        json!({"t":"call","fn":"key","args":[{"t":"number","n":"60","d":"1"}]})
    );
    assert_eq!(
        normalize_value(
            &context,
            &note,
            &["dur".into()],
            &json!({"t":"call","fn":"bar","args":[{"t":"number","n":"2","d":"1"},{"t":"number","n":"1","d":"1"}]})
        )
        .unwrap(),
        json!({"t":"quantity","n":"4","d":"1","u":"q"})
    );
}

#[test]
fn an_effect_with_a_delay_line_is_editable_with_expectations() {
    let source = song(&[]);
    let bundle = SourceBundle::new("main.maac", source.clone());
    let context = BundleEditContext::new(&bundle).unwrap();
    // Each expectation names the defaults the base leaves implicit.
    let zero = json!({"t":"number","n":"0","d":"1"});
    for (node, defaults) in [
        (
            "line",
            json!({"time": {"t":"quantity","n":"500","d":"1","u":"ms"}, "feedback": zero, "damping": zero}),
        ),
        ("lean", json!({"balance": zero})),
        ("wide", json!({"width": {"t":"number","n":"1","d":"1"}})),
    ] {
        let defaults = json!({"t":"record","fields":defaults});
        let mut document = AuthoredDocument::from_document(&maac::parse(&source).unwrap()).unwrap();
        let transaction = Transaction::new(
            document.revision().to_owned(),
            vec![Operation::Set {
                object: ["room", "main", node].map(String::from).to_vec(),
                field: vec!["params".into()],
                value: defaults.clone(),
                expect: Some(defaults),
                expect_absent: false,
            }],
        )
        .unwrap();
        apply_transaction(&mut document, &transaction, &context)
            .unwrap_or_else(|e| panic!("{node}: {e:?}"));
    }
}

#[test]
fn an_expectation_of_a_late_field_at_its_default_matches_its_omission() {
    let source = song(&[("hiss", "config = { seed = 5; };")]);
    let bundle = SourceBundle::new("main.maac", source.clone());
    let context = BundleEditContext::new(&bundle).unwrap();
    let no = json!({"t":"boolean","v":false});
    for (object, field, value) in [
        (vec!["body_pattern", "n"], vec!["slur"], no.clone()),
        (
            vec!["body_pattern", "n"],
            vec!["slide"],
            json!({"t":"quantity","n":"0","d":"1","u":"ms"}),
        ),
        (vec!["tone", "v"], vec!["connected"], no.clone()),
        (
            vec!["tone", "v", "hiss"],
            vec!["config", "per_note"],
            no.clone(),
        ),
        (
            vec!["tone", "v", "amp"],
            vec!["config"],
            json!({"t":"record","fields":{"on_join":{"t":"symbol","v":"hold"}}}),
        ),
    ] {
        let mut document = AuthoredDocument::from_document(&maac::parse(&source).unwrap()).unwrap();
        let transaction = Transaction::new(
            document.revision().to_owned(),
            vec![Operation::Set {
                object: object.iter().map(|id| id.to_string()).collect(),
                field: field.iter().map(|name| name.to_string()).collect(),
                value: value.clone(),
                expect: Some(value),
                expect_absent: false,
            }],
        )
        .unwrap();
        apply_transaction(&mut document, &transaction, &context)
            .unwrap_or_else(|e| panic!("{field:?}: {e:?}"));
    }
}

// ---------------------------------------------------------------------------
// The execution view and hash.

#[test]
fn the_execution_view_drops_object_labels_not_record_labels() {
    let source = song(&[]).replace(
        "note n { at = 0q;",
        "note n { label = \"first\"; expression e { kind = gain; curve = &swell; label = \"rise\"; } at = 0q;",
    ) + "curve swell { clock = seconds; points = [(0s, 1, step)]; }\n";
    let source = source.replace(
        "node mix { type = \"core.sum/1\"; config = { channels = 2; }; }",
        "node mix { type = \"core.sum/1\"; config = { channels = 2; label = \"kept\"; }; label = \"bus\"; }",
    );
    let editing = normalized(&source);
    assert_eq!(
        editing["objects"]["mix"]["fields"]["label"],
        json!({"t":"string","v":"bus"})
    );
    let execution = execution_view(&editing);
    assert!(execution["objects"]["mix"]["fields"].get("label").is_none());
    // A record's `label` field is data, not an object label.
    assert_eq!(
        execution["objects"]["mix"]["fields"]["config"]["fields"]["label"],
        json!({"t":"string","v":"kept"})
    );
    let note = &execution["objects"]["body_pattern"]["children"]["n"];
    assert!(note["fields"].get("label").is_none());
    assert!(note["children"]["e"]["fields"].get("label").is_none());
    let mut relabelled = editing.clone();
    relabelled["objects"]["mix"]["fields"]["label"] = json!({"t":"string","v":"other"});
    assert_eq!(execution_view(&relabelled), execution);
}

#[test]
fn written_defaults_keep_the_execution_hash() {
    assert_eq!(hash(&song(&[])), hash(&song(WRITTEN)));
    assert_ne!(
        hash(&song(&[])),
        hash(&song(&[("note", "velocity = 1/2;")]))
    );
}

// ---------------------------------------------------------------------------
// Core object kinds and processors.

const HEADER: &str = r#"maac 1;
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 3, 4), (6q, 4, 4)]; }
"#;

/// Every core object kind and every core processor accepted by single-document
/// `compile`, with each optional field omitted.
fn core_omitted() -> String {
    format!(
        r#"{HEADER}
project p {{ score = [0q, 12q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; }}
tuning edo {{
  period = 1200ct;
  steps = [0ct, 400ct, 800ct];
  reference_index = 0;
  reference_frequency = 440Hz;
}}
pattern motif {{
  length = 4q;
  note a {{ at = 0q; dur = 1q; pitch = A4; }}
  note b {{ at = 1q; dur = 1q; pitch = C#5; }}
  chord c {{ at = 2q; dur = 1q; pitches = [A4, C#5, E5]; pick = 2; }}
  hit h {{ at = 3q; key = 60; }}
}}
pattern outer {{
  length = 8q;
  use inner {{ pattern = &motif; at = 0q; }}
}}
track melody {{ target = &tone:events; }}
place first {{ pattern = &outer; track = &melody; at = 0q; }}
node tone {{ type = "core.sine/1"; }}
node filter {{ type = "core.onepole/1"; config = {{ channels = 1; }}; }}
node amp {{ type = "core.gain/1"; config = {{ channels = 1; }}; }}
node fader {{ type = "core.fader/1"; config = {{ channels = 1; }}; }}
node noise {{ type = "core.noise/1"; config = {{ channels = 1; }}; }}
node sum {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
node echo {{ type = "core.delay/1"; config = {{ channels = 1; frames = 480; }}; }}
node pan {{ type = "core.pan/1"; }}
node motion {{ type = "core.lfo/1"; config = {{ period = 1q; }}; }}
node still {{ type = "core.constant/1"; }}
node out {{ type = "core.matrix/1"; config = {{ inputs = 2; outputs = 2; coefficients = [[1, 0], [0, 1]]; }}; }}
curve rise {{ clock = score; points = [(0q, -12dB, linear), (4q, 0dB, step)]; }}
automation fade {{ target = &fader.params.level; curve = &rise; at = 0q; }}
region intro {{ span = [0q, bar(2, 1)]; }}
vibrato shake {{ depth = 20ct; rate = 5Hz; }}
"#
    )
}

/// The same document with every specified default written out and with
/// equivalent unit, pitch, constructor and bar spellings.
fn core_written() -> String {
    format!(
        r#"{HEADER}
project p {{ score = [0q, 12q]; rate = 48kHz; tempo = &clock; meter = &metre; output = &out:out; tail = 0ms; seed = 0; requires = []; }}
tuning edo {{
  period = 1200ct;
  steps = [0ct, 400ct, 800ct];
  reference_index = 0;
  reference_frequency = 11/25kHz;
}}
pattern motif {{
  length = 4q;
  note a {{ at = 0q; dur = 1q; pitch = key(69); velocity = 1; release_velocity = 1/2; onset_offset = 0s; release_offset = 0ms; order = 0; }}
  note b {{ at = 1q; dur = 1q; pitch = Db5; velocity = 1; release_velocity = 1/2; onset_offset = 0s; release_offset = 0s; order = 0; }}
  chord c {{ at = 2q; dur = 1q; pitches = [key(69), Db5, E5]; pick = [2]; velocity = 1; release_velocity = 1/2; onset_offset = 0s; release_offset = 0s; order = 0; }}
  hit h {{ at = 3q; key = 60; velocity = 1; onset_offset = 0ms; order = 0; }}
}}
pattern outer {{
  length = 8q;
  use inner {{ pattern = &motif; at = 0q; count = 1; stretch = 1; transpose = 0ct; boundary = spill; }}
}}
track melody {{ target = &tone:events; }}
place first {{ pattern = &outer; track = &melody; at = bar(1, 1); count = 1; stretch = 1; transpose = 0ct; boundary = spill; }}
node tone {{ type = "core.sine/1"; config = {{ voices = 64; }}; params = {{ attack = 5ms; release = 80ms; level = 1/5; }}; }}
node filter {{ type = "core.onepole/1"; config = {{ channels = 1; }}; params = {{ cutoff = 1kHz; }}; }}
node amp {{ type = "core.gain/1"; config = {{ channels = 1; }}; params = {{ gain = 1; }}; }}
node fader {{ type = "core.fader/1"; config = {{ channels = 1; }}; params = {{ level = 0dB; }}; }}
node noise {{ type = "core.noise/1"; config = {{ channels = 1; seed = 0; }}; }}
node sum {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
node echo {{ type = "core.delay/1"; config = {{ channels = 1; frames = 480; }}; }}
node pan {{ type = "core.pan/1"; params = {{ pan = 0; }}; }}
node motion {{ type = "core.lfo/1"; config = {{ period = 1q; wave = sine; phase = 0; }}; }}
node still {{ type = "core.constant/1"; params = {{ value = 0; }}; }}
node out {{ type = "core.matrix/1"; config = {{ inputs = 2; outputs = 2; coefficients = [[1, 0], [0, 1]]; }}; }}
curve rise {{ clock = score; points = [(0q, -12dB, linear), (4q, 0dB, step)]; }}
automation fade {{ target = &fader.params.level; curve = &rise; at = 0q; }}
region intro {{ span = [0q, 3q]; }}
vibrato shake {{ depth = 20ct; rate = 5Hz; delay = 0s; rise = 0ms; shape = centered; }}
"#
    )
}

#[test]
fn every_core_kind_and_processor_default_written_out_normalizes_like_the_default() {
    assert_ne!(core_omitted(), core_written());
    assert_eq!(
        normalized_plain(&core_omitted()),
        normalized_plain(&core_written())
    );
}

#[test]
fn core_defaults_are_expanded_into_the_normalized_view() {
    let normalized = normalized_plain(&core_omitted());
    let objects = &normalized["objects"];
    let project = &objects["p"]["fields"];
    assert_eq!(project["tail"]["u"], "s");
    assert_eq!(project["seed"]["n"], "0");
    assert_eq!(project["requires"]["items"], Value::Array(Vec::new()));
    let note = &objects["motif"]["children"]["a"]["fields"];
    for field in [
        "velocity",
        "release_velocity",
        "onset_offset",
        "release_offset",
        "order",
    ] {
        assert!(note.get(field).is_some(), "note default {field} expanded");
    }
    assert_eq!(note["pitch"]["fn"], "key");
    assert_eq!(
        objects["motif"]["children"]["c"]["fields"]["pick"]["items"][0]["n"],
        "2"
    );
    for fields in [
        &objects["first"]["fields"],
        &objects["outer"]["children"]["inner"]["fields"],
    ] {
        for field in ["count", "stretch", "transpose", "boundary"] {
            assert!(fields.get(field).is_some(), "placement default {field}");
        }
    }
    assert_eq!(
        objects["tone"]["fields"]["config"]["fields"]["voices"]["n"],
        "64"
    );
    assert_eq!(
        objects["intro"]["fields"]["span"]["items"][1],
        json!({"t":"quantity","n":"3","d":"1","u":"q"})
    );
}

#[test]
fn core_values_away_from_their_defaults_are_kept() {
    let base = core_written();
    let reference = normalized_plain(&base);
    for (from, to) in [
        (
            "velocity = 1; release_velocity",
            "velocity = 9/10; release_velocity",
        ),
        ("seed = 0; requires", "seed = 1; requires"),
        ("config = { voices = 64; }", "config = { voices = 63; }"),
        ("cutoff = 1kHz", "cutoff = 999Hz"),
        ("level = 0dB", "level = -1dB"),
        ("boundary = spill; }\n", "boundary = cut; }\n"),
        ("span = [0q, 3q]", "span = [0q, 4q]"),
        ("pick = [2]", "pick = [3]"),
        ("shape = centered", "shape = up"),
    ] {
        assert!(base.contains(from), "vector anchor {from:?}");
        let changed = base.replacen(from, to, 1);
        assert_ne!(
            normalized_plain(&changed),
            reference,
            "{from:?} -> {to:?} must change N(A)"
        );
    }
}

/// Every checked-in single-file example that compiles has an execution
/// identity, so normalization covers every kind and processor they use.
#[test]
fn checked_in_examples_normalize() {
    let mut checked = Vec::new();
    for entry in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("maac") {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        let Ok(parsed) = maac::parse(&source) else {
            continue;
        };
        if maac::compile(&parsed).is_err() {
            continue;
        }
        let identity =
            execution_identity(&parsed).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        identity.validate().unwrap();
        checked.push(path.file_name().unwrap().to_string_lossy().into_owned());
    }
    for expected in [
        "core-delay.maac",
        "core-noise.maac",
        "pitch-expression.maac",
    ] {
        assert!(checked.iter().any(|name| name == expected), "{checked:?}");
    }
}
