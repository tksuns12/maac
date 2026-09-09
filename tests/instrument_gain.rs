use maac::{compiler::compile, dsp::DspEngine, parse, plan::Plan};

fn plan(curve: &str, expression: bool, stereo: bool) -> Plan {
    let pan = if stereo {
        r#"node pan { type = "synth.pan/1"; params = { pan = 0; }; } connect p { from = &osc:out; to = &pan:in; }"#
    } else {
        ""
    };
    let channels = if stereo { 2 } else { 1 };
    let output = if stereo { "pan" } else { "osc" };
    let expr = if expression {
        "expression g { kind = gain; curve = &curve; }"
    } else {
        ""
    };
    let source = format!(
        r#"maac 1;
project p {{ score = [0q, 1/1000q]; tail = 1/12000s; rate = 48000Hz; tempo = &clock; meter = &meter; output = &lead:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter meter {{ points = [(0q, 4, 4)]; }}
instrument instrument {{ channels = {channels}; voice v {{ channels = {channels}; amplitude = &amp; output = &{output}:out;
node amp {{ type = "synth.adsr/1"; params = {{ release = 1/12000s; }}; }}
node osc {{ type = "synth.sine/1"; params = {{ phase = 1/4; }}; }} {pan}
}} }}
node lead {{ instrument = &instrument; config = {{ voices = 2; }}; }}
track t {{ target = &lead:events; }}
curve curve {{ clock = normalized; points = {curve}; }}
pattern notes {{ length = 1/1000q; note a {{ at = 0q; dur = 1/1000q; pitch = 480Hz; velocity = 1/2; {expr} }} }}
place placement {{ pattern = &notes; track = &t; at = 0q; }}"#
    );
    compile(&parse(&source).unwrap()).unwrap()
}
fn collect(plan: &Plan) -> Vec<Vec<f64>> {
    let mut engine = DspEngine::new(plan).unwrap();
    let mut samples = vec![];
    engine
        .render(|frame| {
            samples.push(frame.to_vec());
            Ok(())
        })
        .unwrap();
    let mut repeated = vec![];
    engine
        .render(|frame| {
            repeated.push(frame.to_vec());
            Ok(())
        })
        .unwrap();
    assert_eq!(samples, repeated);
    samples
}
#[test]
fn unity_gain_preserves_exact_mono_and_stereo_samples() {
    for stereo in [false, true] {
        assert_eq!(
            collect(&plan("[(0, 1, step), (1, 1, step)]", true, stereo)),
            collect(&plan("[(0, 1, step), (1, 1, step)]", false, stereo))
        );
    }
}
#[test]
fn gain_modulates_completed_voice_and_holds_gate_endpoint_during_release() {
    for stereo in [false, true] {
        let reference = collect(&plan("[(0, 1, step), (1, 1, step)]", false, stereo));
        let actual = collect(&plan("[(0, 0, linear), (1, 2, step)]", true, stereo));
        for (frame, (actual, reference)) in actual.iter().zip(&reference).enumerate() {
            let gain = 2.0 * (frame.min(24) as f64) / 24.0;
            for (actual, reference) in actual.iter().zip(reference) {
                assert!((actual - reference * gain).abs() < 1e-14, "frame {frame}");
            }
        }
        assert!(reference[25][0].abs() > 0.0);
    }
}

#[test]
fn overlapping_voices_apply_their_own_curves_before_summing() {
    let first = plan("[(0, 0, linear), (1, 2, step)]", true, false);
    let mut second = plan("[(0, 3, step), (1, 3, step)]", true, false);
    if let maac::plan::EventKind::Note { pitch_hz, .. } = &mut second.events[0].kind {
        *pitch_hz = 960.0;
    }
    let mut combined = first.clone();
    let mut event = second.events[0].clone();
    event.address = "placement/0/z".into();
    event.order = 1;
    combined.events.push(event);
    let a = collect(&first);
    let b = collect(&second);
    for ((actual, a), b) in collect(&combined).iter().zip(a).zip(b) {
        assert_eq!(actual[0], a[0] + b[0]);
    }
}
