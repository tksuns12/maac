use maac::dsp::{render, DspEngine};
use maac::plan::{Automation, AutomationClock, AutomationPoint, Interpolation, PortRef};
use maac::{compile, parse, parse_rational, DiagnosticCode, Plan, Rational};

fn r(text: &str) -> Rational {
    parse_rational(text).unwrap()
}

fn source(channels: u8, gain_fields: &str) -> String {
    let (pan, upstream) = if channels == 2 {
        (
            r#"node pan { type = "core.pan/1"; params = { pan = -1/2; }; }
connect sp { from = &sine:out; to = &pan:in; }"#,
            "pan",
        )
    } else {
        ("", "sine")
    };
    format!(
        r#"maac 1;
project p {{ score = [0q, 1/1000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &gain:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 1; }}; }}
{pan}
node gain {{ type = "core.gain/1"; {gain_fields} }}
connect input {{ from = &{upstream}:out; to = &gain:in; }}
pattern phrase {{ length = 1/1000q; note n {{ at = 0q; dur = 1/1000q; pitch = A4; velocity = 1; }} }}
track melody {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &melody; at = 0q; }}
"#
    )
}

fn plan(channels: u8, gain: Option<&str>) -> Plan {
    let fields = format!(
        "config = {{ channels = {channels}; }}; {}",
        gain.map(|g| format!("params = {{ gain = {g}; }};"))
            .unwrap_or_default()
    );
    compile(&parse(&source(channels, &fields)).unwrap()).unwrap()
}

fn samples(plan: &Plan) -> Vec<Vec<f64>> {
    let mut output = Vec::new();
    render(plan, |frame| {
        output.push(frame.to_vec());
        Ok(())
    })
    .unwrap();
    output
}

fn gain_index(plan: &Plan) -> usize {
    plan.nodes
        .iter()
        .position(|node| node.id == "gain")
        .unwrap()
}

fn lane(at: &str, points: &[(&str, &str, Interpolation)]) -> Automation {
    Automation {
        id: "gain_lane".into(),
        target: PortRef::new("gain", "gain").unwrap(),
        clock: AutomationClock::Seconds,
        at: r(at),
        points: points
            .iter()
            .map(|(position, value, shape)| AutomationPoint {
                position: r(position),
                value: r(value),
                shape: *shape,
            })
            .collect(),
    }
}

#[test]
fn mono_stereo_default_zero_and_large_gain_are_exact_multiplication() {
    for channels in [1, 2] {
        let unity = plan(channels, None);
        assert_eq!(unity.nodes[gain_index(&unity)].params["gain"], r("1"));
        let mut bypass = unity.clone();
        bypass.nodes.retain(|node| node.id != "gain");
        bypass.connections.retain(|edge| edge.to.node != "gain");
        bypass.output.output =
            PortRef::new(if channels == 1 { "sine" } else { "pan" }, "out").unwrap();
        let reference = samples(&bypass);
        assert_eq!(samples(&unity), reference);
        if channels == 2 {
            assert_ne!(reference[1][0], reference[1][1]);
        }
        for (authored, gain) in [("0", 0.0), ("1/2", 0.5), ("32", 32.0)] {
            let output = samples(&plan(channels, Some(authored)));
            for (actual, input) in output.iter().zip(&reference) {
                for (actual, input) in actual.iter().zip(input) {
                    assert_eq!(actual.to_bits(), (gain * input).to_bits());
                }
            }
        }
    }
}

