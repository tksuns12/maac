//! Execution-label projection over real objects (evidence row D10, spec §20.2).
//!
//! The portable L1 metadata-role fragment uses abstract `fixture_*` kinds, which
//! the execution normalizer correctly refuses. This test places every role the
//! fragment names on a real object of a validated production bundle and requires
//! `execution_identity` to give the fragment's verdict for each one: an actual
//! object's own `label` is removed; `label` keys inside params, override `set`,
//! understood extension data and object-shaped records are kept, as are object
//! IDs named `label`. The fragment's parent has both a `label` field and a
//! `label` child, which §2 forbids in one real object, so the roles are spread
//! over several objects.
//!
//! §4 lets every object carry a `label`; the last tests hold the library
//! extension's kinds and `import` to that.

use maac::bundle::sha256_digest;
use maac::production_data::SCHEMA_BYTES;
use maac::production_identity::{execution_identity, ExecutionIdentity};
use maac::{compile_bundle, SourceBundle};
use serde_json::Value;

const INPUT: &str =
    include_str!("../conformance/l1/fragments/metadata-role-input.typed-object.json");
const EXPECTED: &str =
    include_str!("../conformance/l1/fragments/metadata-role-expected.typed-object.json");

/// Display labels in `source()`, as `label="..."` fields of actual objects.
const DISPLAY: [&str; 7] = [
    "project display",
    "control display",
    "parent display metadata",
    "note display",
    "track display",
    "child display metadata",
    "extension display",
];

fn source() -> String {
    let delivery = r#"{rate=48000Hz;resampler="maac.src.kaiser/1";targets={label={role=master;output=&s:out;encoding=wav_f32le;dither={type=none;};};};}"#;
    format!(
        r#"maac 1;
project p {{score=[0q,1q];rate=48000Hz;tempo=&clock;meter=&metre;output=&s:out;requires=["maac.production/1"];label="project display";}}
tempo clock {{points=[(0q,120bpm,step)];}}
meter metre {{points=[(0q,4,4)];}}
asset schema {{kind=descriptor;path="production.schema.json";hash="{hash}";}}
instrument sound {{channels=1;
 voice v {{channels=1;output=&osc:out;amplitude=&env;
  node osc {{type="synth.sine/1";}}
  node env {{type="synth.adsr/1";}}
 }}
 control label {{target=&v.osc.params.ratio;default=1;label="control display";}}
}}
node s {{instrument=&sound;params={{label=3;}};label="parent display metadata";}}
pattern riff {{length=1q;note label {{at=0q;dur=1q;pitch=C4;label="note display";}}}}
track label {{target=&s:events;label="track display";}}
place play {{pattern=&riff;track=&label;at=0q;
 override label {{event="0/label";set={{label="child executable payload";}};label="child display metadata";}}
}}
extension deliveries {{namespace="maac.production/1";schema=&schema;render_affecting=true;label="extension display";
 data={{deliveries={{label={delivery};children={delivery};fields={delivery};kind={delivery};}};}};
}}
"#,
        hash = sha256_digest(SCHEMA_BYTES),
    )
}

