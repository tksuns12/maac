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
node room { type="fx.reverb/1"; config={channels=1;}; params={decay=1/2s;mix=0.4;}; }
node gain { type="core.gain/1"; config={channels=1;}; params={gain=3/4;}; }
node bass_pan { type="core.pan/1"; params={pan=-0.2;}; }
node pulse_pan { type="core.pan/1"; params={pan=0.2;}; }
node master { type="core.sum/1"; config={channels=2;}; }
connect a { from=&bass:out; to=&room:in; }
connect b { from=&room:out; to=&gain:in; }
connect c { from=&gain:out; to=&bass_pan:in; }
connect d { from=&bass_pan:out; to=&master:in; }
connect e { from=&pulse:out; to=&pulse_pan:in; }
connect f { from=&pulse_pan:out; to=&master:in; }
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
fn internal_reverb_freeze_relocates_and_preserves_final_wav() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    let moved = temp.path().join("moved");
    let restored = temp.path().join("restored");
    let normal = temp.path().join("normal.wav");
    let reused = temp.path().join("reused.wav");
    project(&source);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &source,
        Path::new("-o"),
        &normal,
    ]));
    let created = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--output-dir"),
        &archive,
    ]));
    assert_eq!(created["format"], "maac.editable-archive/8");
    fs::rename(&archive, &moved).unwrap();
    fs::remove_dir_all(&source).unwrap();
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
        &restored,
    ]));
    let checked = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &moved,
        Path::new("--source"),
        &restored,
        Path::new("--replay"),
    ]));
    assert_eq!(checked["eligibility"], "current");
    assert_eq!(checked["replay"], "matched");
    let rendered = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &moved,
        Path::new("--source"),
        &restored,
        Path::new("-o"),
        &reused,
    ]));
    assert_eq!(rendered["reused"], true);
    assert_eq!(rendered["boundary"]["node"], "room");
    assert_eq!(rendered["cache_digest"], checked["cache_digest"]);
    assert_ne!(rendered["output_digest"], rendered["cache_digest"]);
    assert_eq!(fs::read(&reused).unwrap(), fs::read(&normal).unwrap());
    assert_eq!(
        rendered["output_digest"],
        sha256_digest(&fs::read(&reused).unwrap())
    );
    let unsupported_output = temp.path().join("unsupported.wav");
    let unsupported = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &moved,
        Path::new("--source"),
        &restored,
        Path::new("--reuse-current-nodes"),
        Path::new("-o"),
        &unsupported_output,
    ]));
    assert_eq!(unsupported["code"], "E_CAPABILITY");
    assert!(!unsupported_output.exists());

    fs::write(
        restored.join("main.maac"),
        format!("{SOURCE}// downstream edit\n"),
    )
    .unwrap();
    let stale_output = temp.path().join("stale.wav");
    let stale = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &moved,
        Path::new("--source"),
        &restored,
        Path::new("-o"),
        &stale_output,
    ]));
    assert_eq!(stale["code"], "E_FREEZE_STALE");
    assert!(!stale_output.exists());

    let root_manifest: Value =
        serde_json::from_slice(&fs::read(moved.join("maac-archive.json")).unwrap()).unwrap();
    let freeze_id = root_manifest["checkpoints"][0]["freeze"].as_str().unwrap();
    let payload = moved
        .join("freezes")
        .join(&freeze_id[7..])
        .join("output.f64le");
    let mut bytes = fs::read(&payload).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&payload, bytes).unwrap();
    let tampered = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &moved,
    ]));
    assert_eq!(tampered["code"], "E_HASH");
    let corrupt_output = temp.path().join("corrupt.wav");
    let rejected_render = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &moved,
        Path::new("--source"),
        &restored,
        Path::new("-o"),
        &corrupt_output,
    ]));
    assert_eq!(rejected_render["code"], "E_HASH");
    assert!(!corrupt_output.exists());
}

#[test]
fn downstream_gain_and_pan_edits_reuse_the_cache_in_the_current_graph() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    let original = temp.path().join("original.wav");
    let current = temp.path().join("current.wav");
    let reused = temp.path().join("reused.wav");
    project(&source);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &source,
        Path::new("-o"),
        &original,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--output-dir"),
        &archive,
    ]));

    let edited = SOURCE
        .replace("gain=3/4", "gain=1/2")
        .replace("pan=-0.2", "pan=0.1");
    fs::write(source.join("main.maac"), edited).unwrap();
    let checked = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &archive,
        Path::new("--source"),
        &source,
        Path::new("--replay"),
    ]));
    assert_eq!(checked["eligibility"], "current");
    assert_eq!(checked["replay"], "matched");
    assert_ne!(checked["source_digest"], checked["frozen_source_digest"]);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &source,
        Path::new("-o"),
        &current,
    ]));
    let rendered = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &archive,
        Path::new("--source"),
        &source,
        Path::new("-o"),
        &reused,
    ]));
    assert_eq!(rendered["reused"], true);
    assert_eq!(rendered["source_digest"], checked["source_digest"]);
    assert_eq!(
        rendered["frozen_source_digest"],
        checked["frozen_source_digest"]
    );
    assert_eq!(fs::read(&reused).unwrap(), fs::read(&current).unwrap());
    assert_ne!(fs::read(&reused).unwrap(), fs::read(&original).unwrap());
    assert_eq!(
        rendered["output_digest"],
        sha256_digest(&fs::read(&reused).unwrap())
    );
}

