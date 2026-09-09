use maac::{compiler::compile, dsp::DspEngine, graph::GraphProcessor, parse, Plan};

fn source(generator: &str, notes: &str, curves: &str, placement: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 1/25q]; tail = 1ms; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }} meter metre {{ points = [(0q, 4, 4)]; }}
instrument tone {{ channels = 1; voice v {{ channels = 1; amplitude = &amp; output = &generator:out;
node amp {{ type = "synth.adsr/1"; params = {{ release = 1ms; }}; }}
node generator {{ {generator} }}
node fm {{ type = "synth.lfo/1"; params = {{ frequency = 0Hz; phase = 1/4; }}; }}
}} control ratio {{ target = &v.generator.params.ratio; default = 1; }} }}
node sound {{ instrument = &tone; config = {{ voices = 2; }}; }} track t {{ target = &sound:events; }}
{curves}
pattern notes {{ length = 1/50q; {notes} }} {placement}"#
    )
}
fn compile_source(source: &str) -> Plan {
    compile(&parse(source).unwrap()).unwrap()
}
fn render(plan: &Plan) -> Vec<f64> {
    let loaded = Plan::from_json(&plan.to_json().unwrap()).unwrap();
    let mut engine = DspEngine::new(&loaded).unwrap();
    let mut collect = || {
        let mut samples = vec![];
        engine
            .render(|frame| {
                samples.push(frame[0]);
                Ok(())
            })
            .unwrap();
        samples
    };
    let samples = collect();
    assert_eq!(samples, collect(), "reset must replay voice history");
    samples
}
const PLACEMENT: &str = "place play { pattern = &notes; track = &t; at = 0q; }";
const PLUCK: &str =
    r#"type = "synth.pluck/1"; config = { seed = 1; }; params = { damping = 1/2; decay = 3s; };"#;
