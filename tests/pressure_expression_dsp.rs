use maac::dsp::render;
use maac::{compile, parse};

fn source(notes: &str, curves: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 1/1000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument lead {{ channels = 1;
voice v {{ channels = 1; amplitude = &amp; output = &color:out;
node amp {{ type = "synth.adsr/1"; params = {{ attack = 0s; decay = 0s; sustain = 1; release = 1/12000s; }}; }}
node color {{ type = "synth.pressure/1"; }} }} }}
node synth {{ instrument = &lead; }}
track t {{ target = &synth:events; }}
{curves}
pattern phrase {{ length = 1/1000q; {notes} }}
place p1 {{ pattern = &phrase; track = &t; at = 0q; }}"#
    )
}
fn samples(text: &str) -> Vec<f64> {
    let plan = compile(&parse(text).unwrap()).unwrap();
    let mut result = vec![];
    render(&plan, |frame| {
        result.push(frame[0]);
        Ok(())
    })
    .unwrap();
    result
}
#[test]
fn clocks_overlap_gate_hold_and_default_zero_are_analytic() {
    for (clock, points) in [
        ("normalized", "[(0, 0, linear), (1, 1, step)]"),
        ("seconds", "[(0s, 0, linear), (1/6000s, 1, step)]"),
        ("score", "[(0q, 0, linear), (1/3000q, 1, step)]"),
    ] {
        let text = source(
            r#"note a { at = 0q; dur = 1/3000q; pitch = A4; expression e { kind = pressure; curve = &shade; } }
note b { at = 1/6000q; dur = 1/3000q; pitch = C4; expression e { kind = pressure; curve = &shade; } }
note silent { at = 0q; dur = 1/1000q; pitch = A4; }"#,
            &format!("curve shade {{ clock = {clock}; points = {points}; }}"),
        );
        let result = samples(&text);
        for (frame, actual) in result.iter().enumerate() {
            let voice = |on: usize| {
                if frame < on {
                    0.0
                } else {
                    let age = frame - on;
                    (age.min(8) as f64 / 8.0)
                        * if age < 8 {
                            1.0
                        } else {
                            (1.0 - (age - 8) as f64 / 4.0).max(0.0)
                        }
                }
            };
            assert!(
                (actual - voice(0) - voice(4)).abs() < 1e-12,
                "{clock} frame {frame}: {actual}"
            );
        }
    }
}

#[test]
fn all_four_expressions_signed_mapping_and_phase_continuity_match_sine() {
    let text = source(
        r#"note a { at = 0q; dur = 1/1000q; pitch = 1000Hz;
expression touch { kind = pressure; curve = &touch; }
expression color { kind = timbre; curve = &shade; }
expression dynamics { kind = gain; curve = &gain; }
expression bend { kind = pitch; curve = &pitch; } }"#,
        r#"curve touch { clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }
curve shade { clock = normalized; points = [(0, 1/2, step), (1, 1/2, step)]; }
curve gain { clock = normalized; points = [(0, 2, step), (1, 2, step)]; }
curve pitch { clock = normalized; points = [(0, 1200ct, step), (1, 1200ct, step)]; }"#,
    )
    .replace("output = &color:out", "output = &osc:out")
    .replace(
        "node color {",
        r#"node osc { type = "synth.sine/1"; }
node shade { type = "synth.timbre/1"; }
modulate z_touch { from = &color:out; to = &osc.params.level; depth = -1/4; }
modulate a_shade { from = &shade:out; to = &osc.params.level; depth = 1; }
node color {"#,
    )
    .replace(
        "} }\nnode synth",
        "} control level { target = &v.osc.params.level; default = 1/4; } }\nnode synth",
    );
    let result = samples(&text);
    let mut phase: f64 = 0.0;
    for (frame, actual) in result.iter().enumerate() {
        let position = phase * 2048.0;
        let left = position.floor();
        let a = (std::f64::consts::TAU * left / 2048.0).sin();
        let b = (std::f64::consts::TAU * ((left + 1.0) % 2048.0) / 2048.0).sin();
        let level = 0.75 - 0.25 * frame as f64 / 24.0;
        let expected = 2.0 * level * (a + (b - a) * (position - left));
        phase = (phase + 2000.0 / 48000.0).fract();
        assert!(
            (actual - expected).abs() < 1e-13,
            "frame {frame}: {actual} != {expected}"
        );
    }
}

#[test]
fn first_sample_and_each_pressure_node_receive_current_value() {
    let text = source(
        "note a { at = 0q; dur = 1/1000q; pitch = A4; expression p { kind = pressure; curve = &touch; } }",
        "curve touch { clock = normalized; points = [(0, 1/2, step), (1, 1/2, step)]; }",
    ).replace("output = &color:out", "output = &sum:out")
     .replace("node color {", r#"node sum { type = "synth.mix/1"; config = { channels = 1; }; }
node second { type = "synth.pressure/1"; }
connect a { from = &color:out; to = &sum:in; }
connect b { from = &second:out; to = &sum:in; }
node color {"#);
    assert_eq!(samples(&text), vec![1.0; 24]);
}

#[test]
fn modulation_reductions_use_ids_and_validate_only_final_value() {
    let text = source(
        "note a { at = 0q; dur = 1/1000q; pitch = A4; expression p { kind = pressure; curve = &touch; } }",
        "curve touch { clock = normalized; points = [(0, 1, step), (1, 1, step)]; }",
    ).replace("output = &color:out", "output = &scaled:out")
     .replace("node color {", r#"node scaled { type = "synth.gain/1"; config = { channels = 1; }; params = { level = 1/1152921504606846976; }; }
connect input { from = &color:out; to = &scaled:in; }
modulate z { from = &color:out; to = &scaled.params.level; depth = 1/1152921504606846976; }
modulate b { from = &color:out; to = &scaled.params.level; depth = -128; }
modulate a { from = &color:out; to = &scaled.params.level; depth = 128; }
node color {"#);
    // Binary64 reduction loses the baseline at +128, then restores the tiny z contribution.
    assert_eq!(samples(&text), vec![2.0_f64.powi(-60); 24]);
}

#[test]
fn zero_gain_and_velocity_do_not_hide_late_invalid_mapping() {
    let text = source(
        r#"note a { at = 0q; dur = 1/1000q; pitch = A4; expression t { kind = pressure; curve = &touch; } expression g { kind = gain; curve = &zero; } }"#,
        r#"curve touch { clock = seconds; points = [(0s, 0, step), (1/12000s, 1, step)]; }
curve zero { clock = normalized; points = [(0, 0, step), (1, 0, step)]; }"#,
    ).replace("node color {", r#"node osc { type = "synth.sine/1"; }
modulate m { from = &color:out; to = &osc.params.level; depth = -2; }
node color {"#);
    for text in [
        text.clone(),
        text.replace("pitch = A4;", "pitch = A4; velocity = 0;")
            .replace("expression g { kind = gain; curve = &zero; }", ""),
    ] {
        let plan = compile(&parse(&text).unwrap()).unwrap();
        let mut frames = 0;
        let error = render(&plan, |_| {
            frames += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(
            frames, 4,
            "failure must occur after successful silent frames"
        );
        assert!(error.to_string().contains("level"), "{error}");
    }
}
