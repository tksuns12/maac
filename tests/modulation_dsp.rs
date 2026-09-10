use maac::{dsp::DspEngine, PlanArtifact};
use serde_json::{json, Value};
fn fixture() -> Value {
    let bytes: Vec<u8> = [1f32; 16].iter().flat_map(|x| x.to_le_bytes()).collect();
    json!({"version":7,"output":{"score_start_q":"0/1","score_end_q":"1/6000","tail_seconds":"1/6000","sample_rate_hz":48000,"channels":1,"total_frames":16,"output":{"node":"gain","port":"out"}},"tempo":{"points":[{"q":"0/1","bpm":"60/1","shape":"step"}]},"nodes":[{"id":"clip","processor":{"kind":"warp_rate","clip":{"asset":"sample","channels":1,"at_q":"0/1","source_start_frame":0,"source_end_frame":16,"warp":[{"q":"0/1","source_frame":0},{"q":"1/3000","source_frame":16}],"gain":"1/1","fade_in_seconds":"0/1","fade_out_seconds":"0/1","fade_shape":"linear","source":{"object":"clip","path":["clip"]},"start_frame":0,"end_frame":16}}},{"id":"gain","processor":{"kind":"core","processor":{"kind":"gain","channels":1}},"params":{"gain":"1/1"}},{"id":"motion","processor":{"kind":"lfo","config":{"clock":"seconds","period":"1/6000","phase":"0/1","wave":"sine"}}}],"audio_assets":[{"id":"sample","format":"pcm_f32le_interleaved/1","rate_hz":48000,"channels":1,"frames":16,"hash":maac::bundle::sha256_digest(&bytes),"bytes":bytes}],"connections":[{"id":"audio","from":{"node":"clip","port":"out"},"to":{"node":"gain","port":"in"}}],"modulations":[{"id":"m","from":{"node":"motion","port":"out"},"target":{"node":"gain","port":"gain"},"amount":"1/2"}]})
}
fn artifact(v: &Value) -> PlanArtifact {
    PlanArtifact::from_json(&serde_json::to_vec(v).unwrap()).unwrap()
}
fn render(v: &Value) -> Result<Vec<f64>, maac::dsp::RenderError> {
    let p = artifact(v);
    let mut out = vec![];
    maac::render_artifact(&p, |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })?;
    Ok(out)
}
#[test]
fn retained_lfo_modulates_gain_analytically() {
    let out = render(&fixture()).unwrap();
    for (n, e) in [(0, 1.), (2, 1.5), (4, 1.), (6, 0.5), (8, 1.)] {
        assert!((out[n] - e).abs() < 1e-12);
    }
}
fn constant(id: &str, value: &str) -> Value {
    json!({"id":id,"processor":{"kind":"constant"},"params":{"value":value}})
}
fn modulation(id: &str, from: &str, target: &str, parameter: &str, amount: &str) -> Value {
    json!({"id":id,"from":{"node":from,"port":"out"},"target":{"node":target,"port":parameter},"amount":amount})
}
#[test]
fn all_waves_and_both_tail_clocks() {
    for (wave, expected) in [
        ("sine", [0., 1., 0., -1.]),
        ("triangle", [-1., 0., 1., 0.]),
        ("saw", [-1., -0.5, 0., 0.5]),
        ("square", [1., 1., -1., -1.]),
    ] {
        for clock in ["score", "seconds"] {
            let mut v = fixture();
            v["nodes"][2]["processor"]["config"]["wave"] = json!(wave);
            v["nodes"][2]["processor"]["config"]["clock"] = json!(clock);
            let out = render(&v).unwrap();
            for n in (0..16).step_by(2) {
                let f = if clock == "score" && n >= 8 {
                    expected[0]
                } else {
                    expected[(n / 2) % 4]
                };
                assert!(
                    (out[n] - (1. + 0.5 * f)).abs() < 1e-12,
                    "{wave} {clock} {n}"
                );
            }
        }
    }
}
#[test]
fn automated_constant_feedforward_order_and_reset_have_no_drift() {
    let mut v = fixture();
    v["nodes"][2] = constant("later", "1/1");
    v["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"earlier","processor":{"kind":"constant"}}));
    v["modulations"] = json!([
        modulation("z", "later", "gain", "gain", "1/1"),
        modulation("a", "earlier", "later", "value", "2/1")
    ]);
    v["automation"] = json!([{"id":"lane","target":{"node":"earlier","port":"value"},"clock":"score","at":{"kind":"score","q":"1/24000"},"points":[{"position":"0/1","value":"1/1","shape":"step"},{"position":"1/4000","value":"3/1","shape":"step"}]}]);
    let p = artifact(&v);
    let mut engine = DspEngine::new_artifact(&p).unwrap();
    let mut first = vec![];
    engine
        .render(|f| {
            first.push(f[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(&first[..4], &[2., 2., 4., 4.]);
    assert!(first[8..].iter().all(|v| *v == 4.));
    let mut second = vec![];
    engine
        .render(|f| {
            second.push(f[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(first, second);
}
#[test]
fn signed_sums_follow_modulation_ids_and_only_final_range_policy() {
    let mut v = fixture();
    v["nodes"][2] = constant("one", "1/1");
    v["nodes"][1]["params"]["gain"] = json!("0/1");
    v["modulations"] = json!([
        modulation("c", "one", "gain", "gain", "1/1"),
        modulation("b", "one", "gain", "gain", "-10000000000000000/1"),
        modulation("a", "one", "gain", "gain", "10000000000000000/1")
    ]);
    assert_eq!(render(&v).unwrap(), vec![1.; 16]);
    v["nodes"].as_array_mut().unwrap().reverse();
    v["modulations"].as_array_mut().unwrap().reverse();
    assert_eq!(render(&v).unwrap(), vec![1.; 16]);
    let mut v = fixture();
    v["nodes"][2] = constant("one", "1/1");
    v["nodes"][1]["processor"]["processor"] = json!({"kind":"pan"});
    v["nodes"][1]["params"] = json!({"pan":"2/1"});
    v["output"]["channels"] = json!(2);
    v["modulations"] = json!([modulation("a", "one", "gain", "pan", "-2/1")]);
    let out = render(&v).unwrap();
    for sample in out {
        assert!((sample - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    }
}
#[test]
fn filter_modulation_matches_independent_recurrence() {
    let mut v = fixture();
    v["nodes"][2] = constant("one", "1/1");
    v["nodes"][1]["processor"]["processor"] = json!({"kind":"one_pole","channels":1});
    v["nodes"][1]["params"] = json!({"cutoff":"1000/1"});
    v["modulations"] = json!([modulation("m", "one", "gain", "cutoff", "1000/1")]);
    let out = render(&v).unwrap();
    let coefficient = (-std::f64::consts::TAU * 2000. / 48000.).exp();
    for (n, y) in out.iter().enumerate() {
        assert!((*y - (1. - coefficient.powi(n as i32 + 1))).abs() < 1e-12);
    }
}
#[test]
fn disconnected_nonfinite_product_sum_and_final_range_fail_before_callback() {
    for kind in ["product", "sum", "range"] {
        let mut v = fixture();
        v["nodes"][2] = constant("one", "1/1");
        let huge = format!("1{}/1", "0".repeat(308));
        if kind == "range" {
            v["modulations"] = json!([modulation("m", "one", "gain", "gain", "-2/1")]);
        } else {
            v["nodes"].as_array_mut().unwrap().push(constant(
                "unused",
                if kind == "sum" { &huge } else { "0/1" },
            ));
            v["nodes"][2]["params"]["value"] = json!(if kind == "product" {
                huge.clone()
            } else {
                "1/1".into()
            });
            v["modulations"] = json!([modulation(
                "m",
                "one",
                "unused",
                "value",
                if kind == "product" { "2/1" } else { &huge }
            )]);
        }
        let p = artifact(&v);
        let mut calls = 0;
        let e = maac::render_artifact(&p, |_| {
            calls += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(
            e.code(),
            if kind == "range" {
                "E_RANGE"
            } else {
                "E_NONFINITE"
            }
        );
        assert_eq!(calls, 0);
    }
}
#[test]
fn control_capture_and_output_are_rejected_before_callbacks() {
    let mut v = fixture();
    let p = artifact(&v);
    let mut engine = DspEngine::new_artifact(&p).unwrap();
    let mut calls = 0;
    let e = engine
        .render_ports(
            &[maac::plan::PortRef::new("motion", "out").unwrap()],
            |_| {
                calls += 1;
                Ok(())
            },
        )
        .unwrap_err();
    assert_eq!(e.code(), "E_PORT_TYPE");
    assert_eq!(calls, 0);
    v["output"]["output"] = json!({"node":"motion","port":"out"});
    assert!(PlanArtifact::from_json(&serde_json::to_vec(&v).unwrap()).is_err());
}
fn note(id: &str, on: u64, off: u64) -> Value {
    json!({"address":id,"source":{"object":id,"path":[id]},"target":{"node":"instrument","port":"events"},"kind":{"kind":"note","pitch_hz":480.,"velocity":"1/1"},"score_on_q":rational(on,48000),"score_off_q":rational(off,48000),"onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.5,"on_frame":on,"off_frame":off})
}
fn add_instrument(v: &mut Value) {
    use maac::graph::{
        Control, ControlTarget, GraphNode, GraphProcessor, GraphProgram, GraphStage,
        InstrumentProgram, ProgramSource,
    };
    use std::collections::BTreeMap;
    let r = |s: &str| maac::parse_rational(s).unwrap();
    let program = InstrumentProgram {
        id: "plain".into(),
        voice: GraphProgram {
            channels: 1,
            nodes: vec![
                GraphNode {
                    id: "amp".into(),
                    processor: GraphProcessor::Adsr,
                    params: BTreeMap::from([
                        ("attack".into(), r("0/1")),
                        ("decay".into(), r("0/1")),
                        ("sustain".into(), r("1/1")),
                        ("release".into(), r("0/1")),
                    ]),
                },
                GraphNode {
                    id: "osc".into(),
                    processor: GraphProcessor::Sine,
                    params: BTreeMap::from([("phase".into(), r("1/4"))]),
                },
            ],
            connections: vec![],
            modulations: vec![],
            output: maac::plan::PortRef::new("osc", "out").unwrap(),
            amplitude: Some("amp".into()),
        },
        shared: None,
        controls: BTreeMap::from([(
            "level".into(),
            Control {
                target: ControlTarget {
                    graph: GraphStage::Voice,
                    node: "osc".into(),
                    parameter: "level".into(),
                },
                default: r("1/1"),
            },
        )]),
        source: ProgramSource {
            file: "retained.maac".into(),
            object: "plain".into(),
            span: None,
        },
    };
    let resources = maac::instrument_plan::InstrumentResources {
        entry_source: "retained.maac".into(),
        programs: vec![program],
        wavetables: vec![],
        wavetable_sources: vec![],
        source_files: vec![maac::bundle::SourceIdentity {
            path: "retained.maac".into(),
            hash: maac::bundle::sha256_digest(b"manual retained fixture"),
        }],
        dependencies: vec![],
        libraries: vec![],
    };
    v["instruments"] = serde_json::to_value(resources).unwrap();
    v["nodes"].as_array_mut().unwrap().push(json!({"id":"instrument","processor":{"kind":"core","processor":{"kind":"instrument","program":"plain","channels":1,"voices":1}}}));
}
#[test]
fn retained_mixed_processors_capture_and_instrument_modulation_reset() {
    let mut v = fixture();
    v["nodes"][2] = constant("one", "1/1");
    add_instrument(&mut v);
    v["nodes"].as_array_mut().unwrap().push(json!({"id":"kit","processor":{"kind":"kit","channels":1,"voices":1,"samples":[{"key":"key","asset":"sample"}]}}));
    v["nodes"].as_array_mut().unwrap().push(json!({"id":"rate","processor":{"kind":"audio","clip":{"asset":"sample","channels":1,"at":{"kind":"seconds","seconds":"0/1"},"source_start_frame":0,"source_end_frame":16,"speed":"1/1","reverse":false,"gain":"1/1","fade_in_seconds":"0/1","fade_out_seconds":"0/1","fade_shape":"linear","source":{"object":"rate","path":["rate"]},"start_frame":0,"end_frame":16}}}));
    v["nodes"].as_array_mut().unwrap().push(json!({"id":"eq","processor":{"kind":"core","processor":{"kind":"fx.eq/1","channels":1,"mode":"low_pass"}}}));
    v["connections"].as_array_mut().unwrap().push(
        json!({"id":"fx","from":{"node":"rate","port":"out"},"to":{"node":"eq","port":"in"}}),
    );
    v["events"] = json!([note("n1",0,4),note("n2",4,8),{"address":"hit","source":{"object":"hit","path":["hit"]},"target":{"node":"kit","port":"events"},"kind":{"kind":"hit","key":"key","velocity":"1/1"},"score_on_q":"0/1","onset_offset_seconds":"0/1","release_offset_seconds":"0/1","release_velocity":0.0,"on_frame":0}]);
    v["modulations"] = json!([
        modulation("m1", "one", "instrument", "level", "-1/2"),
        modulation("m2", "one", "kit", "level", "-1/2"),
        modulation("m3", "one", "eq", "frequency", "100/1")
    ]);
    let encoded = artifact(&v).to_json().unwrap();
    let p = PlanArtifact::from_json(&encoded).unwrap();
    let mut engine = DspEngine::new_artifact(&p).unwrap();
    let ports: Vec<_> = ["instrument", "kit", "rate", "clip", "eq"]
        .iter()
        .map(|id| maac::plan::PortRef::new(*id, "out").unwrap())
        .collect();
    let mut first = vec![];
    engine
        .render_ports(&ports, |frames| {
            first.push(frames.to_vec());
            Ok(())
        })
        .unwrap();
    assert!(first.len() >= 8);
    for (n, frame) in first.iter().take(8).enumerate() {
        assert!(
            (frame[0][0] - 0.5 * (std::f64::consts::TAU * 480. * (n % 4) as f64 / 48000.).cos())
                .abs()
                < 2e-6,
            "frame {n}: {:?}",
            frame[0]
        );
    }
    for frame in &first {
        assert_eq!(frame[1][0], 0.5);
        assert_eq!(frame[2][0], 1.);
        assert_eq!(frame[3][0], 1.);
        assert!(frame[4][0].is_finite());
    }
    let mut second = vec![];
    engine
        .render_ports(&ports, |frames| {
            second.push(frames.to_vec());
            Ok(())
        })
        .unwrap();
    assert_eq!(first, second);
}
#[test]
fn invalid_instrument_final_control_fails_even_without_voices() {
    let mut v = fixture();
    v["nodes"][2] = constant("one", "1/1");
    add_instrument(&mut v);
    v["modulations"] = json!([modulation("m", "one", "instrument", "level", "-2/1")]);
    let p = artifact(&v);
    let mut calls = 0;
    assert_eq!(
        maac::render_artifact(&p, |_| {
            calls += 1;
            Ok(())
        })
        .unwrap_err()
        .code(),
        "E_RANGE"
    );
    assert_eq!(calls, 0);
}

fn rational(n: u64, d: u64) -> String {
    let r = maac::Rational::new(n.into(), d.into());
    format!("{}/{}", r.numer(), r.denom())
}
#[test]
fn retained_score_ramp_modulation_matches_analytical_clock() {
    let mut v = fixture();
    v["tempo"]["points"] = json!([{"q":"0/1","bpm":"60/1","shape":"linear"},{"q":"1/6000","bpm":"120/1","shape":"step"}]);
    v["output"]["total_frames"] = json!(14);
    v["nodes"][0]["processor"]["clip"]["end_frame"] = json!(10);
    v["nodes"][2]["processor"]["config"]["clock"] = json!("score");
    v["nodes"][2]["processor"]["config"]["wave"] = json!("saw");
    let out = render(&v).unwrap();
    for (n, sample) in out.iter().enumerate().take(6) {
        let f = (n as f64 / 8.).exp_m1();
        assert!((*sample - (0.5 + f)).abs() < 1e-12);
    }
}
