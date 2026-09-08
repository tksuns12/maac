use maac::synth::{Adsr, Oscillator, Waveform};

const RATE: f64 = 48_000.0;

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected:.17}, got {actual:.17} (tolerance {tolerance})"
    );
}

#[test]
fn sine_samples_current_phase_then_advances_and_reset_wraps_one() {
    let mut oscillator = Oscillator::new(Waveform::Sine, 0.25).unwrap();
    assert_close(oscillator.sample(12_000.0, RATE).unwrap(), 1.0, 1.0e-12);
    assert_close(oscillator.sample(12_000.0, RATE).unwrap(), 0.0, 1.0e-12);

    oscillator.reset(1.0).unwrap();
    assert_close(oscillator.sample(0.0, RATE).unwrap(), 0.0, 1.0e-12);
    assert_close(oscillator.sample(0.0, RATE).unwrap(), 0.0, 1.0e-12);
}

fn fourier_sample(waveform: Waveform, phase: f64, harmonic_limit: usize) -> f64 {
    let angle = std::f64::consts::TAU * phase;
    match waveform {
        Waveform::Sine => angle.sin(),
        Waveform::Saw => {
            let sum: f64 = (1..=harmonic_limit)
                .map(|harmonic| (angle * harmonic as f64).sin() / harmonic as f64)
                .sum();
            -2.0 * sum / std::f64::consts::PI
        }
        Waveform::Square => {
            let sum: f64 = (1..=harmonic_limit)
                .step_by(2)
                .map(|harmonic| (angle * harmonic as f64).sin() / harmonic as f64)
                .sum();
            4.0 * sum / std::f64::consts::PI
        }
        Waveform::Triangle => {
            let sum: f64 = (1..=harmonic_limit)
                .step_by(2)
                .enumerate()
                .map(|(index, harmonic)| {
                    let sign = if index % 2 == 0 { 1.0 } else { -1.0 };
                    sign * (angle * harmonic as f64).sin() / (harmonic * harmonic) as f64
                })
                .sum();
            8.0 * sum / std::f64::consts::PI.powi(2)
        }
    }
}

#[test]
fn waveforms_select_harmonic_banks_and_interpolate_cyclically() {
    // 6 kHz permits four harmonics at 48 kHz. Grid-aligned phases isolate
    // the documented Fourier tables from the interpolation check below.
    for waveform in [Waveform::Saw, Waveform::Square, Waveform::Triangle] {
        let mut oscillator = Oscillator::new(waveform, 0.125).unwrap();
        assert_close(
            oscillator.sample(6_000.0, RATE).unwrap(),
            fourier_sample(waveform, 0.125, 4),
            1.0e-12,
        );
    }

    // At Nyquist, the fundamental bank is retained by the inclusive rule.
    let mut nyquist = Oscillator::new(Waveform::Saw, 0.125).unwrap();
    assert_close(
        nyquist.sample(24_000.0, RATE).unwrap(),
        fourier_sample(Waveform::Saw, 0.125, 1),
        1.0e-12,
    );

    let mut switched = Oscillator::new(Waveform::Saw, 0.125).unwrap();
    let _ = switched.sample(6_000.0, RATE).unwrap();
    assert_close(
        switched.sample(24_000.0, RATE).unwrap(),
        fourier_sample(Waveform::Saw, 0.25, 1),
        1.0e-12,
    );

    // Halfway between table points, cyclic linear interpolation is the
    // arithmetic mean of the adjacent samples.
    let half_index_phase = 0.5 / 2_048.0;
    let mut interpolated = Oscillator::new(Waveform::Sine, half_index_phase).unwrap();
    let expected = 0.5 * (std::f64::consts::TAU / 2_048.0).sin();
    assert_close(interpolated.sample(0.0, RATE).unwrap(), expected, 1.0e-15);
}

#[test]
fn adsr_uses_fractional_sample_instant_stage_boundaries() {
    // Attack lasts 1.5 frames and decay lasts 2.5 frames. Neither duration is
    // rounded, so frame 2 is already 0.5 frame into decay.
    let envelope = Adsr::new(10, 1.5 / RATE, 2.5 / RATE, 0.25).unwrap();
    assert_close(envelope.value(9, RATE).unwrap(), 0.0, 0.0);
    assert_close(envelope.value(10, RATE).unwrap(), 0.0, 0.0);
    assert_close(envelope.value(11, RATE).unwrap(), 2.0 / 3.0, 1.0e-15);
    assert_close(envelope.value(12, RATE).unwrap(), 0.85, 1.0e-15);
    assert_close(envelope.value(13, RATE).unwrap(), 0.55, 1.0e-15);
    assert_close(envelope.value(14, RATE).unwrap(), 0.25, 0.0);
    assert!(!envelope.finished(1_000, RATE).unwrap());
}

