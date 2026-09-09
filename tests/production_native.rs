use maac::dsp::{render, DspEngine};
use maac::{compile, parse, DiagnosticCode, Plan};

fn source(kind: &str, config: &str, params: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score=[0q,1/100q];rate=48000Hz;tempo=&t;meter=&m;output=&fx:out;tail=1/100s;requires=["maac.production/1"]; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
node sine {{ type="core.sine/1";params={{attack=0s;release=0s;level=0.2;}}; }}
node fx {{type="{kind}";config={{{config}}};params={{{params}}};}}
connect input {{from=&sine:out;to=&fx:in;}}
pattern phrase {{length=1/100q;note n {{at=0q;dur=1/100q;pitch=A4;velocity=1;}}}}
track notes {{target=&sine:events;}}
place play {{pattern=&phrase;track=&notes;at=0q;}}
"#
    )
}

#[test]
fn native_source_plan_and_reset_render() {
    for (kind, config, params) in [
        (
            "fx.eq/1",
            "channels=1;mode=peak;",
            "frequency=1000Hz;q=1;gain=0dB;",
        ),
        ("fx.compressor/1", "channels=1;", "ratio=1;"),
        ("fx.reverb/1", "channels=1;", "mix=0;"),
    ] {
        let p = compile(&parse(&source(kind, config, params)).unwrap()).unwrap();
        let loaded = Plan::from_json(&p.to_json().unwrap()).unwrap();
        assert_eq!(p, loaded);
        let mut expected = Vec::new();
        render(&p, |frame| {
            expected.push(frame.to_vec());
            Ok(())
        })
        .unwrap();
        assert!(expected.iter().flatten().any(|x| x.abs() > 0.01));
        let mut engine = DspEngine::new(&loaded).unwrap();
        for _ in 0..2 {
            let mut actual = Vec::new();
            engine
                .render(|frame| {
                    actual.push(frame.to_vec());
                    Ok(())
                })
                .unwrap();
            assert_eq!(expected, actual);
        }
    }
}

#[test]
fn native_requires_is_enforced() {
    let good = source("fx.eq/1", "channels=1;mode=peak;", "");
    for bad in [
        good.replace("requires=[\"maac.production/1\"]", "requires=[]"),
        good.replace("maac.production/1", "unknown.capability/99"),
    ] {
        let errors = compile(&parse(&bad).unwrap()).unwrap_err();
        assert!(errors.iter().any(|d| d.code == DiagnosticCode::Capability));
    }
}

fn compile_source(text: &str) -> Plan {
    compile(&parse(text).unwrap()).unwrap()
}

#[test]
fn eq_matches_independent_impulse_convolution_and_captures_complete_graph() {
    use maac::plan::{PlanLimits, PortRef};
    let p = compile_source(&source(
        "fx.eq/1",
        "channels=1;mode=low_pass;",
        "frequency=12000Hz;q=1/2;",
    ));
    let ports = [
        PortRef::new("sine", "out").unwrap(),
        PortRef::new("fx", "out").unwrap(),
    ];
    let mut x1 = 0.0;
    let mut x2 = 0.0;
    let mut full = Vec::new();
    maac::dsp::render_ports_with_limits(&p, &PlanLimits::default(), &ports, |outputs| {
        let x = outputs[0][0];
        assert!((outputs[1][0] - (0.25 * x + 0.5 * x1 + 0.25 * x2)).abs() < 1e-14);
        x2 = x1;
        x1 = x;
        full.push(outputs[1][0]);
        Ok(())
    })
    .unwrap();
    let mut stem = Vec::new();
    maac::dsp::render_ports_with_limits(&p, &PlanLimits::default(), &ports[1..], |outputs| {
        stem.push(outputs[0][0]);
        Ok(())
    })
    .unwrap();
    assert_eq!(full, stem);
}

#[test]
fn external_detector_does_not_enter_audio_and_is_required() {
    use maac::plan::{PlanLimits, PortRef};
    let base = source(
        "fx.compressor/1",
        "channels=1;detector=external;sidechain_channels=1;",
        "ratio=1;attack=0s;release=0s;",
    );
    assert!(compile(&parse(&base).unwrap()).is_err());
    let good = format!("{base}\nconnect detector {{from=&sine:out;to=&fx:sidechain;}}");
    let p = compile_source(&good);
    maac::dsp::render_ports_with_limits(
        &p,
        &PlanLimits::default(),
        &[
            PortRef::new("sine", "out").unwrap(),
            PortRef::new("fx", "out").unwrap(),
        ],
        |out| {
            assert_eq!(out[0], out[1]);
            Ok(())
        },
    )
    .unwrap();
    for bad in [
        good.replace("sidechain_channels=1", "sidechain_channels=2"),
        good.replace(
            "detector=external;sidechain_channels=1;",
            "detector=internal;",
        ),
        format!("{good}\nconnect duplicate {{from=&sine:out;to=&fx:sidechain;}}"),
        good.replace(
            "from=&sine:out;to=&fx:sidechain",
            "from=&fx:out;to=&fx:sidechain",
        ),
    ] {
        assert!(compile(&parse(&bad).unwrap()).is_err());
    }
}

