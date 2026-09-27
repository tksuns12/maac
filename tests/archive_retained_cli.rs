use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::Value;
use tempfile::tempdir;

fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failure(output: Output) -> Value {
    assert!(!output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn small_wav(path: &Path, first: i16) {
    let mut writer = WavWriter::create(
        path,
        WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )
    .unwrap();
    for sample in [first, 1000, -2000, 3000] {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn large_wav(path: &Path) {
    let data_bytes = 4 * 1024 * 1024 + 2usize;
    let mut bytes = Vec::with_capacity(44 + data_bytes);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36u32 + data_bytes as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    bytes.resize(44 + data_bytes, 0);
    fs::write(path, bytes).unwrap();
}

fn long_stereo_wav(path: &Path, frames: u32) {
    let data_bytes = frames * 4;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&48_000u32.to_le_bytes());
    header.extend_from_slice(&192_000u32.to_le_bytes());
    header.extend_from_slice(&4u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_bytes.to_le_bytes());
    assert_eq!(header.len(), 44);
    fs::write(path, header).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(44 + u64::from(data_bytes))
        .unwrap();
}

fn import(input: &Path, project: &Path, retain: bool) {
    let mut args = vec![
        Path::new("--json"),
        Path::new("import-wav"),
        input,
        Path::new("--start-frame"),
        Path::new("1"),
        Path::new("--end-frame"),
        Path::new("3"),
    ];
    if retain {
        args.push(Path::new("--retain-original"));
    }
    args.extend([Path::new("--output-dir"), project]);
    success(invoke(&args));
}

fn create(project: &Path, archive: &Path, previous: Option<&Path>) -> Value {
    let mut args = vec![
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        project,
        Path::new("--output-dir"),
        archive,
    ];
    if let Some(previous) = previous {
        args.extend([Path::new("--previous"), previous]);
    }
    success(invoke(&args))
}

fn checkpoint_ids(archive: &Path) -> Vec<String> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(archive.join("maac-archive.json")).unwrap()).unwrap();
    manifest["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn retained_large_wav_relocates_and_restores_exact_four_files() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("large.wav");
    let project = temp.path().join("project");
    let archive = temp.path().join("archive");
    let relocated = temp.path().join("relocated");
    let unpacked = temp.path().join("unpacked");
    let before_render = temp.path().join("before.wav");
    let after_render = temp.path().join("after.wav");
    large_wav(&input);
    import(&input, &project, true);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &project,
        Path::new("-o"),
        &before_render,
    ]));
    let expected: Vec<_> = ["main.maac", "media.pcm", "import.json", "original.wav"]
        .iter()
        .map(|name| fs::read(project.join(name)).unwrap())
        .collect();
    let result = create(&project, &archive, None);
    assert_eq!(result["format"], "maac.editable-archive/3");
    fs::rename(&archive, &relocated).unwrap();
    fs::remove_dir_all(&project).unwrap();
    fs::remove_file(&input).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &relocated,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &relocated,
        Path::new("--output-dir"),
        &unpacked,
    ]));
    for (name, bytes) in ["main.maac", "media.pcm", "import.json", "original.wav"]
        .iter()
        .zip(expected)
    {
        assert_eq!(fs::read(unpacked.join(name)).unwrap(), bytes, "{name}");
    }
    success(invoke(&[
        Path::new("--json"),
        Path::new("verify-import"),
        &unpacked,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &unpacked,
        Path::new("-o"),
        &after_render,
    ]));
    assert_eq!(
        fs::read(before_render).unwrap(),
        fs::read(after_render).unwrap()
    );
}

#[test]
fn retained_disk_media_crop_over_four_mib_reopens_from_moved_archive() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("long.wav");
    let project = temp.path().join("project");
    let archive = temp.path().join("archive");
    let moved = temp.path().join("moved");
    let unpacked = temp.path().join("unpacked");
    let frames = 530_000u32;
    long_stereo_wav(&input, frames);
    success(invoke(&[
        Path::new("--json"),
        Path::new("import-wav"),
        &input,
        Path::new("--disk-media"),
        Path::new("--retain-original"),
        Path::new("--output-dir"),
        &project,
    ]));
    assert!(fs::metadata(project.join("media.pcm")).unwrap().len() > 4 * 1024 * 1024);
    assert_eq!(
        create(&project, &archive, None)["format"],
        "maac.editable-archive/3"
    );
    fs::rename(&archive, &moved).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &moved,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--output-dir"),
        &unpacked,
    ]));
    for name in ["main.maac", "media.pcm", "import.json", "original.wav"] {
        assert_eq!(
            fs::read(project.join(name)).unwrap(),
            fs::read(unpacked.join(name)).unwrap(),
            "{name}"
        );
    }
    fs::remove_dir_all(&project).unwrap();
    fs::remove_file(&input).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("verify-import"),
        &unpacked,
        Path::new("--disk-media"),
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &unpacked,
        Path::new("--disk-media"),
    ]));
}

