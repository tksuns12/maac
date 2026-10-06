//! `synth.drive/1`: `tanh` saturation with an offset and first-order
//! antiderivative anti-aliasing.

use maac::bundle::SourceBundle;
use maac::graph::{GraphProcessor, InstrumentProgram};
use maac::library::LibrarySet;
use maac::synth::{drive_sample, log_cosh};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::sync::Arc;

const RATE: f64 = 48_000.0;

/// Run a whole signal through one channel of the kernel from rest.
fn drive_signal(input: &[f64], drive: f64, bias: f64, level: f64) -> Vec<f64> {
    let mut previous = 0.0;
    input
        .iter()
        .map(|&sample| {
            let output = drive_sample(sample, previous, drive, bias, level);
            previous = sample;
            output
        })
        .collect()
}

#[test]
fn log_cosh_is_finite_and_exact_where_cosh_overflows() {
    for u in [0.0_f64, 1e-8, 0.5, 3.0, 20.0, 700.0, 1e6] {
        let direct = u.cosh().ln();
        let stable = log_cosh(u);
        assert_eq!(log_cosh(-u), stable);
        if direct.is_finite() {
            assert!(
                (stable - direct).abs() <= 1e-12 * direct.abs().max(1.0),
                "{u}"
            );
        } else {
            assert!((stable - (u - std::f64::consts::LN_2)).abs() < 1e-9, "{u}");
        }
    }
}

#[test]
fn silence_stays_silent_for_any_bias() {
    for bias in [-4.0, -0.75, 0.0, 1.5, 4.0] {
        let output = drive_signal(&[0.0; 64], 3.0, bias, 2.0);
        assert!(output.iter().all(|sample| *sample == 0.0), "bias {bias}");
    }
}

#[test]
fn slow_signals_match_tanh_half_a_sample_late() {
    // A 20 Hz sine at drive 4 with an offset. First-order ADAA averages
    // tanh over each step, so it matches the curve at the step's midpoint
    // to second order in the step size.
    let (drive, bias, level) = (4.0, 0.5, 0.8);
    let input: Vec<f64> = (0..4_800)
        .map(|n| (2.0 * PI * 20.0 * f64::from(n) / RATE).sin())
        .collect();
    let output = drive_signal(&input, drive, bias, level);
    let mut worst: f64 = 0.0;
    for (n, actual) in output.iter().enumerate().skip(1) {
        let midpoint = (2.0 * PI * 20.0 * (n as f64 - 0.5) / RATE).sin();
        let expected = level * ((drive * midpoint + bias).tanh() - bias.tanh());
        worst = worst.max((actual - expected).abs());
    }
    assert!(worst < 1e-5, "worst error {worst}");
}

/// Power at each 10 Hz bin of a coherent 0.1 s window, by direct DFT.
fn bin_powers(signal: &[f64]) -> Vec<f64> {
    let length = signal.len();
    (0..=length / 2)
        .map(|bin| {
            let (mut real, mut imaginary) = (0.0, 0.0);
            for (n, sample) in signal.iter().enumerate() {
                let angle = 2.0 * PI * ((bin * n) % length) as f64 / length as f64;
                real += sample * angle.cos();
                imaginary -= sample * angle.sin();
            }
            real * real + imaginary * imaginary
        })
        .collect()
}

/// Folded power relative to the fundamental, in dB, over bins `1..top`.
/// The fixture's 4,990 Hz fundamental is bin 499 at 10 Hz per bin; the
/// symmetric curve makes only odd harmonics, so below Nyquist the true
/// harmonics are bins 499 and 1,497. Because 499 is prime and does not divide
/// 4,800, no folded harmonic lands on a true one.
fn alias_ratio_db(powers: &[f64], top: usize) -> f64 {
    let aliased: f64 = powers[1..top]
        .iter()
        .enumerate()
        .filter(|(index, _)| ![498, 1_496].contains(index))
        .map(|(_, power)| power)
        .sum();
    10.0 * (aliased / powers[499]).log10()
}

