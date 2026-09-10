use std::io::Cursor;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle;
use maac::graph::ParameterRate;
use maac::{DiagnosticCode, Plan, Rational};

fn source(voice_body: &str) -> SourceBundle {
    source_with_shared(voice_body, "")
}

fn source_with_shared(voice_body: &str, shared_body: &str) -> SourceBundle {
    let shared_graph = if shared_body.is_empty() {
        String::new()
    } else {
        format!(
            r#"shared fx {{
  channels = 1; output = &gain:out;
  node gain {{ type = "synth.gain/1"; config = {{ channels = 1; }}; }}
  node lfo {{ type = "synth.lfo/1"; }}
  connect input_gain {{ from = &input:out; to = &gain:in; }}
  {shared_body}
 }}"#,
            shared_body = shared_body,
        )
    };
    SourceBundle::new(
        "main.maac",
        format!(
            r#"maac 1;
project song {{ score = [0q, 1q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument local {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; }}
  node osc {{ type = "synth.sine/1"; }}
  node source {{ type = "synth.sine/1"; }}
  {voice_body}
 }}
 {shared_graph}
}}
node sound {{ instrument = &local; config = {{ voices = 1; }}; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = C4; }} }}
track notes {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#,
            voice_body = voice_body,
            shared_graph = shared_graph,
        ),
    )
}

fn phase_source() -> SourceBundle {
    let wav = mono_wav(&[0, 16_384, 0, -16_384, 0, 16_384, 0, -16_384]);
    let hash = sha256_digest(&wav);
    let mut bundle = SourceBundle::new(
        "main.maac",
        format!(
            r#"maac 1;
project song {{ score = [0q, 1q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
wavetable cycle {{ path = "cycle.wav"; hash = "{hash}"; cycle_length = 8; }}
instrument local {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &sine:out;
  node amp {{ type = "synth.adsr/1"; }}
  node sine {{ type = "synth.sine/1"; }}
  node saw {{ type = "synth.saw/1"; }}
  node square {{ type = "synth.square/1"; }}
  node triangle {{ type = "synth.triangle/1"; }}
  node wavetable {{ type = "synth.wavetable/1"; config = {{ table = &cycle; }}; }}
  node lfo {{ type = "synth.lfo/1"; }}
  node source {{ type = "synth.sine/1"; }}
  modulate lfo_phase {{ from = &source:out; to = &lfo.params.phase; depth = 0; }}
  modulate saw_phase {{ from = &source:out; to = &saw.params.phase; depth = 0; }}
  modulate sine_phase {{ from = &source:out; to = &sine.params.phase; depth = -1/2; }}
  modulate square_phase {{ from = &source:out; to = &square.params.phase; depth = 0; }}
  modulate triangle_phase {{ from = &source:out; to = &triangle.params.phase; depth = 0; }}
  modulate wavetable_phase {{ from = &source:out; to = &wavetable.params.phase; depth = 0; }}
 }}
}}
node sound {{ instrument = &local; config = {{ voices = 1; }}; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = C4; }} }}
track notes {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#,
            hash = hash,
        ),
    );
    bundle.assets.insert("cycle.wav".into(), wav);
    bundle
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

fn diagnostic_code(source: &SourceBundle) -> DiagnosticCode {
    compile_bundle(source)
        .expect_err("source should be rejected")
        .first()
        .expect("diagnostic")
        .code
}

