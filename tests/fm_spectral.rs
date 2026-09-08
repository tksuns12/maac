use maac::graph::{
    GraphNode, GraphProcessor, GraphProgram, InstrumentProgram, Modulation, ParameterTarget,
    ProgramSource,
};
use maac::plan::PortRef;
use maac::synth::{Oscillator, Waveform};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

const RATE: f64 = 48_000.0;
const FRAMES: usize = 48_000;
const SPECTRAL_TOLERANCE: f64 = 5.0e-6;

#[test]
fn low_frequency_fm_matches_discrete_phase_integral_and_bessel_sidebands() {
    let carrier_hz = 4_000.0;
    let modulator_hz = 400.0;
    let depth_hz = 400.0;
    let (actual, ideal_reference) = render_primitive_fm(carrier_hz, modulator_hz, depth_hz);

    let maximum_error = actual
        .iter()
        .zip(&ideal_reference)
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0_f64, f64::max);
    assert!(
        maximum_error < SPECTRAL_TOLERANCE,
        "maximum sample error {maximum_error}"
    );

    // This discrete-time index follows from summing the sample-wise frequency
    // increments. It approaches depth / fm in the continuous-time limit.
    let beta = discrete_modulation_index(depth_hz, modulator_hz);
    assert_close(bessel_j(0, 1.0), 0.765_197_686_557_966_6, 1.0e-15);
    assert_close(bessel_j(1, 1.0), 0.440_050_585_744_933_5, 1.0e-15);
    eprintln!(
        "low primitive: beta={beta:.12}, max_sample_error={maximum_error:.3e}, carrier={:.12}, lower1={:.12}, upper1={:.12}, lower2={:.12}, upper2={:.12}, expected_j0={:.12}, expected_j1={:.12}, expected_j2={:.12}",
        amplitude_at(&actual, carrier_hz),
        amplitude_at(&actual, carrier_hz - modulator_hz),
        amplitude_at(&actual, carrier_hz + modulator_hz),
        amplitude_at(&actual, carrier_hz - 2.0 * modulator_hz),
        amplitude_at(&actual, carrier_hz + 2.0 * modulator_hz),
        bessel_j(0, beta).abs(),
        bessel_j(1, beta).abs(),
        bessel_j(2, beta).abs(),
    );
    for order in 0..=2 {
        let expected = bessel_j(order, beta).abs();
        for frequency in sideband_frequencies(carrier_hz, modulator_hz, order) {
            let measured = amplitude_at(&actual, frequency);
            assert_close(measured, expected, SPECTRAL_TOLERANCE);
        }
    }
}

#[test]
fn instrument_runtime_applies_fm_each_sample_and_rejects_out_of_range_sum() {
    let carrier_hz = 4_000;
    let modulator_hz = 400;
    let depth_hz = 400;
    let actual = render_instrument_fm(carrier_hz, modulator_hz, depth_hz, FRAMES).unwrap();
    let beta = discrete_modulation_index(depth_hz as f64, modulator_hz as f64);
    eprintln!(
        "low instrument: carrier={:.12}, lower1={:.12}, upper1={:.12}, lower2={:.12}, upper2={:.12}",
        amplitude_at(&actual, carrier_hz as f64),
        amplitude_at(&actual, (carrier_hz - modulator_hz) as f64),
        amplitude_at(&actual, (carrier_hz + modulator_hz) as f64),
        amplitude_at(&actual, (carrier_hz - 2 * modulator_hz) as f64),
        amplitude_at(&actual, (carrier_hz + 2 * modulator_hz) as f64),
    );

    for order in 0..=2 {
        let expected = bessel_j(order, beta).abs();
        for frequency in sideband_frequencies(carrier_hz as f64, modulator_hz as f64, order) {
            assert_close(
                amplitude_at(&actual, frequency),
                expected,
                SPECTRAL_TOLERANCE,
            );
        }
    }

    let error = render_instrument_fm(23_000, 1_000, 2_000, FRAMES).unwrap_err();
    assert_eq!(error.code(), "E_NONFINITE");
}

#[test]
fn oscillator_holds_at_zero_and_reverses_phase_for_negative_frequency() {
    let mut oscillator = Oscillator::new(Waveform::Sine, 0.25).unwrap();
    assert_close(oscillator.sample(0.0, RATE).unwrap(), 1.0, 1.0e-15);
    assert_close(oscillator.sample(-12_000.0, RATE).unwrap(), 1.0, 1.0e-15);
    assert_close(oscillator.sample(0.0, RATE).unwrap(), 0.0, 1.0e-15);
    assert_close(oscillator.sample(12_000.0, RATE).unwrap(), 0.0, 1.0e-15);
    assert_close(oscillator.sample(0.0, RATE).unwrap(), 1.0, 1.0e-15);

    // A modulated carrier also crosses through zero without clipping. Compare
    // every output sample with an independently integrated ideal-sine phase.
    let (through_zero, reference) = render_primitive_fm(100.0, 400.0, 400.0);
    let maximum_error = through_zero
        .iter()
        .zip(reference)
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0_f64, f64::max);
    eprintln!("through-zero maximum sample error={maximum_error:.3e}");
    assert!(
        maximum_error < SPECTRAL_TOLERANCE,
        "maximum sample error {maximum_error}"
    );
}

