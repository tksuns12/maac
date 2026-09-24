use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const SOURCE: &str = include_str!("fixtures/archive_v1/main.maac");
const LARGE_PCM_BYTES: u64 = 5_000_004;
const LARGE_PCM_FRAMES: u64 = LARGE_PCM_BYTES / 4;

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

fn write_project(path: &Path, source: &str) {
    fs::create_dir(path).unwrap();
    fs::write(path.join("main.maac"), source).unwrap();
}

fn manifest(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path.join("maac-archive.json")).unwrap()).unwrap()
}

fn freeze_path(archive: &Path, record: &Value) -> std::path::PathBuf {
    let id = record["freeze"].as_str().unwrap();
    archive.join("freezes").join(&id[7..]).join("output.wav")
}

fn write_large_pcm_project(root: &Path) {
    fs::create_dir(root).unwrap();
    let pcm = root.join("large.pcm");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pcm)
        .unwrap();
    file.set_len(LARGE_PCM_BYTES).unwrap();
    file.seek(SeekFrom::Start(LARGE_PCM_BYTES - 8)).unwrap();
    file.write_all(&0.5f32.to_le_bytes()).unwrap();
    file.write_all(&(-0.5f32).to_le_bytes()).unwrap();
    drop(file);
    let mut hash = Sha256::new();
    let mut file = File::open(&pcm).unwrap();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let source = format!(
        "maac 1;\n\
         project p {{ score=[0q,1/3000q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&clip:out; }}\n\
         tempo clock {{ points=[(0q,120bpm,step)]; }}\n\
         meter metre {{ points=[(0q,4,4)]; }}\n\
         asset large {{ kind=audio; path=\"large.pcm\"; hash=\"sha256:{:x}\"; format=\"pcm_f32le_interleaved/1\"; rate=24000Hz; channels=1; frames={LARGE_PCM_FRAMES}; }}\n\
         audio clip {{ asset=&large; at=0q; source=[{}frame,{}frame]; mode=rate; }}\n",
        hash.finalize(),
        LARGE_PCM_FRAMES - 2,
        LARGE_PCM_FRAMES,
    );
    fs::write(root.join("main.maac"), source).unwrap();
}

#[test]
fn frozen_output_relocates_replays_and_becomes_stale_after_an_edit() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let archive = temp.path().join("archive");
    let moved = temp.path().join("moved");
    let restored = temp.path().join("restored");
    let before = temp.path().join("before.wav");
    let after = temp.path().join("after.wav");
    write_project(&project, SOURCE);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &project,
        Path::new("-o"),
        &before,
    ]));

    let created = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--freeze-output"),
        Path::new("--output-dir"),
        &archive,
    ]));
    assert_eq!(created["format"], "maac.editable-archive/4");
    let root = manifest(&archive);
    let record = &root["checkpoints"][0];
    assert_eq!(
        fs::read(freeze_path(&archive, record)).unwrap(),
        fs::read(&before).unwrap()
    );
    fs::rename(&archive, &moved).unwrap();
    fs::remove_dir_all(&project).unwrap();

    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &moved,
        Path::new("--expect-hash"),
        Path::new(created["digest"].as_str().unwrap()),
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--output-dir"),
        &restored,
    ]));
    assert_eq!(
        fs::read(restored.join("main.maac")).unwrap(),
        SOURCE.as_bytes()
    );
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &restored,
        Path::new("-o"),
        &after,
    ]));
    assert_eq!(fs::read(&before).unwrap(), fs::read(&after).unwrap());
    let checked = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &moved,
        Path::new("--source"),
        &restored,
        Path::new("--replay"),
    ]));
    assert_eq!(checked["integrity"], "verified");
    assert_eq!(checked["eligibility"], "current");
    assert_eq!(checked["replay"], "matched");

    fs::write(restored.join("main.maac"), format!("{SOURCE}// changed\n")).unwrap();
    let stale = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &moved,
        Path::new("--source"),
        &restored,
    ]));
    assert_eq!(stale["code"], "E_FREEZE_STALE");
    assert_eq!(stale["eligibility"], "stale");
    assert_eq!(stale["replay"], "not_requested");
}

#[test]
fn later_unfrozen_checkpoint_keeps_old_freeze_and_nonhead_tamper_blocks_verify() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let frozen = temp.path().join("frozen");
    let history = temp.path().join("history");
    let restored = temp.path().join("restored");
    write_project(&project, SOURCE);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--freeze-output"),
        Path::new("--output-dir"),
        &frozen,
    ]));
    let first = manifest(&frozen)["checkpoints"][0].clone();
    fs::write(project.join("main.maac"), format!("{SOURCE}// second\n")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--previous"),
        &frozen,
        Path::new("--output-dir"),
        &history,
    ]));
    let root = manifest(&history);
    assert_eq!(root["version"], 4);
    assert_eq!(root["checkpoints"].as_array().unwrap().len(), 2);
    assert_eq!(root["checkpoints"][0]["id"], first["id"]);
    assert!(root["checkpoints"][1].get("freeze").is_none());
    fs::remove_dir_all(&frozen).unwrap();
    fs::remove_dir_all(&project).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &history,
        Path::new("--revision"),
        Path::new(first["id"].as_str().unwrap()),
        Path::new("--output-dir"),
        &restored,
    ]));
    let checked = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &history,
        Path::new("--revision"),
        Path::new(first["id"].as_str().unwrap()),
        Path::new("--source"),
        &restored,
    ]));
    assert_eq!(checked["eligibility"], "current");

    let output = freeze_path(&history, &first);
    let mut bytes = fs::read(&output).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(output, bytes).unwrap();
    let failure = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &history,
    ]));
    assert_eq!(failure["ok"], false);
}

#[test]
fn freeze_matches_disk_media_build_above_the_inline_asset_limit() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let archive = temp.path().join("archive");
    let before = temp.path().join("before.wav");
    write_large_pcm_project(&project);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &project,
        Path::new("--disk-media"),
        Path::new("-o"),
        &before,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--freeze-output"),
        Path::new("--output-dir"),
        &archive,
    ]));
    let record = manifest(&archive)["checkpoints"][0].clone();
    assert_eq!(
        fs::read(&before).unwrap(),
        fs::read(freeze_path(&archive, &record)).unwrap()
    );
    fs::remove_dir_all(&project).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &archive,
    ]));
}
