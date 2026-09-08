use maac::bundle::SourceBundle;
use maac::dsp::DspEngine;
use maac::{compile_bundle, load_plan, DiagnosticCode, Rational};

fn reset_control_source() -> &'static str {
    r#"maac 1;
project reset_control { score = [0q, 1/24000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &instance_value:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument static_lfo {
  channels = 1;
  voice v {
    channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; params = { ratio = 0; frequency = 0Hz; }; }
  }
  shared fx {
    channels = 1; output = &tone:out;
    node tone { type = "synth.lfo/1"; params = { frequency = 0Hz; }; }
  }
  control phase { target = &fx.tone.params.phase; default = 0; }
}
preset quarter { instrument = &static_lfo; params = { phase = 1/4; }; }
node default_value { instrument = &static_lfo; }
node preset_value { instrument = &static_lfo; preset = &quarter; }
node instance_value { instrument = &static_lfo; preset = &quarter; params = { phase = 3/4; }; }
"#
}

fn render_once(engine: &mut DspEngine<'_>) -> Vec<f64> {
    let mut samples = Vec::new();
    engine
        .render(|frame| {
            samples.push(frame[0]);
            Ok(())
        })
        .unwrap();
    samples
}

#[test]
fn shared_reset_control_obeys_precedence_and_resets_repeatably() {
    let plan = compile_bundle(&SourceBundle::new("main.maac", reset_control_source())).unwrap();

    let default_value = plan
        .nodes
        .iter()
        .find(|node| node.id == "default_value")
        .unwrap();
    let preset_value = plan
        .nodes
        .iter()
        .find(|node| node.id == "preset_value")
        .unwrap();
    let instance_value = plan
        .nodes
        .iter()
        .find(|node| node.id == "instance_value")
        .unwrap();
    assert_eq!(
        default_value.params["phase"],
        Rational::from_integer(0.into())
    );
    assert_eq!(
        preset_value.params["phase"],
        Rational::new(1.into(), 4.into())
    );
    assert_eq!(
        instance_value.params["phase"],
        Rational::new(3.into(), 4.into())
    );

    let mut source_engine = DspEngine::new(&plan).unwrap();
    let first = render_once(&mut source_engine);
    let repeated = render_once(&mut source_engine);
    assert_eq!(first, repeated);
    assert_eq!(first.len(), 1);
    assert!((first[0] + 1.0).abs() < 1.0e-12);

    let loaded = load_plan(&plan.to_json().unwrap()).unwrap();
    let mut loaded_engine = DspEngine::new(&loaded).unwrap();
    assert_eq!(render_once(&mut loaded_engine), first);
    assert_eq!(render_once(&mut loaded_engine), first);
}

#[test]
fn source_automation_rejects_a_reset_rate_control() {
    let source = format!(
        "{}\ncurve phases {{ clock = score; points = [(0q, 0, step), (1/24000q, 1/4, step)]; }}\nautomation move_phase {{ target = &instance_value.params.phase; curve = &phases; at = 0q; }}",
        reset_control_source()
    );
    let error = compile_bundle(&SourceBundle::new("main.maac", source)).unwrap_err();
    assert_eq!(error.first().unwrap().code, DiagnosticCode::Capability);
}

#[test]
fn imported_plan_rejects_an_automation_lane_for_a_reset_rate_control() {
    let plan = compile_bundle(&SourceBundle::new("main.maac", reset_control_source())).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    json["automation"] = serde_json::json!([{
        "id": "illegal_reset_automation",
        "target": {"node": "instance_value", "port": "phase"},
        "clock": "score",
        "at": "0/1",
        "points": [
            {"position": "0/1", "value": "0/1", "shape": "step"},
            {"position": "1/24000", "value": "1/4", "shape": "step"}
        ]
    }]);

    let error = load_plan(&serde_json::to_vec(&json).unwrap()).unwrap_err();
    assert_eq!(error.code, "E_CAPABILITY");
}
