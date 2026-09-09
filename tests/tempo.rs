use maac::music::{MusicErrorCode, TempoPoint, TempoShape};
use maac::tempo::{RampTempoMap, TimeValue, TimingBudget};
use maac::{parse_rational as r, Rational};
use num_bigint::BigInt;
use std::cmp::Ordering;
fn p(q: &str, b: &str, s: TempoShape) -> TempoPoint {
    TempoPoint::new(r(q).unwrap(), r(b).unwrap(), s)
}
fn ramp(a: &str, b: &str) -> RampTempoMap {
    RampTempoMap::new(vec![
        p("0", a, TempoShape::Linear),
        p("1", b, TempoShape::Step),
    ])
    .unwrap()
}

#[test]
fn shared_timing_budget_exhausts_across_successful_operations() {
    let logarithmic = ramp("60", "120").seconds_at(&r("1").unwrap()).unwrap();
    let zero = TimeValue::from_rational(r("0").unwrap()).unwrap();
    let mut budget = TimingBudget::new(256);

    assert_eq!(
        logarithmic.compare_with_budget(&zero, &mut budget).unwrap(),
        Ordering::Greater
    );
    assert_eq!(budget.remaining_terms(), 128);
    assert_eq!(
        logarithmic.ceil_frames_with_budget(1, &mut budget).unwrap(),
        BigInt::from(1)
    );
    assert_eq!(budget.remaining_terms(), 0);
    assert_eq!(
        logarithmic
            .approximate_seconds_with_budget(&mut budget)
            .unwrap_err()
            .code,
        MusicErrorCode::ResourceLimit
    );
    assert_eq!(budget.remaining_terms(), 0);
}

#[test]
fn timing_budgets_are_isolated_and_failed_reservations_are_atomic() {
    let logarithmic = ramp("60", "120").seconds_at(&r("1").unwrap()).unwrap();
    let zero = TimeValue::from_rational(r("0").unwrap()).unwrap();
    let mut first = TimingBudget::new(128);
    let mut second = TimingBudget::new(128);

    assert_eq!(
        logarithmic.compare_with_budget(&zero, &mut first).unwrap(),
        Ordering::Greater
    );
    assert_eq!(first.remaining_terms(), 0);
    assert_eq!(second.remaining_terms(), 128);
    assert_eq!(
        logarithmic.compare_with_budget(&zero, &mut second).unwrap(),
        Ordering::Greater
    );

    let mut too_small = TimingBudget::new(127);
    assert_eq!(
        logarithmic
            .compare_with_budget(&zero, &mut too_small)
            .unwrap_err()
            .code,
        MusicErrorCode::ResourceLimit
    );
    assert_eq!(too_small.remaining_terms(), 127);
}

