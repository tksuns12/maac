//! `synth.svf/1`: a resonant trapezoidal state-variable filter.

use maac::bundle::SourceBundle;
use maac::graph::{validate_graph, GraphProcessor, InstrumentProgram};
use maac::library::LibrarySet;
use maac::synth::{SvfCoefficients, SvfMode};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::sync::Arc;

const RATE: f64 = 48_000.0;
const MODES: [SvfMode; 3] = [SvfMode::Lowpass, SvfMode::Bandpass, SvfMode::Highpass];

/// The bilinear transform of the analog prototype with the cutoff
/// prewarped, which the trapezoidal filter realizes exactly.
fn expected_magnitude(mode: SvfMode, frequency: f64, cutoff: f64, q: f64) -> f64 {
    let omega = (PI * frequency / RATE).tan() / (PI * cutoff / RATE).tan();
    let k = 1.0 / q;
    let denominator = ((1.0 - omega * omega).powi(2) + (k * omega).powi(2)).sqrt();
    match mode {
        SvfMode::Lowpass => 1.0 / denominator,
        SvfMode::Bandpass => k * omega / denominator,
        SvfMode::Highpass => omega * omega / denominator,
    }
}

/// The steady-state amplitude of an integer-hertz sine through the filter,
/// from a coherent one-second DFT bin after two seconds of settling: the
/// slowest case, q = 40 at 200 Hz, decays by exp(-31) in that time.
fn measured_magnitude(mode: SvfMode, frequency: f64, cutoff: f64, q: f64) -> f64 {
    let coefficients = SvfCoefficients::new(cutoff, q, RATE).unwrap();
    let mut state = [0.0; 2];
    let (mut real, mut imaginary) = (0.0, 0.0);
    for n in 0..144_000 {
        let phase = 2.0 * PI * frequency * f64::from(n) / RATE;
        let output = coefficients.step(mode, phase.sin(), &mut state);
        if n >= 96_000 {
            real += output * phase.cos();
            imaginary += output * phase.sin();
        }
    }
    2.0 * real.hypot(imaginary) / 48_000.0
}

#[test]
fn magnitude_responses_match_the_bilinear_transfer_function() {
    for mode in MODES {
        for q in [0.1, 0.707, 4.0, 40.0] {
            for cutoff in [200.0, 1_000.0, 12_000.0] {
                for frequency in [50.0, 200.0, 1_000.0, 3_000.0, 12_000.0, 20_000.0] {
                    let expected = expected_magnitude(mode, frequency, cutoff, q);
                    let measured = measured_magnitude(mode, frequency, cutoff, q);
                    assert!(
                        (measured - expected).abs() <= 1e-9 * expected.max(1.0),
                        "{mode:?} q {q} cutoff {cutoff} at {frequency}: {measured} vs {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_cutoff_point_has_the_documented_gains() {
    // Low-pass and high-pass are 3 dB down at a flat q; band-pass has unity
    // gain at the cutoff for every q, and the other two have gain q.
    for mode in [SvfMode::Lowpass, SvfMode::Highpass] {
        let gain = measured_magnitude(mode, 1_000.0, 1_000.0, std::f64::consts::FRAC_1_SQRT_2);
        assert!((20.0 * gain.log10() + 3.0103).abs() < 1e-3, "{mode:?}");
        assert!((measured_magnitude(mode, 1_000.0, 1_000.0, 10.0) - 10.0).abs() < 1e-7);
    }
    for q in [0.1, 0.707, 4.0, 40.0] {
        assert!((measured_magnitude(SvfMode::Bandpass, 1_000.0, 1_000.0, q) - 1.0).abs() < 1e-9);
    }
}

#[test]
fn a_cutoff_moving_every_sample_stays_bounded() {
    // A deterministic jump to a new cutoff each sample over the full range,
    // at the highest q, with a full-scale square-ish input.
    let mut seed: u32 = 0x1234_5678;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        f64::from(seed) / f64::from(u32::MAX)
    };
    for mode in MODES {
        let mut state = [0.0; 2];
        let mut peak: f64 = 0.0;
        for n in 0..480_000 {
            let cutoff = 1.0 + random() * 23_998.0;
            let coefficients = SvfCoefficients::new(cutoff, 40.0, RATE).unwrap();
            let input = if (n / 37) % 2 == 0 { 1.0 } else { -1.0 };
            let output = coefficients.step(mode, input, &mut state);
            assert!(output.is_finite());
            peak = peak.max(output.abs());
        }
        assert!(peak < 1_000.0, "{mode:?} peak {peak}");
    }
}

#[test]
fn coefficients_reject_cutoffs_outside_the_open_band() {
    for cutoff in [0.0, -1.0, 24_000.0, 30_000.0, f64::NAN] {
        assert!(SvfCoefficients::new(cutoff, 1.0, RATE).is_err(), "{cutoff}");
    }
    for q in [0.0, -1.0, f64::INFINITY] {
        assert!(SvfCoefficients::new(1_000.0, q, RATE).is_err(), "{q}");
    }
    SvfCoefficients::new(23_999.999, 40.0, RATE).unwrap();
}

fn library(node: &str) -> Result<LibrarySet, maac::Diagnostics> {
    let source = format!(
        r#"
maac 1;
library studio {{ version = "1.0.0"; }}
instrument filtered {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &tone:out;
  node amp {{ type = "synth.adsr/1"; }}
  node one {{ type = "synth.velocity/1"; }}
  node tone {{ type = "synth.svf/1"; {node} }}
  connect feed {{ from = &one:out; to = &tone:in; }}
 }}
}}
"#
    );
    LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve()?)
}

