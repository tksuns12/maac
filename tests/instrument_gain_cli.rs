use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use hound::{SampleFormat, WavReader};
use maac::plan::{EventKind, Plan};
use tempfile::tempdir;

const EXAMPLE: &str = include_str!("../examples/instrument-gain.maac");
const CUSTOM: &str = r#"maac 1;
project p { score = [0q, 1/10q]; tail = 10ms; rate = 48000Hz; tempo = &clock; meter = &metre; output = &lead:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument local_sine { channels = 1;
  voice v { channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; params = { release = 10ms; }; }
    node osc { type = "synth.sine/1"; }
  }
}
node lead { instrument = &local_sine; }
track t { target = &lead:events; }
curve swell { clock = normalized; points = [(0, 0, linear), (1, 1/2, step)]; }
pattern notes { length = 1/10q;
  note a { at = 0q; dur = 1/10q; pitch = A4; velocity = 1/4;
    expression gain { kind = gain; curve = &swell; }
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

fn roundtrip(source: &str, channels: u16, frames: u32, notes: usize) {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    fs::write(cwd.join("input.maac"), source).unwrap();
    succeeds(cwd, &["check", "input.maac"]);
    succeeds(cwd, &["compile", "input.maac", "-o", "plan.json"]);
    let plan = Plan::from_json(&fs::read(cwd.join("plan.json")).unwrap()).unwrap();
    plan.validate().unwrap();
    assert_eq!(plan.version, 2);
    assert!(!plan.instruments.as_ref().unwrap().programs.is_empty());
    let retained: Vec<_> = plan
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Note {
                gain_expression, ..
            } => Some(gain_expression),
            _ => None,
        })
        .collect();
    assert_eq!(retained.len(), notes);
    assert!(retained.iter().all(|gain| gain.is_some()));
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
fn embedded_stereo_instrument_example_roundtrips_without_source() {
    roundtrip(EXAMPLE, 2, 91_200, 4);
}

#[test]
fn inline_custom_mono_instrument_roundtrips_without_source() {
    roundtrip(CUSTOM, 1, 2_880, 1);
}

#[test]
fn invalid_gain_and_pcm16_overload_preserve_output_even_with_force() {
    for (source, code, detail) in [
        (
            EXAMPLE.replace("(0, 0, linear)", "(0, -1, linear)"),
            "E_RANGE",
            "nonnegative",
        ),
        (
            EXAMPLE.replace("(0s, 1/8, exponential)", "(0s, 0, exponential)"),
            "E_RANGE",
            "positive",
        ),
        (
            CUSTOM.replace("(1, 1/2, step)", "(1, 1000, step)"),
            "E_PCM16_RANGE",
            "outside [-1, 1]",
        ),
    ] {
        let directory = tempdir().unwrap();
        let cwd = directory.path();
        fs::write(cwd.join("input.maac"), source).unwrap();
        if code == "E_PCM16_RANGE" {
            succeeds(cwd, &["check", "input.maac"]);
        }
        let original = b"existing output must survive failure";
        fs::write(cwd.join("existing.wav"), original).unwrap();
        let output = invoke(
            cwd,
            &[
                "build",
                "input.maac",
                "-o",
                "existing.wav",
                "--format",
                "pcm16",
                "--force",
            ],
        );
        assert!(!output.status.success(), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(code) && stderr.contains(detail), "{stderr}");
        assert_eq!(fs::read(cwd.join("existing.wav")).unwrap(), original);
    }
}