#[test]
fn rational_timing_paths_do_not_consume_numerical_budget() {
    let half = TimeValue::from_rational(r("1/2").unwrap()).unwrap();
    let one = TimeValue::from_rational(r("1").unwrap()).unwrap();
    let mut budget = TimingBudget::new(0);

    assert_eq!(
        half.compare_with_budget(&one, &mut budget).unwrap(),
        Ordering::Less
    );
    assert_eq!(
        half.ceil_frames_with_budget(48_000, &mut budget).unwrap(),
        BigInt::from(24_000)
    );
    assert_eq!(
        half.approximate_seconds_with_budget(&mut budget).unwrap(),
        0.5
    );
    assert_eq!(budget.remaining_terms(), 0);
    assert_eq!(TimingBudget::new(u64::MAX).remaining_terms(), 20_000_000);
}
#[test]
fn ascending_descending_and_analytic_bracket() {
    // Independent decimal enclosure of ln(2), 40 decimal places.
    let lo = r("0.6931471805599453094172321214581765680755").unwrap();
    let hi = r("0.6931471805599453094172321214581765680756").unwrap();
    for map in [ramp("60", "120"), ramp("120", "60")] {
        let t = map
            .seconds_between(&r("0").unwrap(), &r("1").unwrap())
            .unwrap();
        assert_eq!(
            t.compare(&TimeValue::from_rational(lo.clone()).unwrap())
                .unwrap(),
            Ordering::Greater
        );
        assert_eq!(
            t.compare(&TimeValue::from_rational(hi.clone()).unwrap())
                .unwrap(),
            Ordering::Less
        );
        assert_eq!(t.ceil_frames(48000).unwrap(), BigInt::from(33272));
        assert!((t.approximate_seconds().unwrap() - std::f64::consts::LN_2).abs() < 1e-15);
    }
}
#[test]
fn flat_mixed_negative_and_exact_cancel() {
    let m = RampTempoMap::new(vec![
        p("-1", "60", TempoShape::Linear),
        p("0", "60", TempoShape::Step),
        p("1", "120", TempoShape::Step),
    ])
    .unwrap();
    let t = m
        .seconds_between(&r("-2").unwrap(), &r("2").unwrap())
        .unwrap();
    assert_eq!(t.as_rational(), Some(&r("7/2").unwrap()));
    let x = ramp("60", "120").seconds_at(&r("1").unwrap()).unwrap();
    assert_eq!(
        x.difference(&x).unwrap().ceil_frames(48000).unwrap(),
        BigInt::from(0)
    );
    assert_eq!(x.compare(&x).unwrap(), Ordering::Equal);
    assert_eq!(
        TimeValue::from_rational(r("-1/3").unwrap())
            .unwrap()
            .ceil_frames(2)
            .unwrap(),
        BigInt::from(0)
    );
}
#[test]
fn offsets_and_huge_local_origin() {
    let base = BigInt::from(1) << 3000;
    let a = Rational::from_integer(base);
    let b = &a + r("1").unwrap();
    let m = RampTempoMap::new(vec![
        TempoPoint::new(a.clone(), r("60").unwrap(), TempoShape::Linear),
        TempoPoint::new(b.clone(), r("120").unwrap(), TempoShape::Step),
    ])
    .unwrap();
    let t = m
        .seconds_between(&a, &b)
        .unwrap()
        .add_offset(&r("-1").unwrap())
        .unwrap();
    assert_eq!(t.ceil_frames(48000).unwrap(), BigInt::from(-14728));
}
#[test]
fn invalid_and_resource_inputs() {
    assert_eq!(
        RampTempoMap::new(vec![]).unwrap_err().code,
        MusicErrorCode::Tempo
    );
    assert_eq!(
        RampTempoMap::new(vec![p("0", "0", TempoShape::Step)])
            .unwrap_err()
            .code,
        MusicErrorCode::Tempo
    );
    assert_eq!(
        RampTempoMap::new(vec![p("0", "60", TempoShape::Linear)])
            .unwrap_err()
            .code,
        MusicErrorCode::Tempo
    );
    assert_eq!(
        TimeValue::from_rational(Rational::from_integer(BigInt::from(1) << 4096))
            .unwrap_err()
            .code,
        MusicErrorCode::ResourceLimit
    );
}
#[test]
fn integer_boundary_refines_and_exhaustion_is_explicit() {
    let t = ramp("60", "120").seconds_at(&r("1").unwrap()).unwrap();
    let lo = r("0.6931471805599453094172321214581765680755").unwrap();
    let hi = r("0.6931471805599453094172321214581765680756").unwrap();
    let after = t.add_offset(&(-lo)).unwrap();
    let before = t.add_offset(&(-hi)).unwrap();
    assert_eq!(
        after.ceil_frames_with_precision(1, 64).unwrap_err().code,
        MusicErrorCode::TimePrecision
    );
    assert_eq!(after.ceil_frames(1).unwrap(), BigInt::from(1));
    assert_eq!(before.ceil_frames(1).unwrap(), BigInt::from(0));
    let tiny = Rational::new(BigInt::from(1), BigInt::from(1) << 2000);
    let almost_flat = RampTempoMap::new(vec![
        p("0", "60", TempoShape::Linear),
        TempoPoint::new(
            r("1").unwrap(),
            r("60").unwrap() * (r("1").unwrap() + tiny),
            TempoShape::Step,
        ),
    ])
    .unwrap();
    assert_eq!(
        almost_flat
            .seconds_at(&r("1").unwrap())
            .unwrap()
            .ceil_frames(1)
            .unwrap_err()
            .code,
        MusicErrorCode::TimePrecision
    );
}
#[test]
fn independent_alternating_series_oracle_and_mixed_map() {
    // ln(1+x)=x-x²/2+x³/3-...; alternating remainder brackets x=1/2.
    // This oracle has no range reduction, atanh, or fixed-point rounding.
    let mut sum = r("0").unwrap();
    let mut power = r("1").unwrap();
    for n in 1..=160 {
        power *= r("1/2").unwrap();
        let term = &power / Rational::from_integer(n.into());
        if n % 2 == 1 {
            sum += term
        } else {
            sum -= term
        }
    }
    let upper = &sum + (&power * r("1/2").unwrap()) / r("161").unwrap();
    let t = ramp("60", "120").seconds_at(&r("1/2").unwrap()).unwrap();
    assert_eq!(
        t.compare(&TimeValue::from_rational(sum).unwrap()).unwrap(),
        Ordering::Greater
    );
    assert_eq!(
        t.compare(&TimeValue::from_rational(upper).unwrap())
            .unwrap(),
        Ordering::Less
    );
    let m = RampTempoMap::new(vec![
        p("-1", "60", TempoShape::Linear),
        p("0", "120", TempoShape::Step),
        p("2", "60", TempoShape::Step),
    ])
    .unwrap();
    let duration = m
        .seconds_between(&r("-2").unwrap(), &r("3").unwrap())
        .unwrap();
    let expected = ramp("60", "120")
        .seconds_at(&r("1").unwrap())
        .unwrap()
        .add_offset(&r("3").unwrap())
        .unwrap();
    assert_eq!(duration.compare(&expected).unwrap(), Ordering::Equal);
    assert_eq!(
        m.seconds_between(&r("3").unwrap(), &r("-2").unwrap())
            .unwrap()
            .compare(&expected.negated())
            .unwrap(),
        Ordering::Equal
    );
}
#[test]
fn bounds_preflight_and_legacy_compatibility() {
    assert_eq!(
        RampTempoMap::new(vec![p("0", "60", TempoShape::Step); 4097])
            .unwrap_err()
            .code,
        MusicErrorCode::ResourceLimit
    );
    assert_eq!(
        TimeValue::from_rational(Rational::new_raw(1.into(), 0.into()))
            .unwrap_err()
            .code,
        MusicErrorCode::Range
    );
    let pts = vec![
        p("-3", "77", TempoShape::Step),
        p("2/3", "121", TempoShape::Step),
    ];
    let legacy = maac::music::TempoMap::new(pts.clone()).unwrap();
    let new = RampTempoMap::new(pts).unwrap();
    for q in ["-9", "-3", "0", "1/7", "2/3", "7"] {
        let q = r(q).unwrap();
        assert_eq!(
            new.seconds_at(&q).unwrap().as_rational(),
            Some(&legacy.seconds_at(&q).unwrap())
        );
    }
    assert_eq!(
        RampTempoMap::new(vec![
            p("0", "60", TempoShape::Step),
            p("0", "70", TempoShape::Step)
        ])
        .unwrap_err()
        .code,
        MusicErrorCode::Tempo
    );
}