#[test]
fn import_record_bytes_are_checkpointed_and_old_originals_are_verified() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("input.wav");
    let project = temp.path().join("project");
    let first = temp.path().join("first");
    let noop = temp.path().join("noop");
    let second = temp.path().join("second");
    let restored = temp.path().join("restored");
    small_wav(&input, 500);
    import(&input, &project, true);
    let first_result = create(&project, &first, None);
    let first_id = checkpoint_ids(&first).remove(0);
    assert_eq!(
        create(&project, &noop, Some(&first))["digest"],
        first_result["digest"]
    );

    let record: Value =
        serde_json::from_slice(&fs::read(project.join("import.json")).unwrap()).unwrap();
    let rewritten = serde_json::to_vec(&record).unwrap();
    fs::write(project.join("import.json"), &rewritten).unwrap();
    assert_ne!(
        fs::read(project.join("import.json")).unwrap(),
        fs::read(
            first
                .join("checkpoints")
                .join(&first_id[7..])
                .join("import.json")
        )
        .unwrap()
    );
    let result = create(&project, &second, Some(&first));
    assert_eq!(result["format"], "maac.editable-archive/3");
    assert_eq!(checkpoint_ids(&second).len(), 2);
    fs::remove_dir_all(&first).unwrap();
    fs::remove_dir_all(&noop).unwrap();
    fs::remove_dir_all(&project).unwrap();
    fs::remove_file(&input).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &second,
        Path::new("--output-dir"),
        &restored,
        Path::new("--revision"),
        Path::new(&first_id),
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("verify-import"),
        &restored,
    ]));

    let old_original = second
        .join("checkpoints")
        .join(&first_id[7..])
        .join("original.wav");
    let mut bytes = fs::read(&old_original).unwrap();
    *bytes.last_mut().unwrap() ^= 1; // outside the imported crop
    fs::write(&old_original, bytes).unwrap();
    let rejected = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &second,
    ]));
    assert_eq!(rejected["ok"], false);
}

#[test]
fn legacy_import_is_closure_only_and_orphan_original_is_rejected() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("input.wav");
    let project = temp.path().join("project");
    let archive = temp.path().join("archive");
    small_wav(&input, 500);
    import(&input, &project, false);
    assert_eq!(
        create(&project, &archive, None)["format"],
        "maac.editable-archive/2"
    );
    fs::write(project.join("original.wav"), fs::read(&input).unwrap()).unwrap();
    let rejected = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--output-dir"),
        &temp.path().join("bad"),
    ]));
    assert_eq!(rejected["ok"], false);
}

#[test]
fn retained_record_cannot_attach_to_another_entry_or_publish_when_incomplete() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("input.wav");
    let project = temp.path().join("project");
    small_wav(&input, 500);
    import(&input, &project, true);
    fs::copy(project.join("main.maac"), project.join("other.maac")).unwrap();

    let wrong_entry = temp.path().join("wrong-entry");
    let rejected = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project.join("other.maac"),
        Path::new("--project-root"),
        &project,
        Path::new("--output-dir"),
        &wrong_entry,
    ]));
    assert_eq!(rejected["ok"], false);
    assert!(!wrong_entry.exists());

    fs::remove_file(project.join("original.wav")).unwrap();
    let incomplete = temp.path().join("incomplete");
    let rejected = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--output-dir"),
        &incomplete,
    ]));
    assert_eq!(rejected["ok"], false);
    assert!(!incomplete.exists());
}

#[test]
fn two_imported_checkpoints_restore_their_distinct_originals() {
    let temp = tempdir().unwrap();
    let first_input = temp.path().join("first.wav");
    let second_input = temp.path().join("second.wav");
    let first_project = temp.path().join("first-project");
    let second_project = temp.path().join("second-project");
    let first_archive = temp.path().join("first-archive");
    let history = temp.path().join("history");
    small_wav(&first_input, 500);
    small_wav(&second_input, -500);
    import(&first_input, &first_project, true);
    import(&second_input, &second_project, true);
    let first_original = fs::read(first_project.join("original.wav")).unwrap();
    let second_original = fs::read(second_project.join("original.wav")).unwrap();
    create(&first_project, &first_archive, None);
    create(&second_project, &history, Some(&first_archive));
    let ids = checkpoint_ids(&history);
    assert_eq!(ids.len(), 2);
    fs::remove_dir_all(&first_archive).unwrap();
    fs::remove_dir_all(&first_project).unwrap();
    fs::remove_dir_all(&second_project).unwrap();
    fs::remove_file(&first_input).unwrap();
    fs::remove_file(&second_input).unwrap();

    for (index, expected) in [first_original, second_original].iter().enumerate() {
        let restored = temp.path().join(format!("restored-{index}"));
        success(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("unpack"),
            &history,
            Path::new("--revision"),
            Path::new(&ids[index]),
            Path::new("--output-dir"),
            &restored,
        ]));
        assert_eq!(&fs::read(restored.join("original.wav")).unwrap(), expected);
        success(invoke(&[
            Path::new("--json"),
            Path::new("verify-import"),
            &restored,
        ]));
    }
}

#[test]
fn v2_checkpoint_identity_survives_promotion_to_retained_history() {
    let fixture = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/archive_v2"
    ));
    let old_id = checkpoint_ids(fixture).remove(0);
    let temp = tempdir().unwrap();
    let input = temp.path().join("input.wav");
    let project = temp.path().join("project");
    let history = temp.path().join("history");
    small_wav(&input, 500);
    import(&input, &project, true);
    let result = create(&project, &history, Some(fixture));
    assert_eq!(result["format"], "maac.editable-archive/3");
    let ids = checkpoint_ids(&history);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], old_id);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &history,
    ]));
}
