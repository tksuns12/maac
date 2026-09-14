use std::collections::BTreeMap;
use std::fmt::Write;
use std::io::Cursor;

use maac::bundle::{
    sha256_digest, SourceBundle, MAX_BUNDLE_FILE_BYTES, MAX_IMPORT_DEPTH, MAX_SYNTAX_OBJECTS,
};
use maac::plan::EventKind;
use maac::{compile_bundle, DiagnosticCode, ModuleArtifact, ModuleArtifactLimits, Rational};

fn library_bundle() -> SourceBundle {
    let leaf = r#"maac 1;
library leaf { version = "1"; }
tuning fifths { period = 1200ct; steps = [0ct, 700ct]; reference_index = 0; reference_frequency = 220Hz; }
curve bend { clock = normalized; points = [(0, 0ct, linear), (1, 100ct, step)]; }
pattern leaf_note {
  length = 1q;
  note tone { at = 0q; dur = 1/2q; pitch = degree(1, &fifths); expression pitch_bend { kind = pitch; curve = &bend; } }
}
"#;
    let leaf_pin = sha256_digest(leaf.as_bytes());
    let root = format!(
        r#"maac 1;
library phrases {{ version = "1"; }}
import notes {{ path = "deps/leaf.maac"; hash = "{leaf_pin}"; }}
curve motion {{ clock = score; points = [(0q, 1/4, linear), (1q, 3/4, step)]; }}
pattern phrase {{ length = 1q; use nested {{ pattern = &notes.leaf_note; at = 0q; }} }}
"#
    );
    SourceBundle {
        entry: "lib/root.maac".into(),
        sources: BTreeMap::from([
            ("lib/root.maac".into(), root),
            ("lib/deps/leaf.maac".into(), leaf.into()),
        ]),
        assets: BTreeMap::new(),
    }
}

fn caller_source(root_pin: &str, bpm: u32, at: u32) -> String {
    format!(
        r#"maac 1;
project song {{ score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, {bpm}bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import music {{ path = "../lib/root.maac"; hash = "{root_pin}"; }}
node synth {{ type = "core.sine/1"; }}
track notes {{ target = &synth:events; }}
place play {{ pattern = &music.phrase; track = &notes; at = {at}q; }}
automation move {{ target = &synth.params.level; curve = &music.motion; at = 0q; }}
"#
    )
}

fn caller_bundle(mut library: SourceBundle, bpm: u32, at: u32) -> SourceBundle {
    let root_pin = sha256_digest(library.sources["lib/root.maac"].as_bytes());
    library
        .sources
        .insert("scores/main.maac".into(), caller_source(&root_pin, bpm, at));
    library.entry = "scores/main.maac".into();
    library
}

fn first_code(error: &maac::Diagnostics) -> DiagnosticCode {
    error.first().expect("a diagnostic").code
}

fn mono_wav(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }
    bytes.into_inner()
}

fn source_record(path: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "hash": sha256_digest(text.as_bytes()),
        "builtin": false,
        "text": text,
    })
}

fn asset_bundle() -> (SourceBundle, Vec<u8>) {
    let wav = mono_wav(&[0, 100, 200, 300, 0, -100, -200, -300]);
    let wav_pin = sha256_digest(&wav);
    let source = format!(
        r#"maac 1;
library complete {{ version = "1"; }}
import basic {{ builtin = "std/basic/1.0.0"; }}
wavetable wave {{ path = "assets/wave.wav"; hash = "{wav_pin}"; cycle_length = 8; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = C4; }} }}
"#
    );
    (
        SourceBundle {
            entry: "root.maac".into(),
            sources: BTreeMap::from([("root.maac".into(), source)]),
            assets: BTreeMap::from([("assets/wave.wav".into(), wav.clone())]),
        },
        wav,
    )
}