#[test]
fn source_configuration_and_parameters_are_strict() {
    for node in [
        "config = { channels = 1; mode = lowpass; };",
        "config = { channels = 1; mode = bandpass; }; params = { cutoff = 0Hz; ratio = 2; q = 40; };",
        "config = { channels = 1; mode = highpass; }; params = { cutoff = 23999Hz; q = 1/10; };",
    ] {
        library(node).unwrap_or_else(|error| panic!("{node}: {error:?}"));
    }
    for node in [
        "",
        "config = { channels = 1; };",
        "config = { mode = lowpass; };",
        "config = { channels = 1; mode = notch; };",
        "config = { channels = 1; mode = \"lowpass\"; };",
        "config = { channels = 3; mode = lowpass; };",
        "config = { channels = 1; mode = lowpass; seed = 1; };",
        "config = { channels = 1; mode = lowpass; }; params = { cutoff = 24000Hz; };",
        "config = { channels = 1; mode = lowpass; }; params = { cutoff = -1Hz; };",
        "config = { channels = 1; mode = lowpass; }; params = { cutoff = 1000; };",
        "config = { channels = 1; mode = lowpass; }; params = { q = 0; };",
        "config = { channels = 1; mode = lowpass; }; params = { q = 41; };",
        "config = { channels = 1; mode = lowpass; }; params = { ratio = 65; };",
        "config = { channels = 1; mode = lowpass; }; params = { ratio = -1; };",
        "config = { channels = 1; mode = lowpass; }; params = { level = 1; };",
    ] {
        assert!(library(node).is_err(), "{node}");
    }
}

