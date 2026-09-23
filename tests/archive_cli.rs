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

    let mut pcm = OpenOptions::new()
        .write(true)
        .open(archive.join("large.pcm"))
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
