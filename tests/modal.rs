//! `synth.modal/1`: source and wire strictness, voice rendering, pitch
//! expression and the work charge. The resonator kernel's own tests are in
//! src/modal.rs.

use maac::bundle::SourceBundle;
use maac::graph::{parameter_descriptor, GraphProcessor, GraphUnit, InstrumentProgram};
use maac::library::LibrarySet;
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

const RATE: f64 = 48_000.0;

/// A voice whose modal bank is struck by a 2 ms pulse.
fn library(node: &str) -> Result<LibrarySet, maac::Diagnostics> {
    let source = format!(
        r#"
maac 1;
library studio {{ version = "1.0.0"; }}
instrument struck {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &bank:out;
  node amp {{ type = "synth.adsr/1"; params = {{ release = 1ms; }}; }}
  node hammer {{ type = "synth.adsr/1"; params = {{ decay = 2ms; sustain = 0; }}; }}
  node bank {{ type = "synth.modal/1"; {node} }}
  connect strike {{ from = &hammer:out; to = &bank:in; }}
 }}
}}
"#
    );
    LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve()?)
}

#[test]
fn source_configuration_and_parameters_are_strict() {
    for node in [
        "config = { modes = 1; };",
        "config = { modes = 64; strings = 3; };",
        "config = { modes = 8; strings = 1; }; params = { ratio = 1/8; stretch = 1/16; decay = 1/100s; damping = 4; position = 1/2; detune = 50; level = 16; };",
        "config = { modes = 8; }; params = { ratio = 8; decay = 60s; };",
    ] {
        library(node).unwrap_or_else(|error| panic!("{node}: {error:?}"));
    }
    for node in [
        "",
        "config = { strings = 2; };",
        "config = { modes = 0; };",
        "config = { modes = 65; };",
        "config = { modes = 8; strings = 0; };",
        "config = { modes = 8; strings = 4; };",
        "config = { modes = 8; seed = 1; };",
        "config = { modes = 8; }; params = { ratio = 0; };",
        "config = { modes = 8; }; params = { stretch = 1/8; };",
        "config = { modes = 8; }; params = { decay = 0s; };",
        "config = { modes = 8; }; params = { decay = 61s; };",
        "config = { modes = 8; }; params = { decay = 2; };",
        "config = { modes = 8; }; params = { damping = 5; };",
        "config = { modes = 8; }; params = { position = 3/5; };",
        "config = { modes = 8; }; params = { detune = 51; };",
        "config = { modes = 8; }; params = { detune = 3ct; };",
        "config = { modes = 8; }; params = { cutoff = 1000Hz; };",
    ] {
        assert!(library(node).is_err(), "{node}");
    }
}

#[test]
fn descriptors_give_every_parameter_its_default_and_range() {
    let processor = GraphProcessor::Modal {
        modes: 8,
        strings: 2,
    };
    for (name, unit, default, min, max) in [
        ("ratio", GraphUnit::Dimensionless, "1", "1/8", "8"),
        ("stretch", GraphUnit::Dimensionless, "0", "0", "1/16"),
        ("decay", GraphUnit::Seconds, "2", "1/100", "60"),
        ("damping", GraphUnit::Dimensionless, "0", "0", "4"),
        ("position", GraphUnit::Dimensionless, "0", "0", "1/2"),
        ("detune", GraphUnit::Dimensionless, "0", "0", "50"),
        ("level", GraphUnit::Dimensionless, "1", "0", "16"),
    ] {
        let spec = parameter_descriptor(&processor, name).unwrap();
        assert_eq!(spec.unit, unit, "{name}");
        assert_eq!(
            spec.default,
            maac::parse_rational(default).unwrap(),
            "{name}"
        );
        assert_eq!(spec.min, maac::parse_rational(min).unwrap(), "{name}");
        assert_eq!(spec.max, maac::parse_rational(max).unwrap(), "{name}");
    }
}

#[test]
fn the_wire_form_is_strict_and_round_trips() {
    let processor = GraphProcessor::Modal {
        modes: 48,
        strings: 2,
    };
    assert_eq!(processor.identity(), "synth.modal/1");
    let wire = serde_json::json!({"kind": "synth.modal/1", "modes": 48, "strings": 2});
    assert_eq!(serde_json::to_value(&processor).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<GraphProcessor>(wire).unwrap(),
        processor
    );
    for wire in [
        serde_json::json!({"kind": "synth.modal/1", "modes": 48}),
        serde_json::json!({"kind": "synth.modal/1", "modes": 48, "strings": 2, "seed": 1}),
        serde_json::json!({"kind": "synth.modal/2", "modes": 48, "strings": 2}),
    ] {
        assert!(serde_json::from_value::<GraphProcessor>(wire).is_err());
    }
    // The plan validator applies the same bounds as the source.
    for (modes, strings) in [(0, 1), (65, 1), (8, 0), (8, 4)] {
        let mut program = program("config = { modes = 8; };");
        let node = program
            .voice
            .nodes
            .iter_mut()
            .find(|node| node.id == "bank")
            .unwrap();
        node.processor = GraphProcessor::Modal { modes, strings };
        assert!(program.validate().is_err(), "{modes} {strings}");
    }
}

