//! `hardness`, `position` and `pickup` on `synth.pluck/1`, through source,
//! plans and voices.

use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle;
use maac::graph::{
    parameter_descriptor, GraphProcessor, GraphUnit, InstrumentProgram, ParameterRate,
};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A one-string instrument; `extra` adds voice nodes and modulations.
fn source(params: &str, extra: &str) -> String {
    source_with_controls(params, extra, "")
}

fn source_with_controls(params: &str, extra: &str, controls: &str) -> String {
    format!(
        r#"maac 1;
project song {{ score = [0q, 1/50q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; tail = 1ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument string {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &pluck:out;
  node amp {{ type = "synth.adsr/1"; params = {{ release = 1ms; }}; }}
  node pluck {{ type = "synth.pluck/1"; config = {{ seed = 1; }}; params = {{ {params} }}; }}
  {extra}
 }}
 {controls}
}}
node sound {{ instrument = &string; config = {{ voices = 4; }}; }}
pattern phrase {{ length = 1/50q; note a {{ at = 0q; dur = 1/100q; pitch = A4; velocity = 0.6; }} }}
track t {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &t; at = 0q; }}
"#
    )
}

fn program(params: &str, extra: &str) -> InstrumentProgram {
    compile_bundle(&SourceBundle::new("pluck.maac", source(params, extra)))
        .unwrap_or_else(|error| panic!("{params} {extra}: {error:?}"))
        .instruments
        .unwrap()
        .programs
        .remove(0)
}

/// The first `frames` of one note at `velocity`, with optional controls.
fn render(
    program: &InstrumentProgram,
    velocity: f64,
    controls: &[(&str, f64)],
    frames: u64,
) -> maac::dsp::Result<Vec<f64>> {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new())?;
    let mut runtime = InstrumentRuntime::new(Arc::new(compiled), 1, 48000.0, &BTreeMap::new())?;
    runtime.update_controls(
        &controls
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect(),
    )?;
    runtime.note_on("note", 220.0, velocity, 0)?;
    (0..frames)
        .map(|frame| runtime.render(frame).map(|output| output[0]))
        .collect()
}

/// Energy of the first difference over energy: higher is brighter, and a
/// change of level leaves it alone.
fn brightness(samples: &[f64]) -> f64 {
    let difference: f64 = samples.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
    difference / samples.iter().map(|value| value * value).sum::<f64>()
}

#[test]
fn descriptors_give_the_new_parameters_their_ranges_and_rates() {
    let processor = GraphProcessor::Pluck { seed: 1 };
    for (name, rate, default, min, max) in [
        ("hardness", ParameterRate::NoteOn, "1", "1/100", "1"),
        ("position", ParameterRate::NoteOn, "0", "0", "1/2"),
        ("pickup", ParameterRate::Sample, "0", "0", "1/2"),
    ] {
        let spec = parameter_descriptor(&processor, name).unwrap();
        assert_eq!(spec.unit, GraphUnit::Dimensionless);
        assert_eq!(spec.rate, rate, "{name}");
        assert_eq!(spec.default, maac::parse_rational(default).unwrap());
        assert_eq!(spec.min, maac::parse_rational(min).unwrap());
        assert_eq!(spec.max, maac::parse_rational(max).unwrap());
    }
    for params in [
        "hardness = 0;",
        "hardness = 2;",
        "position = -1/10;",
        "position = 3/5;",
        "pickup = 1;",
        "pickup = 10ms;",
    ] {
        assert!(
            compile_bundle(&SourceBundle::new("pluck.maac", source(params, ""))).is_err(),
            "{params}"
        );
    }
}

#[test]
fn explicit_defaults_render_bit_identically_to_omitted_ones() {
    let plain = render(&program("", ""), 0.6, &[], 4_800).unwrap();
    let explicit = render(
        &program("hardness = 1; position = 0; pickup = 0;", ""),
        0.6,
        &[],
        4_800,
    )
    .unwrap();
    assert_eq!(plain, explicit);
}

#[test]
fn hardness_position_and_pickup_change_the_sound() {
    let plain = render(&program("", ""), 0.6, &[], 4_800).unwrap();
    for params in ["hardness = 1/4;", "position = 1/7;", "pickup = 1/8;"] {
        let shaped = render(&program(params, ""), 0.6, &[], 4_800).unwrap();
        assert_ne!(plain, shaped, "{params}");
        assert!(shaped.iter().all(|sample| sample.is_finite()), "{params}");
    }
    let soft = render(&program("hardness = 1/10;", ""), 0.6, &[], 960).unwrap();
    assert!(brightness(&soft) < brightness(&plain[..960]));
}

#[test]
fn a_control_sets_hardness_at_note_on() {
    let program = compile_bundle(&SourceBundle::new(
        "pluck.maac",
        source_with_controls(
            "",
            "",
            "control hardness { target = &v.pluck.params.hardness; default = 1; }",
        ),
    ))
    .unwrap()
    .instruments
    .unwrap()
    .programs
    .remove(0);
    let dark = render(&program, 0.6, &[("hardness", 0.1)], 960).unwrap();
    let bright = render(&program, 0.6, &[("hardness", 1.0)], 960).unwrap();
    assert!(brightness(&dark) < brightness(&bright));
    assert_eq!(
        dark,
        render(&self::program("hardness = 1/10;", ""), 0.6, &[], 960).unwrap()
    );
}

#[test]
fn velocity_modulates_hardness_through_the_note_on_capture() {
    let program = program(
        "hardness = 1/20;",
        r#"node vel { type = "synth.velocity/1"; }
  modulate harder { from = &vel:out; to = &pluck.params.hardness; depth = 19/20; }"#,
    );
    let soft = render(&program, 0.2, &[], 960).unwrap();
    let hard = render(&program, 1.0, &[], 960).unwrap();
    assert!(
        brightness(&soft) < 0.5 * brightness(&hard),
        "{} {}",
        brightness(&soft),
        brightness(&hard)
    );
    // Full velocity reaches hardness 1, the unshaped fill.
    let plain = render(&self::program("", ""), 1.0, &[], 960).unwrap();
    assert_eq!(hard, plain);
}

#[test]
fn a_modulated_hardness_out_of_range_fails_at_note_on() {
    let program = program(
        "hardness = 1/2;",
        r#"node vel { type = "synth.velocity/1"; }
  modulate harder { from = &vel:out; to = &pluck.params.hardness; depth = 1; }"#,
    );
    assert!(render(&program, 0.2, &[], 16).is_ok());
    assert!(render(&program, 0.9, &[], 16).is_err());
}
