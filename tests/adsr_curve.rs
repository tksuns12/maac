//! The `curve` parameter of `synth.adsr/1`: curved decay and release
//! segments that keep the linear segments' endpoints.

use maac::bundle::SourceBundle;
use maac::library::LibrarySet;
use maac::synth::{curve_shape, Adsr};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

const RATE: f64 = 48_000.0;

/// `(exp(-c x) - exp(-c)) / (1 - exp(-c))`, written independently of
/// [`curve_shape`].
fn reference_shape(curve: f64, x: f64) -> f64 {
    let floor = 1.0 / curve.exp();
    (1.0 / (curve * x).exp() - floor) / (1.0 - floor)
}

#[test]
fn shape_runs_from_exactly_one_to_exactly_zero() {
    for curve in [1e-12, 1e-9, 1e-3, 0.5, 1.0, 7.0, 16.0] {
        assert_eq!(curve_shape(curve, 0.0), 1.0);
        assert_eq!(curve_shape(curve, 1.0), 0.0);
        let mut previous = 1.0;
        for step in 1..=100 {
            let x = f64::from(step) / 100.0;
            let value = curve_shape(curve, x);
            assert!(value < previous, "curve {curve} step {step}");
            // The plain reference cancels for small curves; there the shape
            // is within curve/8 of the linear segment.
            let (expected, tolerance) = if curve < 0.5 {
                (1.0 - x, curve / 8.0 + 1e-15)
            } else {
                (reference_shape(curve, x), 1e-12)
            };
            assert!(
                (value - expected).abs() <= tolerance,
                "curve {curve} step {step}"
            );
            previous = value;
        }
    }
    // Curve 7 is 31 dB down halfway and 61 dB down at 90 %.
    assert_eq!((20.0 * curve_shape(7.0, 0.5).log10()).round(), -31.0);
    assert_eq!((20.0 * curve_shape(7.0, 0.9).log10()).round(), -61.0);
}

#[test]
fn zero_curve_is_the_linear_envelope_bit_for_bit() {
    let mut linear = Adsr::new(10, 1.5 / RATE, 37.5 / RATE, 0.25).unwrap();
    let mut curved = Adsr::new_curved(10, 1.5 / RATE, 37.5 / RATE, 0.25, 0.0).unwrap();
    for frame in 0..60 {
        assert_eq!(
            linear.value(frame, RATE).unwrap().to_bits(),
            curved.value(frame, RATE).unwrap().to_bits()
        );
    }
    linear.release(60, RATE, 20.5 / RATE).unwrap();
    curved.release(60, RATE, 20.5 / RATE).unwrap();
    for frame in 60..90 {
        assert_eq!(
            linear.value(frame, RATE).unwrap().to_bits(),
            curved.value(frame, RATE).unwrap().to_bits()
        );
        assert_eq!(
            linear.finished(frame, RATE).unwrap(),
            curved.finished(frame, RATE).unwrap()
        );
    }
}

#[test]
fn curved_decay_reaches_sustain_at_the_same_frame() {
    // Attack 2 frames, decay 100 frames, sustain 1/4, curve 7.
    let envelope = Adsr::new_curved(0, 2.0 / RATE, 100.0 / RATE, 0.25, 7.0).unwrap();
    assert_eq!(envelope.value(0, RATE).unwrap(), 0.0);
    assert_eq!(envelope.value(1, RATE).unwrap(), 0.5);
    assert_eq!(envelope.value(2, RATE).unwrap(), 1.0);
    for elapsed in 1..100_u32 {
        let expected = 0.25 + 0.75 * reference_shape(7.0, f64::from(elapsed) / 100.0);
        let actual = envelope.value(2 + u64::from(elapsed), RATE).unwrap();
        assert!((actual - expected).abs() < 1e-12, "decay frame {elapsed}");
    }
    assert_eq!(envelope.value(102, RATE).unwrap(), 0.25);
    assert_eq!(envelope.value(10_000, RATE).unwrap(), 0.25);
}

#[test]
fn curved_release_retires_on_the_linear_frame() {
    for release_frames in [2.5, 40.0, 41.25] {
        let mut linear = Adsr::new(0, 0.0, 0.0, 0.5).unwrap();
        let mut curved = Adsr::new_curved(0, 0.0, 0.0, 0.5, 9.0).unwrap();
        linear.release(10, RATE, release_frames / RATE).unwrap();
        curved.release(10, RATE, release_frames / RATE).unwrap();
        for frame in 10..60 {
            assert_eq!(
                linear.finished(frame, RATE).unwrap(),
                curved.finished(frame, RATE).unwrap(),
                "release {release_frames} frame {frame}"
            );
            let elapsed = (frame - 10) as f64;
            let actual = curved.value(frame, RATE).unwrap();
            if elapsed >= release_frames {
                assert_eq!(actual, 0.0);
            } else {
                let expected = 0.5 * reference_shape(9.0, elapsed / release_frames);
                assert!((actual - expected).abs() < 1e-12);
            }
        }
    }
}