#[test]
fn reverb_tail_first_wet_output_and_reset_are_preserved() {
    use maac::plan::{PlanLimits, PortRef};
    let p = compile_source(
        &source(
            "fx.reverb/1",
            "channels=1;predelay=0s;damping=0;",
            "mix=1;decay=1.5s;",
        )
        .replace("tail=1/100s", "tail=1/10s"),
    );
    let mut original = Vec::new();
    let mut wet = Vec::new();
    maac::dsp::render_ports_with_limits(
        &p,
        &PlanLimits::default(),
        &[
            PortRef::new("sine", "out").unwrap(),
            PortRef::new("fx", "out").unwrap(),
        ],
        |out| {
            original.push(out[0][0]);
            wet.push(out[1][0]);
            Ok(())
        },
    )
    .unwrap();
    assert!(wet[..1493].iter().all(|x| *x == 0.0));
    assert!((wet[1494] - original[1] / 32.0).abs() < 1e-14);
    assert!(wet[1493..].iter().any(|x| x.abs() > 0.00001));
    let mut replay = Vec::new();
    render(&p, |f| {
        replay.push(f[0]);
        Ok(())
    })
    .unwrap();
    assert_eq!(wet, replay);
}

#[test]
fn native_configuration_units_ranges_and_mode_applicability_are_strict() {
    for (kind, config, params) in [
        ("fx.eq/1", "channels=1;", ""),
        ("fx.eq/1", "channels=3;mode=peak;", ""),
        ("fx.eq/1", "channels=1;mode=\"peak\";", ""),
        ("fx.eq/1", "channels=1;mode=low_shelf;", "q=1;"),
        ("fx.eq/1", "channels=1;mode=low_pass;", "gain=0dB;"),
        ("fx.eq/1", "channels=1;mode=peak;", "gain=1;"),
        ("fx.eq/1", "channels=1;mode=peak;", "frequency=24000Hz;"),
        ("fx.eq/1", "channels=1;mode=peak;", "frequency=0Hz;"),
        ("fx.eq/1", "channels=1;mode=peak;", "q=0.09;"),
        (
            "fx.compressor/1",
            "channels=1;detector=internal;sidechain_channels=1;",
            "",
        ),
        ("fx.compressor/1", "channels=1;", "threshold=-121dB;"),
        ("fx.compressor/1", "channels=1;", "attack=1;"),
        ("fx.compressor/1", "channels=1;", "ratio=101;"),
        ("fx.compressor/1", "channels=1;", "knee=25dB;"),
        ("fx.compressor/1", "channels=1;", "release=31s;"),
        ("fx.reverb/1", "channels=1;predelay=251ms;", ""),
        ("fx.reverb/1", "channels=1;damping=1.1;", ""),
        ("fx.reverb/1", "channels=1;", "mix=1.1;"),
        ("fx.reverb/1", "channels=1;", "decay=0.09s;"),
    ] {
        let bad = source(kind, config, params);
        assert!(
            maac::semantic::validate_source(&parse(&bad).unwrap()).is_err(),
            "{bad}"
        );
        assert!(compile(&parse(&bad).unwrap()).is_err());
    }
    let core = include_str!("../example.maac")
        .replace("requires = [];", "requires = [\"unknown.capability/99\"];");
    assert!(compile(&parse(&core).unwrap())
        .unwrap_err()
        .iter()
        .any(|d| d.code == DiagnosticCode::Capability));
}