fn identity(source: &str) -> ExecutionIdentity {
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle
        .assets
        .insert("production.schema.json".into(), SCHEMA_BYTES.to_vec());
    let plan = compile_bundle(&bundle).expect("metadata-role bundle compiles");
    let document = maac::parse(source).unwrap();
    execution_identity(&document, &plan).unwrap()
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// Each role the fragment declares, its location in the fragment, and the
/// locations that carry the same role in `source()`. Real locations are JSON
/// pointers into the authored syntax tree and the normalized execution graph,
/// which share the `objects`/`kind`/`fields`/`children` and tagged-record shape.
const ROLES: &[(&str, &str, &[&str])] = &[
    (
        "an actual object's display label",
        "/fields/label",
        &[
            "/objects/p/fields/label",
            "/objects/s/fields/label",
            "/objects/label/fields/label",
            "/objects/deliveries/fields/label",
        ],
    ),
    (
        "an actual child object's display label",
        "/children/label/fields/label",
        &[
            "/objects/sound/children/label/fields/label",
            "/objects/riff/children/label/fields/label",
            "/objects/play/children/label/fields/label",
        ],
    ),
    (
        "a child object dictionary key named label",
        "/children/label",
        &[
            "/objects/sound/children/label",
            "/objects/riff/children/label",
            "/objects/play/children/label",
        ],
    ),
    (
        "a top-level object ID named label",
        "/children/label",
        &["/objects/label"],
    ),
    (
        "a params record entry named label",
        "/fields/params/fields/label",
        &["/objects/s/fields/params/fields/label"],
    ),
    (
        "a child's record payload entry named label",
        "/children/label/fields/payload/fields/label",
        &["/objects/play/children/label/fields/set/fields/label"],
    ),
    (
        "an understood extension-data record entry named label",
        "/fields/extension_data/fields/label",
        &["/objects/deliveries/fields/data/fields/deliveries/fields/label"],
    ),
    (
        "a nested extension-data record entry named label",
        "/fields/extension_data/fields/nested/fields/label",
        &["/objects/deliveries/fields/data/fields/deliveries/fields/label/fields/targets/fields/label"],
    ),
    (
        "an object-lookalike record's children entry",
        "/fields/extension_data/fields/object_lookalike/fields/children",
        &["/objects/deliveries/fields/data/fields/deliveries/fields/children"],
    ),
    (
        "an object-lookalike record's fields entry",
        "/fields/extension_data/fields/object_lookalike/fields/fields",
        &["/objects/deliveries/fields/data/fields/deliveries/fields/fields"],
    ),
    (
        "an object-lookalike record's kind entry",
        "/fields/extension_data/fields/object_lookalike/fields/kind",
        &["/objects/deliveries/fields/data/fields/deliveries/fields/kind"],
    ),
    (
        "an object-lookalike record's label entry",
        "/fields/extension_data/fields/object_lookalike/fields/label",
        &["/objects/deliveries/fields/data/fields/deliveries/fields/label"],
    ),
    (
        "a label entry inside an object-lookalike record's children",
        "/fields/extension_data/fields/object_lookalike/fields/children/fields/label",
        &["/objects/deliveries/fields/data/fields/deliveries/fields/children/fields/targets/fields/label"],
    ),
    (
        "a label entry inside an object-lookalike record's fields",
        "/fields/extension_data/fields/object_lookalike/fields/fields/fields/label",
        &["/objects/deliveries/fields/data/fields/deliveries/fields/fields/fields/targets/fields/label"],
    ),
];

#[test]
fn execution_projection_agrees_with_the_l1_metadata_role_fragment() {
    let fragment_input = json(INPUT);
    let fragment_expected = json(EXPECTED);
    let source = source();
    let authored = maac::parse(&source).unwrap().to_syntax_json_value();
    let normalized = json(&identity(&source).normalized_source_json);
    let mut disagreements = Vec::new();
    for (role, fragment, locations) in ROLES {
        assert!(
            fragment_input.pointer(fragment).is_some(),
            "{role}: fragment input lacks {fragment}"
        );
        let kept = fragment_expected.pointer(fragment).is_some();
        for location in *locations {
            assert!(
                authored.pointer(location).is_some(),
                "{role}: source lacks {location}"
            );
            if normalized.pointer(location).is_some() != kept {
                disagreements.push(format!(
                    "{role} at {location}: fragment {} it, execution identity {} it",
                    if kept { "keeps" } else { "removes" },
                    if kept { "removes" } else { "keeps" },
                ));
            }
        }
    }
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n"));
}

#[test]
fn every_fragment_difference_is_a_display_label() {
    // The fragment removes exactly the two actual objects' own labels; any
    // other difference would be a role this test does not map.
    fn removed(input: &Value, expected: &Value, path: String, found: &mut Vec<String>) {
        if let (Value::Object(input), Value::Object(expected)) = (input, expected) {
            for (key, value) in input {
                let path = format!("{path}/{key}");
                match expected.get(key) {
                    Some(other) => removed(value, other, path, found),
                    None => found.push(path),
                }
            }
        }
    }
    let mut found = Vec::new();
    removed(&json(INPUT), &json(EXPECTED), String::new(), &mut found);
    assert_eq!(found, ["/children/label/fields/label", "/fields/label"]);
}

#[test]
fn display_labels_do_not_change_the_execution_hash() {
    let source = source();
    let original = identity(&source);
    let mut unlabeled = source.clone();
    for text in DISPLAY {
        let field = format!("label=\"{text}\";");
        assert!(unlabeled.contains(&field), "{field}");
        unlabeled = unlabeled.replace(&field, "");
    }
    let relabeled = DISPLAY.iter().fold(source.clone(), |text, label| {
        text.replace(&format!("\"{label}\""), &format!("\"{label}, edited\""))
    });
    for changed in [unlabeled, relabeled] {
        let other = identity(&changed);
        assert_eq!(original.execution_hash, other.execution_hash);
        assert_ne!(original.source_input_hash, other.source_input_hash);
    }
}

#[test]
fn payload_labels_change_the_execution_hash() {
    let source = source();
    let original = identity(&source).execution_hash;
    for (from, to) in [
        ("params={label=3;}", "params={label=4;}"),
        (
            "set={label=\"child executable payload\";}",
            "set={label=\"another payload\";}",
        ),
    ] {
        assert!(source.contains(from), "{from}");
        assert_ne!(
            identity(&source.replace(from, to)).execution_hash,
            original,
            "{to}"
        );
    }
}

/// A plan without its provenance: source spans and source-file pins move when
/// labels are added, but everything the renderer reads must not.
fn execution_plan(plan: &maac::Plan) -> Value {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.remove("span");
                if let Some(Value::Array(files)) = map.get_mut("source_files") {
                    files.iter_mut().for_each(|file| file["hash"] = Value::Null);
                }
                if let Some(Value::Array(dependencies)) = map.get_mut("dependencies") {
                    dependencies
                        .iter_mut()
                        .for_each(|d| d["hash"] = Value::Null);
                }
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value = json(&plan.to_json_string().unwrap());
    strip(&mut value);
    value
}

/// Insert `label="<kind> <id>";` as the first field of every object
/// declaration, which is the only place an identifier pair precedes `{`.
fn label_every_object(source: &str, kinds: &mut Vec<String>) -> String {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut result = String::with_capacity(source.len() * 2);
    for (i, c) in source.char_indices() {
        result.push(c);
        if c != '{' {
            continue;
        }
        let head = source[..i].trim_end();
        let id_start = head.trim_end_matches(ident).len();
        let between = head[..id_start].trim_end();
        let kind_start = between.trim_end_matches(ident).len();
        let (id, kind) = (&head[id_start..], &between[kind_start..]);
        if id.is_empty() || kind.is_empty() || between.len() == id_start {
            continue;
        }
        kinds.push(kind.to_owned());
        result.push_str(&format!(" label=\"{kind} {id}\";"));
    }
    result
}

#[test]
fn library_extension_objects_accept_display_labels() {
    use maac::bundle_fs::load_bundle;
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let original = load_bundle(Path::new("examples/reusable.maac"), root).unwrap();
    let entry = "examples/reusable.maac";
    let library = "examples/sounds/studio.maac";
    let mut kinds = Vec::new();
    let mut labeled = original.clone();
    let studio = label_every_object(&original.sources[library], &mut kinds);
    let main = label_every_object(&original.sources[entry], &mut kinds).replace(
        &sha256_digest(original.sources[library].as_bytes()),
        &sha256_digest(studio.as_bytes()),
    );
    labeled.sources.insert(library.into(), studio);
    labeled.sources.insert(entry.into(), main);
    for kind in [
        "library",
        "wavetable",
        "instrument",
        "voice",
        "node",
        "connect",
        "modulate",
        "control",
        "preset",
        "import",
    ] {
        assert!(kinds.iter().any(|k| k == kind), "no {kind} labeled");
    }
    maac::check_bundle(&labeled).expect("labels are valid on every object");
    let plan = |bundle: &SourceBundle| execution_plan(&compile_bundle(bundle).unwrap());
    assert_eq!(plan(&original), plan(&labeled), "labels changed the plan");

    let numbered = labeled.sources[entry].replace("label=\"import sounds\";", "label=3;");
    labeled.sources.insert(entry.into(), numbered);
    let error = maac::check_bundle(&labeled).unwrap_err();
    assert_eq!(error.first().unwrap().code, maac::DiagnosticCode::Unit);
}

#[test]
fn sample_objects_accept_display_labels_and_refuse_other_types() {
    let mut wav = std::io::Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::new(&mut wav, spec).unwrap();
        for j in 0..64 {
            writer.write_sample(j as f32 / 64.0).unwrap();
        }
        writer.finalize().unwrap();
    }
    let wav = wav.into_inner();
    let source = |label: &str| {
        format!(
            r#"maac 1;
project song {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &keys:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
sample tone {{ path = "tone.wav"; hash = "{}"; root = A4; {label} }}
instrument sampler {{ channels = 1;
  voice v {{ channels = 1; amplitude = &amp; output = &play:out;
    node amp {{ type = "synth.adsr/1"; }}
    node play {{ type = "synth.sample/1"; config = {{ zones = [{{ sample = &tone; low = key(0); high = key(127); }}]; }}; }}
  }}
}}
node keys {{ instrument = &sampler; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = A4; }} }}
track melody {{ target = &keys:events; }}
place play {{ pattern = &phrase; track = &melody; at = 0q; }}
"#,
            sha256_digest(&wav)
        )
    };
    let bundle = |source: String| {
        let mut bundle = SourceBundle::new("main.maac", source);
        bundle.assets.insert("tone.wav".into(), wav.clone());
        bundle
    };
    let mut kinds = Vec::new();
    let labeled = label_every_object(&source(""), &mut kinds);
    assert!(kinds.iter().any(|k| k == "sample"));
    let plan = |source: String| execution_plan(&compile_bundle(&bundle(source)).unwrap());
    assert_eq!(plan(source("")), plan(labeled), "labels changed the plan");

    let error = compile_bundle(&bundle(source("label = 3;"))).unwrap_err();
    let diagnostic = error.first().unwrap();
    assert_eq!(diagnostic.code, maac::DiagnosticCode::Unit);
    assert_eq!(diagnostic.object_path, ["tone"]);
    assert_eq!(diagnostic.field_path, ["label"]);
}
