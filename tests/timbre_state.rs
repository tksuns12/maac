use maac::{compiler::compile, dsp::DspEngine, graph::GraphProcessor, parse, Plan};

fn fixture(generator: &str, extra: &str, output: &str, control: &str, automation: &str) -> Plan {
    let text = format!(
        r#"maac 1;
project p {{ score = [0q, 1/25q]; tail = 1ms; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }} meter metre {{ points = [(0q, 4, 4)]; }}
instrument tone {{ channels = 1; voice v {{ channels = 1; amplitude = &amp; output = &{output}:out;
node amp {{ type = "synth.adsr/1"; params = {{ release = 1ms; }}; }}
node generator {{ {generator} }} node color {{ type = "synth.timbre/1"; }} {extra}
}} {control} }}
node sound {{ instrument = &tone; config = {{ voices = 2; }}; }} track t {{ target = &sound:events; }}
curve shade {{ clock = normalized; points = [(0, 1/4, linear), (1/3, 1, linear), (2/3, 0, linear), (1, 1/2, step)]; }}
curve other {{ clock = normalized; points = [(0, 3/4, linear), (1, 1/4, step)]; }}
curve bend {{ clock = normalized; points = [(0, 100ct, linear), (1, -100ct, step)]; }}
curve gain {{ clock = seconds; points = [(0s, 1, step), (1ms, 0, step), (3ms, 1, step)]; }}
{automation}
pattern notes {{ length = 1/50q;
note a {{ at = 0q; dur = 1/50q; pitch = 440Hz; velocity = 1/2; expression t {{ kind = timbre; curve = &shade; }} expression g {{ kind = gain; curve = &gain; }} expression p {{ kind = pitch; curve = &bend; }} }}
note b {{ at = 1/200q; dur = 1/50q; pitch = 330Hz; velocity = 1/4; expression t {{ kind = timbre; curve = &other; }} }}
}} place play {{ pattern = &notes; track = &t; at = 0q; }}"#
    );
    compile(&parse(&text).unwrap()).unwrap()
}
fn render(plan: &Plan) -> Vec<f64> {
    // Serialized embedded resources suffice; no source or fixture WAV is read.
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
    assert_eq!(samples, collect(), "reset must replay complete state");
    samples
}
fn shade(index: usize, age: usize) -> f64 {
    let t = age.min(480) as f64 / 160.0;
    if index == 1 {
        return 0.75 - t / 6.0;
    }
    if t < 1.0 {
        0.25 + 0.75 * t
    } else if t < 2.0 {
        2.0 - t
    } else {
        0.5 * (t - 2.0)
    }
}
fn frequency(index: usize, age: usize) -> f64 {
    if index == 0 {
        440.0 * 2.0_f64.powf((100.0 - 200.0 * age.min(480) as f64 / 480.0) / 1200.0)
    } else {
        330.0
    }
}
fn amplitude(index: usize, age: usize) -> f64 {
    let gain = if index == 0 && (48..144).contains(&age) {
        0.0
    } else {
        1.0
    };
    gain * envelope(age, 480) * if index == 0 { 0.5 } else { 0.25 }
}
fn verify(plan: &Plan, tolerance: f64, mut sample: impl FnMut(usize, usize, usize) -> f64) {
    for (frame, actual) in render(plan).iter().enumerate() {
        let mut expected = 0.0;
        for (index, onset) in [0, 120].into_iter().enumerate() {
            if (onset..onset + 528).contains(&frame) {
                let age = frame - onset;
                // Advance each voice even while its final gain is zero.
                expected += sample(index, age, frame) * amplitude(index, age);
            }
        }
        assert!(
            (actual - expected).abs() < tolerance,
            "frame {frame}: {actual} != {expected}"
        );
    }
}
// Independent seed arithmetic and transfer-function derivation; never calls Pluck.
// The 3e-12 tolerance covers accumulated differences between equivalent fractional
// delay formulas, matching the existing pitch-state string oracle tolerance.
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
    fn next(&mut self, frequency: f64, damping: f64) -> f64 {
        let omega = std::f64::consts::TAU * frequency / 48000.0;
        let q = damping / 2.0;
        let loss_phase = (q * omega.sin() / (1.0 - q + q * omega.cos())).atan();
        let delay = (std::f64::consts::TAU - loss_phase) / omega;
        let integer = delay.floor() as usize;
        let tangent = ((delay - integer as f64) * omega).tan();
        let fraction = tangent / (omega.sin() + tangent * (1.0 - omega.cos()));
        let write = self.age % 2402;
        let read = (1.0 - fraction) * self.history[(write + 2402 - integer) % 2402]
            + fraction * self.history[(write + 2401 - integer) % 2402];
        self.history[write] =
            10.0_f64.powf(-3.0 / (frequency * 3.0)) * ((1.0 - q) * read + q * self.previous);
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
fn timbre_damping_keeps_seeded_strings_through_mute_overlap_and_release() {
    let plan = fixture(
        r#"type = "synth.pluck/1"; config = { seed = 1; }; params = { damping = 1; decay = 3s; };"#,
        "modulate damping { from = &color:out; to = &generator.params.damping; depth = -1; }",
        "generator",
        "",
        "",
    );
    let mut strings = [StringOracle::new(), StringOracle::new()];
    verify(&plan, 3e-12, |index, age, _| {
        strings[index].next(frequency(index, age), 1.0 - shade(index, age))
    });
}
const CYCLES: [[f64; 8]; 2] = [
    [0.0, 0.5, 1.0, 0.25, -0.5, -1.0, -0.25, 0.125],
    [0.75, 0.25, -0.5, -0.125, -0.75, 0.5, 1.0, -0.25],
];
fn embed_table(plan: &mut Plan, morph: bool) {
    let resources = plan.instruments.as_mut().unwrap();
    resources.wavetables.push(maac::wavetable::Wavetable {
        id: "cycle".into(),
        cycle_length: 8,
        samples: if morph {
            CYCLES.concat()
        } else {
            CYCLES[0].to_vec()
        },
    });
    // Same retained-plan provenance fixture as instrument_pitch_state.rs.
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
}
fn lookup(cycle: &[f64; 8], phase: f64) -> f64 {
    // At these pitches all four harmonics remain below Nyquist.
    let position = phase * 8.0;
    let left = position.floor() as usize;
    cycle[left % 8] + (cycle[(left + 1) % 8] - cycle[left % 8]) * (position - position.floor())
}
#[test]
fn timbre_morphs_frames_without_resetting_each_voices_phase() {
    let mut plan = fixture(
        r#"type = "synth.sine/1"; params = { phase = 1/8; };"#,
        "",
        "generator",
        "",
        "",
    );
    embed_table(&mut plan, true);
    // Compile a legal modulation against a sample-rate scalar, then retarget the
    // retained graph to the wavetable's position parameter before validation.
    let mapped = fixture(
        r#"type = "synth.sine/1"; params = { phase = 1/8; };"#,
        "modulate morph { from = &color:out; to = &generator.params.ratio; depth = 1; }",
        "generator",
        "",
        "",
    );
    let mut modulation = mapped.instruments.unwrap().programs[0].voice.modulations[0].clone();
    modulation.to.parameter = "position".into();
    plan.instruments.as_mut().unwrap().programs[0]
        .voice
        .modulations
        .push(modulation);
    let mut phases = [0.125, 0.125];
    verify(&plan, 1e-12, |index, age, _| {
        let low = lookup(&CYCLES[0], phases[index]);
        let high = lookup(&CYCLES[1], phases[index]);
        phases[index] = (phases[index] + frequency(index, age) / 48000.0).rem_euclid(1.0);
        low + (high - low) * shade(index, age)
    });
}
#[test]
fn timbre_cutoff_combines_control_and_lfo_while_filter_history_continues() {
    let mut plan = fixture(r#"type = "synth.sine/1"; params = { phase = 1/8; };"#,
        r#"node filter { type = "synth.onepole/1"; config = { channels = 1; }; }
node lfo { type = "synth.lfo/1"; params = { frequency = 0Hz; phase = 1/4; }; }
connect audio { from = &generator:out; to = &filter:in; }
modulate color_cutoff { from = &color:out; to = &filter.params.cutoff; depth = 3000Hz; }
modulate lfo_cutoff { from = &lfo:out; to = &filter.params.cutoff; depth = 100Hz; }"#, "filter",
        "control cutoff { target = &v.filter.params.cutoff; default = 800Hz; }",
        "curve bright { clock = seconds; points = [(0s, 800Hz, step), (2ms, 1200Hz, step), (6ms, 600Hz, step)]; } automation a { target = &sound.params.cutoff; curve = &bright; at = 0s; }");
    embed_table(&mut plan, false);
    let mut phases = [0.125, 0.125];
    let mut history = [0.0, 0.0];
    verify(&plan, 1e-12, |index, age, frame| {
        let input = lookup(&CYCLES[0], phases[index]);
        phases[index] = (phases[index] + frequency(index, age) / 48000.0).rem_euclid(1.0);
        let baseline = if frame < 96 {
            800.0
        } else if frame < 288 {
            1200.0
        } else {
            600.0
        };
        let cutoff = baseline + 3000.0 * shade(index, age) + 100.0;
        let pole = (-std::f64::consts::TAU * cutoff / 48000.0).exp();
        history[index] = (1.0 - pole) * input + pole * history[index];
        history[index]
    });
}
