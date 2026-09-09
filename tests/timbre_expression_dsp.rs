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
node color {{ type = "synth.timbre/1"; }} }} }}
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
            r#"note a { at = 0q; dur = 1/3000q; pitch = A4; expression e { kind = timbre; curve = &shade; } }
note b { at = 1/6000q; dur = 1/3000q; pitch = C4; expression e { kind = timbre; curve = &shade; } }
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
fn mapped_control_with_pitch_gain_and_timbre_matches_sine() {
    let text = source(r#"note a { at = 0q; dur = 1/1000q; pitch = 1000Hz;
expression t { kind = timbre; curve = &shade; } expression g { kind = gain; curve = &gain; } expression p { kind = pitch; curve = &pitch; } }"#, r#"curve shade { clock = normalized; points = [(0, 1/2, step), (1, 1/2, step)]; }
curve gain { clock = normalized; points = [(0, 2, step), (1, 2, step)]; }
curve pitch { clock = normalized; points = [(0, 1200ct, step), (1, 1200ct, step)]; }"#)
    .replace("output = &color:out", "output = &osc:out")
    .replace("node color {", "node osc { type = \"synth.sine/1\"; } modulate m { from = &color:out; to = &osc.params.level; depth = 1; } node color {")
    .replace("} }\nnode synth", "} control level { target = &v.osc.params.level; default = 1/4; } }\nnode synth");
    let result = samples(&text);
    let mut phase: f64 = 0.0;
    for (frame, actual) in result.iter().enumerate() {
        // Independent oracle for the documented 2048-sample sine table.
        let position = phase * 2048.0;
        let left = position.floor();
        let a = (std::f64::consts::TAU * left / 2048.0).sin();
        let b = (std::f64::consts::TAU * ((left + 1.0) % 2048.0) / 2048.0).sin();
        let expected = 1.5 * (a + (b - a) * (position - left));
        phase = (phase + 2000.0 / 48000.0).fract();
        assert!(
            (actual - expected).abs() < 1e-13,
            "frame {frame}: {actual} != {expected}"
        );
    }
}
#[test]
fn zero_gain_does_not_hide_invalid_mapping() {
    let text = source(r#"note a { at = 0q; dur = 1/1000q; pitch = A4; expression t { kind = timbre; curve = &shade; } expression g { kind = gain; curve = &zero; } }"#, r#"curve shade { clock = normalized; points = [(0, 1, step), (1, 1, step)]; } curve zero { clock = normalized; points = [(0, 0, step), (1, 0, step)]; }"#)
    .replace("node color {", "node osc { type = \"synth.sine/1\"; } modulate m { from = &color:out; to = &osc.params.level; depth = -2; } node color {");
    let plan = compile(&parse(&text).unwrap()).unwrap();
    let error = render(&plan, |_| Ok(())).unwrap_err();
    assert!(error.to_string().contains("level"), "{error}");
}
