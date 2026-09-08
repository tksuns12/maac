use std::collections::BTreeMap;
use std::io::Cursor;

use maac::bundle::SourceBundle;
use maac::library::LibrarySet;
use maac::{check_bundle, compile_bundle, DiagnosticCode, Rational};

fn rat(value: i64) -> Rational {
    Rational::from_integer(value.into())
}

fn resolve(source: &str) -> Result<LibrarySet, maac::Diagnostics> {
    let bundle = SourceBundle::new("main.maac", source).resolve()?;
    LibrarySet::resolve(&bundle)
}

fn code(error: &maac::Diagnostics) -> DiagnosticCode {
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

#[test]
fn lowers_a_library_instrument_and_preserves_metadata() {
    let libraries = resolve(
        r#"
maac 1;
library studio { version = "1.0.0"; creator = "Example"; license = "Apache-2.0"; }
instrument lead {
  channels = 1;
  voice v {
    channels = 1;
    amplitude = &amp;
    output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; }
  }
  control ratio { target = &v.osc.params.ratio; default = 2; }
}
"#,
    )
    .expect("valid library");

    assert_eq!(libraries.programs.len(), 1);
    assert_eq!(libraries.programs[0].id, "program_0");
    assert_eq!(libraries.programs[0].controls["ratio"].default, rat(2));
    assert_eq!(libraries.metadata.len(), 1);
    assert_eq!(libraries.metadata[0].file, "main.maac");
    assert_eq!(libraries.metadata[0].object, "studio");
    assert_eq!(libraries.metadata[0].version, "1.0.0");
    assert_eq!(libraries.metadata[0].creator.as_deref(), Some("Example"));
    assert_eq!(libraries.metadata[0].license.as_deref(), Some("Apache-2.0"));
}

#[test]
fn applies_default_preset_and_instance_control_precedence_exactly() {
    let source = r#"
maac 1;
project song {}
instrument lead {
  channels = 1;
  voice v {
    channels = 1;
    amplitude = &amp;
    output = &osc:out;
    node amp { type = "synth.adsr/1"; params = { release = 100ms; }; }
    node osc { type = "synth.sine/1"; }
  }
  control release { target = &v.amp.params.release; default = 100ms; }
  control ratio { target = &v.osc.params.ratio; default = 2; }
}
preset soft { instrument = &lead; params = { release = 500ms; }; }
node synth {
  instrument = &lead;
  preset = &soft;
  config = { voices = 7; };
  params = { release = 750ms; };
}
"#;
    let bundle = SourceBundle::new("main.maac", source).resolve().unwrap();
    let libraries = LibrarySet::resolve(&bundle).unwrap();
    let instance = libraries
        .resolve_instance("main.maac", &bundle.documents["main.maac"].objects["synth"])
        .unwrap();

    assert_eq!(instance.program_id, "program_0");
    assert_eq!(instance.channels, 1);
    assert_eq!(instance.voices, 7);
    assert_eq!(
        instance.params["release"],
        Rational::new(3.into(), 4.into())
    );
    assert_eq!(instance.params["ratio"], rat(2));
    assert!(libraries.entry_document().object("lead").is_none());
    assert!(libraries.entry_document().object("soft").is_none());
    assert!(libraries.entry_document().object("synth").is_some());
}

