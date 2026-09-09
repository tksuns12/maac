use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use hound::{SampleFormat, WavReader};
use maac::plan::{EventKind, Plan};
use tempfile::tempdir;

const EXAMPLE: &str = include_str!("../examples/timbre-expression.maac");
const CUSTOM: &str = r#"maac 1;
project p { score = [0q, 1/100q]; tail = 1ms; rate = 48000Hz; tempo = &clock; meter = &metre; output = &lead:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument local_sine { channels = 1;
  voice v { channels = 1; amplitude = &amp; output = &filter:out;
    node amp { type = "synth.adsr/1"; params = { release = 1ms; }; }
    node color { type = "synth.timbre/1"; }
    node filter { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 800Hz; }; }
    connect filtered { from = &osc:out; to = &filter:in; }
    modulate brightness { from = &color:out; to = &filter.params.cutoff; depth = 4200Hz; }
    node osc { type = "synth.sine/1"; params = { ratio = 1; }; }
  }
}
node lead { instrument = &local_sine; }
track t { target = &lead:events; }
curve shade { clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }
curve bend { clock = normalized; points = [(0, 0ct, linear), (1, 200ct, step)]; }
curve swell { clock = normalized; points = [(0, 1/4, linear), (1, 1/2, step)]; }
pattern notes { length = 1/100q;
  note a { at = 0q; dur = 1/100q; pitch = A4; velocity = 1/2;
    expression color_expr { kind = timbre; curve = &shade; }
    expression pitch_expr { kind = pitch; curve = &bend; }
    expression dynamics { kind = gain; curve = &swell; }
  }
}
place once { pattern = &notes; track = &t; at = 0q; }
"#;
const LATE_RANGE: &str = r#"maac 1;
project p { score = [0q, 1/100q]; tail = 1ms; rate = 48000Hz; tempo = &clock; meter = &metre; output = &lead:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument local_sine { channels = 1;
  voice v { channels = 1; amplitude = &amp; output = &filter:out;
    node amp { type = "synth.adsr/1"; params = { release = 1ms; }; }
    node color { type = "synth.timbre/1"; }
    node filter { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 800Hz; }; }
    connect filtered { from = &osc:out; to = &filter:in; }
    modulate brightness { from = &color:out; to = &filter.params.cutoff; depth = 30000Hz; }
    node osc { type = "synth.sine/1"; params = { ratio = 1; }; }
  }
}
node lead { instrument = &local_sine; }
track t { target = &lead:events; }
curve shade { clock = seconds; points = [(0s, 0, step), (1ms, 1, step), (5ms, 1, step)]; }
curve bend { clock = normalized; points = [(0, 0ct, linear), (1, 200ct, step)]; }
curve swell { clock = normalized; points = [(0, 0, step), (1, 0, step)]; }
pattern notes { length = 1/100q;
  note a { at = 0q; dur = 1/100q; pitch = A4; velocity = 1/2;
    expression color_expr { kind = timbre; curve = &shade; }
    expression pitch_expr { kind = pitch; curve = &bend; }
    expression dynamics { kind = gain; curve = &swell; }
  }
}
place once { pattern = &notes; track = &t; at = 0q; }
"#;

fn invoke(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(directory)
        .args(args)
        .output()
        .expect("maac starts")
}

