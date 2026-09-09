use maac::plan_v3::VersionedPlan;
use maac::{
    compiler::{compile, compile_versioned},
    parse,
};
fn source(shape: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }}
tempo clock {{ points = [(0q, 60bpm, {shape}), (4q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node s {{ type = "core.sine/1"; config = {{ voices = 8; }}; }}
track t {{ target = &s:events; }}
pattern pat {{ length = 4q; note n {{ at = 0q; dur = 4q; pitch = 440Hz; velocity = 1; }} }}
place x {{ pattern = &pat; track = &t; at = 0q; }}"#
    )
}
#[test]
fn ramps_are_additive_and_certified() {
    let doc = parse(&source("linear")).unwrap();
    assert!(compile(&doc).is_err());
    let VersionedPlan::V3(plan) = compile_versioned(&doc).unwrap() else {
        panic!("expected v3")
    };
    assert_eq!(plan.output.total_frames, 133085);
    assert_eq!(plan.events[0].off_frame, Some(133085));
}
#[test]
fn step_output_is_identical() {
    let doc = parse(&source("step")).unwrap();
    assert_eq!(
        compile_versioned(&doc).unwrap(),
        VersionedPlan::Legacy(compile(&doc).unwrap())
    );
}

#[test]
fn anchors_keep_their_source_clock() {
    use maac::plan_v3::AutomationAnchor;
    for (at, score) in [("1q", true), ("bar(1, 2)", true), ("1s", false)] {
        let text=source("linear")+&format!("curve c {{ clock = seconds; points = [(0s, 0s, step), (1s, 1s, step)]; }} automation a {{ target = &s.params.release; curve = &c; at = {at}; }}");
        let VersionedPlan::V3(plan) = compile_versioned(&parse(&text).unwrap()).unwrap() else {
            panic!()
        };
        match &plan.automation[0].at {
            AutomationAnchor::Score { q } => {
                assert!(score);
                assert_eq!(*q, maac::Rational::from_integer(1.into()));
            }
            AutomationAnchor::Seconds { seconds } => {
                assert!(!score);
                assert_eq!(*seconds, maac::Rational::from_integer(1.into()));
            }
        }
    }
}
#[test]
fn shifted_origins_offsets_and_end_clamp() {
    for origin in [-4, 8] {
        let text = source("linear")
            .replace(
                "score = [0q, 4q]",
                &format!("score = [{origin}q, {}q]", origin + 4),
            )
            .replace(
                "(0q, 60bpm, linear), (4q, 120bpm, step)",
                &format!(
                    "({origin}q, 60bpm, linear), ({}q, 120bpm, step)",
                    origin + 4
                ),
            )
            .replace(
                "track = &t; at = 0q",
                &format!("track = &t; at = {origin}q"),
            )
            .replace(
                "dur = 4q;",
                "dur = 5q; onset_offset = 1/10s; release_offset = -1/10s;",
            );
        let VersionedPlan::V3(plan) = compile_versioned(&parse(&text).unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(plan.output.total_frames, 133085);
        assert_eq!(plan.events[0].on_frame, 4800);
        assert_eq!(plan.events[0].off_frame, Some(133085));
    }
}
#[test]
fn invalid_unused_declarations_and_budgets_fail() {
    use maac::compiler::compile_versioned_with_limits;
    let invalid = source("linear") + " tempo unused { points = [(0q, 100bpm, exponential)]; }";
    assert!(compile_versioned(&parse(&invalid).unwrap()).is_err());
    let doc = parse(&source("linear")).unwrap();
    let limits = maac::plan::PlanLimits {
        max_work: 1,
        ..Default::default()
    };
    assert!(compile_versioned_with_limits(&doc, &limits).is_err());
    let limits = maac::plan::PlanLimits {
        max_events: 0,
        ..Default::default()
    };
    assert!(compile_versioned_with_limits(&doc, &limits).is_err());
}

#[test]
fn source_compile_shares_one_numerical_budget_across_scheduling_and_validation() {
    use maac::bundle::{sha256_digest, SourceBundle};
    use maac::compiler::{compile_bundle_versioned_with_limits, compile_versioned_with_limits};
    use maac::plan::PlanLimits;
    use maac::production_data::SCHEMA_BYTES;

    let text = r#"maac 1;
project p { score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }
tempo clock { points = [(0q, 120bpm, linear), (4q, 240bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node s { type = "core.sine/1"; config = { voices = 1; }; }"#;
    let doc = parse(text).unwrap();

    let exhausted = compile_versioned_with_limits(
        &doc,
        &PlanLimits {
            max_work: 256,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        exhausted.first().unwrap().code,
        maac::DiagnosticCode::ResourceLimit
    );

    compile_versioned_with_limits(
        &doc,
        &PlanLimits {
            max_work: 512,
            ..Default::default()
        },
    )
    .unwrap();

    let production_text = text.replace(
        "score = [0q, 4q];",
        "score = [0q, 4q]; requires = [\"maac.production/1\"];",
    ) + &format!(
        r#"
asset schema {{ kind = descriptor; path = "schema.json"; hash = "{}"; }}
extension deliveries {{ namespace = "maac.production/1"; schema = &schema; render_affecting = true; data = {{ deliveries = {{ release = {{ rate = 48000Hz; resampler = "maac.src.kaiser/1"; targets = {{ master = {{ role = master; output = &s:out; encoding = wav_f32le; dither = {{ type = none; }}; }}; }}; }}; }}; }}; }}"#,
        sha256_digest(SCHEMA_BYTES)
    );
    let mut bundle = SourceBundle::new("main.maac", production_text);
    bundle
        .assets
        .insert("schema.json".into(), SCHEMA_BYTES.to_vec());
    let VersionedPlan::V3(plan) = compile_bundle_versioned_with_limits(
        &bundle,
        &PlanLimits {
            max_work: 512,
            ..Default::default()
        },
    )
    .unwrap() else {
        panic!("expected v3")
    };
    assert!(plan.production.is_some());
    plan.validate().unwrap();

    let aggregate_limit = compile_bundle_versioned_with_limits(
        &bundle,
        &PlanLimits {
            max_work: 256,
            max_objects: 7,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        aggregate_limit.first().unwrap().code,
        maac::DiagnosticCode::ResourceLimit
    );
    assert_eq!(
        aggregate_limit.first().unwrap().message,
        "aggregate plan object limit exceeded"
    );

    compile_bundle_versioned_with_limits(
        &bundle,
        &PlanLimits {
            max_work: 512,
            max_objects: 13,
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn version_three_validation_shares_one_fresh_budget_per_call() {
    use maac::plan::{AutomationClock, AutomationPoint, Interpolation, PlanLimits, PortRef};
    use maac::plan_v3::{AutomationAnchor, AutomationV3};

    let doc = parse(
        r#"maac 1;
project p { score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }
tempo clock { points = [(0q, 120bpm, linear), (4q, 240bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node s { type = "core.sine/1"; config = { voices = 1; }; }"#,
    )
    .unwrap();
    let VersionedPlan::V3(mut plan) = compile_versioned(&doc).unwrap() else {
        panic!("expected v3")
    };
    plan.automation.push(AutomationV3 {
        id: "release".into(),
        target: PortRef {
            node: "s".into(),
            port: "release".into(),
        },
        clock: AutomationClock::Seconds,
        at: AutomationAnchor::Score {
            q: maac::Rational::from_integer(1.into()),
        },
        points: vec![AutomationPoint {
            position: maac::Rational::from_integer(0.into()),
            value: maac::Rational::new(1.into(), 10.into()),
            shape: Interpolation::Step,
        }],
    });

    let exhausted = PlanLimits {
        max_work: 256,
        ..Default::default()
    };
    assert_eq!(
        plan.validate_with_limits(&exhausted).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );

    let sufficient = PlanLimits {
        max_work: 384,
        ..Default::default()
    };
    plan.validate_with_limits(&sufficient).unwrap();
    plan.validate_with_limits(&sufficient).unwrap();
}

#[test]
fn instrument_bundle_expression_and_production_paths() {
    use maac::production_data::SCHEMA_BYTES;
    use maac::{
        bundle::{sha256_digest, SourceBundle},
        compiler::{check_bundle_versioned, compile_bundle_versioned},
    };
    let text=source("linear").replace("node s { type = \"core.sine/1\"; config = { voices = 8; }; }",r#"instrument lead { channels = 1; voice v { channels = 1; amplitude = &amp; output = &osc:out; node amp { type = "synth.adsr/1"; } node osc { type = "synth.sine/1"; } } } node s { instrument = &lead; config = { voices = 8; }; }"#)
 .replace("velocity = 1;", "velocity = 1; expression e { kind = gain; curve = &gain; }")
 .replace("score = [0q, 4q];", "score = [0q, 4q]; requires = [\"maac.production/1\"];")
 + &format!(r#"curve gain {{ clock = normalized; points = [(0, 1, linear), (1, 0, step)]; }}
 asset schema {{ kind = descriptor; path = "schema.json"; hash = "{}"; }}
 extension deliveries {{ namespace = "maac.production/1"; schema = &schema; render_affecting = true; data = {{ deliveries = {{ release = {{ rate = 48000Hz; resampler = "maac.src.kaiser/1"; targets = {{ master = {{ role = master; output = &s:out; encoding = wav_f32le; dither = {{ type = none; }}; }}; }}; }}; }}; }}; }}"#,sha256_digest(SCHEMA_BYTES));
    let expression_only = text
        .split(" asset schema")
        .next()
        .unwrap()
        .replace(" requires = [\"maac.production/1\"];", "");
    let expression_bundle = SourceBundle::new("expression.maac", expression_only);
    let VersionedPlan::V3(expression_plan) = compile_bundle_versioned(&expression_bundle).unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        &expression_plan.events[0].kind,
        maac::plan::EventKind::Note {
            gain_expression: Some(_),
            ..
        }
    ));
    let mut combined = SourceBundle::new("combined.maac", text.clone());
    combined
        .assets
        .insert("schema.json".into(), SCHEMA_BYTES.to_vec());
    let old_combined = SourceBundle {
        sources: combined
            .sources
            .iter()
            .map(|(k, v)| (k.clone(), v.replace("60bpm, linear", "60bpm, step")))
            .collect(),
        ..combined.clone()
    };
    assert!(maac::compiler::compile_bundle(&old_combined)
        .unwrap_err()
        .to_string()
        .contains("normalization is not defined for object kind `expression`"));
    let text = text.replace(" expression e { kind = gain; curve = &gain; }", "");
    let mut bundle = SourceBundle::new("main.maac", text);
    bundle
        .assets
        .insert("schema.json".into(), SCHEMA_BYTES.to_vec());
    check_bundle_versioned(&bundle).unwrap();
    let VersionedPlan::V3(plan) = compile_bundle_versioned(&bundle).unwrap() else {
        panic!()
    };
    assert!(plan.instruments.is_some());
    assert!(plan
        .production
        .as_ref()
        .unwrap()
        .execution_identity
        .is_some());
}

#[test]
fn flat_and_mixed_maps_remain_version_three() {
    for (points, expected) in [
        ("(0q, 60bpm, linear), (4q, 60bpm, step)", 192000),
        (
            "(0q, 60bpm, linear), (2q, 120bpm, step), (4q, 90bpm, step)",
            114543,
        ),
    ] {
        let text = source("linear").replace("(0q, 60bpm, linear), (4q, 120bpm, step)", points);
        let VersionedPlan::V3(plan) = compile_versioned(&parse(&text).unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(plan.output.total_frames, expected);
    }
}

#[test]
fn unresolved_source_frame_boundary_reports_precision() {
    // 400 decimal digits place the onset within 10^-399 seconds of frame 48000,
    // beyond the timing evaluator's 1024 fractional-bit certification ceiling.
    let text=source("linear").replace("at = 0q; dur = 4q", "at = 1q; dur = 1q; onset_offset = 0.1074257947431609769348196387606619865015956578079711453148485100504330249269266663263711035986310571361466360777032342705881409687663396577268995432257847852371020166382384989084848841817672875399061801485124645323540610486072897483919023193106350044129784323386245237206624520916849514161618236364647577693104218548140949993914623269823315700724754383933940865109052537061896870005798952289457676204s");
    let error = compile_versioned(&parse(&text).unwrap()).unwrap_err();
    assert_eq!(
        error.first().unwrap().code,
        maac::DiagnosticCode::TimePrecision
    );
}