#[test]
fn source_plan_json_and_reset_render_match_exactly() {
    let original = plan(2, Some("7/3"));
    let json = original.to_json().unwrap();
    let loaded = Plan::from_json(&json).unwrap();
    assert_eq!(original, loaded);
    let expected = samples(&original);
    let mut engine = DspEngine::new(&loaded).unwrap();
    for _ in 0..2 {
        let mut actual = Vec::new();
        engine
            .render(|frame| {
                actual.push(frame.to_vec());
                Ok(())
            })
            .unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn automation_is_immediate_preserves_muted_upstream_and_uses_pre_anchor_base() {
    let reference = samples(&plan(1, None));
    for base in [None, Some("3")] {
        let mut p = plan(1, base);
        // Exercise direct typed-plan default and automation fallback independently of compiler defaults.
        if base.is_none() {
            let i = gain_index(&p);
            p.nodes[i].params.clear();
        }
        p.automation.push(lane(
            "3/48000",
            &[
                ("0", "0", Interpolation::Step),
                ("3/48000", "2", Interpolation::Step),
            ],
        ));
        let output = samples(&p);
        for frame in 0..24 {
            let multiplier = if frame < 3 {
                if base.is_some() {
                    3.0
                } else {
                    1.0
                }
            } else if frame < 6 {
                0.0
            } else {
                2.0
            };
            assert_eq!(
                output[frame][0].to_bits(),
                (multiplier * reference[frame][0]).to_bits()
            );
        }
        assert!(output[6][0].abs() > 0.1);
    }
    let mut p = plan(1, None);
    p.automation.push(lane(
        "0",
        &[
            ("0", "0", Interpolation::Linear),
            ("4/48000", "2", Interpolation::Step),
        ],
    ));
    for (frame, output) in samples(&p).iter().enumerate() {
        let gain = (frame as f64 / 2.0).min(2.0);
        assert!((output[0] - gain * reference[frame][0]).abs() < 1e-15);
    }
}

#[test]
fn source_rejects_missing_wrong_and_extra_configuration_or_parameters() {
    for fields in [
        "",
        "config = {};",
        "config = { channels = 0; };",
        "config = { channels = 3; };",
        "config = { channels = 3/2; };",
        "config = { channels = -1; };",
        "config = { channels = 256; };",
        "config = { channels = 1Hz; };",
        "config = { channels = 1; extra = 0; };",
        "config = { channels = 1; }; params = { gain = -1; };",
        "config = { channels = 1; }; params = { gain = 1dB; };",
        "config = { channels = 1; }; params = { level = 1; };",
    ] {
        let doc = parse(&source(1, fields)).unwrap();
        assert!(
            maac::semantic::validate_source(&doc).is_err(),
            "semantic accepted {fields}"
        );
        assert!(compile(&doc).is_err(), "compiler accepted {fields}");
    }
}

#[test]
fn strict_wire_rejects_malformed_gain_fields_and_typed_parameter_errors() {
    let p = plan(1, None);
    let i = gain_index(&p);
    let original: serde_json::Value = serde_json::from_slice(&p.to_json().unwrap()).unwrap();
    for processor in [
        serde_json::json!({"kind":"gain"}),
        serde_json::json!({"kind":"gain","channels":1,"extra":0}),
        serde_json::json!({"kind":"gain","channels":1.0}),
        serde_json::json!({"kind":"gain","channels":1.5}),
        serde_json::json!({"kind":"gain","channels":-1}),
        serde_json::json!({"kind":"gain","channels":256}),
        serde_json::json!({"kind":"gain","channels":"1"}),
        serde_json::json!({"kind":"gain","channels":0}),
        serde_json::json!({"kind":"gain","channels":3}),
    ] {
        let mut json = original.clone();
        json["nodes"][i]["processor"] = processor;
        assert!(Plan::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    for channels in [0, 3, 255] {
        let mut bad = p.clone();
        bad.nodes[i].processor = maac::plan::Processor::gain(channels);
        assert_eq!(bad.validate().unwrap_err().code, "E_RANGE");
    }
    for value in [
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("NaN"),
        serde_json::json!("Infinity"),
        serde_json::json!({"n":1,"d":0}),
    ] {
        let mut json = original.clone();
        json["nodes"][i]["params"]["gain"] = value;
        assert!(Plan::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    for (parameter, value, code) in [("gain", "-1", "E_RANGE"), ("level", "1", "E_UNKNOWN_FIELD")] {
        let mut bad = p.clone();
        bad.nodes[i].params.insert(parameter.into(), r(value));
        assert_eq!(bad.validate().unwrap_err().code, code);
    }
    let mut bad = p.clone();
    bad.nodes[i]
        .params
        .insert("gain".into(), r(&format!("1{}", "0".repeat(309))));
    assert_eq!(bad.validate().unwrap_err().code, "E_NONFINITE");
}

#[test]
fn invalid_automation_and_single_input_cycles_are_rejected() {
    let p = plan(1, None);
    let mut negative = p.clone();
    negative
        .automation
        .push(lane("0", &[("0", "-1", Interpolation::Step)]));
    assert_eq!(negative.validate().unwrap_err().code, "E_RANGE");
    let mut disconnected = p.clone();
    disconnected.connections.clear();
    assert_eq!(disconnected.validate().unwrap_err().code, "E_PORT_TYPE");
    let mut doubled = p.clone();
    let mut duplicate = doubled.connections[0].clone();
    duplicate.id = "duplicate".into();
    doubled.connections.push(duplicate);
    assert_eq!(doubled.validate().unwrap_err().code, "E_PORT_TYPE");
    let mut cyclic = p.clone();
    cyclic.connections[0].from.node = "gain".into();
    assert_eq!(cyclic.validate().unwrap_err().code, "E_ALGEBRAIC_LOOP");
    let mut wrong = plan(2, None);
    wrong
        .connections
        .iter_mut()
        .find(|e| e.to.node == "gain")
        .unwrap()
        .from
        .node = "sine".into();
    assert_eq!(wrong.validate().unwrap_err().code, "E_PORT_TYPE");
}

#[test]
fn finite_gain_product_overflow_fails_instead_of_clipping() {
    let mut p = plan(1, Some("32"));
    p.nodes
        .iter_mut()
        .find(|n| n.id == "sine")
        .unwrap()
        .params
        .insert("level".into(), r(&format!("1{}", "0".repeat(308))));
    p.validate().unwrap();
    let error = render(&p, |_| Ok(())).unwrap_err();
    assert_eq!(error.code(), "E_NONFINITE");
}

#[test]
fn unsupported_delay_remains_a_capability_error() {
    let doc =
        parse(&source(1, "config = { channels = 1; };").replace("core.gain/1", "core.delay/1"))
            .unwrap();
    assert!(compile(&doc)
        .unwrap_err()
        .iter()
        .any(|d| d.code == DiagnosticCode::Capability));
}

#[test]
fn source_authored_automation_roundtrips_and_checks_units_and_range() {
    let base = source(1, "config = { channels = 1; }; params = { gain = 3; };");
    let automated = format!("{base}\ncurve c {{ clock = seconds; points = [(0s, 0, step), (3/48000s, 2, step)]; }}\nautomation a {{ target = &gain.params.gain; curve = &c; at = 3/48000s; }}");
    let compiled = compile(&parse(&automated).unwrap()).unwrap();
    let mut expected = plan(1, Some("3"));
    expected.automation.push(lane(
        "3/48000",
        &[
            ("0", "0", Interpolation::Step),
            ("3/48000", "2", Interpolation::Step),
        ],
    ));
    assert_eq!(samples(&compiled), samples(&expected));
    assert_eq!(
        samples(&Plan::from_json(&compiled.to_json().unwrap()).unwrap()),
        samples(&expected)
    );
    for value in ["-1", "1Hz", "1dB"] {
        assert!(compile(
            &parse(&automated.replace("0s, 0, step", &format!("0s, {value}, step"))).unwrap()
        )
        .is_err());
    }
    let mut huge = plan(1, None);
    huge.automation
        .push(lane("0", &[("0", "1", Interpolation::Step)]));
    huge.automation[0].points[0].value = r(&format!("1{}", "0".repeat(309)));
    assert_eq!(huge.validate().unwrap_err().code, "E_NONFINITE");
    // Finite positive endpoints can still overflow the engine's exponential interpolation.
    huge.automation[0].points = vec![
        AutomationPoint {
            position: r("0"),
            value: r(&format!("1/1{}", "0".repeat(308))),
            shape: Interpolation::Exponential,
        },
        AutomationPoint {
            position: r("4/48000"),
            value: r(&format!("1{}", "0".repeat(308))),
            shape: Interpolation::Step,
        },
    ];
    huge.validate().unwrap();
    assert_eq!(render(&huge, |_| Ok(())).unwrap_err().code(), "E_NONFINITE");
}
