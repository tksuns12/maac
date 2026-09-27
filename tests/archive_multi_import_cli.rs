use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use hound::{SampleFormat, WavSpec, WavWriter};
use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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

fn write_wav(path: &Path, first: i16) {
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

fn import(input: &Path, output: &Path) {
    success(invoke(&[
        Path::new("--json"),
        Path::new("import-wav"),
        input,
        Path::new("--start-frame"),
        Path::new("1"),
        Path::new("--end-frame"),
        Path::new("3"),
        Path::new("--retain-original"),
        Path::new("--output-dir"),
        output,
    ]));
}

fn project(root: &Path) -> String {
    fs::create_dir_all(root.join("imports")).unwrap();
    for (name, sample) in [("drums", 500), ("vocals", -500)] {
        let input = root.parent().unwrap().join(format!("{name}.wav"));
        write_wav(&input, sample);
        import(&input, &root.join("imports").join(name));
    }
    let asset = |name: &str| {
        let pcm = fs::read(root.join("imports").join(name).join("media.pcm")).unwrap();
        let hash = format!("sha256:{:x}", Sha256::digest(pcm));
        format!(
            "asset {name} {{kind=audio;path=\"imports/{name}/media.pcm\";hash=\"{hash}\";format=\"pcm_f32le_interleaved/1\";rate=48000Hz;channels=1;frames=2;}}\n"
        )
    };
    let source = format!(
        "maac 1;\nproject p {{score=[0q,1/3000q];tail=0s;rate=48000Hz;tempo=&clock;meter=&metre;output=&first:out;}}\n\
         tempo clock {{points=[(0q,120bpm,step)];}}\n\
         meter metre {{points=[(0q,4,4)];}}\n\
         {}{}\
         audio first {{asset=&drums;at=0q;source=[0frame,2frame];mode=rate;}}\n\
         audio second {{asset=&vocals;at=0q;source=[0frame,2frame];mode=rate;}}\n",
        asset("drums"),
        asset("vocals"),
    );
    fs::write(root.join("main.maac"), &source).unwrap();
    source
}

fn create(root: &Path, output: &Path, extra: &[&Path]) -> Output {
    let entry = root.join("main.maac");
    let mut args = vec![
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &entry,
        Path::new("--project-root"),
        root,
        Path::new("--output-dir"),
        output,
    ];
    args.extend_from_slice(extra);
    invoke(&args)
}

fn selected() -> [&'static Path; 4] {
    [
        Path::new("--retain-import"),
        Path::new("imports/drums"),
        Path::new("--retain-import"),
        Path::new("imports/vocals"),
    ]
}

fn manifest(archive: &Path) -> Value {
    serde_json::from_slice(&fs::read(archive.join("maac-archive.json")).unwrap()).unwrap()
}

#[test]
fn two_imports_relocate_reopen_and_journal_without_losing_provenance() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    let archive = temp.path().join("archive");
    let moved = temp.path().join("moved");
    let edited = temp.path().join("edited");
    let unpacked = temp.path().join("unpacked");
    let before_wav = temp.path().join("before.wav");
    let after_wav = temp.path().join("after.wav");
    let patch_file = temp.path().join("patch.json");
    let source = project(&root);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &root.join("main.maac"),
        Path::new("--project-root"),
        &root,
        Path::new("-o"),
        &before_wav,
    ]));
    let expected: Vec<_> = ["drums", "vocals"]
        .into_iter()
        .flat_map(|name| {
            ["media.pcm", "import.json", "original.wav"]
                .map(move |file| format!("imports/{name}/{file}"))
        })
        .map(|path| (path.clone(), fs::read(root.join(path)).unwrap()))
        .collect();
    let created = success(create(&root, &archive, &selected()));
    assert_eq!(created["format"], "maac.editable-archive/6");
    fs::rename(&archive, &moved).unwrap();
    fs::remove_dir_all(&root).unwrap();
    for name in ["drums.wav", "vocals.wav"] {
        fs::remove_file(temp.path().join(name)).unwrap();
    }
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
    for (path, bytes) in expected {
        assert_eq!(fs::read(unpacked.join(&path)).unwrap(), bytes, "{path}");
    }
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &unpacked.join("main.maac"),
        Path::new("--project-root"),
        &unpacked,
        Path::new("-o"),
        &after_wav,
    ]));
    assert_eq!(
        fs::read(&before_wav).unwrap(),
        fs::read(&after_wav).unwrap()
    );

    let transaction = Transaction::new(
        SourceDocument::parse(&source)
            .unwrap()
            .revision()
            .to_owned(),
        vec![Operation::Set {
            object: vec!["p".into()],
            field: vec!["tail".into()],
            value: json!({"t":"quantity","n":"1","d":"48000","u":"s"}),
            expect: Some(json!({"t":"quantity","n":"0","d":"1","u":"s"})),
            expect_absent: false,
        }],
    )
    .unwrap();
    fs::write(&patch_file, transaction.to_json().unwrap()).unwrap();
    let patched = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch"),
        &moved,
        &patch_file,
        Path::new("--output-dir"),
        &edited,
    ]));
    assert_eq!(patched["format"], "maac.editable-archive/6");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &edited,
    ]));
    assert_eq!(
        manifest(&edited)["checkpoints"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn selected_nested_disk_media_import_over_four_mib_reopens_from_moved_archive() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("long.wav");
    let root = temp.path().join("project");
    let child = root.join("imports/long");
    let archive = temp.path().join("archive");
    let moved = temp.path().join("moved");
    let unpacked = temp.path().join("unpacked");
    let frames = 530_000u32;
    long_stereo_wav(&input, frames);
    fs::create_dir_all(root.join("imports")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("import-wav"),
        &input,
        Path::new("--disk-media"),
        Path::new("--retain-original"),
        Path::new("--output-dir"),
        &child,
    ]));
    let source = fs::read_to_string(child.join("main.maac")).unwrap();
    assert!(source.contains("path = \"media.pcm\""));
    fs::write(
        root.join("main.maac"),
        source.replace("path = \"media.pcm\"", "path = \"imports/long/media.pcm\""),
    )
    .unwrap();
    assert!(fs::metadata(child.join("media.pcm")).unwrap().len() > 4 * 1024 * 1024);
    assert_eq!(
        success(create(
            &root,
            &archive,
            &[Path::new("--retain-import"), Path::new("imports/long")],
        ))["format"],
        "maac.editable-archive/6"
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
    for name in ["media.pcm", "import.json", "original.wav"] {
        assert_eq!(
            fs::read(child.join(name)).unwrap(),
            fs::read(unpacked.join("imports/long").join(name)).unwrap(),
            "{name}"
        );
    }
    fs::remove_dir_all(&root).unwrap();
    fs::remove_file(&input).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &unpacked.join("main.maac"),
        Path::new("--project-root"),
        &unpacked,
        Path::new("--disk-media"),
    ]));
}

