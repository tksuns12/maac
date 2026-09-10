use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle;
use maac::graph::ParameterRate;
use maac::{DiagnosticCode, Plan, Rational};

fn source(voice_body: &str) -> SourceBundle {
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
}}
node sound {{ instrument = &local; config = {{ voices = 1; }}; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = C4; }} }}
track notes {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#,
            voice_body = voice_body,
        ),
    )
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
fn invalid_internal_event_modulations_keep_units_ports_references_and_dags_strict() {
    let cases = [
        (
            r#"modulate wrong_unit { from = &source:out; to = &amp.params.sustain; depth = 1s; }"#,
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
            r#"modulate source_amp { from = &source:out; to = &amp.params.attack; depth = 0s; }
modulate amp_source { from = &amp:out; to = &source.params.frequency; depth = 1Hz; }"#,
            DiagnosticCode::AlgebraicLoop,
        ),
        (
            r#"modulate phase { from = &source:out; to = &osc.params.phase; depth = 0; }"#,
            DiagnosticCode::PortType,
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(diagnostic_code(&source(body)), expected, "{body}");
    }
}

fn rat(numerator: i64, denominator: i64) -> Rational {
    Rational::new(numerator.into(), denominator.into())
}