#[test]
fn a_modal_bank_is_voice_only() {
    let source = r#"
maac 1;
library studio { version = "1.0.0"; }
instrument struck {
 channels = 1;
 voice v { channels = 1; amplitude = &amp; output = &tone:out;
  node amp { type = "synth.adsr/1"; }
  node tone { type = "synth.sine/1"; }
 }
 shared s { channels = 1; output = &bank:out;
  node bank { type = "synth.modal/1"; config = { modes = 4; }; }
  connect feed { from = &input:out; to = &bank:in; }
 }
}
"#;
    assert!(
        LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve().unwrap()).is_err()
    );
}

fn program(node: &str) -> InstrumentProgram {
    library(node)
        .unwrap_or_else(|error| panic!("{node}: {error:?}"))
        .programs
        .remove(0)
}

/// The first `frames` frames of one full-velocity note at `pitch_hz`.
fn render(program: &InstrumentProgram, pitch_hz: f64, frames: u64) -> maac::dsp::Result<Vec<f64>> {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new())?;
    let mut runtime = InstrumentRuntime::new(Arc::new(compiled), 1, RATE, &BTreeMap::new())?;
    runtime.note_on("note", pitch_hz, 1.0, 0)?;
    (0..frames)
        .map(|frame| runtime.render(frame).map(|output| output[0]))
        .collect()
}

/// The frequency of the strongest component near `around`, by zero-padded
/// correlation over `samples`.
fn peak_near(samples: &[f64], around: f64) -> f64 {
    let power = |frequency: f64| {
        let (mut re, mut im) = (0.0, 0.0);
        for (index, sample) in samples.iter().enumerate() {
            let phase = std::f64::consts::TAU * frequency * index as f64 / RATE;
            re += sample * phase.cos();
            im += sample * phase.sin();
        }
        re * re + im * im
    };
    let mut best = (around, 0.0);
    let mut step = around * 0.02;
    let mut centre = around;
    for _ in 0..6 {
        for k in -10..=10 {
            let frequency = centre + step * k as f64 / 10.0;
            let value = power(frequency);
            if value > best.1 {
                best = (frequency, value);
            }
        }
        centre = best.0;
        step /= 10.0;
    }
    best.0
}

#[test]
fn a_struck_voice_rings_at_its_stretched_partials() {
    let program = program(
        "config = { modes = 6; }; params = { stretch = 1/200; decay = 4s; level = 1/100; };",
    );
    let samples = render(&program, 220.0, 24_000).unwrap();
    assert!(samples.iter().all(|sample| sample.is_finite()));
    assert!(samples.iter().any(|sample| sample.abs() > 1e-4));
    for n in [1.0, 3.0, 5.0] {
        let expected = n * 220.0 * (1.0 + n * n / 200.0f64).sqrt();
        let measured = peak_near(&samples[2_400..], expected);
        let cents = 1200.0 * (measured / expected).log2();
        assert!(cents.abs() < 1.0, "mode {n}: {cents} cents");
    }
}

#[test]
fn the_bank_follows_the_voice_ratio_and_starts_silent() {
    let samples = render(
        &program("config = { modes = 1; }; params = { ratio = 2; level = 1/100; };"),
        220.0,
        24_000,
    )
    .unwrap();
    // The hammer starts at zero, so the first output is exactly zero.
    assert_eq!(samples[0], 0.0);
    let measured = peak_near(&samples[2_400..], 440.0);
    assert!(
        (1200.0 * (measured / 440.0).log2()).abs() < 1.0,
        "{measured}"
    );
}

#[test]
fn notes_above_the_ceiling_are_silent_not_errors() {
    let samples = render(
        &program("config = { modes = 4; }; params = { ratio = 8; level = 1/100; };"),
        3_000.0,
        480,
    )
    .unwrap();
    assert!(samples.iter().all(|sample| *sample == 0.0));
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
fn the_bank_is_charged_one_unit_per_four_resonators() {
    // Each extra node adds its own charge plus one unit for its connection.
    let base = minimum_budget("");
    let gain = minimum_budget(r#"type = "synth.gain/1"; config = { channels = 1; };"#) - base;
    assert!(gain > 0 && gain.is_multiple_of(2));
    let unit = gain / 2;
    for (modes, strings, charge) in [(1, 1, 1), (4, 1, 1), (5, 1, 2), (48, 2, 24), (64, 3, 48)] {
        assert_eq!(maac::graph::modal_sample_work(modes, strings), charge);
        let measured = minimum_budget(&format!(
            r#"type = "synth.modal/1"; config = {{ modes = {modes}; strings = {strings}; }};"#
        )) - base;
        assert_eq!(measured, unit * (charge + 1), "{modes} x {strings}");
    }
}
