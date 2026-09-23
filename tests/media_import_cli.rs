use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::Value;
use tempfile::tempdir;

fn wav(path: &Path) {
    let spec = WavSpec {
        channels: 2,
        sample_rate: 24_000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for sample in [0i16, 0, 16_384, -16_384, 8_192, -8_192, i16::MAX, i16::MIN] {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failed(output: Output) -> Value {
    assert!(!output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn import_publishes_a_reopenable_project_and_never_replaces_a_destination() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("recording.wav");
    let project = directory.path().join("project");
    let relocated = directory.path().join("relocated project");
    let source_free_plan = directory.path().join("retained.json");
    let first_render = directory.path().join("first.wav");
    let second_render = directory.path().join("second.wav");
    wav(&input);
    let source_hash = maac::bundle::sha256_digest(&fs::read(&input).unwrap());

    let imported = success(invoke(&[
        Path::new("--json"),
        Path::new("import-wav"),
        &input,
        Path::new("--start-frame"),
        Path::new("1"),
        Path::new("--end-frame"),
        Path::new("3"),
        Path::new("--output-dir"),
        &project,
    ]));
    assert_eq!(imported["command"], "import-wav");
    assert_eq!(imported["frames"], 2);
    assert_eq!(imported["digest"], source_hash);
    assert_eq!(
        fs::read_dir(&project)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>()
            .len(),
        3
    );

    let manifest: Value =
        serde_json::from_slice(&fs::read(project.join("import.json")).unwrap()).unwrap();
    assert_eq!(manifest["source"]["sha256"], source_hash);
    assert_eq!(manifest["selection"]["start_frame"], 1);
    assert_eq!(manifest["selection"]["end_frame"], 3);
    assert_eq!(manifest["output"]["frames"], 2);

    let occupied = directory.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("sentinel"), b"keep").unwrap();
    let refusal = failed(invoke(&[
        Path::new("--json"),
        Path::new("import-wav"),
        &relocated.join("media.pcm"),
        Path::new("--output-dir"),
        &occupied,
    ]));
    assert_eq!(refusal["code"], "E_OUTPUT_EXISTS");
    assert_eq!(fs::read(occupied.join("sentinel")).unwrap(), b"keep");

    fs::remove_file(&input).unwrap();
    let checked = success(invoke(&[Path::new("--json"), Path::new("check"), &project]));
    assert_eq!(checked["audio_clips"], 1);
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &project,
        Path::new("-o"),
        &source_free_plan,
    ]));
    let built = success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &project,
        Path::new("-o"),
        &first_render,
    ]));
    assert_eq!(built["frames"], 4);
    fs::rename(&project, &relocated).unwrap();
    let relocated_check = success(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &relocated,
    ]));
    assert_eq!(relocated_check["audio_clips"], 1);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &relocated,
        Path::new("-o"),
        &second_render,
    ]));
    assert_eq!(
        fs::read(&first_render).unwrap(),
        fs::read(&second_render).unwrap()
    );

    // The retained plan remains a separate, source-free replay boundary.
    fs::remove_dir_all(&relocated).unwrap();
    let replayed = success(invoke(&[
        Path::new("--json"),
        Path::new("render"),
        &source_free_plan,
        Path::new("-o"),
        &directory.path().join("replayed.wav"),
    ]));
    assert_eq!(replayed["frames"], 4);
    assert_eq!(
        fs::read(&first_render).unwrap(),
        fs::read(directory.path().join("replayed.wav")).unwrap()
    );
}
