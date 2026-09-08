#![allow(clippy::float_cmp)]

use std::sync::Arc;

use maac::music::{
    validate_frequency, BarCoordinate, MeterMap, MeterPoint, MusicErrorCode, Pitch, TempoMap,
    TempoPoint, TempoShape, Tuning,
};
use num_bigint::BigInt;
use num_rational::BigRational;

fn r(numerator: i64, denominator: i64) -> BigRational {
    BigRational::new(numerator.into(), denominator.into())
}

fn step_tempo(position_q: i64, bpm: i64) -> TempoPoint {
    TempoPoint::new(r(position_q, 1), r(bpm, 1), TempoShape::Step)
}

#[test]
fn step_tempo_is_exact_and_invertible_across_thirds_and_changes() {
    let map = TempoMap::new(vec![step_tempo(0, 120), step_tempo(2, 60)]).unwrap();
    assert_eq!(map.seconds_at(&r(1, 3)).unwrap(), r(1, 6));
    assert_eq!(map.seconds_at(&r(3, 1)).unwrap(), r(2, 1));
    assert_eq!(map.seconds_at(&r(-1, 1)).unwrap(), r(-1, 2));
    assert_eq!(map.q_at(&r(1, 6)).unwrap(), r(1, 3));
    assert_eq!(map.q_at(&r(2, 1)).unwrap(), r(3, 1));
    assert_eq!(map.q_at(&r(-1, 2)).unwrap(), r(-1, 1));
    assert_eq!(
        map.frame_at(&r(1, 3), &r(0, 1), 48_000).unwrap(),
        BigInt::from(8_000)
    );
}

#[test]
fn tempo_validation_and_linear_capability_are_explicit() {
    let unordered = TempoMap::new(vec![step_tempo(1, 120), step_tempo(0, 120)]).unwrap_err();
    assert_eq!(unordered.code, MusicErrorCode::Tempo);

    let linear = TempoMap::new(vec![
        TempoPoint::new(r(0, 1), r(120, 1), TempoShape::Linear),
        step_tempo(4, 180),
    ])
    .unwrap_err();
    let error = linear;
    assert_eq!(error.code, MusicErrorCode::Capability);
}

#[test]
fn meter_bar_coordinates_support_fractional_and_negative_bars() {
    let map = MeterMap::new(vec![MeterPoint::new(r(0, 1), 6, 8)]).unwrap();
    assert_eq!(map.bar_to_q(1, &r(1, 1)).unwrap(), r(0, 1));
    assert_eq!(map.bar_to_q(2, &r(4, 1)).unwrap(), r(9, 2));
    assert_eq!(map.bar_to_q(0, &r(1, 1)).unwrap(), r(-3, 1));
    assert_eq!(map.bar_to_q(-1, &r(4, 1)).unwrap(), r(-9, 2));
    assert_eq!(
        map.q_to_bar(&r(-3, 2)).unwrap(),
        BarCoordinate {
            bar: 0,
            unit: r(4, 1)
        }
    );
    assert_eq!(
        map.q_to_bar(&r(-3, 1)).unwrap(),
        BarCoordinate {
            bar: 0,
            unit: r(1, 1)
        }
    );
}

#[test]
fn tempo_inverse_walks_backward_through_asymmetric_segments() {
    let map = TempoMap::new(vec![step_tempo(-4, 60), step_tempo(0, 120)]).unwrap();
    assert_eq!(map.seconds_at(&r(-2, 1)).unwrap(), r(-2, 1));
    assert_eq!(map.q_at(&r(-2, 1)).unwrap(), r(-2, 1));
    assert_eq!(map.q_at(&r(-4, 1)).unwrap(), r(-4, 1));
}

#[test]
fn q_to_bar_divides_by_bar_length() {
    let map = MeterMap::new(vec![MeterPoint::new(r(0, 1), 4, 4)]).unwrap();
    assert_eq!(
        map.q_to_bar(&r(4, 1)).unwrap(),
        BarCoordinate {
            bar: 2,
            unit: r(1, 1)
        }
    );
}

#[test]
fn meter_changes_must_land_on_preceding_bar_boundaries() {
    let valid = MeterMap::new(vec![
        MeterPoint::new(r(0, 1), 4, 4),
        MeterPoint::new(r(4, 1), 3, 4),
    ])
    .unwrap();
    assert_eq!(valid.bar_to_q(2, &r(1, 1)).unwrap(), r(4, 1));

    let invalid = MeterMap::new(vec![
        MeterPoint::new(r(0, 1), 4, 4),
        MeterPoint::new(r(3, 1), 3, 4),
    ])
    .unwrap_err();
    assert_eq!(invalid.code, MusicErrorCode::MeterBoundary);
    assert_eq!(
        MeterPoint::new(r(0, 1), 4, 3)
            .bar_length_q()
            .unwrap_err()
            .code,
        MusicErrorCode::Range
    );
}

#[test]
fn spelled_key_pitch_preserves_accidentals_and_crosses_octaves() {
    assert!((Pitch::key(69).frequency_hz().unwrap() - 440.0).abs() < 1e-12);
    assert!((Pitch::key(60).frequency_hz().unwrap() - 261.6255653005986).abs() < 1e-12);
    assert_eq!(Pitch::parse_spelled("C4").unwrap().key_index(), Some(60));
    assert_eq!(Pitch::parse_spelled("B#3").unwrap().key_index(), Some(60));
    assert_eq!(Pitch::parse_spelled("Cb4").unwrap().key_index(), Some(59));
    assert_eq!(Pitch::parse_spelled("F##-1").unwrap().key_index(), Some(7));
    assert!((Pitch::parse_spelled("A4").unwrap().frequency_hz().unwrap() - 440.0).abs() < 1e-12);
    assert!(Pitch::parse_spelled("C#b4").is_err());
}

#[test]
fn tuning_uses_floor_division_for_negative_degree_indexes() {
    let tuning = Arc::new(
        Tuning::new(
            r(1200, 1),
            (0..19).map(|step| r(1200 * step, 19)).collect(),
            0,
            r(440, 1),
        )
        .unwrap(),
    );
    assert_eq!(tuning.cents_at(-1).unwrap(), r(-1200, 19));
    let expected = 440.0 * 2.0_f64.powf(-1.0 / 19.0);
    let actual = Pitch::degree(-1, tuning).frequency_hz().unwrap();
    assert!((actual - expected).abs() < 1e-12);
}

#[test]
fn absolute_ratio_and_nyquist_rules_reject_nonfinite_or_aliased_values() {
    assert!((Pitch::hz(r(3, 2)).unwrap().frequency_hz().unwrap() - 1.5).abs() < 1e-12);
    assert!(
        (Pitch::ratio(r(3, 2), r(440, 1))
            .unwrap()
            .frequency_hz()
            .unwrap()
            - 660.0)
            .abs()
            < 1e-12
    );
    assert_eq!(
        Pitch::hz(r(24_000, 1))
            .unwrap()
            .resolve_hz(Some(24_000.0))
            .unwrap_err()
            .code,
        MusicErrorCode::Range
    );
    assert_eq!(
        validate_frequency(f64::NAN, None).unwrap_err().code,
        MusicErrorCode::Nonfinite
    );
    assert!(Pitch::key(i64::MAX).frequency_hz().is_err());
}

#[test]
fn rational_bounds_apply_to_pitch_inputs() {
    let too_wide = BigRational::from_integer(BigInt::from(1u8) << 4096);
    let error = Pitch::hz(too_wide).unwrap_err();
    assert_eq!(error.code, MusicErrorCode::ResourceLimit);
}