const BEND: &str = "curve bend { clock = normalized; points = [(0, 300ct, linear), (1/3, -300ct, linear), (2/3, 450ct, linear), (1, -150ct, step)]; }";
fn bend(frame: usize) -> f64 {
    let t = frame.min(480) as f64 / 160.0;
    if t < 1.0 {
        300.0 - 600.0 * t
    } else if t < 2.0 {
        -300.0 + 750.0 * (t - 1.0)
    } else {
        450.0 - 600.0 * (t - 2.0)
    }
}
// Independent seed arithmetic and transfer-function derivation; never calls Pluck.
struct StringOracle {
    history: Vec<f64>,
    previous: f64,
    age: usize,
}
impl StringOracle {
    fn new() -> Self {
        let mut seed = 1_u64;
        let history = (0..2402)
            .map(|_| {
                seed ^= (seed << 13) & 0xffff_ffff;
                seed ^= seed >> 17;
                seed ^= (seed << 5) & 0xffff_ffff;
                seed as f64 / 2147483648.0 - 1.0
            })
            .collect();
        Self {
            history,
            previous: 0.0,
            age: 0,
        }
    }
    fn next(&mut self, frequency: f64) -> f64 {
        let omega = std::f64::consts::TAU * frequency / 48000.0;
        let loss_phase = (0.25 * omega.sin() / (0.75 + 0.25 * omega.cos())).atan();
        let delay = (std::f64::consts::TAU - loss_phase) / omega;
        let integer = delay.floor() as usize;
        let tangent = ((delay - integer as f64) * omega).tan();
        let fraction = tangent / (omega.sin() + tangent * (1.0 - omega.cos()));
        let write = self.age % 2402;
        let read = (1.0 - fraction) * self.history[(write + 2402 - integer) % 2402]
            + fraction * self.history[(write + 2401 - integer) % 2402];
        self.history[write] =
            10.0_f64.powf(-3.0 / (frequency * 3.0)) * (0.75 * read + 0.25 * self.previous);
        self.previous = read;
        self.age += 1;
        read
    }
}
fn envelope(age: usize, gate: usize) -> f64 {
    if age < gate {
        1.0
    } else {
        (1.0 - (age - gate) as f64 / 48.0).max(0.0)
    }
}
#[test]
fn pluck_vibrato_mute_resume_release_and_overlap_match_seeded_recurrences() {
    let notes = r#"note a { at = 0q; dur = 1/50q; pitch = 440Hz; velocity = 1/2; expression p { kind = pitch; curve = &bend; } expression g { kind = gain; curve = &gain; } }
note b { at = 1/200q; dur = 1/50q; pitch = 330Hz; velocity = 1/4; expression p { kind = pitch; curve = &bend; } }"#;
    let curves = format!("{BEND} curve gain {{ clock = seconds; points = [(0s, 1, step), (1ms, 0, step), (3ms, 1, step)]; }}");
    let plan = compile_source(&source(PLUCK, notes, &curves, PLACEMENT));
    let mut strings = [StringOracle::new(), StringOracle::new()];
    for (frame, actual) in render(&plan).iter().enumerate() {
        let mut expected = 0.0;
        for (index, (onset, pitch, velocity)) in [(0, 440.0, 0.5), (120, 330.0, 0.25)]
            .into_iter()
            .enumerate()
        {
            if (onset..onset + 528).contains(&frame) {
                let age = frame - onset;
                let sample = strings[index].next(pitch * 2.0_f64.powf(bend(age) / 1200.0));
                let gain = if index == 0 && (48..144).contains(&age) {
                    0.0
                } else {
                    1.0
                };
                expected += sample * envelope(age, 480) * velocity * gain;
            }
        }
        assert!(
            (actual - expected).abs() < 3e-12,
            "frame {frame}: {actual} != {expected}"
        );
    }
}
#[test]
fn wavetable_pitch_live_ratio_and_fm_keep_phase_through_negative_frequency() {
    let generator = r#"type = "synth.sine/1"; params = { phase = 1/8; frequency = -900Hz; };"#;
    let notes = "note a { at = 0q; dur = 1/50q; pitch = 440Hz; velocity = 1/2; expression p { kind = pitch; curve = &bend; } }";
    let curves = format!("{BEND} curve tuning {{ clock = seconds; points = [(0s, 1, step), (2ms, 2, step), (6ms, 1/2, step)]; }} automation tune {{ target = &sound.params.ratio; curve = &tuning; at = 0s; }}");
    let text = source(generator, notes, &curves, PLACEMENT).replace("node fm {", "modulate fm_frequency { from = &fm:out; to = &generator.params.frequency; depth = 100Hz; } node fm {");
    let mut plan = compile_source(&text);
    let cycle = [0.0, 0.5, 1.0, 0.25, -0.5, -1.0, -0.25, 0.125];
    let resources = plan.instruments.as_mut().unwrap();
    resources.wavetables.push(maac::wavetable::Wavetable {
        id: "cycle".into(),
        cycle_length: 8,
        samples: cycle.to_vec(),
    });
    resources
        .wavetable_sources
        .push(maac::library::WavetableSource {
            table: "cycle".into(),
            file: resources.entry_source.clone(),
            object: "cycle".into(),
            path: "fixtures/cycle.wav".into(),
            hash: format!("sha256:{}", "0".repeat(64)),
        });
    resources.programs[0]
        .voice
        .nodes
        .iter_mut()
        .find(|node| node.id == "generator")
        .unwrap()
        .processor = GraphProcessor::Wavetable {
        table: "cycle".into(),
    };
    // All frequencies keep all four harmonics legal: direct cyclic interpolation
    // is an independent lookup oracle without relying on production TableBank.
    let mut phase: f64 = 0.125;
    for (frame, actual) in render(&plan).iter().enumerate() {
        let expected = if frame < 528 {
            let position = phase * 8.0;
            let left = position.floor() as usize;
            let value = cycle[left % 8]
                + (cycle[(left + 1) % 8] - cycle[left % 8]) * (position - position.floor());
            let ratio = if frame < 96 {
                1.0
            } else if frame < 288 {
                2.0
            } else {
                0.5
            };
            let frequency = 440.0 * 2.0_f64.powf(bend(frame) / 1200.0) * ratio - 900.0 + 100.0;
            phase = (phase + frequency / 48000.0).rem_euclid(1.0);
            value * envelope(frame, 480) * 0.5
        } else {
            0.0
        };
        assert!(
            (actual - expected).abs() < 1e-12,
            "frame {frame}: {actual} != {expected}"
        );
    }
}
#[test]
fn source_score_stretch_and_offsets_follow_scheduled_gate_not_unshifted_duration() {
    let notes = "note a { at = 0q; dur = 1/1000q; onset_offset = 1/96000s; release_offset = -1/96000s; pitch = 440Hz; expression p { kind = pitch; curve = &bend; } }";
    let curve =
        "curve bend { clock = score; points = [(0q, 600ct, linear), (1/1000q, -600ct, step)]; }";
    let placement = "place play { pattern = &notes; track = &t; at = 0q; stretch = 2; }";
    let plan = compile_source(&source(PLUCK, notes, curve, placement));
    assert_eq!(plan.events[0].on_frame, 1);
    assert_eq!(plan.events[0].off_frame, Some(48));
    let mut string = StringOracle::new();
    for (frame, actual) in render(&plan).iter().enumerate() {
        let expected = if (1..96).contains(&frame) {
            let age = frame - 1;
            let cents = 600.0 - 1200.0 * age.min(47) as f64 / 47.0;
            string.next(440.0 * 2.0_f64.powf(cents / 1200.0)) * envelope(age, 47)
        } else {
            0.0
        };
        assert!(
            (actual - expected).abs() < 3e-12,
            "frame {frame}: {actual} != {expected}"
        );
    }
}
#[test]
fn zero_gain_does_not_hide_live_pluck_frequency_error() {
    for (pitch, ratio) in [("1200ct", "1"), ("0ct", "2")] {
        let notes = "note a { at = 0q; dur = 1/50q; pitch = 3000Hz; expression p { kind = pitch; curve = &bend; } expression g { kind = gain; curve = &zero; } }";
        let curves = format!("curve bend {{ clock = seconds; points = [(0s, 0ct, step), (1ms, {pitch}, step)]; }} curve zero {{ clock = normalized; points = [(0, 0, step), (1, 0, step)]; }} curve tuning {{ clock = seconds; points = [(0s, 1, step), (1ms, {ratio}, step)]; }} automation tune {{ target = &sound.params.ratio; curve = &tuning; at = 0s; }}");
        let plan = compile_source(&source(PLUCK, notes, &curves, PLACEMENT));
        let mut engine = DspEngine::new(&plan).unwrap();
        let mut rendered = 0;
        let error = engine
            .render(|frame| {
                assert_eq!(frame[0], 0.0);
                rendered += 1;
                Ok(())
            })
            .unwrap_err();
        assert_eq!(rendered, 48);
        assert_eq!(error.code(), "E_NONFINITE");
    }
}
