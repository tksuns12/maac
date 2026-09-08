use std::collections::BTreeMap;
use std::io::Cursor;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::{check_bundle, compile_bundle};
use maac::plan::Processor;
use maac::{check, compile, load_plan, parse, render, DiagnosticCode, Rational, ValueKind};

fn composition() -> String {
    r#"maac 1;
project song { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &master:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument lead {
  channels = 1;
  voice v {
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; params = { release = 100ms; }; }
    node osc { type = "synth.sine/1"; }
  }
  control attack { target = &v.amp.params.attack; default = 0s; }
  control decay { target = &v.amp.params.decay; default = 0s; }
  control sustain { target = &v.amp.params.sustain; default = 1; }
  control release { target = &v.amp.params.release; default = 100ms; }
  control phase { target = &v.osc.params.phase; default = 0; }
  control ratio { target = &v.osc.params.ratio; default = 1; }
}
preset soft { instrument = &lead; params = { release = 500ms; ratio = 2; }; }
node synth { instrument = &lead; preset = &soft; config = { voices = 7; }; params = { release = 750ms; }; }
node master { type = "core.sum/1"; config = { channels = 1; }; }
connect synth_master { from = &synth:out; to = &master:in; }
pattern phrase { length = 1q; note n { at = 0q; dur = 1/2q; pitch = C4; } }
track melody { target = &synth:events; }
place play { pattern = &phrase; track = &melody; at = 0q; }
"#
    .into()
}

fn code(error: &maac::Diagnostics) -> DiagnosticCode {
    error.first().expect("diagnostic").code
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
        for sample in samples {
            writer.write_sample(*sample).unwrap();
        }
        writer.finalize().unwrap();
    }
    bytes.into_inner()
}

#[test]
fn compiles_local_instrument_instances_to_a_self_contained_v2_plan() {
    let plan = compile_bundle(&SourceBundle::new("main.maac", composition())).unwrap();
    assert_eq!(plan.version, 2);
    let resources = plan.instruments.as_ref().expect("embedded resources");
    assert_eq!(resources.entry_source, "main.maac");
    assert_eq!(resources.programs.len(), 1);
    assert_eq!(resources.source_files.len(), 1);
    let synth = plan.nodes.iter().find(|node| node.id == "synth").unwrap();
    assert_eq!(
        synth.processor,
        Processor::Instrument {
            program: "program_0".into(),
            voices: 7,
            channels: 1,
        }
    );
    assert_eq!(synth.params["release"], Rational::new(3.into(), 4.into()));
    assert_eq!(synth.params["ratio"], Rational::from_integer(2.into()));
    assert_eq!(plan.events[0].target.node, "synth");
}

#[test]
fn dynamic_control_units_and_rates_govern_automation() {
    let sample_rate = composition().replace(
        "place play { pattern = &phrase; track = &melody; at = 0q; }",
        "place play { pattern = &phrase; track = &melody; at = 0q; }\ncurve ratios { clock = score; points = [(0q, 1, linear), (1q, 2, step)]; }\nautomation move { target = &synth.params.ratio; curve = &ratios; at = 0q; }",
    );
    compile_bundle(&SourceBundle::new("main.maac", sample_rate)).unwrap();

    let wrong_unit = composition().replace(
        "place play { pattern = &phrase; track = &melody; at = 0q; }",
        "place play { pattern = &phrase; track = &melody; at = 0q; }\ncurve ratios { clock = score; points = [(0q, 1Hz, linear), (1q, 2Hz, step)]; }\nautomation move { target = &synth.params.ratio; curve = &ratios; at = 0q; }",
    );
    assert_eq!(
        code(&compile_bundle(&SourceBundle::new("main.maac", wrong_unit)).unwrap_err()),
        DiagnosticCode::Unit
    );

    let event_rates = composition().replace(
        "place play { pattern = &phrase; track = &melody; at = 0q; }",
        "place play { pattern = &phrase; track = &melody; at = 0q; }
curve attacks { clock = score; points = [(0q, 0s, step), (1q, 100ms, step)]; }
curve decays { clock = score; points = [(0q, 0s, step), (1q, 100ms, step)]; }
curve sustains { clock = score; points = [(0q, 1, step), (1q, 1/2, step)]; }
curve phases { clock = score; points = [(0q, 0, step), (1q, 1/4, step)]; }
curve releases { clock = score; points = [(0q, 100ms, linear), (1q, 200ms, step)]; }
automation move_attack { target = &synth.params.attack; curve = &attacks; at = 0q; }
automation move_decay { target = &synth.params.decay; curve = &decays; at = 0q; }
automation move_sustain { target = &synth.params.sustain; curve = &sustains; at = 0q; }
automation move_phase { target = &synth.params.phase; curve = &phases; at = 0q; }
automation move_release { target = &synth.params.release; curve = &releases; at = 0q; }",
    );
    compile_bundle(&SourceBundle::new("main.maac", event_rates)).unwrap();

    let private_target = composition().replace(
        "place play { pattern = &phrase; track = &melody; at = 0q; }",
        "place play { pattern = &phrase; track = &melody; at = 0q; }\ncurve ratios { clock = score; points = [(0q, 1, step)]; }\nautomation move { target = &synth.v.osc.params.ratio; curve = &ratios; at = 0q; }",
    );
    assert_eq!(
        code(&compile_bundle(&SourceBundle::new("main.maac", private_target)).unwrap_err()),
        DiagnosticCode::Reference
    );

    let out_of_range = composition().replace(
        "place play { pattern = &phrase; track = &melody; at = 0q; }",
        "place play { pattern = &phrase; track = &melody; at = 0q; }\ncurve ratios { clock = score; points = [(0q, 65, step)]; }\nautomation move { target = &synth.params.ratio; curve = &ratios; at = 0q; }",
    );
    assert_eq!(
        code(&compile_bundle(&SourceBundle::new("main.maac", out_of_range)).unwrap_err()),
        DiagnosticCode::Range
    );
}

