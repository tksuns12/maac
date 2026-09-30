//! D09: the editing N(A) view and the execution-hash projection agree across
//! every core object kind and core processor, defaults are expanded rather
//! than chosen by spelling, and normalization never mutates authored A.

use std::fs;
use std::path::Path;

use maac::editing::{AuthoredDocument, FoundationEditContext};
use maac::production_identity::{canonical_json_bytes, execution_identity};
use serde_json::Value;

fn strip_object_labels(tree: &mut Value) {
    fn visit(objects: &mut serde_json::Map<String, Value>) {
        for object in objects.values_mut() {
            object["fields"].as_object_mut().unwrap().remove("label");
            visit(object["children"].as_object_mut().unwrap());
        }
    }
    visit(tree["objects"].as_object_mut().unwrap());
}

struct Views {
    authored: Vec<u8>,
    editing: Value,
    execution: Value,
    execution_hash: String,
}

/// Normalize one source through both public normalizers and assert that
/// they agree byte for byte once object labels are removed.
fn views(name: &str, source: &str) -> Views {
    let parsed = maac::parse(source).unwrap_or_else(|e| panic!("{name} parses: {e:?}"));
    let plan = maac::compile(&parsed).unwrap_or_else(|e| panic!("{name} compiles: {e:?}"));
    let identity = execution_identity(&parsed, &plan)
        .unwrap_or_else(|e| panic!("{name} has an execution identity: {e:?}"));
    let authored = AuthoredDocument::from_document(&parsed).unwrap();
    let before = authored.canonical_bytes().to_vec();
    let mut editing = FoundationEditContext
        .normalize_document(&authored)
        .unwrap_or_else(|e| panic!("{name} normalizes for editing: {e:?}"));
    assert_eq!(authored.canonical_bytes(), before, "{name}: A was mutated");
    let labelled = editing.clone();
    strip_object_labels(&mut editing);
    let execution: Value = serde_json::from_str(&identity.normalized_source_json).unwrap();
    assert_eq!(
        editing, execution,
        "{name}: editing N(A) and execution projection disagree"
    );
    assert_eq!(
        canonical_json_bytes(&editing).unwrap(),
        identity.normalized_source_json.as_bytes(),
        "{name}: canonical bytes differ"
    );
    Views {
        authored: before,
        editing: labelled,
        execution,
        execution_hash: identity.execution_hash,
    }
}

const HEADER: &str = r#"maac 1;
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 3, 4), (6q, 4, 4)]; }
"#;

/// Every core object kind and every core processor accepted by single-document
/// `compile`, with each optional field omitted. `core.lfo/1`,
/// `core.constant/1`, and `modulate` belong to the bundle artifact profile.
fn omitted() -> String {
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
node out {{ type = "core.matrix/1"; config = {{ inputs = 2; outputs = 2; coefficients = [[1, 0], [0, 1]]; }}; }}
connect c1 {{ from = &tone:out; to = &filter:in; }}
connect c2 {{ from = &filter:out; to = &amp:in; }}
connect c3 {{ from = &amp:out; to = &sum:in; }}
connect c4 {{ from = &noise:out; to = &echo:in; }}
connect c5 {{ from = &echo:out; to = &sum:in; }}
connect c6 {{ from = &sum:out; to = &fader:in; }}
connect c7 {{ from = &fader:out; to = &pan:in; }}
connect c8 {{ from = &pan:out; to = &out:in; }}
curve rise {{ clock = score; points = [(0q, -12dB, linear), (4q, 0dB, step)]; }}
automation fade {{ target = &fader.params.level; curve = &rise; at = 0q; }}
region intro {{ span = [0q, bar(2, 1)]; }}
"#
    )
}

/// The same composition with every specified default written explicitly
/// and with equivalent unit, pitch, constructor, and bar spellings.
fn explicit() -> String {
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
node out {{ type = "core.matrix/1"; config = {{ inputs = 2; outputs = 2; coefficients = [[1, 0], [0, 1]]; }}; }}
connect c1 {{ from = &tone:out; to = &filter:in; }}
connect c2 {{ from = &filter:out; to = &amp:in; }}
connect c3 {{ from = &amp:out; to = &sum:in; }}
connect c4 {{ from = &noise:out; to = &echo:in; }}
connect c5 {{ from = &echo:out; to = &sum:in; }}
connect c6 {{ from = &sum:out; to = &fader:in; }}
connect c7 {{ from = &fader:out; to = &pan:in; }}
connect c8 {{ from = &pan:out; to = &out:in; }}
curve rise {{ clock = score; points = [(0q, -12dB, linear), (4q, 0dB, step)]; }}
automation fade {{ target = &fader.params.level; curve = &rise; at = 0q; }}
region intro {{ span = [0q, 3q]; }}
"#
    )
}

#[test]
fn every_core_kind_and_processor_normalizes_identically_in_both_views() {
    let omitted = views("omitted", &omitted());
    let explicit = views("explicit", &explicit());
    assert_ne!(omitted.authored, explicit.authored);
    assert_eq!(omitted.execution, explicit.execution);
    assert_eq!(omitted.editing, explicit.editing);
    assert_eq!(omitted.execution_hash, explicit.execution_hash);
}

#[test]
fn defaults_are_expanded_into_the_normalized_view() {
    let normalized = views("omitted", &omitted()).execution;
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
    for (object, children) in [(&objects["first"], false), (&objects["outer"], true)] {
        let fields = if children {
            &object["children"]["inner"]["fields"]
        } else {
            &object["fields"]
        };
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
        serde_json::json!({"t":"quantity","n":"3","d":"1","u":"q"})
    );
}

#[test]
fn non_default_values_are_not_collapsed_into_defaults() {
    let base = explicit();
    let reference = views("explicit", &base).execution_hash;
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
    ] {
        assert!(base.contains(from), "vector anchor {from:?}");
        let changed = base.replacen(from, to, 1);
        let hash = views(to, &changed).execution_hash;
        assert_ne!(hash, reference, "{from:?} -> {to:?} must change N(A)");
    }
}

#[test]
fn checked_in_foundation_examples_agree_across_normalizers() {
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
        let Ok(authored) = AuthoredDocument::from_document(&parsed) else {
            continue;
        };
        // Only examples inside the Foundation editing profile are compared;
        // the rest are covered by their own bundle/production contexts.
        if FoundationEditContext.normalize_document(&authored).is_err()
            || maac::compile(&parsed).is_err()
        {
            continue;
        }
        views(&path.display().to_string(), &source);
        checked.push(path.file_name().unwrap().to_string_lossy().into_owned());
    }
    checked.sort();
    for expected in [
        "core-delay.maac",
        "core-matrix.maac",
        "core-noise.maac",
        "gain-expression.maac",
        "pitch-expression.maac",
    ] {
        assert!(checked.iter().any(|name| name == expected), "{checked:?}");
    }
}
