use hound::{SampleFormat, WavReader};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

const EXAMPLE: &str = include_str!("../examples/internal-phase-modulation.maac");

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
    assert_eq!(reader.duration(), 49_920);
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
fn internal_phase_example_roundtrips_v2_plan_without_source_in_both_wav_formats() {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    let source = cwd.join("internal-phase.maac");
    let plan = cwd.join("internal-phase.json");
    fs::write(&source, EXAMPLE).unwrap();

    succeeds(cwd, &["check", "internal-phase.maac"]);
    succeeds(
        cwd,
        &[
            "compile",
            "internal-phase.maac",
            "-o",
            "internal-phase.json",
        ],
    );
    let encoded: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    assert_eq!(encoded["version"], 2);
    assert!(encoded.get("modulations").is_none());
    let modulations = &encoded["instruments"]["programs"][0]["voice"]["modulations"];
    assert_eq!(modulations.as_array().unwrap().len(), 2);
    assert!(modulations.as_array().unwrap().iter().any(|edge| {
        edge["from"]["node"] == "pressure"
            && edge["to"] == serde_json::json!({"node": "osc", "parameter": "phase"})
    }));
    assert!(modulations.as_array().unwrap().iter().any(|edge| {
        edge["from"]["node"] == "timbre"
            && edge["to"] == serde_json::json!({"node": "lfo", "parameter": "phase"})
    }));

    for format in ["float32", "pcm16"] {
        let output = format!("build-{format}.wav");
        succeeds(
            cwd,
            &[
                "build",
                "internal-phase.maac",
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

fn assert_phase_failure_preserves_output(
    cwd: &Path,
    args: &[&str],
    output_name: &str,
    exists: bool,
) {
    let output_path = cwd.join(output_name);
    let original = b"existing output must survive internal phase failure";
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
fn invalid_captured_phase_keeps_build_and_render_outputs_atomic_with_force() {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    let invalid_source = EXAMPLE.replace(
        "to = &osc.params.phase;\n      depth = 1/4;",
        "to = &osc.params.phase;\n      depth = -1;",
    );
    assert_ne!(invalid_source, EXAMPLE);
    fs::write(cwd.join("invalid.maac"), invalid_source).unwrap();
    succeeds(cwd, &["compile", "invalid.maac", "-o", "invalid.json"]);
    let invalid: Value =
        serde_json::from_slice(&fs::read(cwd.join("invalid.json")).unwrap()).unwrap();
    assert_eq!(invalid["version"], 2);

    for (output_name, exists) in [("build-existing.wav", true), ("build-absent.wav", false)] {
        assert_phase_failure_preserves_output(
            cwd,
            &["build", "invalid.maac", "-o", output_name, "--force"],
            output_name,
            exists,
        );
    }
    for (output_name, exists) in [("render-existing.wav", true), ("render-absent.wav", false)] {
        assert_phase_failure_preserves_output(
            cwd,
            &["render", "invalid.json", "-o", output_name, "--force"],
            output_name,
            exists,
        );
    }
}
