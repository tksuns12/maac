use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use hound::{SampleFormat, WavReader};
use tempfile::tempdir;

const EXAMPLE: &str = include_str!("../examples/gain-expression.maac");

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

#[test]
fn gain_expression_source_and_source_free_retained_plan_produce_identical_audio() {
    for source in [
        EXAMPLE.to_owned(),
        EXAMPLE
            .replace("clock = seconds;", "clock = score;")
            .replace(
                "(0s, 1/8, exponential), (1s, 1/2, step)",
                "(0q, 1/8, exponential), (2q, 1/2, step)",
            ),
    ] {
        let directory = tempdir().unwrap();
        let cwd = directory.path();
        fs::write(cwd.join("example.maac"), source).unwrap();
        succeeds(cwd, &["check", "example.maac"]);
        succeeds(cwd, &["compile", "example.maac", "-o", "plan.json"]);
        for format in ["float32", "pcm16"] {
            succeeds(
                cwd,
                &[
                    "build",
                    "example.maac",
                    "-o",
                    &format!("build-{format}.wav"),
                    "--format",
                    format,
                ],
            );
        }
        fs::remove_file(cwd.join("example.maac")).unwrap();
        for format in ["float32", "pcm16"] {
            let rendered = format!("render-{format}.wav");
            succeeds(
                cwd,
                &["render", "plan.json", "-o", &rendered, "--format", format],
            );
            assert_eq!(
                fs::read(cwd.join(format!("build-{format}.wav"))).unwrap(),
                fs::read(cwd.join(&rendered)).unwrap()
            );
            let mut wav = WavReader::open(cwd.join(rendered)).unwrap();
            assert_eq!(wav.duration(), 81_600);
            assert_eq!(wav.spec().sample_rate, 48_000);
            assert_eq!(wav.spec().channels, 1);
            if format == "float32" {
                assert_eq!(wav.spec().sample_format, SampleFormat::Float);
                let samples = wav.samples::<f32>().collect::<Result<Vec<_>, _>>().unwrap();
                assert!(samples.iter().all(|sample| sample.is_finite()));
                assert!(samples.iter().any(|sample| *sample != 0.0));
            } else {
                assert_eq!(wav.spec().sample_format, SampleFormat::Int);
                assert_eq!(wav.spec().bits_per_sample, 16);
                assert!(wav.samples::<i16>().any(|sample| sample.unwrap() != 0));
            }
        }
    }
}

#[test]
fn invalid_gain_and_pcm16_overload_preserve_existing_output_even_with_force() {
    for source in [
        EXAMPLE.replace("(0, 0, linear)", "(0, -1, linear)"),
        EXAMPLE.replace("(0s, 1/8, exponential)", "(0s, 0, exponential)"),
        EXAMPLE.replace("(1/2, 1, linear)", "(1/2, 100, linear)"),
    ] {
        assert_ne!(source, EXAMPLE);
        let directory = tempdir().unwrap();
        let cwd = directory.path();
        fs::write(cwd.join("input.maac"), source).unwrap();
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
        assert_eq!(fs::read(cwd.join("existing.wav")).unwrap(), original);
    }
}
