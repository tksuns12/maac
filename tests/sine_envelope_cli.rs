use hound::{SampleFormat, WavReader};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

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
fn huge_sine_attack_and_level_preserve_linear_finite_samples_and_retained_bytes() {
    let directory = tempdir().unwrap();
    let cwd = directory.path();
    let huge = format!("1{}", "0".repeat(308));
    let source = format!(
        r#"maac 1;
project huge_envelope {{ score = [0q, 1/12000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sine:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sine {{ type = "core.sine/1"; params = {{ attack = {huge}s; release = 0s; level = {huge}; }}; }}
pattern phrase {{ length = 1/12000q; note n {{ at = 0q; dur = 1/12000q; pitch = 12000Hz; velocity = 1; }} }}
track notes {{ target = &sine:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
"#
    );
    let source_path = cwd.join("huge-envelope.maac");
    let plan_path = cwd.join("huge-envelope.json");
    let build_path = cwd.join("build.wav");
    fs::write(&source_path, source).unwrap();

    succeeds(cwd, &["check", "huge-envelope.maac"]);
    succeeds(
        cwd,
        &["compile", "huge-envelope.maac", "-o", "huge-envelope.json"],
    );
    succeeds(
        cwd,
        &[
            "build",
            "huge-envelope.maac",
            "-o",
            "build.wav",
            "--format",
            "float32",
        ],
    );

    let mut reader = WavReader::open(&build_path).unwrap();
    assert_eq!(reader.duration(), 4);
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.spec().channels, 1);
    assert_eq!(reader.spec().sample_format, SampleFormat::Float);
    assert_eq!(reader.spec().bits_per_sample, 32);
    let samples = reader
        .samples::<f32>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(samples.iter().all(|sample| sample.is_finite()));
    assert!(samples[0].abs() < 1.0e-10);
    assert!((f64::from(samples[1]) - 1.0 / 48_000.0).abs() < 1.0e-10);
    assert!(samples[2].abs() < 1.0e-10);
    assert!((f64::from(samples[3]) + 3.0 / 48_000.0).abs() < 1.0e-10);

    fs::remove_file(&source_path).unwrap();
    let empty = tempdir().unwrap();
    let rendered_path = empty.path().join("retained.wav");
    succeeds(
        empty.path(),
        &[
            "render",
            plan_path.to_str().unwrap(),
            "-o",
            rendered_path.to_str().unwrap(),
            "--format",
            "float32",
        ],
    );
    assert_eq!(
        fs::read(&rendered_path).unwrap(),
        fs::read(&build_path).unwrap()
    );
}
