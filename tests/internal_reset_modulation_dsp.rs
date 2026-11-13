use maac::graph::InstrumentProgram;
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

#[path = "support/programs.rs"]
mod programs;

const RATE: f64 = 48_000.0;

/// A control `phase_bias` for the phase of the shared LFO `target`.
const PHASE_BIAS: &str = "control phase_bias { target = &bus.target.params.phase; default = 1/4; }";

/// An instrument whose shared graph renders the 100 Hz LFO `target` at
/// `target_phase`. With `source_phase`, a held LFO `source` stands beside it
/// at that phase; with `modulation_from`, that node's output moves the phase
/// of `target` by `modulation_depth`.
fn reset_lfo_program(
    target_phase: &str,
    source_phase: Option<&str>,
    modulation_from: Option<&str>,
    modulation_depth: &str,
    controls: &str,
) -> InstrumentProgram {
    let source = source_phase.map_or_else(String::new, |phase| {
        format!(
            r#"node source {{ type = "synth.lfo/1"; params = {{ frequency = 0Hz; phase = {phase}; level = 1; }}; }}"#
        )
    });
    let modulation = modulation_from.map_or_else(String::new, |from| {
        format!(
            "modulate reset_phase {{ from = &{from}:out; to = &target.params.phase; depth = {modulation_depth}; }}"
        )
    });
    programs::program(&format!(
        r#"instrument reset_phase_voice {{
 channels = 1;
 voice note {{
  channels = 1; amplitude = &amp; output = &voice:out;
  node amp {{ type = "synth.adsr/1"; params = {{ attack = 0s; decay = 0s; sustain = 1; release = 0s; }}; }}
  node voice {{ type = "synth.sine/1"; params = {{ ratio = 0; frequency = 0Hz; phase = 1/4; level = 1; }}; }}
 }}
 shared bus {{
  channels = 1; output = &target:out;
  node target {{ type = "synth.lfo/1"; params = {{ frequency = 100Hz; phase = {target_phase}; level = 1; }}; }}
  {source}
  {modulation}
 }}
 {controls}
}}"#
    ))
}

fn runtime(program: &InstrumentProgram, controls: &BTreeMap<String, f64>) -> InstrumentRuntime {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new()).unwrap();
    InstrumentRuntime::new(Arc::new(compiled), 1, RATE, controls).unwrap()
}

fn bits(samples: Vec<f64>) -> Vec<u64> {
    samples.into_iter().map(f64::to_bits).collect()
}

fn render(runtime: &mut InstrumentRuntime, frames: usize) -> Vec<f64> {
    (0..frames as u64)
        .map(|frame| runtime.render(frame).unwrap()[0])
        .collect()
}

fn render_input_sequence(runtime: &mut InstrumentRuntime, frames: usize) -> Vec<f64> {
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    let mut samples = vec![runtime.render(0).unwrap()[0]];
    samples.extend((1..frames as u64).map(|frame| runtime.render(frame).unwrap()[0]));
    samples
}

#[test]
fn shared_lfo_reset_phase_capture_matches_static_reference_and_replays() {
    let captured = reset_lfo_program("0", Some("1/4"), Some("source"), "1/4", "");
    let static_reference = reset_lfo_program("1/4", None, None, "0", "");
    let mut actual = runtime(&captured, &BTreeMap::new());
    let mut expected = runtime(&static_reference, &BTreeMap::new());
    let first = render(&mut actual, 8);
    assert_eq!(bits(first.clone()), bits(render(&mut expected, 8)));

    actual.reset(&BTreeMap::new()).unwrap();
    assert_eq!(bits(first), bits(render(&mut actual, 8)));

    for (target_phase, reference_phase) in [("0", "0"), ("1", "0")] {
        let endpoint = reset_lfo_program(target_phase, Some("1/4"), Some("source"), "0", "");
        let reference = reset_lfo_program(reference_phase, None, None, "0", "");
        assert_eq!(
            bits(render(&mut runtime(&endpoint, &BTreeMap::new()), 8)),
            bits(render(&mut runtime(&reference, &BTreeMap::new()), 8))
        );
    }
}

#[test]
fn shared_input_phase_captures_silence_before_notes_and_ignores_voice_samples() {
    let instance_controls = [("phase_bias".into(), 3.0 / 4.0)]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let captured = reset_lfo_program("0", None, Some("input"), "1/4", PHASE_BIAS);
    let static_reference = reset_lfo_program("0", None, None, "0", PHASE_BIAS);
    let mut actual = runtime(&captured, &instance_controls);
    let mut expected = runtime(&static_reference, &instance_controls);
    let first = render_input_sequence(&mut actual, 8);
    assert_eq!(
        bits(first.clone()),
        bits(render_input_sequence(&mut expected, 8))
    );
    assert!((first[0] + 1.0).abs() < 1.0e-12);
    assert!(first[1] < -0.99);

    actual.reset(&instance_controls).unwrap();
    expected.reset(&instance_controls).unwrap();
    let replay_actual = render_input_sequence(&mut actual, 8);
    let replay_expected = render_input_sequence(&mut expected, 8);
    assert_eq!(bits(first), bits(replay_actual.clone()));
    assert_eq!(bits(replay_actual), bits(replay_expected));
}