#[test]
fn the_wire_form_is_strict_and_round_trips() {
    let processor = GraphProcessor::Svf {
        channels: 2,
        mode: SvfMode::Bandpass,
    };
    assert_eq!(processor.identity(), "synth.svf/1");
    let wire = serde_json::json!({"kind": "synth.svf/1", "channels": 2, "mode": "bandpass"});
    assert_eq!(serde_json::to_value(&processor).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<GraphProcessor>(wire).unwrap(),
        processor
    );
    for wire in [
        serde_json::json!({"kind": "synth.svf/1", "channels": 1}),
        serde_json::json!({"kind": "synth.svf/1", "channels": 1, "mode": "notch"}),
        serde_json::json!({"kind": "synth.svf/1", "channels": 1, "mode": "lowpass", "q": 1}),
        serde_json::json!({"kind": "synth.svf/2", "channels": 1, "mode": "lowpass"}),
    ] {
        assert!(serde_json::from_value::<GraphProcessor>(wire).is_err());
    }
}

fn program(node: &str) -> InstrumentProgram {
    library(node).unwrap().programs.remove(0)
}

fn runtime(program: &InstrumentProgram) -> InstrumentRuntime {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new()).unwrap();
    InstrumentRuntime::new(Arc::new(compiled), 2, RATE, &BTreeMap::new()).unwrap()
}

/// A unit step through the kernel.
fn kernel_step_response(mode: SvfMode, cutoff: f64, q: f64, frames: usize) -> Vec<f64> {
    let coefficients = SvfCoefficients::new(cutoff, q, RATE).unwrap();
    let mut state = [0.0; 2];
    (0..frames)
        .map(|_| coefficients.step(mode, 1.0, &mut state))
        .collect()
}

#[test]
fn a_voice_filter_tracks_the_key_through_ratio() {
    // At velocity 1 the velocity source is a unit step into the filter; the
    // cutoff is 440 Hz * 2 + 120 Hz.
    let program = program(
        "config = { channels = 1; mode = lowpass; }; params = { cutoff = 120Hz; ratio = 2; q = 3; };",
    );
    let expected = kernel_step_response(SvfMode::Lowpass, 1_000.0, 3.0, 256);
    let mut runtime = runtime(&program);
    runtime.note_on("note", 440.0, 1.0, 0).unwrap();
    for (frame, expected) in expected.iter().enumerate() {
        assert_eq!(runtime.render(frame as u64).unwrap()[0], *expected);
    }
}

#[test]
fn each_voice_starts_from_zero_state() {
    let program = program(
        "config = { channels = 1; mode = bandpass; }; params = { cutoff = 500Hz; q = 8; };",
    );
    let expected = kernel_step_response(SvfMode::Bandpass, 500.0, 8.0, 300);
    let mut runtime = runtime(&program);
    runtime.note_on("first", 440.0, 1.0, 0).unwrap();
    for frame in 0..100 {
        runtime.render(frame).unwrap();
    }
    runtime.note_off("first", 100).unwrap();
    runtime.note_on("second", 440.0, 1.0, 100).unwrap();
    // The first voice has a zero release and retires at its note-off, so
    // the output from frame 100 is the second voice alone, from rest.
    for frame in 100..300 {
        assert_eq!(
            runtime.render(frame).unwrap()[0],
            expected[(frame - 100) as usize]
        );
    }
}

#[test]
fn an_effective_cutoff_past_nyquist_fails_at_render() {
    let program = program(
        "config = { channels = 1; mode = lowpass; }; params = { cutoff = 1000Hz; ratio = 64; };",
    );
    let mut runtime = runtime(&program);
    runtime.note_on("low", 220.0, 1.0, 0).unwrap();
    runtime.render(0).unwrap();
    // 440 Hz * 64 + 1000 Hz = 29,160 Hz.
    runtime.note_on("high", 440.0, 1.0, 1).unwrap();
    assert!(runtime.render(1).is_err());
}

