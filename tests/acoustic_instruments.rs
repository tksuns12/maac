use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::Path;

use maac::bundle::sha256_digest;
use maac::export::{write_wav, WavFormat};
use maac::{compile_bundle, render, Plan, SourceBundle};

const NAMES: [&str; 3] = ["nylon_guitar", "steel_guitar", "muted_guitar"];
const CONTROLS: [&str; 6] = [
    "level",
    "brightness",
    "release",
    "pan",
    "bend_ratio",
    "vibrato_amount",
];

fn fixture(name: &str, notes: &str, params: &str, quarters: &str, tail: &str) -> Plan {
    fixture_extra(name, notes, params, quarters, tail, "")
}

fn fixture_extra(
    name: &str,
    notes: &str,
    params: &str,
    quarters: &str,
    tail: &str,
    extra: &str,
) -> Plan {
    let source = format!(
        r#"maac 1;
project test {{ score = [0q, {quarters}q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; tail = {tail}s; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import acoustic {{ builtin = "std/acoustic/1.0.0"; }}
node sound {{ instrument = &acoustic.{name}; config = {{ voices = 8; }}; params = {{ {params} }}; }}
pattern phrase {{ length = {quarters}q; {notes} }}
track notes {{ target = &sound:events; }}
place once {{ pattern = &phrase; track = &notes; at = 0q; }}
{extra}
"#
    );
    compile_bundle(&SourceBundle::new("isolated.maac", source))
        .unwrap_or_else(|error| panic!("{name} compiles: {error:?}"))
}

fn audio(plan: &Plan) -> Vec<[f64; 2]> {
    assert_eq!(plan.output.channels, 2);
    let mut samples = Vec::new();
    render(plan, |frame| {
        assert_eq!(frame.len(), 2);
        assert!(frame.iter().all(|value| value.is_finite()));
        samples.push([frame[0], frame[1]]);
        Ok(())
    })
    .expect("finite stereo render");
    assert_eq!(samples.len() as u64, plan.output.total_frames);
    samples
}

fn peak(samples: &[[f64; 2]]) -> f64 {
    samples
        .iter()
        .flatten()
        .fold(0.0_f64, |peak, value| peak.max(value.abs()))
}

fn signature(samples: &[[f64; 2]]) -> String {
    let bytes: Vec<_> = samples
        .iter()
        .flatten()
        .flat_map(|sample| sample.to_bits().to_le_bytes())
        .collect();
    sha256_digest(&bytes)
}

fn window(samples: &[[f64; 2]], start: f64, end: f64) -> &[[f64; 2]] {
    &samples[(start * 48_000.0) as usize..(end * 48_000.0) as usize]
}

fn pcm16(plan: &Plan) {
    let mut sink = Cursor::new(Vec::new());
    let stats = write_wav(&mut sink, plan, WavFormat::Pcm16).expect("PCM16 headroom");
    assert_eq!(stats.frames, plan.output.total_frames);
    assert_eq!(stats.channels, 2);
}

fn pitch(midi: u8) -> String {
    let names = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", names[(midi % 12) as usize], midi / 12 - 1)
}

#[test]
fn acoustic_examples_are_standalone_distinct_and_pcm16_safe() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut signatures = BTreeSet::new();
    for name in NAMES.into_iter().chain(["fingerpicked_phrase"]) {
        let source =
            std::fs::read_to_string(root.join("examples/acoustic").join(format!("{name}.maac")))
                .unwrap();
        let bundle = SourceBundle::new("standalone.maac", source);
        assert_eq!(bundle.sources.len(), 1);
        assert!(bundle.assets.is_empty());
        let plan = compile_bundle(&bundle).unwrap();
        let programs = &plan.instruments.as_ref().unwrap().programs;
        assert_eq!(
            programs
                .iter()
                .map(|program| program.source.object.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(NAMES)
        );
        for program in programs {
            assert_eq!(
                program
                    .controls
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(CONTROLS)
            );
        }
        let samples = audio(&plan);
        let observed_peak = peak(&samples);
        assert!(
            observed_peak > 0.01 && observed_peak < 0.85,
            "{name}: peak={observed_peak}"
        );
        assert!(
            peak(&samples[samples.len() - 480..]) < 1e-9,
            "{name} ends silently"
        );
        assert!(
            signatures.insert(signature(&samples)),
            "variants have distinct sample streams"
        );
        pcm16(&plan);
        eprintln!(
            "acoustic {name}: frames={}, peak={observed_peak:.9}",
            samples.len()
        );
    }
}

#[test]
fn every_semitone_and_combined_expression_extremes_render() {
    for name in NAMES {
        let notes = (40..=88)
            .enumerate()
            .map(|(i, midi)| {
                format!(
                    "note n{i} {{ at = {}/10q; dur = 3/10q; pitch = {}; velocity = 1; }}",
                    i * 6,
                    pitch(midi)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let plan = fixture(name, &notes, "release = 80ms;", "30", "0.2");
        let samples = audio(&plan);
        for i in 0..49 {
            assert!(
                peak(window(&samples, i as f64 * 0.3, i as f64 * 0.3 + 0.2)) > 1e-5,
                "{name}, MIDI {}",
                i + 40
            );
        }
        assert!(
            peak(&samples) < 0.25,
            "single-note sweep peak {}",
            peak(&samples)
        );
        for (label, note_pitch, ratio) in [
            ("down", "E2", "0.890898718140"),
            ("up", "E6", "1.122462048309"),
        ] {
            let note =
                format!("note n {{ at = 0q; dur = 2q; pitch = {note_pitch}; velocity = 1; }}");
            let plan = fixture(
                name,
                &note,
                &format!("bend_ratio = {ratio}; vibrato_amount = 1;"),
                "2",
                "0.4",
            );
            let samples = audio(&plan);
            assert!(peak(&samples) > 1e-5 && peak(&samples) < 0.25);
            eprintln!(
                "acoustic {name} {label}+vibrato: peak={:.9}",
                peak(&samples)
            );
        }
        eprintln!("acoustic {name} MIDI40..88: peak={:.9}", peak(&samples));
    }
}

#[test]
fn controls_release_velocity_scaling_and_retained_plan_replay() {
    let note = "note n { at = 0q; dur = 1/5q; pitch = C4; velocity = 1; }";
    for name in NAMES {
        let plan = fixture(name, note, "", "1", "0.5");
        let baseline = audio(&plan);
        assert_eq!(baseline, audio(&plan), "same-host reset replay");
        let retained = Plan::from_json(&plan.to_json().unwrap()).unwrap();
        assert_eq!(
            baseline,
            audio(&retained),
            "source-free retained plan replay"
        );
        assert_eq!(
            peak(&audio(&fixture(name, note, "level = 0;", "1", "0.5"))),
            0.0
        );
        let left = audio(&fixture(name, note, "pan = -1;", "1", "0.5"));
        assert_eq!(
            left.iter()
                .fold(0.0_f64, |peak, frame| peak.max(frame[1].abs())),
            0.0
        );
        assert!(peak(&left) > peak(&baseline));
        let soft = audio(&fixture(
            name,
            &note.replace("velocity = 1", "velocity = 1/4"),
            "",
            "1",
            "0.5",
        ));
        assert!(baseline
            .iter()
            .zip(&soft)
            .flat_map(|(a, b)| a.iter().zip(b))
            .all(|(a, b)| (a * 0.25 - b).abs() < 1e-14));
        let dark = audio(&fixture(name, note, "brightness = 500Hz;", "1", "0.5"));
        assert!(baseline
            .iter()
            .zip(&dark)
            .flat_map(|(a, b)| a.iter().zip(b))
            .any(|(a, b)| (a - b).abs() > 1e-3));
        let short = audio(&fixture(name, note, "release = 30ms;", "1", "0.5"));
        let long = audio(&fixture(name, note, "release = 400ms;", "1", "0.5"));
        assert!(peak(window(&short, 0.3, 0.4)) < 1e-9);
        assert!(peak(window(&long, 0.2, 0.3)) > 1e-7);
        assert!(peak(window(&long, 0.8, 0.9)) < 1e-9);
        eprintln!(
            "acoustic {name} control baseline: peak={:.9}, release-tail={:.9}",
            peak(&baseline),
            peak(window(&long, 0.2, 0.3))
        );
    }
}

#[test]
fn long_gates_keep_string_decay_and_chords_have_headroom() {
    for name in NAMES {
        let plan = fixture(
            name,
            "note held { at = 0q; dur = 8q; pitch = C4; velocity = 1; }",
            "",
            "8",
            "0.5",
        );
        let samples = audio(&plan);
        if name != "muted_guitar" {
            assert!(
                peak(window(&samples, 1.5, 2.0)) > 1e-5,
                "{name} still decays beyond old natural zero"
            );
        }
        assert!(peak(window(&samples, 4.4, 4.5)) < 1e-9);
        let chord = ["E2", "B2", "E3", "G3", "B3", "E4"]
            .iter()
            .enumerate()
            .map(|(i, pitch)| {
                format!("note n{i} {{ at = 0q; dur = 2q; pitch = {pitch}; velocity = 1; }}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let plan = fixture(name, &chord, "", "2", "0.5");
        let chord_audio = audio(&plan);
        assert!(peak(&chord_audio) < 0.85);
        pcm16(&plan);
        eprintln!(
            "acoustic {name}: held-late-peak={:.9}, chord-peak={:.9}",
            peak(window(&samples, 1.5, 2.0)),
            peak(&chord_audio)
        );
    }
}

#[test]
fn smooth_bends_and_vibrato_with_held_notes_are_finite_and_pcm_safe() {
    let extra = "curve bend { clock = score; points = [(0q, 1, linear), (1q, 1.122462048309, linear), (2q, 0.890898718140, linear), (3q, 1, step)]; } automation bend_lane { target = &sound.params.bend_ratio; curve = &bend; at = 0q; } curve vibrato { clock = score; points = [(0q, 0, linear), (1q, 1, step), (3q, 0, step)]; } automation vibrato_lane { target = &sound.params.vibrato_amount; curve = &vibrato; at = 0q; }";
    for name in NAMES {
        let plan = fixture_extra(
            name,
            "note hold { at = 0q; dur = 4q; pitch = A3; velocity = 0.8; }",
            "",
            "4",
            "0.5",
            extra,
        );
        let samples = audio(&plan);
        assert!(peak(&samples) > 0.01 && peak(&samples) < 0.25);
        pcm16(&plan);
    }
}
