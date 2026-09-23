use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const PCM_BYTES: u64 = 5_000_004;
const PCM_FRAMES: u64 = PCM_BYTES / 4;

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

fn rejected(output: Output) -> Value {
    assert!(!output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn hash_file(path: &Path) -> String {
    let mut file = File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    format!("sha256:{:x}", hash.finalize())
}

fn project(root: &Path) {
    fs::create_dir(root).unwrap();
    let pcm = root.join("large.pcm");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pcm)
        .unwrap();
    file.set_len(PCM_BYTES).unwrap();
    file.seek(SeekFrom::Start(PCM_BYTES - 8)).unwrap();
    file.write_all(&0.5f32.to_le_bytes()).unwrap();
    file.write_all(&(-0.5f32).to_le_bytes()).unwrap();
    let hash = hash_file(&pcm);
    let source = format!(
        "maac 1;\n// Keep this exact authored comment.\n\
         project p {{ score=[0q,1/3000q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&clip:out; }}\n\
         tempo clock {{ points=[(0q,120bpm,step)]; }}\n\
         meter metre {{ points=[(0q,4,4)]; }}\n\
         asset large {{ kind=audio; path=\"large.pcm\"; hash=\"{hash}\"; format=\"pcm_f32le_interleaved/1\"; rate=24000Hz; channels=1; frames={PCM_FRAMES}; }}\n\
         audio clip {{ asset=&large; at=0q; source=[{}frame,{}frame]; mode=rate; }}\n",
        PCM_FRAMES - 2,
        PCM_FRAMES,
    );
    fs::write(root.join("main.maac"), source).unwrap();
}

fn checkpoint_ids(archive: &Path) -> Vec<String> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(archive.join("maac-archive.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], 2);
    manifest["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

fn checkpoint_dir(archive: &Path, id: &str) -> std::path::PathBuf {
    archive
        .join("checkpoints")
        .join(id.strip_prefix("sha256:").unwrap())
}

#[test]
fn large_media_archive_relocates_verifies_and_unpacks_exact_sources() {
    let directory = tempdir().unwrap();
    let original = directory.path().join("original");
    let archive = directory.path().join("archive");
    let moved = directory.path().join("moved-archive");
    let unpacked = directory.path().join("unpacked");
    let wav = directory.path().join("render.wav");
    project(&original);
    let expected_source = fs::read(original.join("main.maac")).unwrap();
    let expected_pcm = hash_file(&original.join("large.pcm"));

    let created = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &original,
        Path::new("--output-dir"),
        &archive,
    ]));
    let digest = created["digest"].as_str().unwrap();
    assert!(digest.starts_with("sha256:"));
    fs::rename(&archive, &moved).unwrap();
    fs::remove_dir_all(&original).unwrap();

    let verified = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &moved,
        Path::new("--expect-hash"),
        Path::new(digest),
    ]));
    assert_eq!(verified["digest"], digest);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--output-dir"),
        &unpacked,
    ]));
    assert_eq!(
        fs::read(unpacked.join("main.maac")).unwrap(),
        expected_source
    );
    assert_eq!(hash_file(&unpacked.join("large.pcm")), expected_pcm);
    assert!(!unpacked.join("maac-archive.json").exists());
    success(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &unpacked,
        Path::new("--disk-media"),
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &unpacked,
        Path::new("--disk-media"),
        Path::new("-o"),
        &wav,
    ]));
    assert!(wav.exists());
}