#[test]
fn release_starts_from_the_curved_level_at_note_off() {
    let mut envelope = Adsr::new_curved(0, 0.0, 100.0 / RATE, 0.0, 6.0).unwrap();
    let at_note_off = envelope.value(30, RATE).unwrap();
    envelope.release(30, RATE, 50.0 / RATE).unwrap();
    assert_eq!(envelope.value(30, RATE).unwrap(), at_note_off);
    let expected = at_note_off * reference_shape(6.0, 0.5);
    assert!((envelope.value(55, RATE).unwrap() - expected).abs() < 1e-12);
}

#[test]
fn curve_range_is_enforced() {
    Adsr::new_curved(0, 0.0, 0.0, 1.0, 16.0).unwrap();
    for curve in [-1e-9, -1.0, 16.000001, f64::NAN, f64::INFINITY] {
        assert!(
            Adsr::new_curved(0, 0.0, 0.0, 1.0, curve).is_err(),
            "{curve}"
        );
    }
}

fn library(params: &str) -> Result<LibrarySet, maac::Diagnostics> {
    let source = format!(
        r#"
maac 1;
library studio {{ version = "1.0.0"; }}
instrument struck {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &one:out;
  node amp {{ type = "synth.adsr/1"; params = {{ {params} }}; }}
  node one {{ type = "synth.velocity/1"; }}
 }}
 control curve {{ target = &v.amp.params.curve; default = 0; }}
}}
"#
    );
    LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve()?)
}

fn runtime(set: &LibrarySet, controls: &BTreeMap<String, f64>) -> InstrumentRuntime {
    let compiled = CompiledInstrument::compile(&set.programs[0], &BTreeMap::new()).unwrap();
    InstrumentRuntime::new(Arc::new(compiled), 2, RATE, controls).unwrap()
}

#[test]
fn library_source_accepts_curves_within_the_range() {
    for params in ["curve = 0;", "curve = 7;", "curve = 16;", "curve = 1/3;"] {
        library(params).unwrap();
    }
    for params in ["curve = -1;", "curve = 17;", "curve = 1s;"] {
        assert!(library(params).is_err(), "{params}");
    }
}

/// At velocity 1 the velocity source outputs a constant 1, so each rendered
/// frame is the amplitude envelope.
fn rendered_envelope(set: &LibrarySet, controls: &BTreeMap<String, f64>) -> Vec<f64> {
    let mut runtime = runtime(set, controls);
    runtime.note_on("note", 440.0, 1.0, 0).unwrap();
    let mut frames = Vec::new();
    for frame in 0..200 {
        if frame == 120 {
            runtime.note_off("note", frame).unwrap();
        }
        frames.push(runtime.render(frame).unwrap()[0]);
    }
    frames
}

#[test]
fn the_amplitude_envelope_curves_in_a_rendered_voice() {
    let params = "decay = 1/480s; sustain = 1/5; release = 1/800s;";
    let linear = rendered_envelope(&library(params).unwrap(), &BTreeMap::new());
    let explicit_zero = rendered_envelope(
        &library(&format!("{params} curve = 0;")).unwrap(),
        &BTreeMap::new(),
    );
    assert_eq!(linear, explicit_zero);

    let curved_set = library(params).unwrap();
    let curved = rendered_envelope(&curved_set, &BTreeMap::from([("curve".into(), 8.0)]));
    assert_eq!(curved[0], 1.0);
    for (frame, actual) in curved.iter().enumerate().take(100).skip(1) {
        let expected = 0.2 + 0.8 * reference_shape(8.0, frame as f64 / 100.0);
        assert!((actual - expected).abs() < 1e-12, "frame {frame}");
    }
    assert_eq!(curved[100], 0.2);
    assert_eq!(curved[119], 0.2);
    // Release: 60 frames from 1/5.
    for (elapsed, actual) in curved[120..180].iter().enumerate() {
        let expected = 0.2 * reference_shape(8.0, elapsed as f64 / 60.0);
        assert!((actual - expected).abs() < 1e-12, "release frame {elapsed}");
    }
    assert_eq!(curved[180], 0.0);
    // The linear voice decays through the same endpoints.
    assert_eq!(linear[100], 0.2);
    assert_eq!(linear[180], 0.0);
}