#[test]
fn anti_aliasing_lowers_folded_harmonics_on_the_reference_fixture() {
    // 4,990 Hz at 0.9 full scale into drive 8, the fixture the instrument
    // contract records. The 0.1 s window starts after two periods.
    let input: Vec<f64> = (0..4_800 + 96)
        .map(|n| 0.9 * (2.0 * PI * 4_990.0 * f64::from(n) / RATE).sin())
        .collect();
    let naive: Vec<f64> = input[96..].iter().map(|x| (8.0 * x).tanh()).collect();
    let anti_aliased = drive_signal(&input, 8.0, 0.0, 1.0)[96..].to_vec();
    let (naive, anti_aliased) = (bin_powers(&naive), bin_powers(&anti_aliased));
    // Folds that land below 5 kHz sit among the music; folds near Nyquist
    // dominate the total but are the least audible and the least reduced.
    let low = (
        alias_ratio_db(&naive, 500),
        alias_ratio_db(&anti_aliased, 500),
    );
    let total = (
        alias_ratio_db(&naive, naive.len()),
        alias_ratio_db(&anti_aliased, anti_aliased.len()),
    );
    println!(
        "folded power below 5 kHz: naive {:.1} dB, anti-aliased {:.1} dB; whole band: {:.1} dB, {:.1} dB",
        low.0, low.1, total.0, total.1
    );
    assert!(low.1 < low.0 - 25.0, "below 5 kHz {low:?}");
    assert!(total.1 < total.0 - 5.0, "whole band {total:?}");
}

fn library(node: &str) -> Result<LibrarySet, maac::Diagnostics> {
    let source = format!(
        r#"
maac 1;
library studio {{ version = "1.0.0"; }}
instrument driven {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &shape:out;
  node amp {{ type = "synth.adsr/1"; }}
  node osc {{ type = "synth.sine/1"; params = {{ ratio = 1; }}; }}
  node shape {{ type = "synth.drive/1"; {node} }}
  connect feed {{ from = &osc:out; to = &shape:in; }}
 }}
}}
"#
    );
    LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve()?)
}

#[test]
fn source_configuration_and_parameters_are_strict() {
    for node in [
        "config = { channels = 1; };",
        "config = { channels = 1; }; params = { drive = 64; bias = -4; level = 16; };",
        "config = { channels = 1; }; params = { drive = 0; bias = 4; level = 0; };",
    ] {
        library(node).unwrap_or_else(|error| panic!("{node}: {error:?}"));
    }
    for node in [
        "",
        "config = {};",
        "config = { channels = 3; };",
        "config = { channels = 1; mode = lowpass; };",
        "config = { channels = 1; }; params = { drive = 65; };",
        "config = { channels = 1; }; params = { drive = -1; };",
        "config = { channels = 1; }; params = { bias = 5; };",
        "config = { channels = 1; }; params = { level = 17; };",
        "config = { channels = 1; }; params = { cutoff = 1000Hz; };",
    ] {
        assert!(library(node).is_err(), "{node}");
    }
}

#[test]
fn the_wire_form_is_strict_and_round_trips() {
    let processor = GraphProcessor::Drive { channels: 1 };
    assert_eq!(processor.identity(), "synth.drive/1");
    let wire = serde_json::json!({"kind": "synth.drive/1", "channels": 1});
    assert_eq!(serde_json::to_value(&processor).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<GraphProcessor>(wire).unwrap(),
        processor
    );
    for wire in [
        serde_json::json!({"kind": "synth.drive/1"}),
        serde_json::json!({"kind": "synth.drive/1", "channels": 1, "drive": 2}),
        serde_json::json!({"kind": "synth.drive/2", "channels": 1}),
    ] {
        assert!(serde_json::from_value::<GraphProcessor>(wire).is_err());
    }
}

fn program(node: &str) -> InstrumentProgram {
    library(node).unwrap().programs.remove(0)
}

#[test]
fn a_voice_drive_matches_the_kernel_on_its_input() {
    let program =
        program("config = { channels = 1; }; params = { drive = 5; bias = 1/4; level = 1/2; };");
    let mut plain = program.clone();
    plain.voice.output = maac::plan::PortRef::new("osc", "out").unwrap();
    let compile = |program: &InstrumentProgram| {
        let compiled = CompiledInstrument::compile(program, &BTreeMap::new()).unwrap();
        InstrumentRuntime::new(Arc::new(compiled), 1, RATE, &BTreeMap::new()).unwrap()
    };
    let mut source = compile(&plain);
    let mut driven = compile(&program);
    source.note_on("note", 440.0, 1.0, 0).unwrap();
    driven.note_on("note", 440.0, 1.0, 0).unwrap();
    let input: Vec<f64> = (0..512)
        .map(|frame| source.render(frame).unwrap()[0])
        .collect();
    let expected = drive_signal(&input, 5.0, 0.25, 0.5);
    for (frame, expected) in expected.iter().enumerate() {
        assert_eq!(driven.render(frame as u64).unwrap()[0], *expected);
    }
}