#[test]
fn canonical_json_roundtrip_digest_and_exports_are_derived() {
    let artifact = ModuleArtifact::from_source_bundle(&library_bundle()).unwrap();
    artifact.validate().unwrap();
    let bytes = artifact.to_json().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(value["format"], "maac.module-source");
    assert_eq!(value["version"], 1);
    assert!(value.get("hash").is_none());
    assert!(value.get("dependencies").is_none());
    assert!(value.get("exports").is_none());
    assert_eq!(artifact.digest().unwrap(), sha256_digest(&bytes));
    assert_eq!(
        artifact
            .exports()
            .iter()
            .map(|export| (export.kind(), export.name()))
            .collect::<Vec<_>>(),
        vec![("curve", "motion"), ("pattern", "phrase")]
    );

    let decoded = ModuleArtifact::from_json(&bytes).unwrap();
    assert_eq!(decoded.to_json().unwrap(), bytes);
    assert_eq!(decoded.digest().unwrap(), artifact.digest().unwrap());
    assert_eq!(decoded.to_source_bundle().unwrap(), library_bundle());
}

#[test]
fn restored_sources_compile_two_callers_without_capturing_context_or_changing_plan_json() {
    let original = library_bundle();
    let artifact = ModuleArtifact::from_source_bundle(&original).unwrap();
    let restored = ModuleArtifact::from_json(&artifact.to_json().unwrap())
        .unwrap()
        .to_source_bundle()
        .unwrap();

    let original_plan = compile_bundle(&caller_bundle(original, 120, 0)).unwrap();
    let fast = compile_bundle(&caller_bundle(restored.clone(), 120, 0)).unwrap();
    let slow = compile_bundle(&caller_bundle(restored, 60, 2)).unwrap();
    assert_eq!(fast.to_json().unwrap(), original_plan.to_json().unwrap());
    assert_eq!(fast.events[0].address, "play/0/nested/0/tone");
    assert_eq!(slow.events[0].address, "play/0/nested/0/tone");
    assert_eq!(
        fast.events[0].source.path,
        vec!["music", "notes", "leaf_note", "tone"]
    );
    assert_eq!(slow.events[0].source.path, fast.events[0].source.path);
    assert_eq!(fast.events[0].score_on_q, Rational::from_integer(0.into()));
    assert_eq!(slow.events[0].score_on_q, Rational::from_integer(2.into()));
    assert_eq!(fast.events[0].on_seconds, Rational::from_integer(0.into()));
    assert_eq!(slow.events[0].on_seconds, Rational::from_integer(2.into()));
    let fast_pitch = match fast.events[0].kind {
        EventKind::Note { pitch_hz, .. } => pitch_hz,
        ref other => panic!("expected note, got {other:?}"),
    };
    let slow_pitch = match slow.events[0].kind {
        EventKind::Note { pitch_hz, .. } => pitch_hz,
        ref other => panic!("expected note, got {other:?}"),
    };
    assert_eq!(fast_pitch, slow_pitch);
}

