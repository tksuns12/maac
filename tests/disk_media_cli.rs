use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const PCM_BYTES: u64 = 20_000_004;
const PCM_FRAMES: u64 = PCM_BYTES / 4;

fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}

fn result(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failure(output: Output) -> Value {
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

fn make_project(root: &Path) {
    fs::create_dir(root).unwrap();
    let pcm = root.join("long.pcm");
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
        "maac 1;\n\
         project p {{ score=[0q,1/3000q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&clip:out; }}\n\
         tempo clock {{ points=[(0q,120bpm,step)]; }}\n\
         meter metre {{ points=[(0q,4,4)]; }}\n\
         asset long {{ kind=audio; path=\"long.pcm\"; hash=\"{hash}\"; format=\"pcm_f32le_interleaved/1\"; rate=24000Hz; channels=1; frames={PCM_FRAMES}; }}\n\
         audio clip {{ asset=&long; at=0q; source=[{}frame,{}frame]; mode=rate; }}\n",
        PCM_FRAMES - 2,
        PCM_FRAMES,
    );
    fs::write(root.join("main.maac"), source).unwrap();
}

#[test]
fn oversized_native_pcm_checks_builds_and_reopens_with_disk_media() {
    let directory = tempdir().unwrap();
    let project = directory.path().join("project");
    let relocated = directory.path().join("relocated");
    let first = directory.path().join("first.wav");
    let second = directory.path().join("second.wav");
    make_project(&project);

    assert_eq!(
        failure(invoke(&[Path::new("--json"), Path::new("check"), &project]))["code"],
        "E_RESOURCE_LIMIT"
    );
    let checked = result(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &project,
        Path::new("--disk-media"),
    ]));
    assert_eq!(checked["audio_clips"], 1);
    result(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &project,
        Path::new("--disk-media"),
        Path::new("-o"),
        &first,
    ]));

    fs::rename(&project, &relocated).unwrap();
    result(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &relocated,
        Path::new("--disk-media"),
    ]));
    result(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &relocated,
        Path::new("--disk-media"),
        Path::new("-o"),
        &second,
    ]));
    assert_eq!(fs::read(first).unwrap(), fs::read(second).unwrap());
}

#[test]
fn bad_external_hash_preserves_existing_wav_with_force() {
    let directory = tempdir().unwrap();
    let project = directory.path().join("project");
    let destination = directory.path().join("keep.wav");
    make_project(&project);
    let mut pcm = OpenOptions::new()
        .write(true)
        .open(project.join("long.pcm"))
        .unwrap();
    pcm.seek(SeekFrom::Start(PCM_BYTES - 4)).unwrap();
    pcm.write_all(&0.25f32.to_le_bytes()).unwrap();
    fs::write(&destination, b"existing output").unwrap();

    let rejected = failure(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &project,
        Path::new("--disk-media"),
        Path::new("--force"),
        Path::new("-o"),
        &destination,
    ]));
    assert_eq!(rejected["code"], "E_HASH");
    assert_eq!(fs::read(destination).unwrap(), b"existing output");
}

#[test]
fn compact_clips_kits_notes_and_routing_match_embedded_render() {
    let directory = tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = root.join("examples/audio-clips.maac");
    for format in ["float32", "pcm16"] {
        let embedded = directory.path().join(format!("embedded-{format}.wav"));
        let disk = directory.path().join(format!("disk-{format}.wav"));
        let args = [
            Path::new("--json"),
            Path::new("build"),
            source.as_path(),
            Path::new("--project-root"),
            root,
            Path::new("--format"),
            Path::new(format),
            Path::new("-o"),
            embedded.as_path(),
        ];
        result(invoke(&args));
        let disk_args = [
            Path::new("--json"),
            Path::new("build"),
            source.as_path(),
            Path::new("--project-root"),
            root,
            Path::new("--format"),
            Path::new(format),
            Path::new("--disk-media"),
            Path::new("-o"),
            disk.as_path(),
        ];
        result(invoke(&disk_args));
        assert_eq!(fs::read(embedded).unwrap(), fs::read(disk).unwrap());
    }
}
