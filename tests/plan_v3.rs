use maac::{
    load_plan_versioned,
    plan_v3::{PlanV3, VersionedPlan},
};

fn ramp_json() -> Vec<u8> {
    br#"{"version":3,"output":{"score_start_q":"0/1","score_end_q":"4/1","tail_seconds":"0/1","sample_rate_hz":48000,"channels":1,"total_frames":66543,"output":{"node":"sine","port":"out"}},"tempo":{"points":[{"q":"0/1","bpm":"120/1","shape":"linear"},{"q":"4/1","bpm":"240/1","shape":"step"}]},"nodes":[{"id":"sine","processor":{"kind":"sine","voices":1},"params":{}}]}"#.to_vec()
}
#[test]
fn analytical_ascending_ramp_roundtrips_without_wrapper() {
    // 60/30 * ln(240/120) * 48000 = 66542.129333...
    let plan = PlanV3::from_json(&ramp_json()).unwrap();
    assert_eq!(plan.output.total_frames, 66543);
    let json = plan.to_json().unwrap();
    assert!(matches!(
        load_plan_versioned(&json).unwrap(),
        VersionedPlan::V3(_)
    ));
}
#[test]
fn forged_frames_and_duplicate_version_reject() {
    let json = String::from_utf8(ramp_json()).unwrap();
    assert!(PlanV3::from_json(json.replace("66543", "66542").as_bytes()).is_err());
    assert!(load_plan_versioned(
        json.replace("\"version\":3", "\"version\":3,\"version\":3")
            .as_bytes()
    )
    .is_err());
}

