use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::{DspEngine, RenderError};
use maac::PlanArtifact;

const RATE: u64 = 48_000;

fn frame_q(frame: u64) -> String {
    format!("{frame}/{RATE}q")
}

fn source(varying: bool, invalid_later: bool) -> SourceBundle {
    let (source_nodes, source_modulations, automation) = if varying || invalid_later {
        let later = if invalid_later { "2/1" } else { "0/1" };
        (
            r#"
node source { type = "core.constant/1"; params = { value = 1/4; }; }
node bridge { type = "core.constant/1"; params = { value = 0; }; }
"#,
            r#"
modulate source_bridge { from = &source:out; target = &bridge.params.value; amount = 1; }
modulate bridge_phase { from = &bridge:out; target = &sound.params.phase; amount = 1; }
"#,
            format!(
                "curve source_values {{ clock = score; points = [(0q, 1/4, step), ({}, {}, step)]; }}\nautomation source_lane {{ target = &source.params.value; curve = &source_values; at = 0q; }}\n",
                frame_q(2),
                later
            ),
        )
    } else {
        ("", "", "".into())
    };
    let phase = if varying || invalid_later { "0" } else { "1/4" };
    let text = format!(
        r#"maac 1;
project reset_rate {{ score = [0q, 1/12000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument local {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; params = {{ attack = 0s; decay = 0s; sustain = 1; release = 0s; }}; }}
  node osc {{ type = "synth.sine/1"; params = {{ frequency = 0Hz; phase = 1/4; level = 1; }}; }}
 }}
 shared fx {{
  channels = 1; output = &lfo:out;
  node lfo {{ type = "synth.lfo/1"; params = {{ frequency = 100Hz; phase = 0; level = 1; }}; }}
 }}
 control phase {{ target = &fx.lfo.params.phase; default = 0; }}
}}
node sound {{ instrument = &local; params = {{ phase = {phase}; }}; config = {{ voices = 1; }}; }}
{source_nodes}{source_modulations}{automation}
"#,
        phase = phase,
        source_nodes = source_nodes,
        source_modulations = source_modulations,
        automation = automation,
    );
    SourceBundle::new("reset-rate.maac", text)
}

fn artifact(bundle: SourceBundle) -> PlanArtifact {
    let compiled = compile_bundle_artifact(&bundle).expect("reset-rate source should compile");
    PlanArtifact::from_json(&compiled.to_json().unwrap()).unwrap()
}

fn render(artifact: &PlanArtifact) -> Result<Vec<f64>, RenderError> {
    let mut engine = DspEngine::new_artifact(artifact)?;
    let mut samples = Vec::new();
    engine.render(|frame| {
        samples.push(frame[0]);
        Ok(())
    })?;
    Ok(samples)
}

#[test]
fn automated_chained_constant_captures_shared_lfo_phase_at_reset() {
    let dynamic = artifact(source(true, false));
    let static_reference = artifact(source(false, false));
    let mut engine = DspEngine::new_artifact(&dynamic).unwrap();
    let mut first = Vec::new();
    engine
        .render(|frame| {
            first.push(frame[0]);
            Ok(())
        })
        .unwrap();

    engine.reset();
    let mut repeated = Vec::new();
    engine
        .render(|frame| {
            repeated.push(frame[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(first, repeated);
    assert_eq!(first, render(&static_reference).unwrap());
}

#[test]
fn later_chained_constant_value_reports_reset_phase_range_during_render() {
    let artifact = artifact(source(false, true));
    let mut engine = DspEngine::new_artifact(&artifact).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_RANGE");
    assert_eq!(callbacks, 2);
}
