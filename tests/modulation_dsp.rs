use maac::compiler::compile_bundle_artifact;
use maac::{dsp::DspEngine, PlanArtifact, SourceBundle};
use serde_json::{json, Value};

/// Sixteen frames at 48 kHz and 60 bpm, eight of score and eight of tail: an
/// audio clip of ones through node `gain`, whose parameter a control node
/// moves.
struct Scene {
    /// The points of the tempo map.
    tempo: String,
    /// Fields of the project besides its score, tail, rate, tempo, meter and
    /// output.
    project: String,
    /// The declaration of node `gain`, which the clip feeds and the project
    /// renders.
    gain: String,
    /// The control nodes and the modulations.
    controls: String,
    /// Further declarations.
    more: String,
}

const UNIT_GAIN: &str =
    r#"node gain { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 1; }; }"#;

/// An LFO `motion` of eight frames a cycle, which moves the gain by a half.
fn lfo(period: &str, wave: &str) -> String {
    format!(
        "node motion {{ type = \"core.lfo/1\"; config = {{ period = {period}; phase = 0; wave = {wave}; }}; }}\n{}",
        modulation("m", "motion", "gain", "gain", "1/2")
    )
}

fn constant(id: &str, value: &str) -> String {
    format!("node {id} {{ type = \"core.constant/1\"; params = {{ value = {value}; }}; }}")
}

fn modulation(id: &str, from: &str, target: &str, parameter: &str, amount: &str) -> String {
    format!(
        "modulate {id} {{ from = &{from}:out; target = &{target}.params.{parameter}; amount = {amount}; }}"
    )
}

fn fixture() -> Scene {
    Scene {
        tempo: "(0q, 60bpm, step)".to_owned(),
        project: String::new(),
        gain: UNIT_GAIN.to_owned(),
        controls: lfo("1/6000s", "sine"),
        more: String::new(),
    }
}

/// The scene with a constant `one` in place of the LFO, and `modulations`.
fn with_one(modulations: &[String]) -> Scene {
    Scene {
        controls: format!("{}\n{}", constant("one", "1"), modulations.join("\n")),
        ..fixture()
    }
}