#[test]
fn resolves_direct_imports_and_rejects_imported_compositions() {
    let library_source = r#"
maac 1;
library sounds { version = "1"; }
instrument lead {
  channels = 1;
  voice v {
    channels = 1;
    amplitude = &amp;
    output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; }
  }
}
"#;
    let pin = maac::bundle::sha256_digest(library_source.as_bytes());
    let entry = format!(
        r#"
maac 1;
project song {{}}
import sounds {{ path = "sounds.maac"; hash = "{pin}"; }}
node lead {{ instrument = &sounds.lead; }}
"#
    );
    let resolved = SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), entry),
            ("sounds.maac".into(), library_source.into()),
        ]),
        assets: BTreeMap::new(),
    }
    .resolve()
    .unwrap();
    let libraries = LibrarySet::resolve(&resolved).unwrap();
    let instance = libraries
        .resolve_instance(
            "main.maac",
            &resolved.documents["main.maac"].objects["lead"],
        )
        .unwrap();
    assert_eq!(instance.program_id, "program_0");
    assert_eq!(instance.voices, 64);

    let imported_project = "maac 1; project wrong {}";
    let bad_pin = maac::bundle::sha256_digest(imported_project.as_bytes());
    let bad_entry = format!(
        "maac 1; project song {{}} import wrong {{ path = \"wrong.maac\"; hash = \"{bad_pin}\"; }}"
    );
    let bad = SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), bad_entry),
            ("wrong.maac".into(), imported_project.into()),
        ]),
        assets: BTreeMap::new(),
    }
    .resolve()
    .unwrap();
    assert_eq!(
        code(&LibrarySet::resolve(&bad).unwrap_err()),
        DiagnosticCode::Reference
    );
}

#[test]
fn validates_unused_exports_and_typed_control_values() {
    let invalid_unused = r#"
maac 1;
library studio { version = "1"; }
instrument broken {
  channels = 1;
  voice v {
    channels = 1;
    amplitude = &amp;
    output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; params = { typo = 1; }; }
  }
}
"#;
    assert_eq!(
        code(&resolve(invalid_unused).unwrap_err()),
        DiagnosticCode::UnknownField
    );

    let wrong_unit = invalid_unused.replace("typo = 1", "frequency = 2s");
    assert_eq!(
        code(&resolve(&wrong_unit).unwrap_err()),
        DiagnosticCode::Unit
    );
    let out_of_range = invalid_unused.replace("typo = 1", "level = 17");
    assert_eq!(
        code(&resolve(&out_of_range).unwrap_err()),
        DiagnosticCode::Range
    );
}

#[test]
fn embeds_wavetable_bytes_and_strict_source_provenance() {
    let wav = mono_wav(&[0, 100, 200, 300, 0, -100, -200, -300]);
    let pin = maac::bundle::sha256_digest(&wav);
    let source = format!(
        r#"
maac 1;
library studio {{ version = "1"; }}
wavetable colors {{ path = "assets/colors.wav"; hash = "{pin}"; cycle_length = 8; }}
instrument lead {{
  channels = 1;
  voice v {{
    channels = 1;
    amplitude = &amp;
    output = &osc:out;
    node amp {{ type = "synth.adsr/1"; }}
    node osc {{ type = "synth.wavetable/1"; config = {{ table = &colors; }}; }}
  }}
}}
"#
    );
    let libraries = LibrarySet::resolve(
        &SourceBundle {
            entry: "main.maac".into(),
            sources: BTreeMap::from([("main.maac".into(), source)]),
            assets: BTreeMap::from([("assets/colors.wav".into(), wav)]),
        }
        .resolve()
        .unwrap(),
    )
    .unwrap();

    assert_eq!(libraries.wavetables[0].id, "table_0");
    assert_eq!(libraries.wavetable_sources[0].table, "table_0");
    assert_eq!(libraries.wavetable_sources[0].path, "assets/colors.wav");
    assert_eq!(libraries.wavetable_sources[0].hash, pin);
    assert_eq!(
        libraries.programs[0].voice.nodes[1].processor,
        maac::graph::GraphProcessor::Wavetable {
            table: "table_0".into()
        }
    );
}