use maac::plan::{
    AutomationClock, AutomationPoint, EventKind, EventTarget, Interpolation, PlanLimits,
    SourceMapping,
};
use maac::plan_v3::{AutomationAnchor, AutomationV3, ResolvedEventV3};
fn r(n: i64, d: i64) -> maac::Rational {
    maac::Rational::new(n.into(), d.into())
}
fn base() -> PlanV3 {
    PlanV3::from_json(&ramp_json()).unwrap()
}
fn note() -> ResolvedEventV3 {
    ResolvedEventV3 {
        address: "main/0/n1".into(),
        source: SourceMapping {
            object: "n1".into(),
            path: vec!["main".into(), "n1".into()],
            span: None,
        },
        target: EventTarget::new("sine", "events").unwrap(),
        kind: EventKind::Note {
            pitch_hz: 440.,
            velocity: r(1, 1),
            pitch_expression: None,
            gain_expression: None,
            timbre_expression: None,
            pressure_expression: None,
        },
        score_on_q: r(0, 1),
        score_off_q: Some(r(4, 1)),
        onset_offset_seconds: r(0, 1),
        release_offset_seconds: r(0, 1),
        release_velocity: 0.,
        on_frame: 0,
        off_frame: Some(66543),
        order: 0,
    }
}
#[test]
fn descending_flat_mixed_and_huge_origin() {
    let mut p = base();
    p.tempo.points[0].bpm = r(240, 1);
    p.tempo.points[1].bpm = r(120, 1);
    p.validate().unwrap();
    p.tempo.points[0].bpm = r(120, 1);
    p.output.total_frames = 96000;
    p.validate().unwrap();
    p = base();
    p.output.score_end_q = r(6, 1);
    p.output.total_frames = 90543;
    p.validate().unwrap();
    p = base();
    let origin = maac::Rational::from_integer(num_bigint::BigInt::from(1) << 500);
    p.output.score_start_q = origin.clone();
    p.output.score_end_q = &origin + r(4, 1);
    p.tempo.points[0].q = origin.clone();
    p.tempo.points[1].q = &origin + r(4, 1);
    PlanV3::from_json(&p.to_json().unwrap()).unwrap();
}
#[test]
fn event_frames_offsets_clamp_and_wire_fields() {
    let mut p = base();
    p.events.push(note());
    p.validate().unwrap();
    let json = String::from_utf8(p.to_json().unwrap()).unwrap();
    assert!(PlanV3::from_json(
        json.replace("\"on_frame\":0", "\"on_frame\":0,\"on_seconds\":\"0/1\"")
            .as_bytes()
    )
    .is_err());
    p.events[0].off_frame = Some(66542);
    assert!(p.validate().is_err());
    p.events[0].off_frame = Some(66543);
    p.events[0].release_offset_seconds = r(1, 1);
    p.validate().unwrap();
    p.events[0].onset_offset_seconds = r(-1, 1);
    assert!(p.validate().is_err());
    p.events[0].onset_offset_seconds = r(2, 1);
    assert!(p.validate().is_err());
    p.events[0] = note();
    p.events[0].score_off_q = Some(r(8, 1));
    p.events[0].release_offset_seconds = r(-2, 1);
    assert!(p.validate().is_err());
}
#[test]
fn strict_anchors_shapes_and_budget_preflight() {
    let mut p = base();
    p.tempo.points[0].shape = Interpolation::Exponential;
    assert!(p.validate().is_err());
    p = base();
    p.automation.push(AutomationV3 {
        id: "volume".into(),
        target: maac::plan::PortRef::new("sine", "gain").unwrap(),
        clock: AutomationClock::Score,
        at: AutomationAnchor::Seconds { seconds: r(0, 1) },
        points: vec![AutomationPoint {
            position: r(0, 1),
            value: r(1, 1),
            shape: Interpolation::Step,
        }],
    });
    assert!(p.validate().is_err());
    p = base();
    let limits = PlanLimits {
        max_work: 23,
        ..PlanLimits::default()
    };
    assert_eq!(
        p.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
    let limits = PlanLimits {
        max_json_bytes: 10,
        ..PlanLimits::default()
    };
    assert_eq!(
        PlanV3::from_json_with_limits(&ramp_json(), &limits)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    p.version = 1;
    assert_eq!(p.validate().unwrap_err().code, "E_VERSION");
}

#[test]
fn typed_versions_cannot_cross_wire_schemas() {
    let mut p = base();
    for version in [1, 2] {
        p.version = version;
        assert_eq!(p.to_json().unwrap_err().code, "E_VERSION");
    }
    let legacy_json = String::from_utf8(ramp_json())
        .unwrap()
        .replace("\"version\":3", "\"version\":1")
        .replace("\"linear\"", "\"step\"")
        .replace("66543", "96000");
    let mut legacy = maac::Plan::from_json(legacy_json.as_bytes()).unwrap();
    legacy.version = 3;
    assert_eq!(legacy.validate().unwrap_err().code, "E_VERSION");
    assert!(legacy.to_json().is_err());
    assert!(PlanV3::from_json(legacy_json.as_bytes()).is_err());
    assert!(maac::Plan::from_json(&ramp_json()).is_err());
}
#[test]
fn automation_before_origin_and_strict_nested_anchor() {
    let mut p = base();
    p.automation.push(AutomationV3 {
        id: "level".into(),
        target: maac::plan::PortRef::new("sine", "level").unwrap(),
        clock: AutomationClock::Seconds,
        at: AutomationAnchor::Seconds { seconds: r(-1, 1) },
        points: vec![AutomationPoint {
            position: r(0, 1),
            value: r(1, 1),
            shape: Interpolation::Step,
        }],
    });
    p.validate().unwrap();
    let json = String::from_utf8(p.to_json().unwrap()).unwrap();
    assert!(PlanV3::from_json(
        json.replace(
            "\"seconds\":\"-1/1\"",
            "\"seconds\":\"-1/1\",\"seconds\":\"-1/1\""
        )
        .as_bytes()
    )
    .is_err());
    assert!(PlanV3::from_json(
        json.replace("\"seconds\":\"-1/1\"", "\"seconds\":\"-1/1\",\"q\":\"0/1\"")
            .as_bytes()
    )
    .is_err());
    p.automation[0].clock = AutomationClock::Score;
    assert!(p.validate().is_err());
    p.automation[0].at = AutomationAnchor::Score { q: r(-1, 1) };
    p.validate().unwrap();
}
#[test]
fn unresolved_precision_and_collapsed_positive_gate_fail() {
    let mut p = base();
    let tiny = maac::Rational::new(1.into(), num_bigint::BigInt::from(1) << 2000);
    p.tempo.points[0].bpm = r(60, 1);
    p.tempo.points[1].bpm = r(60, 1) * (r(1, 1) + tiny);
    assert_eq!(p.validate().unwrap_err().code, "E_TIME_PRECISION");
    p = base();
    p.events.push(note());
    p.events[0].score_on_q = r(1, 1000000);
    p.events[0].score_off_q = Some(r(2, 1000000));
    p.events[0].on_frame = 1;
    p.events[0].off_frame = Some(1);
    assert_eq!(p.validate().unwrap_err().code, "E_SUBSAMPLE_NOTE");
}

#[test]
fn rational_and_collection_bombs_precede_numeric_work() {
    let mut p = base();
    p.output.score_start_q = maac::Rational::from_integer(num_bigint::BigInt::from(1) << 5000);
    assert_eq!(p.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
    let json = String::from_utf8(ramp_json()).unwrap();
    assert_eq!(
        PlanV3::from_json(
            json.replace(
                "\"score_start_q\":\"0/1\"",
                &format!("\"score_start_q\":\"{}/1\"", "9".repeat(5000))
            )
            .as_bytes()
        )
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );
    p = base();
    p.events = vec![note(); 3];
    let limits = PlanLimits {
        max_events: 2,
        ..PlanLimits::default()
    };
    assert_eq!(
        p.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
    p = base();
    // Traversal allowance alone is 42; structural work must be charged too.
    let limits = PlanLimits {
        max_work: 42,
        ..PlanLimits::default()
    };
    assert_eq!(
        p.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}