impl Scene {
    /// The compiled plan, as it loads from its JSON.
    fn artifact(&self) -> PlanArtifact {
        let Self {
            tempo,
            project,
            gain,
            controls,
            more,
        } = self;
        let bytes: Vec<u8> = [1f32; 16].iter().flat_map(|x| x.to_le_bytes()).collect();
        let hash = maac::bundle::sha256_digest(&bytes);
        let source = format!(
            r#"maac 1;
project p {{ score = [0q, 1/6000q]; tail = 1/6000s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &gain:out; {project} }}
tempo clock {{ points = [{tempo}]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
asset sample {{ kind = audio; path = "sample.pcm"; hash = "{hash}"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = 16; }}
audio clip {{ asset = &sample; at = 0q; source = [0frame, 16frame]; mode = warp_rate; warp = [(0q, 0frame), (1/3000q, 16frame)]; }}
{gain}
connect audio {{ from = &clip:out; to = &gain:in; }}
{controls}
{more}
"#
        );
        let mut bundle = SourceBundle::new("main.maac", source.as_str());
        bundle.assets.insert("sample.pcm".into(), bytes);
        let compiled = compile_bundle_artifact(&bundle)
            .unwrap_or_else(|diagnostics| panic!("{diagnostics:?}\n{source}"));
        PlanArtifact::from_json(&compiled.to_json().unwrap()).unwrap()
    }
}

fn render(p: &PlanArtifact) -> Result<Vec<f64>, maac::dsp::RenderError> {
    let mut out = vec![];
    maac::render_artifact(p, |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })?;
    Ok(out)
}

/// The plan with `change` made to its JSON.
fn changed(p: &PlanArtifact, change: impl FnOnce(&mut Value)) -> Result<PlanArtifact, String> {
    let mut v: Value = serde_json::from_slice(&p.to_json().unwrap()).unwrap();
    change(&mut v);
    PlanArtifact::from_json(&serde_json::to_vec(&v).unwrap()).map_err(|error| format!("{error:?}"))
}

#[test]
fn retained_lfo_modulates_gain_analytically() {
    let out = render(&fixture().artifact()).unwrap();
    for (n, e) in [(0, 1.), (2, 1.5), (4, 1.), (6, 0.5), (8, 1.)] {
        assert!((out[n] - e).abs() < 1e-12);
    }
}
#[test]
fn all_waves_and_both_tail_clocks() {
    for (wave, expected) in [
        ("sine", [0., 1., 0., -1.]),
        ("triangle", [-1., 0., 1., 0.]),
        ("saw", [-1., -0.5, 0., 0.5]),
        ("square", [1., 1., -1., -1.]),
    ] {
        for (clock, period) in [("score", "1/6000q"), ("seconds", "1/6000s")] {
            let scene = Scene {
                controls: lfo(period, wave),
                ..fixture()
            };
            let out = render(&scene.artifact()).unwrap();
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
    let scene = Scene {
        controls: [
            constant("later", "1"),
            "node earlier { type = \"core.constant/1\"; }".to_owned(),
            modulation("z", "later", "gain", "gain", "1"),
            modulation("a", "earlier", "later", "value", "2"),
            "curve lane_curve { clock = score; points = [(0q, 1, step), (1/4000q, 3, step)]; }"
                .to_owned(),
            "automation lane { target = &earlier.params.value; curve = &lane_curve; at = 1/24000q; }"
                .to_owned(),
        ]
        .join("\n"),
        ..fixture()
    };
    let p = scene.artifact();
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
    let scene = Scene {
        gain: UNIT_GAIN.replace("gain = 1;", "gain = 0;"),
        ..with_one(&[
            modulation("c", "one", "gain", "gain", "1"),
            modulation("b", "one", "gain", "gain", "-10000000000000000"),
            modulation("a", "one", "gain", "gain", "10000000000000000"),
        ])
    };
    let p = scene.artifact();
    assert_eq!(render(&p).unwrap(), vec![1.; 16]);
    // The compiler lists nodes and modulations by ID. A plan may list them
    // in any order, so list them in reverse.
    let reversed = changed(&p, |v| {
        v["nodes"].as_array_mut().unwrap().reverse();
        let modulations = v["modulations"].as_array_mut().unwrap();
        assert_eq!(modulations.len(), 3);
        modulations.reverse();
    })
    .unwrap();
    assert_eq!(render(&reversed).unwrap(), vec![1.; 16]);
    let scene = Scene {
        gain: r#"node gain { type = "core.pan/1"; params = { pan = 2; }; }"#.to_owned(),
        ..with_one(&[modulation("a", "one", "gain", "pan", "-2")])
    };
    let out = render(&scene.artifact()).unwrap();
    for sample in out {
        assert!((sample - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    }
}
#[test]
fn filter_modulation_matches_independent_recurrence() {
    let scene = Scene {
        gain: r#"node gain { type = "core.onepole/1"; config = { channels = 1; }; params = { cutoff = 1000Hz; }; }"#
            .to_owned(),
        ..with_one(&[modulation("m", "one", "gain", "cutoff", "1000Hz")])
    };
    let out = render(&scene.artifact()).unwrap();
    let coefficient = (-std::f64::consts::TAU * 2000. / 48000.).exp();
    for (n, y) in out.iter().enumerate() {
        assert!((*y - (1. - coefficient.powi(n as i32 + 1))).abs() < 1e-12);
    }
}
#[test]
fn disconnected_nonfinite_product_sum_and_final_range_fail_before_callback() {
    for kind in ["product", "sum", "range"] {
        let huge = format!("1{}", "0".repeat(308));
        let scene = if kind == "range" {
            with_one(&[modulation("m", "one", "gain", "gain", "-2")])
        } else {
            // A constant nothing reads, moved past what binary64 holds: by a
            // huge source times two, or by a huge base plus a huge amount.
            let (one, unused, amount) = if kind == "product" {
                (huge.as_str(), "0", "2")
            } else {
                ("1", huge.as_str(), huge.as_str())
            };
            Scene {
                controls: [
                    constant("one", one),
                    constant("unused", unused),
                    modulation("m", "one", "unused", "value", amount),
                ]
                .join("\n"),
                ..fixture()
            }
        };
        let p = scene.artifact();
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
    let p = fixture().artifact();
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
    // A plan that renders a control output does not load.
    assert!(changed(&p, |v| {
        v["output"]["output"] = json!({"node":"motion","port":"out"});
    })
    .is_err());
}

/// A node `instrument` with one voice: a sine at a quarter turn of phase
/// under an envelope that opens and closes at once, with a control for its
/// level.
const INSTRUMENT: &str = r#"instrument plain {
 channels = 1;
 voice voice {
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp { type = "synth.adsr/1"; params = { attack = 0s; decay = 0s; sustain = 1; release = 0s; }; }
  node osc { type = "synth.sine/1"; params = { phase = 1/4; }; }
 }
 control level { target = &voice.osc.params.level; default = 1; }
}
node instrument { instrument = &plain; config = { voices = 1; }; }"#;

#[test]
fn retained_mixed_processors_capture_and_instrument_modulation_reset() {
    let scene = Scene {
        project: "requires = [\"maac.production/1\"];".to_owned(),
        more: format!(
            r#"{INSTRUMENT}
node kit {{ type = "core.kit/1"; config = {{ channels = 1; voices = 1; samples = [{{ key = "key"; asset = &sample; }}]; }}; }}
audio rate {{ asset = &sample; at = 0s; source = [0frame, 16frame]; mode = rate; }}
node eq {{ type = "fx.eq/1"; config = {{ channels = 1; mode = low_pass; }}; }}
connect fx {{ from = &rate:out; to = &eq:in; }}
track notes {{ target = &instrument:events; }}
track hits {{ target = &kit:events; }}
pattern played {{ length = 1/6000q;
 note n1 {{ at = 0q; dur = 4/48000q; pitch = 480Hz; velocity = 1; }}
 note n2 {{ at = 4/48000q; dur = 4/48000q; pitch = 480Hz; velocity = 1; }}
}}
pattern struck {{ length = 1/6000q; hit hit {{ at = 0q; key = "key"; velocity = 1; }} }}
place play {{ pattern = &played; track = &notes; at = 0q; }}
place strike {{ pattern = &struck; track = &hits; at = 0q; }}"#
        ),
        ..with_one(&[
            modulation("m1", "one", "instrument", "level", "-1/2"),
            modulation("m2", "one", "kit", "level", "-1/2"),
            modulation("m3", "one", "eq", "frequency", "100Hz"),
        ])
    };
    let p = scene.artifact();
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
    let scene = Scene {
        more: INSTRUMENT.to_owned(),
        ..with_one(&[modulation("m", "one", "instrument", "level", "-2")])
    };
    let p = scene.artifact();
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

#[test]
fn retained_score_ramp_modulation_matches_analytical_clock() {
    let scene = Scene {
        tempo: "(0q, 60bpm, linear), (1/6000q, 120bpm, step)".to_owned(),
        controls: lfo("1/6000q", "saw"),
        ..fixture()
    };
    let out = render(&scene.artifact()).unwrap();
    for (n, sample) in out.iter().enumerate().take(6) {
        let f = (n as f64 / 8.).exp_m1();
        assert!((*sample - (0.5 + f)).abs() < 1e-12);
    }
}