#[test]
fn strict_wire_rejects_tampering_missing_extra_and_malformed_members() {
    let bytes = ModuleArtifact::from_source_bundle(&library_bundle())
        .unwrap()
        .to_json()
        .unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut cases = Vec::new();

    let mut altered_source = original.clone();
    altered_source["sources"][0]["text"] = "maac 1;".into();
    cases.push((altered_source, DiagnosticCode::Hash));
    let mut altered_hash = original.clone();
    altered_hash["sources"][0]["hash"] = format!("sha256:{}", "0".repeat(64)).into();
    cases.push((altered_hash, DiagnosticCode::Hash));
    let mut missing = original.clone();
    missing["sources"].as_array_mut().unwrap().pop();
    cases.push((missing, DiagnosticCode::Reference));
    let mut extra = original.clone();
    let text = "maac 1; library extra { version = \"1\"; }";
    extra["sources"].as_array_mut().unwrap().push(serde_json::json!({
        "path": "extra.maac", "hash": sha256_digest(text.as_bytes()), "builtin": false, "text": text
    }));
    cases.push((extra, DiagnosticCode::Reference));
    let mut bad_version = original.clone();
    bad_version["version"] = 2.into();
    cases.push((bad_version, DiagnosticCode::Version));
    let mut bad_format = original.clone();
    bad_format["format"] = "maac.plan".into();
    cases.push((bad_format, DiagnosticCode::Version));
    let mut bad_path = original.clone();
    bad_path["sources"][0]["path"] = "../escape.maac".into();
    cases.push((bad_path, DiagnosticCode::Reference));
    let mut malformed_hex = original.clone();
    malformed_hex["assets"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": "bad.bin", "hash": format!("sha256:{}", "0".repeat(64)), "bytes": "0xz"
        }));
    cases.push((malformed_hex, DiagnosticCode::Syntax));

    for (case, code) in cases {
        assert_eq!(
            first_code(
                &ModuleArtifact::from_json(&serde_json::to_vec(&case).unwrap()).unwrap_err()
            ),
            code
        );
    }

    let unknown = String::from_utf8(bytes.clone()).unwrap().replacen(
        "\"version\":1",
        "\"unknown\":0,\"version\":1",
        1,
    );
    assert_eq!(
        first_code(&ModuleArtifact::from_json(unknown.as_bytes()).unwrap_err()),
        DiagnosticCode::UnknownField
    );
    let duplicate = String::from_utf8(bytes).unwrap().replacen(
        "\"version\":1",
        "\"version\":1,\"version\":1",
        1,
    );
    assert_eq!(
        first_code(&ModuleArtifact::from_json(duplicate.as_bytes()).unwrap_err()),
        DiagnosticCode::DuplicateField
    );
    assert_eq!(
        first_code(
            &ModuleArtifact::from_json(
                br#"{"format":"maac.module-source","version":1,"entry":"\ud800","sources":[],"assets":[]}"#
            )
            .unwrap_err()
        ),
        DiagnosticCode::Syntax
    );
}

#[test]
fn builtin_and_asset_closure_roundtrip_exactly_without_unpacking_builtin_files() {
    let (bundle, wav) = asset_bundle();
    let artifact = ModuleArtifact::from_source_bundle(&bundle).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    assert!(json["sources"].as_array().unwrap().iter().any(|source| {
        source["path"] == maac::stdlib::BASIC_SOURCE_PATH
            && source["builtin"] == true
            && source["text"] == maac::stdlib::BASIC_SOURCE
    }));
    let restored = artifact.to_source_bundle().unwrap();
    assert_eq!(restored, bundle);
    assert!(!restored
        .sources
        .keys()
        .any(|path| path.starts_with("@builtin/")));
    assert_eq!(restored.assets["assets/wave.wav"], wav);
}

#[test]
fn missing_extra_and_altered_assets_are_rejected() {
    let (bundle, _) = asset_bundle();
    let artifact = ModuleArtifact::from_source_bundle(&bundle).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();

    let mut missing = original.clone();
    missing["assets"].as_array_mut().unwrap().clear();
    assert_eq!(
        first_code(&ModuleArtifact::from_json(&serde_json::to_vec(&missing).unwrap()).unwrap_err()),
        DiagnosticCode::Asset
    );

    let mut extra = original.clone();
    extra["assets"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": "assets/extra.bin", "hash": sha256_digest(&[0]), "bytes": "00"
        }));
    assert_eq!(
        first_code(&ModuleArtifact::from_json(&serde_json::to_vec(&extra).unwrap()).unwrap_err()),
        DiagnosticCode::Asset
    );

    let mut altered = original;
    altered["assets"][0]["bytes"] = "00".into();
    assert_eq!(
        first_code(&ModuleArtifact::from_json(&serde_json::to_vec(&altered).unwrap()).unwrap_err()),
        DiagnosticCode::Hash
    );
}