#[test]
fn a_shared_filter_cannot_track_a_key() {
    let shared_source = |ratio: &str| {
        format!(
            r#"
maac 1;
library studio {{ version = "1.0.0"; }}
instrument filtered {{
 channels = 2;
 voice v {{
  channels = 1; amplitude = &amp; output = &one:out;
  node amp {{ type = "synth.adsr/1"; }}
  node one {{ type = "synth.velocity/1"; }}
 }}
 shared out {{
  channels = 2; output = &tone:out;
  node pan {{ type = "synth.pan/1"; params = {{ pan = 1/2; }}; }}
  node tone {{ type = "synth.svf/1"; config = {{ channels = 2; mode = highpass; }}; params = {{ cutoff = 300Hz; q = 2; ratio = {ratio}; }}; }}
  connect in_pan {{ from = &input:out; to = &pan:in; }}
  connect pan_tone {{ from = &pan:out; to = &tone:in; }}
 }}
}}
"#
        )
    };
    let resolve = |ratio: &str| {
        LibrarySet::resolve(&SourceBundle::new("main.maac", shared_source(ratio)).resolve()?)
    };
    assert!(resolve("1").is_err());
    let set = resolve("0").unwrap();
    let shared = set.programs[0].shared.clone().unwrap();
    validate_graph(&shared, false, Some(1)).unwrap();

    // Each stereo channel filters independently: the panned unit step
    // reaches the left and right high-pass filters at different levels.
    let theta = (0.5 + 1.0) * PI / 4.0;
    let expected = kernel_step_response(SvfMode::Highpass, 300.0, 2.0, 64);
    let mut runtime = runtime(&set.programs[0]);
    runtime.note_on("note", 440.0, 1.0, 0).unwrap();
    for (frame, expected) in expected.iter().enumerate() {
        let output = runtime.render(frame as u64).unwrap();
        assert!((output[0] - theta.cos() * expected).abs() < 1e-12);
        assert!((output[1] - theta.sin() * expected).abs() < 1e-12);
    }
}

fn work_source(extra: &str) -> SourceBundle {
    let (node, output) = match extra {
        "" => (String::new(), "osc"),
        body => (
            format!(
                "node extra {{ {body} }}\n connect into {{ from = &osc:out; to = &extra:in; }}"
            ),
            "extra",
        ),
    };
    SourceBundle::new(
        "score.maac",
        format!(
            r#"maac 1;
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument tone {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &{output}:out;
  node amp {{ type = "synth.adsr/1"; }}
  node osc {{ type = "synth.sine/1"; }}
  {node}
 }}
}}
node sound {{ instrument = &tone; }}
track t {{ target = &sound:events; }}
pattern one {{ length = 1q; note a {{ at = 0q; dur = 1q; pitch = A4; }} }}
place p1 {{ pattern = &one; track = &t; at = 0q; }}
"#
        ),
    )
}

fn minimum_budget(extra: &str) -> u64 {
    use maac::plan::PlanLimits;
    let limits = |max_execution_work| PlanLimits {
        max_execution_work,
        ..PlanLimits::song()
    };
    let artifact = maac::compiler::compile_bundle_artifact_with_limits(
        &work_source(extra),
        &PlanLimits::song(),
    )
    .unwrap();
    let (mut low, mut high) = (0, PlanLimits::song().max_execution_work);
    while low < high {
        let middle = low + (high - low) / 2;
        if artifact.validate_with_limits(&limits(middle)).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}

#[test]
fn the_filter_and_drive_are_charged_two_units_per_sample() {
    // Each extra node adds its own charge plus one unit for its connection.
    let base = minimum_budget("");
    let gain = minimum_budget(r#"type = "synth.gain/1"; config = { channels = 1; };"#) - base;
    let svf =
        minimum_budget(r#"type = "synth.svf/1"; config = { channels = 1; mode = lowpass; };"#)
            - base;
    let drive = minimum_budget(r#"type = "synth.drive/1"; config = { channels = 1; };"#) - base;
    assert!(gain > 0);
    assert_eq!(gain % 2, 0);
    assert_eq!(svf, gain / 2 * (maac::graph::SVF_SAMPLE_WORK + 1));
    assert_eq!(drive, gain / 2 * (maac::graph::DRIVE_SAMPLE_WORK + 1));
}
