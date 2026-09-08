use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle;
use maac::plan::EventKind;
use maac::{render, Plan};

const FRACTIONAL_PITCH_INSTRUMENT: &str = r#"maac 1;
project song { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &lead:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument tone {
  channels = 1;
  voice v {
    channels = 1;
    amplitude = &amp;
    output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; }
  }
}
node lead { instrument = &tone; config = { voices = 1; }; }
pattern phrase { length = 1q; note n { at = 0q; dur = 1/2q; pitch = Bb5; } }
track melody { target = &lead:events; }
place play { pattern = &phrase; track = &melody; at = 0q; }
"#;

const FRACTIONAL_PITCH_LEGACY: &str = r#"maac 1;
project song { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &tone:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node tone { type = "core.sine/1"; config = { voices = 1; }; }
pattern phrase { length = 1q; note n { at = 0q; dur = 1/2q; pitch = Bb5; } }
track melody { target = &tone:events; }
place play { pattern = &phrase; track = &melody; at = 0q; }
"#;

fn pitch_bits(plan: &Plan) -> u64 {
    match plan.events[0].kind {
        EventKind::Note { pitch_hz, .. } => pitch_hz.to_bits(),
        _ => panic!("fixture must compile to a note"),
    }
}

fn rendered_bits(plan: &Plan) -> Vec<u64> {
    let mut bits = Vec::new();
    render(plan, |frame| {
        bits.extend(frame.iter().map(|sample| sample.to_bits()));
        Ok(())
    })
    .unwrap();
    bits
}

fn assert_plan_audio_roundtrip(compiled: &Plan) {
    let loaded = Plan::from_json(&compiled.to_json().unwrap()).unwrap();

    let compiled_pitch = pitch_bits(compiled);
    let loaded_pitch = pitch_bits(&loaded);
    let compiled_audio = rendered_bits(compiled);
    let loaded_audio = rendered_bits(&loaded);
    let first_audio_difference = compiled_audio
        .iter()
        .zip(&loaded_audio)
        .position(|(left, right)| left != right);

    assert!(
        compiled_pitch == loaded_pitch && first_audio_difference.is_none(),
        "JSON reload changed pitch {compiled_pitch:016x} -> {loaded_pitch:016x}; first rendered f64 difference: {first_audio_difference:?}"
    );
}

#[test]
fn v2_json_preserves_fractional_pitch_and_rendered_audio_exactly() {
    let compiled =
        compile_bundle(&SourceBundle::new("main.maac", FRACTIONAL_PITCH_INSTRUMENT)).unwrap();
    assert_eq!(compiled.version, 2);
    assert_plan_audio_roundtrip(&compiled);
}

#[test]
fn v1_json_also_preserves_fractional_pitch_and_rendered_audio_exactly() {
    let document = maac::parse(FRACTIONAL_PITCH_LEGACY).unwrap();
    let compiled = maac::compile(&document).unwrap();
    assert_eq!(compiled.version, 1);
    assert_plan_audio_roundtrip(&compiled);
}
