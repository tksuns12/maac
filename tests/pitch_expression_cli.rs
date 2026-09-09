use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use hound::{SampleFormat, WavReader};
use tempfile::tempdir;

const EXAMPLE: &str = include_str!("../examples/pitch-expression.maac");

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
fn pitch_expression_source_and_source_free_retained_plan_produce_identical_audio() {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    fs::write(cwd.join("example.maac"), EXAMPLE).unwrap();
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
        assert_eq!(wav.duration(), 110_400);
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

#[test]
fn invalid_pitch_curve_does_not_replace_existing_output_even_with_force() {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    let invalid = EXAMPLE.replace("(1, 1200ct, step)", "(0, 1200ct, step)");
    assert_ne!(invalid, EXAMPLE);
    fs::write(cwd.join("invalid.maac"), invalid).unwrap();
    let original = b"existing output must survive failed validation";
    fs::write(cwd.join("existing.wav"), original).unwrap();
    let output = invoke(
        cwd,
        &["build", "invalid.maac", "-o", "existing.wav", "--force"],
    );
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fs::read(cwd.join("existing.wav")).unwrap(), original);
}