#[test]
fn invalid_explicit_selections_fail_before_publication() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    project(&root);
    let unused = temp.path().join("unused.wav");
    write_wav(&unused, 250);
    import(&unused, &root.join("imports/unused"));
    for (name, args) in [
        (
            "duplicate",
            vec![
                Path::new("--retain-import"),
                Path::new("imports/drums"),
                Path::new("--retain-import"),
                Path::new("imports/drums"),
            ],
        ),
        (
            "traversal",
            vec![
                Path::new("--retain-import"),
                Path::new("imports/../imports/drums"),
            ],
        ),
        (
            "unreferenced",
            vec![Path::new("--retain-import"), Path::new("imports/unused")],
        ),
    ] {
        let output = temp.path().join(name);
        let rejected = failure(create(&root, &output, &args));
        assert_eq!(rejected["ok"], false, "{name}");
        assert!(!output.exists(), "{name}");
    }
}

#[test]
fn previous_history_keeps_nonhead_originals_checked() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    let first = temp.path().join("first");
    let later = temp.path().join("later");
    project(&root);
    success(create(&root, &first, &selected()));
    let first_id = manifest(&first)["head"].as_str().unwrap().to_owned();
    success(create(&root, &later, &[Path::new("--previous"), &first]));
    let history = manifest(&later);
    assert_eq!(history["version"], 6);
    assert_eq!(history["checkpoints"].as_array().unwrap().len(), 2);
    assert_eq!(history["checkpoints"][0]["id"], first_id);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &later,
    ]));

    let old_original = later
        .join("checkpoints")
        .join(&first_id[7..])
        .join("imports/drums/original.wav");
    let mut bytes = fs::read(&old_original).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&old_original, bytes).unwrap();
    let rejected = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &later,
    ]));
    assert_eq!(rejected["ok"], false);
}

#[test]
fn selected_import_freeze_checks_current_and_stale_sources() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    let archive = temp.path().join("archive");
    let source = project(&root);
    let mut args = selected().to_vec();
    args.push(Path::new("--freeze-output"));
    let created = success(create(&root, &archive, &args));
    assert_eq!(created["format"], "maac.editable-archive/6");
    let current = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &archive,
        Path::new("--source"),
        &root.join("main.maac"),
        Path::new("--project-root"),
        &root,
        Path::new("--replay"),
    ]));
    assert_eq!(current["eligibility"], "current");
    assert_eq!(current["replay"], "matched");
    let reused = temp.path().join("reused.wav");
    let reuse = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &archive,
        Path::new("--source"),
        &root.join("main.maac"),
        Path::new("--project-root"),
        &root,
        Path::new("-o"),
        &reused,
    ]));
    assert_eq!(reuse["reused"], true);
    assert!(reused.exists());
    fs::write(root.join("main.maac"), format!("{source}// changed\n")).unwrap();
    let stale = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &archive,
        Path::new("--source"),
        &root.join("main.maac"),
        Path::new("--project-root"),
        &root,
    ]));
    assert_eq!(stale["code"], "E_FREEZE_STALE");

    let without_vocals = source
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !line.starts_with("asset vocals") && !line.starts_with("audio second")
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(root.join("main.maac"), format!("{without_vocals}\n")).unwrap();
    fs::remove_dir_all(root.join("imports/vocals")).unwrap();
    let removed = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &archive,
        Path::new("--source"),
        &root.join("main.maac"),
        Path::new("--project-root"),
        &root,
    ]));
    assert_eq!(removed["code"], "E_FREEZE_STALE");
    assert_eq!(removed["eligibility"], "stale");
    let rejected_output = temp.path().join("removed.wav");
    let rejected_reuse = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &archive,
        Path::new("--source"),
        &root.join("main.maac"),
        Path::new("--project-root"),
        &root,
        Path::new("-o"),
        &rejected_output,
    ]));
    assert_eq!(rejected_reuse["code"], "E_FREEZE_STALE");
    assert!(!rejected_output.exists());
}

#[test]
fn explicit_root_import_uses_v6_without_changing_default_root_archive() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("input.wav");
    let root = temp.path().join("project");
    let legacy = temp.path().join("legacy");
    let selected = temp.path().join("selected");
    write_wav(&input, 500);
    import(&input, &root);
    assert_eq!(
        success(create(&root, &legacy, &[]))["format"],
        "maac.editable-archive/3"
    );
    assert_eq!(
        success(create(
            &root,
            &selected,
            &[Path::new("--retain-import"), Path::new(".")],
        ))["format"],
        "maac.editable-archive/6"
    );
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &selected,
    ]));
}
