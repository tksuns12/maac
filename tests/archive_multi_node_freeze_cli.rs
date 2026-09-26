use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::bundle::sha256_digest;
use serde_json::Value;
use tempfile::tempdir;

const SOURCE: &str = r#"maac 1;
project p { score=[0q,1/10q]; tail=1/5s; rate=48000Hz; tempo=&clock; meter=&metre; output=&master:out; requires=["maac.production/1"]; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node bass { type="core.sine/1"; params={attack=0s;release=0s;level=0.2;}; }
node pulse { type="core.sine/1"; params={attack=0s;release=0s;level=0.1;}; }
node bass_drive { type="core.gain/1"; config={channels=1;}; params={gain=0.8;}; }
node room { type="fx.reverb/1"; config={channels=1;}; params={decay=1/2s;mix=0.4;}; }
node gain { type="core.gain/1"; config={channels=1;}; params={gain=0.7;}; }
node bass_pan { type="core.pan/1"; params={pan=-0.2;}; }
node pulse_pan { type="core.pan/1"; params={pan=0.2;}; }
node eq { type="fx.eq/1"; config={channels=2;mode=peak;}; params={gain=-3dB;}; }
node master { type="core.sum/1"; config={channels=2;}; }
connect a { from=&bass:out; to=&bass_drive:in; }
connect b { from=&bass_drive:out; to=&room:in; }
connect c { from=&room:out; to=&gain:in; }
connect d { from=&gain:out; to=&bass_pan:in; }
connect e { from=&bass_pan:out; to=&master:in; }
connect f { from=&pulse:out; to=&pulse_pan:in; }
connect g { from=&pulse_pan:out; to=&eq:in; }
connect h { from=&eq:out; to=&master:in; }
pattern phrase { length=1/10q; note n { at=0q; dur=1/10q; pitch=A4; velocity=1; } }
track bass_track { target=&bass:events; }
track pulse_track { target=&pulse:events; }
place bass_notes { pattern=&phrase; track=&bass_track; at=0q; }
place pulse_notes { pattern=&phrase; track=&pulse_track; at=0q; }
"#;

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

fn project(root: &Path) {
    fs::create_dir(root).unwrap();
    fs::write(root.join("main.maac"), SOURCE).unwrap();
}

#[test]
fn two_independent_effects_relocate_and_reuse_the_edited_mix() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    let reversed = temp.path().join("reversed");
    let relocated = temp.path().join("relocated");
    let restored = temp.path().join("restored");
    let normal = temp.path().join("normal.wav");
    let reused = temp.path().join("reused.wav");
    project(&source);
    let created = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--freeze-node"),
        Path::new("eq"),
        Path::new("--output-dir"),
        &archive,
    ]));
    assert_eq!(created["format"], "maac.editable-archive/9");
    let reverse = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("eq"),
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--output-dir"),
        &reversed,
    ]));
    assert_eq!(created["digest"], reverse["digest"]);
    fs::rename(&archive, &relocated).unwrap();
    fs::remove_dir_all(&source).unwrap();
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
        &restored,
    ]));

    fs::write(
        restored.join("main.maac"),
        SOURCE.replace("gain=0.7", "gain=0.5"),
    )
    .unwrap();
    let checked = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &relocated,
        Path::new("--source"),
        &restored,
        Path::new("--replay"),
    ]));
    assert_eq!(checked["eligibility"], "current");
    assert_eq!(checked["replay"], "matched");
    assert_eq!(checked["nodes"].as_array().unwrap().len(), 2);
    assert_ne!(checked["source_digest"], checked["frozen_source_digest"]);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &restored,
        Path::new("-o"),
        &normal,
    ]));
    let rendered = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &relocated,
        Path::new("--source"),
        &restored,
        Path::new("-o"),
        &reused,
    ]));
    assert_eq!(rendered["reused"], true);
    assert_eq!(rendered["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(rendered["source_digest"], checked["source_digest"]);
    assert_eq!(
        rendered["frozen_source_digest"],
        checked["frozen_source_digest"]
    );
    assert_eq!(fs::read(&reused).unwrap(), fs::read(&normal).unwrap());
    assert_eq!(
        rendered["output_digest"],
        sha256_digest(&fs::read(&reused).unwrap())
    );
}

#[test]
fn one_stale_member_prevents_group_publication() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    let output = temp.path().join("stale.wav");
    project(&source);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--freeze-node"),
        Path::new("eq"),
        Path::new("--output-dir"),
        &archive,
    ]));
    fs::write(
        source.join("main.maac"),
        SOURCE.replace("gain=0.8", "gain=0.6"),
    )
    .unwrap();
    let checked = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &archive,
        Path::new("--source"),
        &source,
    ]));
    assert_eq!(checked["code"], "E_FREEZE_STALE");
    let rendered = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &archive,
        Path::new("--source"),
        &source,
        Path::new("-o"),
        &output,
    ]));
    assert_eq!(rendered["code"], "E_FREEZE_STALE");
    assert!(!output.exists());
}

#[test]
fn removed_effect_reports_a_stale_group() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    project(&source);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--freeze-node"),
        Path::new("eq"),
        Path::new("--output-dir"),
        &archive,
    ]));
    let edited = SOURCE
        .replace(
            "node eq { type=\"fx.eq/1\"; config={channels=2;mode=peak;}; params={gain=-3dB;}; }\n",
            "",
        )
        .replace("connect g { from=&pulse_pan:out; to=&eq:in; }\n", "")
        .replace(
            "connect h { from=&eq:out; to=&master:in; }",
            "connect h { from=&pulse_pan:out; to=&master:in; }",
        );
    fs::write(source.join("main.maac"), edited).unwrap();
    let checked = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &archive,
        Path::new("--source"),
        &source,
    ]));
    assert_eq!(checked["code"], "E_FREEZE_STALE");
    assert_eq!(checked["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(checked["nodes"][0]["eligibility"], "stale");
}

#[test]
fn duplicate_group_boundary_fails_before_archive_publication() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    project(&source);
    let result = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--output-dir"),
        &archive,
    ]));
    assert_eq!(result["ok"], false);
    assert!(!archive.exists());
}