#[test]
fn independent_sibling_gain_and_pan_edits_reuse_the_frozen_effect() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    let original = temp.path().join("original.wav");
    let current = temp.path().join("current.wav");
    let reused = temp.path().join("reused.wav");
    project(&source);
    let sibling_source = SOURCE
        .replace(
            "node pulse_pan",
            "node pulse_gain { type=\"core.gain/1\"; config={channels=1;}; params={gain=0.6;}; }\nnode pulse_pan",
        )
        .replace(
            "connect e { from=&pulse:out; to=&pulse_pan:in; }",
            "connect e { from=&pulse:out; to=&pulse_gain:in; }\nconnect e2 { from=&pulse_gain:out; to=&pulse_pan:in; }",
        );
    fs::write(source.join("main.maac"), &sibling_source).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &source,
        Path::new("-o"),
        &original,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--output-dir"),
        &archive,
    ]));
    let root_manifest: Value =
        serde_json::from_slice(&fs::read(archive.join("maac-archive.json")).unwrap()).unwrap();
    let freeze_id = root_manifest["checkpoints"][0]["freeze"].as_str().unwrap();
    let node_manifest: Value = serde_json::from_slice(
        &fs::read(
            archive
                .join("freezes")
                .join(&freeze_id[7..])
                .join("maac-node-freeze.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(node_manifest["version"], 3);

    fs::write(
        source.join("main.maac"),
        sibling_source
            .replace("gain=0.6", "gain=0.4")
            .replace("pan=0.2", "pan=-0.1"),
    )
    .unwrap();
    let checked = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &archive,
        Path::new("--source"),
        &source,
        Path::new("--replay"),
    ]));
    assert_eq!(checked["eligibility"], "current");
    assert_eq!(checked["replay"], "matched");
    assert_ne!(checked["source_digest"], checked["frozen_source_digest"]);
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &source,
        Path::new("-o"),
        &current,
    ]));
    let rendered = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &archive,
        Path::new("--source"),
        &source,
        Path::new("-o"),
        &reused,
    ]));
    assert_eq!(rendered["source_digest"], checked["source_digest"]);
    assert_eq!(
        rendered["frozen_source_digest"],
        checked["frozen_source_digest"]
    );
    assert_eq!(rendered["cache_digest"], checked["cache_digest"]);
    assert_eq!(fs::read(&reused).unwrap(), fs::read(&current).unwrap());
    assert_ne!(fs::read(&reused).unwrap(), fs::read(&original).unwrap());
    assert_eq!(
        rendered["output_digest"],
        sha256_digest(&fs::read(&reused).unwrap())
    );
}

#[test]
fn frozen_effect_and_upstream_edits_stale_the_node_cache() {
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
        Path::new("--output-dir"),
        &archive,
    ]));

    for (name, edited) in [
        ("effect", SOURCE.replace("mix=0.4", "mix=0.5")),
        ("upstream", SOURCE.replace("level=0.2", "level=0.3")),
    ] {
        fs::write(source.join("main.maac"), edited).unwrap();
        let checked = rejected(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("freeze-check"),
            &archive,
            Path::new("--source"),
            &source,
        ]));
        assert_eq!(checked["code"], "E_FREEZE_STALE", "{name}");
        let destination = temp.path().join(format!("{name}.wav"));
        let rendered = rejected(invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("freeze-render"),
            &archive,
            Path::new("--source"),
            &source,
            Path::new("-o"),
            &destination,
        ]));
        assert_eq!(rendered["code"], "E_FREEZE_STALE", "{name}");
        assert!(!destination.exists(), "{name}");
    }
}

#[test]
fn invalid_node_selection_fails_without_publishing_archive() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    project(&source);
    let unsupported = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("bass"),
        Path::new("--output-dir"),
        &archive,
    ]));
    assert_eq!(unsupported["ok"], false);
    assert!(!archive.exists());
    let final_boundary = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-node"),
        Path::new("master"),
        Path::new("--output-dir"),
        &archive,
    ]));
    assert_eq!(final_boundary["ok"], false);
    assert!(!archive.exists());
}
