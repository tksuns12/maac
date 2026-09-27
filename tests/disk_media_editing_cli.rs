use std::{
    fs::{self, File},
    io::{Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Output},
};

use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};

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

fn failure(output: Output) -> Value {
    assert!(!output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn imported_project(root: &Path) -> std::path::PathBuf {
    let input = root.join("recording.wav");
    let project = root.join("project");
    let data_bytes = 530_000u32 * 4;
    let mut file = File::create(&input).unwrap();
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + data_bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&48_000u32.to_le_bytes()).unwrap();
    file.write_all(&192_000u32.to_le_bytes()).unwrap();
    file.write_all(&4u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_bytes.to_le_bytes()).unwrap();
    file.set_len(44 + u64::from(data_bytes)).unwrap();
    file.seek(SeekFrom::End(-16)).unwrap();
    for _ in 0..4 {
        file.write_all(&16_384i16.to_le_bytes()).unwrap();
        file.write_all(&(-16_384i16).to_le_bytes()).unwrap();
    }
    drop(file);
    success(invoke(&[
        Path::new("--json"),
        Path::new("import-wav"),
        &input,
        Path::new("--disk-media"),
        Path::new("--retain-original"),
        Path::new("--output-dir"),
        &project,
    ]));
    fs::remove_file(input).unwrap();
    let moved = root.join("relocated project");
    fs::rename(project, &moved).unwrap();
    moved
}

fn quantity(n: u64, d: u64, unit: &str) -> Value {
    json!({"t":"quantity","n":n.to_string(),"d":d.to_string(),"u":unit})
}

fn set(object: &str, field: &str, value: Value) -> Operation {
    Operation::Set {
        object: vec![object.into()],
        field: vec![field.into()],
        value,
        expect: None,
        expect_absent: false,
    }
}

fn write_patch(path: &Path, source: &str, operations: Vec<Operation>) {
    let document = SourceDocument::parse(source).unwrap();
    fs::write(
        path,
        Transaction::new(document.revision().into(), operations)
            .unwrap()
            .to_json()
            .unwrap(),
    )
    .unwrap();
}

fn patch(project: &Path, transaction: &Path) -> Output {
    invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        project,
        transaction,
        Path::new("--disk-media"),
        Path::new("-o"),
        &project.join("main.maac"),
        Path::new("--force"),
    ])
}

#[test]
fn long_import_clip_edits_render_archive_reopen_and_inverse() {
    let root = tempfile::tempdir().unwrap();
    let project = imported_project(root.path());
    let main = project.join("main.maac");
    let original = fs::read_to_string(&main).unwrap();
    fs::write(&main, format!("// keep arrangement comment\n{original}")).unwrap();
    let original = fs::read_to_string(&main).unwrap();
    let original_revision = SourceDocument::parse(original.clone())
        .unwrap()
        .revision()
        .to_owned();
    let base_archive = root.path().join("base archive");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--output-dir"),
        &base_archive,
    ]));
    let transaction = root.path().join("crop.json");
    write_patch(
        &transaction,
        &original,
        vec![
            set(
                "clip",
                "source",
                json!({"t":"list","items":[quantity(529_996,1,"frame"),quantity(530_000,1,"frame")]}),
            ),
            set("clip", "at", quantity(2, 48_000, "s")),
            set("clip", "fade_in", quantity(1, 48_000, "s")),
            set("clip", "fade_out", quantity(2, 48_000, "s")),
            set(
                "imported_media",
                "score",
                json!({"t":"list","items":[quantity(0,1,"q"),quantity(6,48_000,"q")]}),
            ),
        ],
    );
    let result = success(patch(&project, &transaction));
    assert!(fs::read_to_string(&main)
        .unwrap()
        .contains("// keep arrangement comment"));
    assert_eq!(result["edit"]["new_revision"], result["digest"]);
    let edited = fs::read(&main).unwrap();
    assert_eq!(failure(patch(&project, &transaction))["code"], "E_CONFLICT");
    assert_eq!(fs::read(&main).unwrap(), edited);
    success(invoke(&[
        Path::new("--json"),
        Path::new("verify-import"),
        &project,
        Path::new("--disk-media"),
    ]));
    let wav = root.path().join("edited.wav");
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &project,
        Path::new("--disk-media"),
        Path::new("-o"),
        &wav,
    ]));
    let samples = hound::WavReader::open(&wav)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(
        samples,
        vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, -0.5, 0.5, -0.5, 0.25, -0.25]
    );

    let archive = root.path().join("archive");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch"),
        &base_archive,
        &transaction,
        Path::new("--output-dir"),
        &archive,
    ]));
    let moved = root.path().join("moved archive");
    fs::rename(archive, &moved).unwrap();
    let reopened = root.path().join("reopened");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--output-dir"),
        &reopened,
    ]));
    assert_eq!(
        SourceDocument::parse(fs::read_to_string(reopened.join("main.maac")).unwrap())
            .unwrap()
            .revision(),
        result["digest"].as_str().unwrap()
    );
    let replay = root.path().join("replay.wav");
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &reopened,
        Path::new("--disk-media"),
        Path::new("-o"),
        &replay,
    ]));
    assert_eq!(fs::read(wav).unwrap(), fs::read(replay).unwrap());

    let inverse = root.path().join("inverse.json");
    fs::write(
        &inverse,
        serde_json::to_vec(&result["edit"]["inverse"]).unwrap(),
    )
    .unwrap();
    let restored = success(patch(&project, &inverse));
    assert_eq!(restored["digest"], original_revision);
    assert_eq!(
        SourceDocument::parse(fs::read_to_string(main).unwrap())
            .unwrap()
            .revision(),
        original_revision
    );
}