#[test]
fn strong_high_frequency_fm_has_a_deterministic_folded_second_sideband() {
    let carrier_hz = 10_000.0;
    let modulator_hz = 9_000.0;
    let depth_hz = 6_000.0;
    let (first, _) = render_primitive_fm(carrier_hz, modulator_hz, depth_hz);
    let (second, _) = render_primitive_fm(carrier_hz, modulator_hz, depth_hz);
    assert!(first
        .iter()
        .zip(&second)
        .all(|(left, right)| left.to_bits() == right.to_bits()));

    let beta = discrete_modulation_index(depth_hz, modulator_hz);
    let expected_second_sideband = bessel_j(2, beta).abs();
    let folded_peak = amplitude_at(&first, 20_000.0);
    eprintln!(
        "strong FM: beta={beta:.12}, folded_20khz={folded_peak:.12}, expected_j2={expected_second_sideband:.12}"
    );
    assert_close(folded_peak, expected_second_sideband, SPECTRAL_TOLERANCE);
    assert!(
        folded_peak > 0.05,
        "folded 20 kHz peak was only {folded_peak}"
    );
}

fn render_primitive_fm(carrier_hz: f64, modulator_hz: f64, depth_hz: f64) -> (Vec<f64>, Vec<f64>) {
    let mut modulator = Oscillator::new(Waveform::Sine, 0.0).unwrap();
    let mut carrier = Oscillator::new(Waveform::Sine, 0.0).unwrap();
    let mut actual = Vec::with_capacity(FRAMES);
    let mut reference = Vec::with_capacity(FRAMES);
    let mut reference_phase = 0.0_f64;

    for frame in 0..FRAMES {
        let modulation = modulator.sample(modulator_hz, RATE).unwrap();
        actual.push(
            carrier
                .sample(carrier_hz + depth_hz * modulation, RATE)
                .unwrap(),
        );

        reference.push((std::f64::consts::TAU * reference_phase).sin());
        let ideal_modulation = (std::f64::consts::TAU * modulator_hz * frame as f64 / RATE).sin();
        reference_phase =
            (reference_phase + (carrier_hz + depth_hz * ideal_modulation) / RATE).rem_euclid(1.0);
    }
    (actual, reference)
}

fn render_instrument_fm(
    carrier_hz: i64,
    modulator_hz: i64,
    depth_hz: i64,
    frames: usize,
) -> Result<Vec<f64>, maac::dsp::RenderError> {
    let program = fm_program(carrier_hz, modulator_hz, depth_hz);
    let compiled = CompiledInstrument::compile(&program, &BTreeMap::new())?;
    let mut runtime = InstrumentRuntime::new(Arc::new(compiled), 1, RATE, &BTreeMap::new())?;
    runtime.note_on("fm/note", 0.0, 1.0, 0)?;

    let mut output = Vec::with_capacity(frames);
    for frame in 0..frames {
        output.push(runtime.render(frame as u64)?[0]);
    }
    Ok(output)
}

fn fm_program(carrier_hz: i64, modulator_hz: i64, depth_hz: i64) -> InstrumentProgram {
    let oscillator = |id: &str, frequency: i64| GraphNode {
        id: id.into(),
        processor: GraphProcessor::Sine,
        params: [
            ("ratio".into(), rational(0)),
            ("frequency".into(), rational(frequency)),
        ]
        .into_iter()
        .collect(),
    };
    InstrumentProgram {
        id: "fm_fixture".into(),
        voice: GraphProgram {
            channels: 1,
            nodes: vec![
                GraphNode {
                    id: "amp".into(),
                    processor: GraphProcessor::Adsr,
                    params: BTreeMap::new(),
                },
                oscillator("carrier", carrier_hz),
                oscillator("modulator", modulator_hz),
            ],
            connections: Vec::new(),
            modulations: vec![Modulation {
                id: "frequency_modulation".into(),
                from: PortRef::new("modulator", "out").unwrap(),
                to: ParameterTarget {
                    node: "carrier".into(),
                    parameter: "frequency".into(),
                },
                depth: rational(depth_hz),
            }],
            output: PortRef::new("carrier", "out").unwrap(),
            amplitude: Some("amp".into()),
        },
        shared: None,
        controls: BTreeMap::new(),
        source: ProgramSource {
            file: "tests/fm_spectral.rs".into(),
            object: "fm_fixture".into(),
            span: None,
        },
    }
}

fn rational(value: i64) -> maac::Rational {
    maac::parse_rational(&value.to_string()).unwrap()
}

fn discrete_modulation_index(depth_hz: f64, modulator_hz: f64) -> f64 {
    std::f64::consts::PI * depth_hz / (RATE * (std::f64::consts::PI * modulator_hz / RATE).sin())
}

fn bessel_j(order: usize, value: f64) -> f64 {
    let half = value * 0.5;
    let mut term = half.powi(order as i32) / factorial(order);
    let mut sum = term;
    for index in 1..=64 {
        term *= -(half * half) / (index * (index + order)) as f64;
        sum += term;
        if term.abs() < 1.0e-18 {
            break;
        }
    }
    sum
}

fn factorial(value: usize) -> f64 {
    (1..=value).map(|factor| factor as f64).product()
}

fn sideband_frequencies(carrier_hz: f64, modulator_hz: f64, order: usize) -> Vec<f64> {
    if order == 0 {
        vec![carrier_hz]
    } else {
        vec![
            carrier_hz - order as f64 * modulator_hz,
            carrier_hz + order as f64 * modulator_hz,
        ]
    }
}

fn amplitude_at(samples: &[f64], frequency_hz: f64) -> f64 {
    let (real, imaginary) =
        samples
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(real, imaginary), (index, sample)| {
                let angle = std::f64::consts::TAU * frequency_hz * index as f64 / RATE;
                (
                    real + sample * angle.cos(),
                    imaginary - sample * angle.sin(),
                )
            });
    2.0 * real.hypot(imaginary) / samples.len() as f64
}

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected:.12}, measured {actual:.12}, difference {:.3e}, tolerance {tolerance:.3e}",
        (actual - expected).abs()
    );
}
