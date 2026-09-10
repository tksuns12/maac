use hound::{SampleFormat, WavReader};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

const EXAMPLE: &str = include_str!("../examples/core-delay.maac");

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

fn assert_audio(path: &Path, format: &str) {
    let mut reader = WavReader::open(path).unwrap();
    assert_eq!(reader.duration(), 38_400);
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.spec().channels, 1);
    let peak = match format {
        "float32" => {
            assert_eq!(reader.spec().sample_format, SampleFormat::Float);
            assert_eq!(reader.spec().bits_per_sample, 32);
            reader
                .samples::<f32>()
                .map(|sample| {
                    let sample = sample.unwrap();
                    assert!(sample.is_finite());
                    f64::from(sample.abs())
                })
                .fold(0.0, f64::max)
        }
        "pcm16" => {
            assert_eq!(reader.spec().sample_format, SampleFormat::Int);
            assert_eq!(reader.spec().bits_per_sample, 16);
            reader
                .samples::<i16>()
                .map(|sample| f64::from(i32::from(sample.unwrap()).abs()) / 32_768.0)
                .fold(0.0, f64::max)
        }
        _ => panic!("unknown format {format}"),
    };
    assert!(peak > 0.0, "{format} render is silent");
    assert!(peak < 1.0, "{format} peak is out of range: {peak}");
}

#[test]
fn delay_example_compiles_and_replays_without_source_in_both_wav_formats() {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    let source = cwd.join("core-delay.maac");
    let plan = cwd.join("core-delay.json");
    fs::write(&source, EXAMPLE).unwrap();

    succeeds(cwd, &["check", "core-delay.maac"]);
    succeeds(
        cwd,
        &["compile", "core-delay.maac", "-o", "core-delay.json"],
    );

    let encoded: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    assert_eq!(encoded["version"], 2);
    let delay = encoded["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "delay")
        .unwrap();
    assert_eq!(
        delay["processor"],
        serde_json::json!({"kind": "delay", "channels": 1, "frames": 2400})
    );
    assert_eq!(delay["params"], serde_json::json!({}));

    for format in ["float32", "pcm16"] {
        let output = format!("build-{format}.wav");
        succeeds(
            cwd,
            &[
                "build",
                "core-delay.maac",
                "-o",
                output.as_str(),
                "--format",
                format,
            ],
        );
        assert_audio(&cwd.join(&output), format);
    }

    fs::remove_file(&source).unwrap();
    assert!(!source.exists());
    let empty = tempdir().unwrap();
    let plan_arg = plan.to_str().unwrap();
    for format in ["float32", "pcm16"] {
        let expected = fs::read(cwd.join(format!("build-{format}.wav"))).unwrap();
        let rendered = cwd.join(format!("render-{format}.wav"));
        succeeds(
            empty.path(),
            &[
                "render",
                plan_arg,
                "-o",
                rendered.to_str().unwrap(),
                "--format",
                format,
            ],
        );
        assert_eq!(fs::read(&rendered).unwrap(), expected);
        assert_audio(&rendered, format);
    }
}

fn assert_delay_failure_preserves_output(
    cwd: &Path,
    args: &[&str],
    output_name: &str,
    exists: bool,
) {
    let output_path = cwd.join(output_name);
    let original = b"existing output must survive zero-delay failure";
    if exists {
        fs::write(&output_path, original).unwrap();
    }
    let output = invoke(cwd, args);
    assert!(!output.status.success(), "{args:?}: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("E_RANGE"), "{stderr}");
    if exists {
        assert_eq!(fs::read(&output_path).unwrap(), original);
    } else {
        assert!(!output_path.exists());
    }
}

#[test]
fn zero_frame_delay_keeps_build_and_render_outputs_atomic_with_force() {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    let invalid_source = EXAMPLE.replace("frames = 2400", "frames = 0");
    assert_ne!(invalid_source, EXAMPLE);
    fs::write(cwd.join("invalid.maac"), invalid_source).unwrap();

    for (output_name, exists) in [("build-existing.wav", true), ("build-absent.wav", false)] {
        assert_delay_failure_preserves_output(
            cwd,
            &["build", "invalid.maac", "-o", output_name, "--force"],
            output_name,
            exists,
        );
    }

    fs::write(cwd.join("valid.maac"), EXAMPLE).unwrap();
    succeeds(cwd, &["compile", "valid.maac", "-o", "valid.json"]);
    let mut invalid_plan: Value =
        serde_json::from_slice(&fs::read(cwd.join("valid.json")).unwrap()).unwrap();
    let delay = invalid_plan["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "delay")
        .unwrap();
    delay["processor"]["frames"] = 0.into();
    fs::write(
        cwd.join("invalid.json"),
        serde_json::to_vec(&invalid_plan).unwrap(),
    )
    .unwrap();

    for (output_name, exists) in [("render-existing.wav", true), ("render-absent.wav", false)] {
        assert_delay_failure_preserves_output(
            cwd,
            &["render", "invalid.json", "-o", output_name, "--force"],
            output_name,
            exists,
        );
    }
}