#[test]
fn retained_native_plans_reject_invalid_parameters_configuration_and_budgets() {
    use maac::plan::PlanLimits;
    let p = compile_source(&source("fx.eq/1", "channels=1;mode=peak;", ""));
    let i = p.nodes.iter().position(|n| n.id == "fx").unwrap();
    let value: serde_json::Value = serde_json::from_slice(&p.to_json().unwrap()).unwrap();
    assert_eq!(value["nodes"][i]["processor"]["kind"], "fx.eq/1");
    for processor in [
        serde_json::json!({"kind":"fx.eq/1","channels":1}),
        serde_json::json!({"kind":"fx.eq/1","channels":3,"mode":"peak"}),
        serde_json::json!({"kind":"fx.eq/1","channels":1,"mode":"other"}),
        serde_json::json!({"kind":"fx.eq/1","channels":1,"mode":"peak","extra":0}),
    ] {
        let mut bad = value.clone();
        bad["nodes"][i]["processor"] = processor;
        assert!(Plan::from_json(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    for (key, val) in [
        ("frequency", "24000"),
        ("q", "19"),
        ("gain", "25"),
        ("extra", "0"),
    ] {
        let mut bad = p.clone();
        bad.nodes[i]
            .params
            .insert(key.into(), maac::parse_rational(val).unwrap());
        assert!(bad.validate().is_err());
    }
    let mut calls = 0;
    let limits = PlanLimits {
        max_execution_work: 1,
        ..PlanLimits::default()
    };
    assert!(maac::dsp::render_with_limits(&p, &limits, |_| {
        calls += 1;
        Ok(())
    })
    .is_err());
    assert_eq!(calls, 0);
    let p = compile_source(&source("fx.reverb/1", "channels=1;", ""));
    let limits = PlanLimits {
        max_production_delay_cells: 0,
        ..PlanLimits::default()
    };
    assert!(DspEngine::new_with_limits(&p, &limits).is_err());
}

#[test]
fn db_automation_is_linear_immediate_and_retained_plans_keep_unit_rules() {
    let base = source("fx.eq/1", "channels=1;mode=peak;", "gain=-3dB;");
    let lane="curve c {clock=seconds;points=[(0s,0dB,linear),(1/1000s,6dB,step)];}\nautomation a {target=&fx.params.gain;curve=&c;at=0s;}";
    let p = compile_source(&format!("{base}\n{lane}"));
    assert_eq!(
        p.automation[0].points[1].value,
        maac::parse_rational("6").unwrap()
    );
    let mut first = Vec::new();
    render(&p, |f| {
        first.push(f[0]);
        Ok(())
    })
    .unwrap();
    let loaded = Plan::from_json(&p.to_json().unwrap()).unwrap();
    let mut second = Vec::new();
    render(&loaded, |f| {
        second.push(f[0]);
        Ok(())
    })
    .unwrap();
    assert_eq!(first, second);
    let mut bad = p.clone();
    bad.automation[0].points[0].value = maac::parse_rational("1").unwrap();
    bad.automation[0].points[0].shape = maac::plan::Interpolation::Exponential;
    assert!(bad.validate().is_err());
    assert!(compile(
        &parse(&format!(
            "{base}\n{}",
            lane.replace("0dB,linear", "1dB,exponential")
        ))
        .unwrap()
    )
    .is_err());
    assert!(compile(&parse(&format!("{base}\n{}", lane.replace("6dB", "25dB"))).unwrap()).is_err());
    assert!(compile(
        &parse(&format!(
            "{base}\n{}",
            lane.replace("params.gain", "params.mode")
        ))
        .unwrap()
    )
    .is_err());
}

#[test]
fn named_deliveries_compile_into_retained_plan_without_changing_legacy_json() {
    let mut bundle = maac::SourceBundle::new(
        "examples/production.maac",
        include_str!("../examples/production.maac"),
    );
    bundle.assets.insert(
        "production.schema.json".into(),
        include_bytes!("../production.schema.json").to_vec(),
    );
    let plan = maac::compile_bundle(&bundle).unwrap();
    assert_eq!(plan.version, 2);
    assert_eq!(plan.production.as_ref().unwrap().deliveries.len(), 2);
    let retained = Plan::from_json(&plan.to_json().unwrap()).unwrap();
    assert_eq!(plan, retained);
    let legacy = compile_source(include_str!("../example.maac"));
    let json: serde_json::Value = serde_json::from_slice(&legacy.to_json().unwrap()).unwrap();
    assert!(json.get("production").is_none());
}

#[test]
fn capture_count_and_copy_work_fail_before_callbacks() {
    use maac::plan::{PlanLimits, PortRef};
    let p = compile_source(include_str!("../example.maac"));
    let output = PortRef::new("master", "out").unwrap();
    let mut calls = 0;
    let error = maac::dsp::render_ports_with_limits(
        &p,
        &PlanLimits::default(),
        &vec![output.clone(); 257],
        |_| {
            calls += 1;
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), "E_RESOURCE_LIMIT");
    assert_eq!(calls, 0);
    let limits = PlanLimits {
        max_execution_work: 1,
        ..PlanLimits::default()
    };
    let error = maac::dsp::render_ports_with_limits(&p, &limits, &[output], |_| {
        calls += 1;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code(), "E_RESOURCE_LIMIT");
    assert_eq!(calls, 0);
}
