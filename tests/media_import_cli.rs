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

fn retained_project(directory: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let input = directory.join("recording.wav");
    let project = directory.join("project");
    wav(&input);
    success(invoke(&[
        Path::new("--json"),
        Path::new("import-wav"),
        &input,
        Path::new("--start-frame"),
        Path::new("1"),
        Path::new("--end-frame"),
        Path::new("3"),
        Path::new("--retain-original"),
        Path::new("--output-dir"),
        &project,
    ]));
    (input, project)
}

fn verify(project: &Path) -> Output {
    invoke(&[Path::new("--json"), Path::new("verify-import"), project])
}

fn manifest(project: &Path) -> Value {
    serde_json::from_slice(&fs::read(project.join("import.json")).unwrap()).unwrap()
}

fn write_manifest(project: &Path, value: &Value) {
    fs::write(
        project.join("import.json"),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
}

#[test]
fn retained_import_verifies_after_relocation_and_independent_source_edit() {
    let directory = tempdir().unwrap();
    let (input, project) = retained_project(directory.path());
    let original = fs::read(&input).unwrap();
    assert_eq!(fs::read(project.join("original.wav")).unwrap(), original);
    let record = manifest(&project);
    assert_eq!(record["version"], 2);
    assert_eq!(record["original"]["path"], "original.wav");
    assert_eq!(record["original"]["sha256"], record["source"]["sha256"]);
    fs::remove_file(input).unwrap();
    let relocated = directory.path().join("moved project");
    fs::rename(&project, &relocated).unwrap();
    let result = success(verify(&relocated));
    assert_eq!(result["command"], "verify-import");
    assert_eq!(result["frames"], 2);
    let mut source = fs::read_to_string(relocated.join("main.maac")).unwrap();
    source.push_str("\n  \n");
    fs::write(relocated.join("main.maac"), source).unwrap();
    success(verify(&relocated));
    success(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &relocated,
    ]));
}

#[test]
fn retained_verifier_rejects_corrupt_missing_and_inconsistent_members() {
    let directory = tempdir().unwrap();
    let (_, project) = retained_project(directory.path());
    let original = fs::read(project.join("original.wav")).unwrap();
    let pcm = fs::read(project.join("media.pcm")).unwrap();
    let record = manifest(&project);

    fs::remove_file(project.join("original.wav")).unwrap();
    assert_eq!(failed(verify(&project))["code"], "E_REFERENCE");
    fs::write(project.join("original.wav"), &original).unwrap();

    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(project.join("original.wav"), corrupt).unwrap();
    assert_eq!(failed(verify(&project))["code"], "E_IMPORT_MISMATCH");
    fs::write(project.join("original.wav"), &original).unwrap();

    fs::remove_file(project.join("media.pcm")).unwrap();
    assert_eq!(failed(verify(&project))["code"], "E_REFERENCE");
    fs::write(project.join("media.pcm"), &pcm).unwrap();
    let mut corrupt = pcm.clone();
    corrupt[0] ^= 1;
    fs::write(project.join("media.pcm"), corrupt).unwrap();
    assert_eq!(failed(verify(&project))["code"], "E_IMPORT_MISMATCH");
    fs::write(project.join("media.pcm"), &pcm).unwrap();

    fs::write(project.join("import.json"), b"{").unwrap();
    assert_eq!(failed(verify(&project))["code"], "E_IMPORT_MANIFEST");
    write_manifest(&project, &record);
    let mut wrong_decoder = record.clone();
    wrong_decoder["decoder"] = "other-decoder".into();
    write_manifest(&project, &wrong_decoder);
    assert_eq!(failed(verify(&project))["code"], "E_IMPORT_MANIFEST");
    let mut wrong_version = record.clone();
    wrong_version["version"] = 1.into();
    write_manifest(&project, &wrong_version);
    assert_eq!(failed(verify(&project))["code"], "E_IMPORT_MANIFEST");
    let mut unknown_field = record.clone();
    unknown_field["unrecognized"] = true.into();
    write_manifest(&project, &unknown_field);
    assert_eq!(failed(verify(&project))["code"], "E_IMPORT_MANIFEST");
    let mut wrong_crop = record.clone();
    wrong_crop["selection"]["start_frame"] = 0.into();
    write_manifest(&project, &wrong_crop);
    assert_eq!(failed(verify(&project))["code"], "E_IMPORT_MISMATCH");
    write_manifest(&project, &record);

    fs::write(project.join("main.maac"), b"not maac").unwrap();
    assert!(!verify(&project).status.success());
}

#[test]
fn retained_verifier_requires_imported_asset_in_current_dependency_closure() {
    let directory = tempdir().unwrap();
    let (_, project) = retained_project(directory.path());
    fs::copy(project.join("media.pcm"), project.join("other.pcm")).unwrap();
    let source = fs::read_to_string(project.join("main.maac")).unwrap();
    assert!(source.contains("path = \"media.pcm\""));
    fs::write(
        project.join("main.maac"),
        source.replace("path = \"media.pcm\"", "path = \"other.pcm\""),
    )
    .unwrap();
    success(invoke(&[Path::new("--json"), Path::new("check"), &project]));
    assert_eq!(failed(verify(&project))["code"], "E_IMPORT_MISMATCH");
}

#[cfg(unix)]
#[test]
fn retained_verifier_rejects_original_symlink_escaping_project() {
    use std::os::unix::fs::symlink;
    let directory = tempdir().unwrap();
    let (input, project) = retained_project(directory.path());
    fs::remove_file(project.join("original.wav")).unwrap();
    symlink(input, project.join("original.wav")).unwrap();
    assert_eq!(failed(verify(&project))["code"], "E_REFERENCE");
}

#[test]
fn retained_import_crops_source_larger_than_native_asset_limit() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("large.wav");
    let project = directory.path().join("project");
    let data_bytes = 4 * 1024 * 1024 + 2usize;
    let mut wav_bytes = Vec::with_capacity(44 + data_bytes);
    wav_bytes.extend_from_slice(b"RIFF");
    wav_bytes.extend_from_slice(&(36u32 + data_bytes as u32).to_le_bytes());
    wav_bytes.extend_from_slice(b"WAVEfmt ");
    wav_bytes.extend_from_slice(&16u32.to_le_bytes());
    wav_bytes.extend_from_slice(&1u16.to_le_bytes());
    wav_bytes.extend_from_slice(&1u16.to_le_bytes());
    wav_bytes.extend_from_slice(&48_000u32.to_le_bytes());
    wav_bytes.extend_from_slice(&96_000u32.to_le_bytes());
    wav_bytes.extend_from_slice(&2u16.to_le_bytes());
    wav_bytes.extend_from_slice(&16u16.to_le_bytes());
    wav_bytes.extend_from_slice(b"data");
    wav_bytes.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    wav_bytes.resize(44 + data_bytes, 0);
    fs::write(&input, &wav_bytes).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("import-wav"),
        &input,
        Path::new("--start-frame"),
        Path::new("100"),
        Path::new("--end-frame"),
        Path::new("200"),
        Path::new("--retain-original"),
        Path::new("--output-dir"),
        &project,
    ]));
    assert_eq!(
        fs::metadata(project.join("original.wav")).unwrap().len(),
        wav_bytes.len() as u64
    );
    assert_eq!(fs::metadata(project.join("media.pcm")).unwrap().len(), 400);
    success(verify(&project));
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