fn event_rate_capture_source() -> &'static str {
    r#"maac 1;
project capture { score = [0q, 1/4000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument constant {
  channels = 1;
  voice v {
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; params = { ratio = 0; frequency = 0Hz; phase = 1/4; }; }
  }
  control attack { target = &v.amp.params.attack; default = 1/48000s; }
  control release { target = &v.amp.params.release; default = 1/48000s; }
}
node synth { instrument = &constant; config = { voices = 2; }; }
pattern phrase {
  length = 1/4000q;
  note first { at = 0q; dur = 1/12000q; pitch = C4; }
  note second { at = 1/24000q; dur = 1/12000q; pitch = C4; }
}
track melody { target = &synth:events; }
place play { pattern = &phrase; track = &melody; at = 0q; }
curve attacks { clock = score; points = [(0q, 1/48000s, step), (1/24000q, 0s, step)]; }
automation move_attack { target = &synth.params.attack; curve = &attacks; at = 0q; }
curve releases { clock = score; points = [(0q, 1/48000s, step), (1/12000q, 1/24000s, step), (1/8000q, 0s, step)]; }
automation move_release { target = &synth.params.release; curve = &releases; at = 0q; }
"#
}

fn render_mono(plan: &maac::Plan) -> Vec<f64> {
    let mut samples = Vec::new();
    render(plan, |frame| {
        samples.push(frame[0]);
        Ok(())
    })
    .unwrap();
    samples
}

#[test]
fn source_and_loaded_plan_capture_public_controls_at_note_boundaries() {
    let compiled =
        compile_bundle(&SourceBundle::new("main.maac", event_rate_capture_source())).unwrap();
    let source_samples = render_mono(&compiled);

    let serialized = compiled.to_json().unwrap();
    let loaded = load_plan(&serialized).unwrap();
    let loaded_samples = render_mono(&loaded);

    assert_eq!(source_samples, loaded_samples);
    assert_eq!(source_samples.len(), 6);
    let expected = [0.0, 2.0, 2.0, 0.5, 0.0, 0.0];
    for (frame, (actual, expected)) in source_samples.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1.0e-12,
            "frame {frame}: expected {expected}, got {actual}"
        );
    }
}

#[test]
fn embeds_transitive_program_table_and_source_provenance() {
    let wav = mono_wav(&[0, 100, 200, 300, 0, -100, -200, -300]);
    let wav_pin = sha256_digest(&wav);
    let colors = format!(
        "maac 1; library colors {{ version = \"1\"; }} wavetable basic {{ path = \"assets/basic.wav\"; hash = \"{wav_pin}\"; cycle_length = 8; }}"
    );
    let colors_pin = sha256_digest(colors.as_bytes());
    let sounds = format!(
        r#"maac 1;
library sounds {{ version = "1"; }}
import colors {{ path = "colors.maac"; hash = "{colors_pin}"; }}
instrument lead {{
  channels = 1;
  voice v {{
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp {{ type = "synth.adsr/1"; }}
    node osc {{ type = "synth.wavetable/1"; config = {{ table = &colors.basic; }}; }}
  }}
}}"#
    );
    let sounds_pin = sha256_digest(sounds.as_bytes());
    let entry = format!(
        r#"maac 1;
project song {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &lead:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import sounds {{ path = "lib/sounds.maac"; hash = "{sounds_pin}"; }}
node lead {{ instrument = &sounds.lead; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = C4; }} }}
track melody {{ target = &lead:events; }}
place play {{ pattern = &phrase; track = &melody; at = 0q; }}"#
    );
    let plan = compile_bundle(&SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), entry),
            ("lib/sounds.maac".into(), sounds),
            ("lib/colors.maac".into(), colors),
        ]),
        assets: BTreeMap::from([("lib/assets/basic.wav".into(), wav)]),
    })
    .unwrap();
    let resources = plan.instruments.unwrap();
    assert_eq!(resources.programs.len(), 1);
    assert_eq!(resources.wavetables.len(), 1);
    assert_eq!(resources.wavetable_sources[0].path, "lib/assets/basic.wav");
    assert_eq!(resources.source_files.len(), 3);
    assert_eq!(resources.dependencies.len(), 2);
    assert_eq!(resources.libraries.len(), 2);
}