#[test]
fn range_reduction_reciprocal_cancellation_and_dsp_exhaustion() {
    let t = ramp("60", "960").seconds_at(&r("1").unwrap()).unwrap();
    assert!((t.approximate_seconds().unwrap() - 16_f64.ln() / 15.0).abs() < 1e-15);
    let forward = ramp("60", "120").seconds_at(&r("1").unwrap()).unwrap();
    let backward = ramp("120", "60").seconds_at(&r("1").unwrap()).unwrap();
    assert_eq!(forward.compare(&backward).unwrap(), Ordering::Equal);
    let tiny = Rational::new(BigInt::from(1), BigInt::from(1) << 2000);
    let map = RampTempoMap::new(vec![
        p("0", "60", TempoShape::Linear),
        TempoPoint::new(
            r("1").unwrap(),
            r("60").unwrap() * (r("1").unwrap() + tiny),
            TempoShape::Step,
        ),
    ])
    .unwrap();
    assert_eq!(
        map.seconds_at(&r("1").unwrap())
            .unwrap()
            .approximate_seconds()
            .unwrap_err()
            .code,
        MusicErrorCode::TimePrecision
    );
}

#[test]
fn public_invalid_boundaries_and_maximum_rate_are_explicit() {
    let raw_negative_denominator = Rational::new_raw((-1).into(), (-2).into());
    for point in [
        TempoPoint::new(
            raw_negative_denominator.clone(),
            r("60").unwrap(),
            TempoShape::Step,
        ),
        TempoPoint::new(
            r("0").unwrap(),
            raw_negative_denominator.clone(),
            TempoShape::Step,
        ),
    ] {
        assert_eq!(
            RampTempoMap::new(vec![point]).unwrap_err().code,
            MusicErrorCode::Range
        );
    }
    let value = TimeValue::from_rational(r("1/2").unwrap()).unwrap();
    assert_eq!(
        value
            .add_offset(&raw_negative_denominator)
            .unwrap_err()
            .code,
        MusicErrorCode::Range
    );
    assert_eq!(
        value.ceil_frames(0).unwrap_err().code,
        MusicErrorCode::Range
    );
    for cap in [0, 63, 65, 2048, usize::MAX] {
        assert_eq!(
            value
                .ceil_frames_with_precision(48000, cap)
                .unwrap_err()
                .code,
            MusicErrorCode::Range
        );
    }
    // u64::MAX is odd: +/- rate/2 have different signed mathematical ceilings.
    assert_eq!(
        value.ceil_frames(u64::MAX).unwrap(),
        BigInt::from(1u8) << 63
    );
    assert_eq!(
        value.negated().ceil_frames(u64::MAX).unwrap(),
        -(BigInt::from(1u8) << 63usize) + 1
    );
    let logarithmic = ramp("60", "120").seconds_at(&r("1").unwrap()).unwrap();
    assert_eq!(
        logarithmic.ceil_frames(0).unwrap_err().code,
        MusicErrorCode::Range
    );
}