#[test]
fn corrupt_archive_and_existing_destination_fail_without_publication() {
    let directory = tempdir().unwrap();
    let original = directory.path().join("original");
    let archive = directory.path().join("archive");
    let destination = directory.path().join("destination");
    project(&original);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &original,
        Path::new("--output-dir"),
        &archive,
    ]));

    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep"), b"existing").unwrap();
    let refused = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &archive,
        Path::new("--output-dir"),
        &destination,
    ]));
    assert_eq!(refused["code"], "E_OUTPUT_EXISTS");
    assert_eq!(fs::read(destination.join("keep")).unwrap(), b"existing");

    let head = checkpoint_ids(&archive).pop().unwrap();
    let mut pcm = OpenOptions::new()
        .write(true)
        .open(checkpoint_dir(&archive, &head).join("large.pcm"))
        .unwrap();
    pcm.seek(SeekFrom::Start(PCM_BYTES - 4)).unwrap();
    pcm.write_all(&0.25f32.to_le_bytes()).unwrap();
    let refused = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &archive,
    ]));
    assert_eq!(refused["code"], "E_HASH");
    assert_eq!(fs::read(destination.join("keep")).unwrap(), b"existing");
}

#[test]
fn native_production_descriptor_and_builtin_import_reopen_offline() {
    let directory = tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, source, profile) in [
        ("production", "examples/production.maac", "song"),
        ("builtin", "examples/basic/bell.maac", "default"),
    ] {
        let archive = directory.path().join(format!("{name}-archive"));
        let unpacked = directory.path().join(format!("{name}-unpacked"));
        success(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("create"),
            root.join(source).as_path(),
            Path::new("--project-root"),
            root,
            Path::new("--profile"),
            Path::new(profile),
            Path::new("--output-dir"),
            &archive,
        ]));
        success(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("verify"),
            &archive,
        ]));
        success(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("unpack"),
            &archive,
            Path::new("--output-dir"),
            &unpacked,
        ]));
        assert_eq!(
            fs::read(root.join(source)).unwrap(),
            fs::read(unpacked.join(source)).unwrap()
        );
        success(invoke(&[
            Path::new("--json"),
            Path::new("check"),
            unpacked.join(source).as_path(),
            Path::new("--project-root"),
            &unpacked,
            Path::new("--profile"),
            Path::new(profile),
        ]));
    }
}