#[test]
fn rejects_unknown_fields_and_mismatched_presets_and_voice_limits() {
    let source = r#"
maac 1;
project song {}
instrument one {
  channels = 1;
  voice v {
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; }
  }
}
instrument two {
  channels = 1;
  voice v {
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; }
  }
}
preset other { instrument = &two; params = {}; }
node selected { instrument = &one; preset = &other; }
"#;
    let bundle = SourceBundle::new("main.maac", source).resolve().unwrap();
    let libraries = LibrarySet::resolve(&bundle).unwrap();
    assert_eq!(
        code(
            &libraries
                .resolve_instance(
                    "main.maac",
                    &bundle.documents["main.maac"].objects["selected"]
                )
                .unwrap_err()
        ),
        DiagnosticCode::Reference
    );

    let too_many =
        maac::parse("maac 1; node selected { instrument = &one; config = { voices = 4097; }; }")
            .unwrap();
    assert_eq!(
        code(
            &libraries
                .resolve_instance("main.maac", &too_many.objects["selected"])
                .unwrap_err()
        ),
        DiagnosticCode::Range
    );
    let unknown = maac::parse("maac 1; node selected { instrument = &one; mystery = 1; }").unwrap();
    assert_eq!(
        code(
            &libraries
                .resolve_instance("main.maac", &unknown.objects["selected"])
                .unwrap_err()
        ),
        DiagnosticCode::UnknownField
    );
}

#[test]
fn lowers_connections_modulation_shared_graphs_and_canonical_units() {
    let libraries = resolve(
        r#"
maac 1;
library studio { version = "1"; }
instrument stereo {
  channels = 2;
  voice v {
    channels = 1;
    amplitude = &amp;
    output = &gain:out;
    node amp { type = "synth.adsr/1"; params = { attack = 2ms; }; }
    node osc { type = "synth.sine/1"; params = { frequency = 1kHz; }; }
    node gain { type = "synth.gain/1"; config = { channels = 1; }; }
    connect audio { from = &osc:out; to = &gain:in; }
    modulate fm { from = &amp:out; to = &osc.params.frequency; depth = 250Hz; }
  }
  shared tail {
    channels = 2;
    output = &pan:out;
    node pan { type = "synth.pan/1"; }
    connect input_pan { from = &input:out; to = &pan:in; }
  }
  control attack { target = &v.amp.params.attack; default = 5ms; }
  control pan { target = &tail.pan.params.pan; default = 0; }
}
"#,
    )
    .unwrap();

    let program = &libraries.programs[0];
    assert_eq!(program.channels(), 2);
    assert_eq!(program.voice.connections.len(), 1);
    assert_eq!(program.voice.modulations[0].depth, rat(250));
    assert_eq!(program.voice.nodes[2].params["frequency"], rat(1_000));
    assert_eq!(
        program.controls["attack"].default,
        Rational::new(1.into(), 200.into())
    );
    assert_eq!(program.shared.as_ref().unwrap().connections.len(), 1);
}

#[test]
fn keeps_transitive_import_aliases_local_to_the_declaring_library() {
    let wav = mono_wav(&[0, 100, 200, 300, 0, -100, -200, -300]);
    let wav_pin = maac::bundle::sha256_digest(&wav);
    let tables = format!(
        r#"
maac 1;
library tables {{ version = "1"; }}
wavetable colors {{ path = "colors.wav"; hash = "{wav_pin}"; cycle_length = 8; }}
"#
    );
    let tables_pin = maac::bundle::sha256_digest(tables.as_bytes());
    let instruments = format!(
        r#"
maac 1;
library instruments {{ version = "1"; }}
import tables {{ path = "tables.maac"; hash = "{tables_pin}"; }}
instrument lead {{
  channels = 1;
  voice v {{
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp {{ type = "synth.adsr/1"; }}
    node osc {{ type = "synth.wavetable/1"; config = {{ table = &tables.colors; }}; }}
  }}
}}
"#
    );
    let instruments_pin = maac::bundle::sha256_digest(instruments.as_bytes());
    let entry = format!(
        r#"
maac 1;
project song {{}}
import sounds {{ path = "instruments.maac"; hash = "{instruments_pin}"; }}
node lead {{ instrument = &sounds.lead; }}
"#
    );
    let bundle = SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), entry),
            ("instruments.maac".into(), instruments),
            ("tables.maac".into(), tables),
        ]),
        assets: BTreeMap::from([("colors.wav".into(), wav)]),
    }
    .resolve()
    .unwrap();
    let libraries = LibrarySet::resolve(&bundle).unwrap();

    assert_eq!(
        libraries
            .resolve_instance("main.maac", &bundle.documents["main.maac"].objects["lead"])
            .unwrap()
            .program_id,
        "program_0"
    );
    assert_eq!(
        libraries.programs[0].voice.nodes[1].processor,
        maac::graph::GraphProcessor::Wavetable {
            table: "table_0".into()
        }
    );
}