#[test]
fn internal_adsr_event_rates_compile_and_roundtrip_as_v2_program_data() {
    let body = r#"
modulate attack_zero { from = &source:out; to = &amp.params.attack; depth = 0s; }
modulate decay_signed { from = &source:out; to = &amp.params.decay; depth = -1/2s; }
modulate sustain_zero { from = &source:out; to = &amp.params.sustain; depth = 0; }
modulate release_signed { from = &source:out; to = &amp.params.release; depth = -1/2s; }
"#;
    let plan = compile_bundle(&source(body)).expect("internal ADSR modulation should compile");
    assert_eq!(plan.version, 2);
    plan.validate().unwrap();

    let program = plan
        .instrument_program("program_0")
        .expect("embedded instrument program");
    assert_eq!(program.voice.modulations.len(), 4);
    for (modulation, (parameter, rate, depth)) in program.voice.modulations.iter().zip([
        ("attack", ParameterRate::NoteOn, rat(0, 1)),
        ("decay", ParameterRate::NoteOn, rat(-1, 2)),
        ("release", ParameterRate::NoteOff, rat(-1, 2)),
        ("sustain", ParameterRate::NoteOn, rat(0, 1)),
    ]) {
        assert_eq!(modulation.to.parameter, parameter);
        assert_eq!(modulation.depth, depth);
        assert_eq!(
            maac::graph::parameter_descriptor_for_stage(
                &program.voice.nodes[0].processor,
                parameter,
                maac::graph::GraphStage::Voice,
            )
            .unwrap()
            .rate,
            rate
        );
    }

    let bytes = plan.to_json().unwrap();
    let encoded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(encoded["version"], serde_json::json!(2));
    assert!(encoded.get("modulations").is_none());
    assert_eq!(
        encoded["instruments"]["programs"][0]["voice"]["modulations"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let retained = Plan::from_json(&bytes).unwrap();
    retained.validate().unwrap();
    assert_eq!(retained, plan);
    assert_eq!(
        retained
            .instrument_program("program_0")
            .unwrap()
            .voice
            .modulations
            .len(),
        4
    );
}

#[test]
fn internal_voice_phase_rates_compile_for_every_supported_processor_and_roundtrip() {
    let plan = compile_bundle(&phase_source()).expect("voice phase modulation should compile");
    assert_eq!(plan.version, 2);
    let program = plan
        .instrument_program("program_0")
        .expect("embedded instrument program");
    assert_eq!(program.voice.modulations.len(), 6);
    let mut targets = program
        .voice
        .modulations
        .iter()
        .map(|modulation| {
            (
                modulation.to.node.as_str(),
                modulation.to.parameter.as_str(),
            )
        })
        .collect::<Vec<_>>();
    targets.sort_unstable();
    assert_eq!(
        targets,
        [
            ("lfo", "phase"),
            ("saw", "phase"),
            ("sine", "phase"),
            ("square", "phase"),
            ("triangle", "phase"),
            ("wavetable", "phase"),
        ]
    );
    assert!(program.voice.nodes.iter().any(|node| matches!(
        node.processor,
        maac::graph::GraphProcessor::Wavetable { .. }
    )));

    let bytes = plan.to_json().unwrap();
    let encoded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(encoded["version"], serde_json::json!(2));
    assert!(encoded.get("modulations").is_none());
    assert_eq!(
        encoded["instruments"]["wavetables"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let retained = Plan::from_json(&bytes).unwrap();
    retained.validate().unwrap();
    assert_eq!(retained, plan);
}

#[test]
fn shared_lfo_reset_modulation_compiles_and_roundtrips_as_v2_program_data() {
    let reset = source_with_shared(
        "",
        r#"modulate reset { from = &input:out; to = &lfo.params.phase; depth = 1/2; }"#,
    );
    let plan = compile_bundle(&reset).expect("shared LFO reset modulation should compile");
    assert_eq!(plan.version, 2);
    let program = plan
        .instrument_program("program_0")
        .expect("embedded instrument program");
    let shared = program.shared.as_ref().expect("shared graph");
    assert_eq!(shared.modulations.len(), 1);
    assert_eq!(shared.modulations[0].to.node, "lfo");
    assert_eq!(shared.modulations[0].to.parameter, "phase");
    assert_eq!(shared.modulations[0].depth, rat(1, 2));

    let bytes = plan.to_json().unwrap();
    let retained = Plan::from_json(&bytes).unwrap();
    retained.validate().unwrap();
    assert_eq!(retained, plan);
}

#[test]
fn invalid_internal_event_modulations_keep_units_ports_references_and_dags_strict() {
    let cases = [
        (
            r#"modulate wrong_unit { from = &source:out; to = &amp.params.sustain; depth = 1s; }"#,
            DiagnosticCode::Unit,
        ),
        (
            r#"modulate wrong_phase_unit { from = &source:out; to = &osc.params.phase; depth = 1s; }"#,
            DiagnosticCode::Unit,
        ),
        (
            r#"modulate wrong_port { from = &source:in; to = &amp.params.attack; depth = 0s; }"#,
            DiagnosticCode::PortType,
        ),
        (
            r#"modulate missing_source { from = &missing:out; to = &amp.params.attack; depth = 0s; }"#,
            DiagnosticCode::Reference,
        ),
        (
            r#"node stereo { type = "synth.pan/1"; }
connect stereo_input { from = &source:out; to = &stereo:in; }
modulate stereo_source { from = &stereo:out; to = &amp.params.attack; depth = 0s; }"#,
            DiagnosticCode::PortType,
        ),
        (
            r#"modulate self_cycle { from = &source:out; to = &source.params.frequency; depth = 1Hz; }"#,
            DiagnosticCode::AlgebraicLoop,
        ),
        (
            r#"modulate phase_self_cycle { from = &source:out; to = &source.params.phase; depth = 0; }"#,
            DiagnosticCode::AlgebraicLoop,
        ),
        (
            r#"modulate source_amp { from = &source:out; to = &amp.params.attack; depth = 0s; }
modulate amp_source { from = &amp:out; to = &source.params.frequency; depth = 1Hz; }"#,
            DiagnosticCode::AlgebraicLoop,
        ),
        (
            r#"modulate source_phase { from = &source:out; to = &osc.params.phase; depth = 0; }
modulate osc_source { from = &osc:out; to = &source.params.frequency; depth = 1Hz; }"#,
            DiagnosticCode::AlgebraicLoop,
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(diagnostic_code(&source(body)), expected, "{body}");
    }

    let wrong_unit_reset = source_with_shared(
        "",
        r#"modulate wrong_reset_unit { from = &input:out; to = &lfo.params.phase; depth = 1s; }"#,
    );
    assert_eq!(diagnostic_code(&wrong_unit_reset), DiagnosticCode::Unit);

    let stereo_reset = source_with_shared(
        "",
        r#"node pan { type = "synth.pan/1"; }
connect input_pan { from = &input:out; to = &pan:in; }
modulate stereo_reset { from = &pan:out; to = &lfo.params.phase; depth = 0; }"#,
    );
    assert_eq!(diagnostic_code(&stereo_reset), DiagnosticCode::PortType);

    let self_reset = source_with_shared(
        "",
        r#"modulate self_reset { from = &lfo:out; to = &lfo.params.phase; depth = 0; }"#,
    );
    assert_eq!(diagnostic_code(&self_reset), DiagnosticCode::AlgebraicLoop);

    let mixed_reset = source_with_shared(
        "",
        r#"node filter { type = "synth.onepole/1"; config = { channels = 1; }; }
connect lfo_filter { from = &lfo:out; to = &filter:in; }
modulate filter_reset { from = &filter:out; to = &lfo.params.phase; depth = 0; }"#,
    );
    assert_eq!(diagnostic_code(&mixed_reset), DiagnosticCode::AlgebraicLoop);
}

fn rat(numerator: i64, denominator: i64) -> Rational {
    Rational::new(numerator.into(), denominator.into())
}