#[test]
fn disk_patch_rejects_invalid_crops_unseen_media_and_preserves_destination() {
    let root = tempfile::tempdir().unwrap();
    let project = imported_project(root.path());
    let source = fs::read_to_string(project.join("main.maac")).unwrap();
    fs::copy(project.join("media.pcm"), project.join("unseen.pcm")).unwrap();
    let transaction = root.path().join("invalid.json");
    let output = root.path().join("owner.maac");
    fs::write(&output, b"existing source").unwrap();
    for operation in [
        set(
            "clip",
            "source",
            json!({"t":"list","items":[quantity(530_000,1,"frame"),quantity(530_001,1,"frame")]}),
        ),
        set("media", "path", json!({"t":"string","v":"unseen.pcm"})),
    ] {
        write_patch(&transaction, &source, vec![operation]);
        let rejected = invoke(&[
            Path::new("--json"),
            Path::new("patch"),
            &project,
            &transaction,
            Path::new("--disk-media"),
            Path::new("-o"),
            &output,
            Path::new("--force"),
        ]);
        assert!(!rejected.status.success(), "{rejected:?}");
        assert_eq!(fs::read(&output).unwrap(), b"existing source");
        assert_eq!(
            fs::read_to_string(project.join("main.maac")).unwrap(),
            source
        );
    }
    write_patch(
        &transaction,
        &source,
        vec![set("clip", "gain", json!({"t":"number","n":"1","d":"2"}))],
    );
    let pcm_path = project.join("media.pcm");
    let mut pcm = fs::read(&pcm_path).unwrap();
    pcm[..4].copy_from_slice(&0.25f32.to_le_bytes());
    fs::write(&pcm_path, pcm).unwrap();
    let args = [
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &transaction,
        Path::new("--disk-media"),
        Path::new("-o"),
        &output,
        Path::new("--force"),
    ];
    assert_eq!(failure(invoke(&args))["code"], "E_HASH");
    assert_eq!(fs::read(&output).unwrap(), b"existing source");
    fs::remove_file(pcm_path).unwrap();
    assert_eq!(failure(invoke(&args))["code"], "E_ASSET");
    assert_eq!(fs::read(output).unwrap(), b"existing source");
}

#[test]
fn disk_patch_selects_song_work_profile_without_changing_public_patch_api() {
    let root = tempfile::tempdir().unwrap();
    let project = imported_project(root.path());
    let main = project.join("main.maac");
    let source = fs::read_to_string(&main)
        .unwrap()
        .replace("530000/48000q", "210q")
        .replace("mode = rate;", "mode = rate; speed = 53/1008;");
    assert!(source.contains("210q"));
    fs::write(&main, &source).unwrap();
    let transaction = root.path().join("gain.json");
    write_patch(
        &transaction,
        &source,
        vec![set("clip", "gain", json!({"t":"number","n":"1","d":"2"}))],
    );
    assert_eq!(
        failure(patch(&project, &transaction))["code"],
        "E_RESOURCE_LIMIT"
    );
    let result = success(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        &project,
        &transaction,
        Path::new("--disk-media"),
        Path::new("--profile"),
        Path::new("song"),
        Path::new("-o"),
        &main,
        Path::new("--force"),
    ]));
    assert!(result["edit"]["inverse"]["operations"].is_array());
    let usage = failure(invoke(&[
        Path::new("--json"),
        Path::new("patch"),
        Path::new("missing.maac"),
        Path::new("missing.json"),
        Path::new("--profile"),
        Path::new("song"),
        Path::new("-o"),
        &root.path().join("absent.maac"),
    ]));
    assert_eq!(usage["code"], "E_USAGE");
}
