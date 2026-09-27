use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};
use tempfile::tempdir;

const SOURCE: &str = r#"maac 1;
project p { score=[0q,1/10q]; tail=1/5s; rate=48000Hz; tempo=&clock; meter=&metre; output=&master:out; requires=["maac.production/1"]; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node bass { type="core.sine/1"; params={attack=0s;release=0s;level=0.2;}; }
node room { type="fx.reverb/1"; config={channels=1;}; params={decay=1/2s;mix=0.4;}; }
node delay { type="core.delay/1"; config={channels=1;frames=16;}; }
node gain { type="core.gain/1"; config={channels=1;}; params={gain=0.7;}; }
node pan { type="core.pan/1"; params={pan=-0.2;}; }
node master { type="core.sum/1"; config={channels=2;}; }
connect a { from=&bass:out; to=&room:in; }
connect b { from=&room:out; to=&delay:in; }
connect c { from=&delay:out; to=&gain:in; }
connect d { from=&gain:out; to=&pan:in; }
connect e { from=&pan:out; to=&master:in; }
pattern phrase { length=1/10q; note n { at=0q; dur=1/10q; pitch=A4; velocity=1; } }
track bass_track { target=&bass:events; }
place bass_notes { pattern=&phrase; track=&bass_track; at=0q; }
"#;

fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}

#[track_caller]
fn success(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn archive_manifest(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("maac-archive.json")).unwrap()).unwrap()
}

fn checkpoint(root: &Path) -> std::path::PathBuf {
    let manifest = archive_manifest(root);
    root.join("checkpoints")
        .join(&manifest["head"].as_str().unwrap()[7..])
}

#[test]
fn native_processor_context_reopens_and_keeps_selective_node_freeze_reuse() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let archive = temp.path().join("archive");
    let moved = temp.path().join("moved");
    let restored = temp.path().join("restored");
    let appended = temp.path().join("appended");
    let journaled = temp.path().join("journaled");
    let journal_unpacked = temp.path().join("journal-unpacked");
    let patch = temp.path().join("gain.json");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("main.maac"), SOURCE).unwrap();
    let created = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--processor-context"),
        Path::new("--freeze-node"),
        Path::new("room"),
        Path::new("--output-dir"),
        &archive,
    ]));
    assert_eq!(created["format"], "maac.editable-archive/12");
    let context_path = checkpoint(&archive).join("maac-processors.json");
    let context_bytes = fs::read(&context_path).unwrap();
    let context: Value = serde_json::from_slice(&context_bytes).unwrap();
    assert_eq!(context["format"], "maac.archive-processors");
    assert_eq!(context["version"], 1);
    let delay = context["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "delay")
        .unwrap();
    assert_eq!(delay["latency_frames"], 16);
    assert_eq!(delay["state_mode"], "reset");

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
    assert_eq!(
        fs::read_to_string(restored.join("main.maac")).unwrap(),
        SOURCE
    );
    assert_eq!(
        fs::read(restored.join("maac-processors.json")).unwrap(),
        context_bytes
    );

    fs::write(
        restored.join("main.maac"),
        SOURCE.replace("gain=0.7", "gain=0.5"),
    )
    .unwrap();
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

    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &restored,
        Path::new("--previous"),
        &moved,
        Path::new("--output-dir"),
        &appended,
    ]));
    assert_eq!(archive_manifest(&appended)["version"], 12);
    assert!(checkpoint(&appended).join("maac-processors.json").exists());
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &appended,
    ]));

    let transaction = Transaction::new(
        SourceDocument::parse(SOURCE).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec!["gain".into()],
            field: vec!["params".into(), "gain".into()],
            value: json!({"t":"number","n":"1","d":"2"}),
            expect: Some(json!({"t":"number","n":"7","d":"10"})),
            expect_absent: false,
        }],
    )
    .unwrap();
    fs::write(&patch, transaction.to_json().unwrap()).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch"),
        &moved,
        &patch,
        Path::new("--output-dir"),
        &journaled,
    ]));
    assert_eq!(archive_manifest(&journaled)["version"], 12);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &journaled,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &journaled,
        Path::new("--output-dir"),
        &journal_unpacked,
    ]));
    assert_ne!(
        fs::read(journal_unpacked.join("maac-processors.json")).unwrap(),
        context_bytes
    );
}