#[test]
fn common_log_cancellation_leaves_exact_nonzero_integer_frames() {
    let t = ramp("60", "120").seconds_at(&r("1").unwrap()).unwrap();
    let offset = r("7/48000").unwrap();
    let remainder = t.add_offset(&offset).unwrap().difference(&t).unwrap();
    assert_eq!(remainder.as_rational(), Some(&offset));
    assert_eq!(remainder.ceil_frames(48000).unwrap(), BigInt::from(7));
    assert_eq!(
        remainder.negated().ceil_frames(48000).unwrap(),
        BigInt::from(-7)
    );
}

#[test]
fn wide_non_power_of_two_log_has_independent_rational_bracket() {
    // ln(3 * 2^200) = ln(3/2) + 201 ln(2). Enclose ln(3/2) with
    // the independent alternating ln(1+x) series, not the atanh evaluator.
    let mut lower = r("0").unwrap();
    let mut power = r("1").unwrap();
    for n in 1..=80 {
        power *= r("1/2").unwrap();
        let term = &power / Rational::from_integer(n.into());
        if n % 2 == 1 {
            lower += term;
        } else {
            lower -= term;
        }
    }
    let mut upper = &lower + (&power / r("2").unwrap()) / r("81").unwrap();
    lower += r("201").unwrap() * r("0.6931471805599453094172321214581765680755").unwrap();
    upper += r("201").unwrap() * r("0.6931471805599453094172321214581765680756").unwrap();
    let ratio = Rational::from_integer(BigInt::from(3u8) << 200);
    let first_bpm = r("60").unwrap() / (&ratio - r("1").unwrap());
    let last_bpm = &first_bpm * ratio;
    for (first, last) in [(first_bpm.clone(), last_bpm.clone()), (last_bpm, first_bpm)] {
        let map = RampTempoMap::new(vec![
            TempoPoint::new(r("0").unwrap(), first, TempoShape::Linear),
            TempoPoint::new(r("1").unwrap(), last, TempoShape::Step),
        ])
        .unwrap();
        let duration = map.seconds_at(&r("1").unwrap()).unwrap();
        assert_eq!(
            duration
                .compare(&TimeValue::from_rational(lower.clone()).unwrap())
                .unwrap(),
            Ordering::Greater
        );
        assert_eq!(
            duration
                .compare(&TimeValue::from_rational(upper.clone()).unwrap())
                .unwrap(),
            Ordering::Less
        );
    }
}

#[test]
fn distinct_log_term_allowance_accepts_4096_and_rejects_4097() {
    fn consecutive_logs(first: u32, count: u32) -> TimeValue {
        // Each unit segment has slope 60 and contributes ln((n+1)/n).
        // Reduced arguments are distinct; two map constructions avoid repeated
        // cloning via thousands of incremental TimeValue::sum calls.
        let points = (0..=count)
            .map(|i| {
                TempoPoint::new(
                    Rational::from_integer(i.into()),
                    Rational::from_integer((60 * (first + i)).into()),
                    if i == count {
                        TempoShape::Step
                    } else {
                        TempoShape::Linear
                    },
                )
            })
            .collect();
        RampTempoMap::new(points)
            .unwrap()
            .seconds_at(&Rational::from_integer(count.into()))
            .unwrap()
    }
    let first = consecutive_logs(1, 2048);
    let second = consecutive_logs(2049, 2048);
    let at_limit = first.sum(&second).unwrap();
    assert!(at_limit.as_rational().is_none());
    let extra = consecutive_logs(4097, 1);
    assert_eq!(
        at_limit.sum(&extra).unwrap_err().code,
        MusicErrorCode::ResourceLimit
    );
    // A matching term cancels at capacity without consuming another slot.
    assert!(at_limit.sum(&first.negated()).unwrap().sum(&extra).is_ok());
}