fn succeeds(directory: &Path, args: &[&str]) {
    let output = invoke(directory, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
}

fn failure_preserves(directory: &Path, output_name: &str, args: &[&str], code: &str, detail: &str) {
    let original = b"existing output must survive failure";
    fs::write(directory.join(output_name), original).unwrap();
    let output = invoke(directory, args);
    assert!(!output.status.success(), "{args:?}: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(code) && stderr.contains(detail), "{stderr}");
    assert_eq!(fs::read(directory.join(output_name)).unwrap(), original);
}

fn roundtrip(source: &str, channels: u16, frames: u32, notes: usize, timbre_nodes: usize) {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    fs::write(cwd.join("input.maac"), source).unwrap();
    succeeds(cwd, &["check", "input.maac"]);
    succeeds(cwd, &["compile", "input.maac", "-o", "plan.json"]);
    let plan = Plan::from_json(&fs::read(cwd.join("plan.json")).unwrap()).unwrap();
    plan.validate().unwrap();
    assert_eq!(plan.version, 2);
    let resources = plan.instruments.as_ref().unwrap();
    assert!(!resources.programs.is_empty());
    assert!(resources.dependencies.is_empty());
    assert_eq!(
        resources
            .programs
            .iter()
            .flat_map(|program| &program.voice.nodes)
            .filter(|node| matches!(node.processor, maac::graph::GraphProcessor::Timbre))
            .count(),
        timbre_nodes
    );
    let retained: Vec<_> = plan
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Note {
                timbre_expression,
                pitch_expression,
                gain_expression,
                ..
            } => Some((timbre_expression, pitch_expression, gain_expression)),
            _ => None,
        })
        .collect();
    assert_eq!(retained.len(), notes);
    assert_eq!(
        retained
            .iter()
            .filter(|(timbre, _, _)| timbre.is_some())
            .count(),
        notes
    );
    assert_eq!(
        retained
            .iter()
            .filter(|(_, pitch, _)| pitch.is_some())
            .count(),
        1
    );
    assert_eq!(
        retained
            .iter()
            .filter(|(_, _, gain)| gain.is_some())
            .count(),
        1
    );
    for format in ["float32", "pcm16"] {
        succeeds(
            cwd,
            &[
                "build",
                "input.maac",
                "-o",
                &format!("build-{format}.wav"),
                "--format",
                format,
            ],
        );
    }
    fs::remove_file(cwd.join("input.maac")).unwrap();
    for format in ["float32", "pcm16"] {
        let expected = fs::read(cwd.join(format!("build-{format}.wav"))).unwrap();
        for run in 0..2 {
            let rendered = format!("render-{format}-{run}.wav");
            succeeds(
                cwd,
                &["render", "plan.json", "-o", &rendered, "--format", format],
            );
            assert_eq!(fs::read(cwd.join(&rendered)).unwrap(), expected);
            let mut wav = WavReader::open(cwd.join(rendered)).unwrap();
            assert_eq!(wav.duration(), frames);
            assert_eq!(wav.spec().sample_rate, 48_000);
            assert_eq!(wav.spec().channels, channels);
            if format == "float32" {
                assert_eq!(wav.spec().sample_format, SampleFormat::Float);
                assert_eq!(wav.spec().bits_per_sample, 32);
                let samples = wav.samples::<f32>().collect::<Result<Vec<_>, _>>().unwrap();
                assert!(samples.iter().all(|sample| sample.is_finite()));
                assert!(samples.iter().any(|sample| *sample != 0.0));
            } else {
                assert_eq!(wav.spec().sample_format, SampleFormat::Int);
                assert_eq!(wav.spec().bits_per_sample, 16);
                let samples = wav.samples::<i16>().collect::<Result<Vec<_>, _>>().unwrap();
                assert!(samples.iter().any(|sample| *sample != 0));
            }
        }
    }
}

#[test]
fn embedded_stereo_timbre_example_roundtrips_without_source() {
    roundtrip(EXAMPLE, 2, 91_200, 4, 2);
}

#[test]
fn inline_custom_mono_timbre_pitch_and_gain_roundtrip_without_source() {
    roundtrip(CUSTOM, 1, 288, 1, 1);
}

#[test]
fn timbre_errors_preserve_output_even_with_force_and_without_source() {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    for (source, code) in [
        (CUSTOM.replace("(1, 1, step)", "(1, 2, step)"), "E_RANGE"),
        (
            CUSTOM.replace("(0, 0, linear)", "(0, -1, linear)"),
            "E_RANGE",
        ),
        (
            CUSTOM.replace("synth.timbre/1", "synth.sine/1"),
            "E_CAPABILITY",
        ),
        (
            CUSTOM.replace(
                "node lead { instrument = &local_sine; }",
                "node lead { type = \"core.sine/1\"; }",
            ),
            "E_CAPABILITY",
        ),
    ] {
        assert_ne!(source, CUSTOM);
        fs::write(cwd.join("invalid.maac"), source).unwrap();
        let output = invoke(cwd, &["check", "invalid.maac"]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(code), "{stderr}");
    }
    fs::write(cwd.join("late.maac"), LATE_RANGE).unwrap();
    succeeds(cwd, &["check", "late.maac"]);
    succeeds(cwd, &["compile", "late.maac", "-o", "late.json"]);
    let plan = Plan::from_json(&fs::read(cwd.join("late.json")).unwrap()).unwrap();
    plan.validate().unwrap();
    let EventKind::Note {
        timbre_expression: Some(timbre),
        gain_expression: Some(gain),
        ..
    } = &plan.events[0].kind
    else {
        panic!("late range fixture must retain timbre and zero gain");
    };
    let zero = maac::parse_rational("0").unwrap();
    assert!(gain.points.iter().all(|point| point.gain == zero));
    assert_eq!(timbre.points[0].value, zero);
    for format in ["float32", "pcm16"] {
        failure_preserves(
            cwd,
            "late-build.wav",
            &[
                "build",
                "late.maac",
                "-o",
                "late-build.wav",
                "--format",
                format,
                "--force",
            ],
            "E_NONFINITE",
            "cutoff",
        );
    }
    fs::remove_file(cwd.join("late.maac")).unwrap();
    for format in ["float32", "pcm16"] {
        failure_preserves(
            cwd,
            "late-render.wav",
            &[
                "render",
                "late.json",
                "-o",
                "late-render.wav",
                "--format",
                format,
                "--force",
            ],
            "E_NONFINITE",
            "cutoff",
        );
    }
}