#[test]
fn instrument_instances_keep_independent_resolved_params() {
    let source = composition().replace(
        "node master { type = \"core.sum/1\"; config = { channels = 1; }; }",
        "node second { instrument = &lead; params = { ratio = 3; }; }\nnode master { type = \"core.sum/1\"; config = { channels = 1; }; }\nconnect second_master { from = &second:out; to = &master:in; }",
    );
    let plan = compile_bundle(&SourceBundle::new("main.maac", source)).unwrap();
    let synth = plan.nodes.iter().find(|node| node.id == "synth").unwrap();
    let second = plan.nodes.iter().find(|node| node.id == "second").unwrap();
    assert_eq!(synth.params["ratio"], Rational::from_integer(2.into()));
    assert_eq!(second.params["ratio"], Rational::from_integer(3.into()));
}

#[test]
fn library_single_document_imports_and_legacy_v1_remain_explicit() {
    let library = parse(
        r#"maac 1; library studio { version = "1"; }
instrument lead { channels = 1; voice v { channels = 1; amplitude = &amp; output = &osc:out; node amp { type = "synth.adsr/1"; } node osc { type = "synth.sine/1"; } } }"#,
    )
    .unwrap();
    check(&library).unwrap();
    assert_eq!(
        code(&compile(&library).unwrap_err()),
        DiagnosticCode::Conflict
    );
    check_bundle(&SourceBundle::new("library.maac", library.source())).unwrap();

    let unresolved = parse(
        "maac 1; import sounds { path = \"sounds.maac\"; hash = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"; }",
    )
    .unwrap();
    assert_eq!(
        code(&check(&unresolved).unwrap_err()),
        DiagnosticCode::Reference
    );

    let legacy = r#"maac 1;
project song { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node synth { type = "core.sine/1"; config = { voices = 7; }; }
pattern phrase { length = 1q; note n { at = 0q; dur = 1/2q; pitch = C4; } }
track melody { target = &synth:events; }
place play { pattern = &phrase; track = &melody; at = 0q; }"#;
    let plan = compile(&parse(legacy).unwrap()).unwrap();
    assert_eq!(plan.version, 1);
    assert!(plan.instruments.is_none());
    let bundle_plan = compile_bundle(&SourceBundle::new("main.maac", legacy)).unwrap();
    assert_eq!(bundle_plan.version, 2);
    assert!(bundle_plan.instruments.is_some());
}

#[test]
fn selected_preset_must_name_the_same_instrument() {
    let source = composition().replace(
        "preset soft { instrument = &lead; params = { release = 500ms; ratio = 2; }; }",
        "instrument other { channels = 1; voice v { channels = 1; amplitude = &amp; output = &osc:out; node amp { type = \"synth.adsr/1\"; } node osc { type = \"synth.sine/1\"; } } }\npreset soft { instrument = &other; params = {}; }",
    );
    assert_eq!(
        code(&compile_bundle(&SourceBundle::new("main.maac", source)).unwrap_err()),
        DiagnosticCode::Reference
    );
}

#[test]
fn standalone_compilation_uses_the_supplied_document_ast() {
    let mut document = parse(&composition()).unwrap();
    let params = &mut document
        .objects
        .get_mut("soft")
        .unwrap()
        .fields
        .get_mut("params")
        .unwrap()
        .value
        .kind;
    let ValueKind::Record(fields) = params else {
        panic!("preset params record");
    };
    fields.get_mut("ratio").unwrap().value.kind =
        ValueKind::Number(Rational::from_integer(4.into()));

    let plan = compile(&document).expect("mutated parsed document should compile");
    let synth = plan.nodes.iter().find(|node| node.id == "synth").unwrap();
    assert_eq!(synth.params["ratio"], Rational::from_integer(4.into()));
}