#[test]
fn module_decode_reuses_cycle_depth_object_and_source_byte_limits() {
    let template: serde_json::Value = serde_json::from_slice(
        &ModuleArtifact::from_source_bundle(&library_bundle())
            .unwrap()
            .to_json()
            .unwrap(),
    )
    .unwrap();
    let zero_pin = format!("sha256:{}", "0".repeat(64));

    let a = format!(
        "maac 1; library a {{ version = \"1\"; }} pattern p {{ length = 1q; }} import b {{ path = \"b.maac\"; hash = \"{zero_pin}\"; }}"
    );
    let b = format!(
        "maac 1; library b {{ version = \"1\"; }} import a {{ path = \"a.maac\"; hash = \"{zero_pin}\"; }}"
    );
    let mut cycle = template.clone();
    cycle["entry"] = "a.maac".into();
    cycle["sources"] = vec![source_record("a.maac", &a), source_record("b.maac", &b)].into();
    assert_eq!(
        first_code(&ModuleArtifact::from_json(&serde_json::to_vec(&cycle).unwrap()).unwrap_err()),
        DiagnosticCode::Reference
    );

    let mut depth_sources = Vec::new();
    for index in 0..=MAX_IMPORT_DEPTH + 1 {
        let import = if index <= MAX_IMPORT_DEPTH {
            format!(
                " import next {{ path = \"s{}.maac\"; hash = \"{zero_pin}\"; }}",
                index + 1
            )
        } else {
            String::new()
        };
        let musical = if index == 0 {
            " pattern p { length = 1q; }"
        } else {
            ""
        };
        let source = format!("maac 1; library l{index} {{ version = \"1\"; }}{musical}{import}");
        depth_sources.push(source_record(&format!("s{index}.maac"), &source));
    }
    let mut depth = template.clone();
    depth["entry"] = "s0.maac".into();
    depth["sources"] = depth_sources.into();
    assert_eq!(
        first_code(&ModuleArtifact::from_json(&serde_json::to_vec(&depth).unwrap()).unwrap_err()),
        DiagnosticCode::ResourceLimit
    );

    let oversized = format!("maac 1;{}", " ".repeat(MAX_BUNDLE_FILE_BYTES));
    let mut bytes = template.clone();
    bytes["entry"] = "large.maac".into();
    bytes["sources"] = vec![source_record("large.maac", &oversized)].into();
    assert_eq!(
        first_code(&ModuleArtifact::from_json(&serde_json::to_vec(&bytes).unwrap()).unwrap_err()),
        DiagnosticCode::ResourceLimit
    );

    let mut objects =
        String::from("maac 1; library many { version = \"1\"; } pattern p { length = 1q; }");
    for index in 0..MAX_SYNTAX_OBJECTS {
        write!(&mut objects, " node n{index} {{}}").unwrap();
    }
    assert!(objects.len() < MAX_BUNDLE_FILE_BYTES);
    let mut object_limit = template;
    object_limit["entry"] = "objects.maac".into();
    object_limit["sources"] = vec![source_record("objects.maac", &objects)].into();
    assert_eq!(
        first_code(
            &ModuleArtifact::from_json(&serde_json::to_vec(&object_limit).unwrap()).unwrap_err()
        ),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn composition_roots_and_explicit_json_byte_limits_are_rejected() {
    let composition = SourceBundle::new(
        "main.maac",
        "maac 1; project song { score = [0q, 1q]; rate = 48000Hz; }",
    );
    assert_eq!(
        first_code(&ModuleArtifact::from_source_bundle(&composition).unwrap_err()),
        DiagnosticCode::Conflict
    );
    let no_musical = SourceBundle::new("main.maac", "maac 1; library empty { version = \"1\"; }");
    assert_eq!(
        first_code(&ModuleArtifact::from_source_bundle(&no_musical).unwrap_err()),
        DiagnosticCode::Conflict
    );

    let artifact = ModuleArtifact::from_source_bundle(&library_bundle()).unwrap();
    let bytes = artifact.to_json().unwrap();
    let tight = ModuleArtifactLimits::new(bytes.len() - 1);
    assert_eq!(
        first_code(&ModuleArtifact::from_json_with_limits(&bytes, &tight).unwrap_err()),
        DiagnosticCode::ResourceLimit
    );
    assert_eq!(
        first_code(&artifact.to_json_with_limits(&tight).unwrap_err()),
        DiagnosticCode::ResourceLimit
    );
}