#[test]
fn history_retains_removed_media_and_reopens_both_revisions() {
    let directory = tempdir().unwrap();
    let project_dir = directory.path().join("project");
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    let no_op = directory.path().join("no-op");
    let old = directory.path().join("old");
    let current = directory.path().join("current");
    project(&project_dir);
    let old_source = fs::read(project_dir.join("main.maac")).unwrap();
    let old_pcm_hash = hash_file(&project_dir.join("large.pcm"));

    let first_result = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project_dir,
        Path::new("--output-dir"),
        &first,
    ]));
    let first_hash = first_result["digest"].as_str().unwrap();
    let first_id = checkpoint_ids(&first)[0].clone();
    let nested_output = first.join("nested-output");
    assert_eq!(
        rejected(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("create"),
            &project_dir,
            Path::new("--previous"),
            &first,
            Path::new("--output-dir"),
            &nested_output,
        ]))["code"],
        "E_REFERENCE"
    );
    assert!(!nested_output.exists());
    #[cfg(unix)]
    {
        let alias = directory.path().join("prior-alias");
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let aliased_output = alias.join("nested-output");
        assert_eq!(
            rejected(invoke(&[
                Path::new("--json"),
                Path::new("archive"),
                Path::new("create"),
                &project_dir,
                Path::new("--previous"),
                &first,
                Path::new("--output-dir"),
                &aliased_output,
            ]))["code"],
            "E_REFERENCE"
        );
        assert!(!first.join("nested-output").exists());
    }
    let wrong_pin_output = directory.path().join("wrong-pin");
    assert_eq!(
        rejected(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("create"),
            &project_dir,
            Path::new("--previous"),
            &first,
            Path::new("--expect-previous-hash"),
            Path::new("sha256:0000000000000000000000000000000000000000000000000000000000000000"),
            Path::new("--output-dir"),
            &wrong_pin_output,
        ]))["code"],
        "E_HASH"
    );
    assert!(!wrong_pin_output.exists());

    let new_source =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/archive_v1/main.maac"))
            .unwrap();
    fs::write(project_dir.join("main.maac"), &new_source).unwrap();
    fs::remove_file(project_dir.join("large.pcm")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project_dir,
        Path::new("--previous"),
        &first,
        Path::new("--expect-previous-hash"),
        Path::new(first_hash),
        Path::new("--output-dir"),
        &second,
    ]));
    let ids = checkpoint_ids(&second);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], first_id);
    assert_ne!(ids[0], ids[1]);

    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project_dir,
        Path::new("--previous"),
        &second,
        Path::new("--output-dir"),
        &no_op,
    ]));
    assert_eq!(checkpoint_ids(&no_op), ids);
    fs::remove_dir_all(&project_dir).unwrap();
    fs::remove_dir_all(&first).unwrap();
    fs::remove_dir_all(&second).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &no_op,
    ]));
    let nested_restore = no_op.join("nested-restore");
    assert_eq!(
        rejected(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("unpack"),
            &no_op,
            Path::new("--output-dir"),
            &nested_restore,
        ]))["code"],
        "E_REFERENCE"
    );
    assert!(!nested_restore.exists());
    let unknown_output = directory.path().join("unknown-output");
    rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &no_op,
        Path::new("--revision"),
        Path::new("sha256:0000000000000000000000000000000000000000000000000000000000000000"),
        Path::new("--output-dir"),
        &unknown_output,
    ]));
    assert!(!unknown_output.exists());
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &no_op,
        Path::new("--revision"),
        Path::new(&first_id),
        Path::new("--output-dir"),
        &old,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &no_op,
        Path::new("--output-dir"),
        &current,
    ]));
    assert_eq!(fs::read(old.join("main.maac")).unwrap(), old_source);
    assert_eq!(hash_file(&old.join("large.pcm")), old_pcm_hash);
    assert_eq!(fs::read(current.join("main.maac")).unwrap(), new_source);
    assert!(!current.join("large.pcm").exists());
    success(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &old,
        Path::new("--disk-media"),
    ]));
    success(invoke(&[Path::new("--json"), Path::new("check"), &current]));

    let mut pcm = OpenOptions::new()
        .write(true)
        .open(checkpoint_dir(&no_op, &first_id).join("large.pcm"))
        .unwrap();
    pcm.seek(SeekFrom::Start(PCM_BYTES - 4)).unwrap();
    pcm.write_all(&0.25f32.to_le_bytes()).unwrap();
    assert_eq!(
        rejected(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("verify"),
            &no_op,
        ]))["code"],
        "E_HASH"
    );
}

#[test]
fn version_one_fixture_keeps_digest_and_upgrades_without_changing_its_snapshot() {
    const V1_DIGEST: &str =
        "sha256:15d422b07aeb803b3f488c42a9b07d307f0cfdb78f09bd1173c4face9bb9842a";
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let legacy = root.join("tests/fixtures/archive_v1");
    let directory = tempdir().unwrap();
    let upgraded = directory.path().join("upgraded");
    let unpacked = directory.path().join("unpacked");
    let checked = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &legacy,
        Path::new("--expect-hash"),
        Path::new(V1_DIGEST),
    ]));
    assert_eq!(checked["digest"], V1_DIGEST);
    assert_eq!(checked["format"], "maac.editable-archive/1");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        legacy.join("main.maac").as_path(),
        Path::new("--previous"),
        &legacy,
        Path::new("--output-dir"),
        &upgraded,
    ]));
    let ids = checkpoint_ids(&upgraded);
    assert_eq!(ids.len(), 1);
    assert_eq!(
        fs::read(legacy.join("maac-archive.json")).unwrap(),
        fs::read(checkpoint_dir(&upgraded, &ids[0]).join("maac-archive.json")).unwrap()
    );
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &upgraded,
        Path::new("--revision"),
        Path::new(&ids[0]),
        Path::new("--output-dir"),
        &unpacked,
    ]));
    assert_eq!(
        fs::read(legacy.join("main.maac")).unwrap(),
        fs::read(unpacked.join("main.maac")).unwrap()
    );
}