#[test]
fn adsr_release_captures_the_note_off_instant_and_finishes_at_zero() {
    let mut envelope = Adsr::new(10, 4.0 / RATE, 0.0, 1.0).unwrap();
    envelope.release(12, RATE, 2.5 / RATE).unwrap();

    assert_close(envelope.value(12, RATE).unwrap(), 0.5, 0.0);
    assert_close(envelope.value(13, RATE).unwrap(), 0.3, 1.0e-15);
    assert_close(envelope.value(14, RATE).unwrap(), 0.1, 1.0e-15);
    assert!(!envelope.finished(14, RATE).unwrap());
    assert_close(envelope.value(15, RATE).unwrap(), 0.0, 0.0);
    assert!(envelope.finished(15, RATE).unwrap());
}

#[test]
fn zero_adsr_stages_take_effect_at_the_event_frame() {
    let mut envelope = Adsr::new(20, 0.0, 0.0, 0.375).unwrap();
    assert_close(envelope.value(19, RATE).unwrap(), 0.0, 0.0);
    assert_close(envelope.value(20, RATE).unwrap(), 0.375, 0.0);
    assert!(!envelope.finished(20, RATE).unwrap());

    envelope.release(22, RATE, 0.0).unwrap();
    assert_close(envelope.value(22, RATE).unwrap(), 0.0, 0.0);
    assert!(envelope.finished(22, RATE).unwrap());
}

#[test]
fn oscillator_zero_and_negative_frequency_are_deterministic() {
    let mut oscillator = Oscillator::new(Waveform::Sine, 0.125).unwrap();
    let held = oscillator.sample(0.0, RATE).unwrap();
    assert_eq!(
        held.to_bits(),
        oscillator.sample(0.0, RATE).unwrap().to_bits()
    );

    oscillator.reset(0.125).unwrap();
    assert_eq!(
        held.to_bits(),
        oscillator.sample(-6_000.0, RATE).unwrap().to_bits()
    );
    assert_close(oscillator.sample(0.0, RATE).unwrap(), 0.0, 1.0e-15);

    let mut first = Oscillator::new(Waveform::Saw, 0.319).unwrap();
    let mut second = Oscillator::new(Waveform::Saw, 0.319).unwrap();
    for frequency in [0.0, 137.25, 6_000.0, 24_000.0, -11_999.5, -24_000.0] {
        assert_eq!(
            first.sample(frequency, RATE).unwrap().to_bits(),
            second.sample(frequency, RATE).unwrap().to_bits()
        );
    }
}

#[test]
fn zero_frequency_uses_the_richest_bank_and_interpolation_wraps() {
    let phase = 1.0 / 2_048.0;
    let mut richest = Oscillator::new(Waveform::Saw, phase).unwrap();
    assert_close(
        richest.sample(0.0, RATE).unwrap(),
        fourier_sample(Waveform::Saw, phase, 1_024),
        1.0e-12,
    );

    let wrap_phase = 2_047.5 / 2_048.0;
    let mut wrapped = Oscillator::new(Waveform::Sine, wrap_phase).unwrap();
    let expected = 0.5 * (std::f64::consts::TAU * 2_047.0 / 2_048.0).sin();
    assert_close(wrapped.sample(0.0, RATE).unwrap(), expected, 1.0e-15);
}

#[test]
fn synthesis_primitives_reject_invalid_values_without_mutating_phase() {
    for phase in [f64::NAN, f64::INFINITY, -0.001, 1.001] {
        assert!(Oscillator::new(Waveform::Sine, phase).is_err());
    }

    let mut oscillator = Oscillator::new(Waveform::Sine, 0.25).unwrap();
    assert!(oscillator.reset(-0.25).is_err());
    assert_close(oscillator.sample(0.0, RATE).unwrap(), 1.0, 1.0e-12);
    for (frequency, rate) in [
        (f64::NAN, RATE),
        (f64::INFINITY, RATE),
        (24_000.000_001, RATE),
        (-24_000.000_001, RATE),
        (440.0, 0.0),
        (440.0, -RATE),
        (440.0, f64::NAN),
    ] {
        assert!(oscillator.sample(frequency, rate).is_err());
    }

    for (attack, decay, sustain) in [
        (-0.001, 0.0, 1.0),
        (1_800.001, 0.0, 1.0),
        (0.0, f64::NAN, 1.0),
        (0.0, 0.0, -0.001),
        (0.0, 0.0, 1.001),
    ] {
        assert!(Adsr::new(0, attack, decay, sustain).is_err());
    }

    let mut envelope = Adsr::new(0, 0.0, 0.0, 1.0).unwrap();
    assert!(envelope.value(0, 0.0).is_err());
    assert!(envelope.finished(0, f64::INFINITY).is_err());
    assert!(envelope.release(0, RATE, -0.001).is_err());
    assert!(envelope.release(0, RATE, 1_800.001).is_err());
}
