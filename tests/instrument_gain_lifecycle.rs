use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle;
use maac::dsp::DspEngine;
use maac::Plan;

fn note(id: &str, on: usize, gain: &str, velocity: &str) -> String {
    let expression = if gain.is_empty() {
        String::new()
    } else {
        format!("expression dynamics {{ kind = gain; curve = &{gain}; }}")
    };
    format!("note {id} {{ at = {on}/24000q; dur = 1/100q; pitch = A4; velocity = {velocity}; {expression} }}")
}

fn plan(pluck: bool, shared: bool, voices: usize, notes: &str) -> Plan {
    let generator = if pluck {
        r#"type = "synth.pluck/1"; config = { seed = 1; };"#
    } else {
        r#"type = "synth.sine/1"; params = { phase = 0.25; };"#
    };
    let shared_graph = if shared {
        r#"shared effect {
            channels = 1; output = &low:out;
            node low { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 1000Hz; }; }
            connect input_low { from = &input:out; to = &low:in; }
        }"#
    } else {
        ""
    };
    compile_bundle(&SourceBundle::new(
        "lifecycle.maac",
        format!(
            r#"maac 1;
project song {{ score = [0q, 1/40q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; tail = 2ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument tone {{
    channels = 1;
    voice v {{
        channels = 1; amplitude = &amp; output = &filter:out;
        node amp {{ type = "synth.adsr/1"; params = {{ release = 1ms; }}; }}
        node generator {{ {generator} }}
        node filter {{ type = "synth.onepole/1"; config = {{ channels = 1; }}; params = {{ cutoff = 2000Hz; }}; }}
        connect filtered {{ from = &generator:out; to = &filter:in; }}
    }}
    {shared_graph}
}}
node sound {{ instrument = &tone; config = {{ voices = {voices}; }}; }}
curve returning {{ clock = seconds; points = [(0s, 1/2, step), (1ms, 0, step), (3ms, 1/2, step)]; }}
curve fading {{ clock = seconds; points = [(0s, 1/4, step), (2ms, 0, step)]; }}
curve zero {{ clock = normalized; points = [(0, 0, step), (1, 0, step)]; }}
pattern phrase {{ length = 1/40q; {notes} }}
track t {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &t; at = 0q; }}
"#
        ),
    ))
    .unwrap()
}

fn collect(engine: &mut DspEngine<'_>) -> Vec<f64> {
    let mut out = Vec::new();
    engine
        .render(|frame| {
            out.push(frame[0]);
            Ok(())
        })
        .unwrap();
    out
}

fn render(plan: &Plan) -> Vec<f64> {
    collect(&mut DspEngine::new(plan).unwrap())
}

#[test]
fn mute_resume_preserves_oscillator_filter_and_pluck_state_and_independent_overlap() {
    for pluck in [false, true] {
        let first = render(&plan(pluck, false, 2, &note("a", 0, "", "0.6")));
        let second = render(&plan(pluck, false, 2, &note("b", 48, "", "0.3")));
        let gained = plan(
            pluck,
            false,
            2,
            &(note("a", 0, "returning", "0.6") + &note("b", 48, "fading", "0.3")),
        );
        let mut engine = DspEngine::new(&gained).unwrap();
        let actual = collect(&mut engine);
        assert_eq!(actual, collect(&mut engine), "reset must repeat all state");
        for frame in 0..actual.len() {
            let gain_a = if (48..144).contains(&frame) { 0.0 } else { 0.5 };
            let gain_b = if (48..144).contains(&frame) {
                0.25
            } else {
                0.0
            };
            assert_eq!(
                actual[frame],
                first[frame] * gain_a + second[frame] * gain_b,
                "pluck={pluck}, frame={frame}"
            );
        }
        assert!(actual[144..240].iter().any(|sample| *sample != 0.0));
        // The resumed signal is the already-running voice, not a fresh trigger.
        assert_ne!(&actual[144..160], &actual[..16]);
    }
}

#[test]
fn shared_filter_receives_weighted_voice_sum_and_keeps_history_after_mute_and_retirement() {
    let first = render(&plan(false, false, 2, &note("a", 0, "", "0.6")));
    let second = render(&plan(false, false, 2, &note("b", 48, "", "0.3")));
    let actual = render(&plan(
        false,
        true,
        2,
        &(note("a", 0, "fading", "0.6") + &note("b", 48, "fading", "0.3")),
    ));
    // Independent one-pole recurrence, never the production filter helper.
    let decay = (-std::f64::consts::TAU * 1000.0 / 48000.0).exp();
    let mut expected = 0.0;
    for frame in 0..actual.len() {
        let a = if frame < 96 { first[frame] * 0.25 } else { 0.0 };
        let b = if (48..144).contains(&frame) {
            second[frame] * 0.25
        } else {
            0.0
        };
        expected = (1.0 - decay) * (a + b) + decay * expected;
        assert!((actual[frame] - expected).abs() < 1e-15, "frame={frame}");
    }
    // Both gains are zero at 144; both gates and 48-frame releases end by 336.
    assert_ne!(actual[144], 0.0);
    assert_ne!(actual[336], 0.0);
    assert!(actual[337].abs() < actual[336].abs());
}

#[test]
fn silent_gain_or_velocity_retains_capacity_through_gate_and_release_then_reuses_voice() {
    for (gain, velocity) in [("", "0.6"), ("zero", "0.6"), ("", "0"), ("zero", "0")] {
        for onset in [48, 240, 287] {
            let limited = plan(
                false,
                false,
                1,
                &(note("a", 0, gain, velocity) + &note("b", onset, "", "0.3")),
            );
            let error = DspEngine::new(&limited)
                .unwrap()
                .render(|_| Ok(()))
                .unwrap_err();
            assert_eq!(
                error.code(),
                "E_VOICE_LIMIT",
                "gain={gain}, velocity={velocity}, onset={onset}"
            );
        }
        let reused = render(&plan(
            false,
            false,
            1,
            &(note("a", 0, gain, velocity) + &note("b", 288, "returning", "0.3")),
        ));
        let fresh = render(&plan(false, false, 1, &note("b", 288, "returning", "0.3")));
        assert_eq!(&reused[288..], &fresh[288..]);
        assert!(reused[432..528].iter().any(|sample| *sample != 0.0));
    }
}