#[test]
fn requires_control_defaults_and_rejects_duplicate_control_targets() {
    let missing_default = r#"
maac 1;
library studio { version = "1"; }
instrument lead {
  channels = 1;
  voice v {
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; }
  }
  control ratio { target = &v.osc.params.ratio; }
}
"#;
    assert_eq!(
        code(&resolve(missing_default).unwrap_err()),
        DiagnosticCode::UnknownField
    );

    let duplicate = missing_default.replace(
        "control ratio { target = &v.osc.params.ratio; }",
        "control ratio { target = &v.osc.params.ratio; default = 1; }\n  control ratio_again { target = &v.osc.params.ratio; default = 2; }",
    );
    assert_eq!(
        code(&resolve(&duplicate).unwrap_err()),
        DiagnosticCode::AutomationWriter
    );
}

#[test]
fn empty_optional_library_metadata_survives_check_compile_and_v2_validation() {
    let library = r#"
maac 1;
library studio { version = "1"; creator = ""; license = ""; }
instrument lead {
  channels = 1;
  voice v {
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; params = { level = 0.1; }; }
  }
}
"#;
    let pin = maac::bundle::sha256_digest(library.as_bytes());
    let composition = format!(
        r#"
maac 1;
project song {{
  score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &lead:out;
}}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import sounds {{ path = "studio.maac"; hash = "{pin}"; }}
node lead {{ instrument = &sounds.lead; config = {{ voices = 1; }}; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = C4; }} }}
track notes {{ target = &lead:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#
    );
    let bundle = SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), composition),
            ("studio.maac".into(), library.into()),
        ]),
        assets: BTreeMap::new(),
    };

    check_bundle(&bundle).expect("empty optional metadata is valid source");
    let plan = compile_bundle(&bundle).expect("empty optional metadata embeds in v2");
    plan.validate().expect("embedded metadata validates");
    let bytes = plan.to_json().expect("v2 payload serializes");
    let imported = maac::Plan::from_json(&bytes).expect("v2 payload validates independently");
    let metadata = &imported.instruments.as_ref().unwrap().libraries[0];
    assert_eq!(metadata.creator.as_deref(), Some(""));
    assert_eq!(metadata.license.as_deref(), Some(""));
}

#[test]
fn source_library_metadata_enforces_the_4096_byte_boundary_before_lowering() {
    let boundary = "x".repeat(maac::library::MAX_LIBRARY_METADATA_BYTES);
    resolve(&format!(
        "maac 1; library studio {{ version = \"{boundary}\"; creator = \"{boundary}\"; license = \"{boundary}\"; }}"
    ))
    .expect("4096-byte metadata fields are valid");

    let oversized = "x".repeat(maac::library::MAX_LIBRARY_METADATA_BYTES + 1);
    for declaration in [
        format!("version = \"{oversized}\";"),
        format!("version = \"1\"; creator = \"{oversized}\";"),
        format!("version = \"1\"; license = \"{oversized}\";"),
    ] {
        let error = resolve(&format!("maac 1; library studio {{ {declaration} }}"))
            .expect_err("oversized source metadata must fail");
        assert_eq!(code(&error), DiagnosticCode::ResourceLimit);
    }

    assert_eq!(
        code(&resolve("maac 1; library studio { version = \"\"; }").unwrap_err()),
        DiagnosticCode::Version
    );
}
