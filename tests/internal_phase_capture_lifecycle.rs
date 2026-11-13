use maac::graph::InstrumentProgram;
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

#[path = "support/programs.rs"]
mod programs;

const RATE: f64 = 48_000.0;

/// A sine `target` whose phase takes a quarter turn from `onset_source` at
/// note-on, and whose own output sets the release of `amp` at note-off.
fn program() -> InstrumentProgram {
    programs::program(
        r#"instrument phase_lifecycle {
 channels = 1;
 voice voice {
  channels = 1; amplitude = &amp; output = &target:out;
  node amp { type = "synth.adsr/1"; params = { attack = 0s; decay = 0s; sustain = 1; release = 0s; }; }
  node onset_source { type = "synth.sine/1"; params = { ratio = 0; frequency = 12000Hz; phase = 1/4; level = 1; }; }
  node target { type = "synth.sine/1"; params = { ratio = 0; frequency = 3000Hz; phase = 0; level = 1; }; }
  modulate on_phase { from = &onset_source:out; to = &target.params.phase; depth = 1/4; }
  modulate off_release { from = &target:out; to = &amp.params.release; depth = 1/12000s; }
 }
}"#,
    )
}

fn lifecycle(runtime: &mut InstrumentRuntime) -> Vec<f64> {
    runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
    let mut samples = vec![runtime.render(0).unwrap()[0], runtime.render(1).unwrap()[0]];
    runtime.note_off("voice", 2).unwrap();
    samples.push(runtime.render(2).unwrap()[0]);
    samples.push(runtime.render(3).unwrap()[0]);
    samples
}

#[test]
fn note_off_uses_live_phase_without_revisiting_its_onset_only_ancestor() {
    let compiled = Arc::new(CompiledInstrument::compile(&program(), &BTreeMap::new()).unwrap());
    let mut runtime = InstrumentRuntime::new(compiled, 1, RATE, &BTreeMap::new()).unwrap();
    let first = lifecycle(&mut runtime);

    let live_off_sample = std::f64::consts::FRAC_1_SQRT_2;
    let release_frames = 4.0 * live_off_sample;
    let expected = [
        1.0,
        (5.0 * std::f64::consts::PI / 8.0).sin(),
        live_off_sample,
        (7.0 * std::f64::consts::PI / 8.0).sin() * (1.0 - 1.0 / release_frames),
    ];
    for (frame, (actual, expected)) in first.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1.0e-12,
            "frame {frame}: {actual} != {expected}"
        );
    }

    // At off, the onset source's live output is -1. Re-evaluating the
    // onset-only phase edge would therefore produce an invalid -1/4 phase.
    // Successful note-off plus this analytic tail proves the captured target
    // phase was retained and previewed at its current accumulated position.
    runtime.reset(&BTreeMap::new()).unwrap();
    assert_eq!(lifecycle(&mut runtime), first);
}
