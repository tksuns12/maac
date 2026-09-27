use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::bundle::sha256_digest;
use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};
use tempfile::tempdir;

const COMMON: &str = "maac 1;\nlibrary common { version=\"1\"; }\npattern pulse { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }\n";

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

fn failure(output: Output) -> Value {
    assert!(!output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn branch(name: &str, pitch: &str, common_hash: &str, twice: bool) -> String {
    let duplicate = if twice {
        format!("import second {{ path=\"common.maac\"; hash=\"{common_hash}\"; }}\n")
    } else {
        String::new()
    };
    format!(
        "maac 1;\nlibrary {name} {{ version=\"1\"; }}\n\
         import common {{ path=\"common.maac\"; hash=\"{common_hash}\"; }}\n\
         {duplicate}\
         pattern phrase {{ length=1q; note n {{ at=0q; dur=1/2q; pitch={pitch}; }} }}\n"
    )
}

fn project(root: &Path, twice: bool) -> (String, String, String) {
    fs::create_dir(root).unwrap();
    fs::write(root.join("common.maac"), COMMON).unwrap();
    let common_hash = sha256_digest(COMMON.as_bytes());
    let left = branch("left", "E4", &common_hash, twice);
    let right = branch("right", "G4", &common_hash, false);
    fs::write(root.join("left.maac"), &left).unwrap();
    fs::write(root.join("right.maac"), &right).unwrap();
    let entry = format!(
        "maac 1;\nproject p {{ score=[0q,2q]; tail=0s; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}\n\
         tempo t {{ points=[(0q,120bpm,step)]; }}\n\
         meter m {{ points=[(0q,4,4)]; }}\n\
         import left {{ path=\"left.maac\"; hash=\"{}\"; }}\n\
         import right {{ path=\"right.maac\"; hash=\"{}\"; }}\n\
         node s {{ type=\"core.sine/1\"; }}\n\
         track notes {{ target=&s:events; }}\n\
         place first {{ pattern=&left.phrase; track=&notes; at=0q; }}\n\
         place second {{ pattern=&right.phrase; track=&notes; at=1q; }}\n",
        sha256_digest(left.as_bytes()),
        sha256_digest(right.as_bytes()),
    );
    fs::write(root.join("main.maac"), &entry).unwrap();
    (entry, left, right)
}

fn duration_patch(source: &str, pattern: &str, duration: &str) -> Vec<u8> {
    let (n, d) = duration.split_once('/').unwrap();
    Transaction::new(
        SourceDocument::parse(source).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec![pattern.into(), "n".into()],
            field: vec!["dur".into()],
            value: json!({"t":"quantity","n":n,"d":d,"u":"q"}),
            expect: Some(json!({"t":"quantity","n":"1","d":"2","u":"q"})),
            expect_absent: false,
        }],
    )
    .unwrap()
    .to_json()
    .unwrap()
}

fn archive_manifest(archive: &Path) -> Value {
    serde_json::from_slice(&fs::read(archive.join("maac-archive.json")).unwrap()).unwrap()
}

#[test]
fn shared_diamond_repins_every_edge_and_reopens_after_relocation() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let base = temp.path().join("base");
    let edited = temp.path().join("edited");
    let reordered = temp.path().join("reordered");
    let moved = temp.path().join("moved");
    let prior = temp.path().join("prior");
    let current = temp.path().join("current");
    let common_patch = temp.path().join("common.json");
    let left_patch = temp.path().join("left.json");
    let (_entry, left, _right) = project(&source, true);
    fs::write(&common_patch, duration_patch(COMMON, "pulse", "1/4")).unwrap();
    fs::write(&left_patch, duration_patch(&left, "phrase", "1/8")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--output-dir"),
        &base,
    ]));
    let original_id = archive_manifest(&base)["head"].as_str().unwrap().to_owned();
    let common_spec = format!("common.maac={}", common_patch.display());
    let left_spec = format!("left.maac={}", left_patch.display());
    let args = |output: &Path, reverse: bool| {
        let (first, second) = if reverse {
            (&left_spec, &common_spec)
        } else {
            (&common_spec, &left_spec)
        };
        invoke(&[
            Path::new("--json"),
            Path::new("archive"),
            Path::new("patch-group"),
            &base,
            Path::new("--source"),
            Path::new(first),
            Path::new("--source"),
            Path::new(second),
            Path::new("--output-dir"),
            output,
        ])
    };
    let result = success(args(&edited, true));
    let reordered_result = success(args(&reordered, false));
    assert_eq!(result["format"], "maac.editable-archive/11");
    assert_eq!(result["edit_digest"], reordered_result["edit_digest"]);
    assert_eq!(result["checkpoint"], reordered_result["checkpoint"]);
    assert_eq!(result["sources"].as_array().unwrap().len(), 2);
    for source in result["sources"].as_array().unwrap() {
        assert_eq!(
            source["edit"]["impact"]["render_invalidation_scope"],
            "full"
        );
        assert_eq!(source["edit"]["impact"]["full_render_invalidated"], true);
    }
    let generated = result["generated"].as_array().unwrap();
    assert_eq!(generated.len(), 3);
    assert_eq!(
        generated
            .iter()
            .map(|step| step["edges"].as_array().unwrap().len())
            .sum::<usize>(),
        5
    );
    fs::rename(&edited, &moved).unwrap();
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
        Path::new("--revision"),
        Path::new(&original_id),
        Path::new("--output-dir"),
        &prior,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--output-dir"),
        &current,
    ]));
    assert_eq!(
        fs::read_to_string(prior.join("common.maac")).unwrap(),
        COMMON
    );
    let new_common = fs::read_to_string(current.join("common.maac")).unwrap();
    let new_left = fs::read_to_string(current.join("left.maac")).unwrap();
    let new_right = fs::read_to_string(current.join("right.maac")).unwrap();
    let new_entry = fs::read_to_string(current.join("main.maac")).unwrap();
    let common_hash = sha256_digest(new_common.as_bytes());
    assert_eq!(new_left.matches(&common_hash).count(), 2);
    assert_eq!(new_right.matches(&common_hash).count(), 1);
    assert!(new_entry.contains(&sha256_digest(new_left.as_bytes())));
    assert!(new_entry.contains(&sha256_digest(new_right.as_bytes())));
    let left_report = result["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["source"] == "left.maac")
        .unwrap();
    assert_eq!(left_report["final_pin"], sha256_digest(new_left.as_bytes()));
    assert_ne!(left_report["pin"], left_report["final_pin"]);
}

#[test]
fn graph_source_mode_rejects_duplicate_and_existing_output() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let base = temp.path().join("base");
    let output = temp.path().join("output");
    let patch = temp.path().join("common.json");
    let (_, left, _) = project(&source, false);
    fs::write(&patch, duration_patch(COMMON, "pulse", "1/4")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--output-dir"),
        &base,
    ]));
    let spec = format!("common.maac={}", patch.display());
    let duplicate = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        &base,
        Path::new("--source"),
        Path::new(&spec),
        Path::new("--source"),
        Path::new(&spec),
        Path::new("--output-dir"),
        &output,
    ]));
    assert_eq!(duplicate["code"], "E_USAGE");
    assert!(!output.exists());
    fs::create_dir(&output).unwrap();
    fs::write(output.join("keep"), b"untouched").unwrap();
    failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        &base,
        Path::new("--source"),
        Path::new(&spec),
        Path::new("--output-dir"),
        &output,
    ]));
    assert_eq!(fs::read(output.join("keep")).unwrap(), b"untouched");

    let invalid = temp.path().join("invalid.json");
    let changed_pin = Transaction::new(
        SourceDocument::parse(&left).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec!["common".into()],
            field: vec!["hash".into()],
            value: json!({"t":"string","v":sha256_digest(b"unrelated")}),
            expect: Some(json!({"t":"string","v":sha256_digest(COMMON.as_bytes())})),
            expect_absent: false,
        }],
    )
    .unwrap();
    fs::write(&invalid, changed_pin.to_json().unwrap()).unwrap();
    let invalid_spec = format!("left.maac={}", invalid.display());
    let rejected = temp.path().join("rejected");
    failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        &base,
        Path::new("--source"),
        Path::new(&invalid_spec),
        Path::new("--output-dir"),
        &rejected,
    ]));
    assert!(!rejected.exists());
}

#[test]
fn no_op_graph_edit_keeps_pin_graph_and_version_on_append() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let base = temp.path().join("base");
    let edited = temp.path().join("edited");
    let appended = temp.path().join("appended");
    let patch = temp.path().join("noop.json");
    project(&source, false);
    fs::write(&patch, duration_patch(COMMON, "pulse", "1/2")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--output-dir"),
        &base,
    ]));
    let original = archive_manifest(&base);
    let spec = format!("common.maac={}", patch.display());
    let result = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        &base,
        Path::new("--source"),
        Path::new(&spec),
        Path::new("--output-dir"),
        &edited,
    ]));
    assert_eq!(result["format"], "maac.editable-archive/11");
    assert!(result["generated"].as_array().unwrap().is_empty());
    let journaled = archive_manifest(&edited);
    assert_ne!(journaled["head"], original["head"]);
    assert_eq!(journaled["checkpoints"].as_array().unwrap().len(), 2);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--previous"),
        &edited,
        Path::new("--output-dir"),
        &appended,
    ]));
    assert_eq!(archive_manifest(&appended)["version"], 11);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &appended,
    ]));
}

#[test]
fn source_mode_promotes_v10_history_without_changing_prior_checkpoints() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let base = temp.path().join("base");
    let grouped = temp.path().join("grouped");
    let graph = temp.path().join("graph");
    let unpacked = temp.path().join("unpacked");
    let first_patch = temp.path().join("first.json");
    let second_patch = temp.path().join("second.json");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("common.maac"), COMMON).unwrap();
    let entry = format!(
        "maac 1;\nproject p {{ score=[0q,1q]; tail=0s; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}\n\
         tempo t {{ points=[(0q,120bpm,step)]; }}\n\
         meter m {{ points=[(0q,4,4)]; }}\n\
         import common {{ path=\"common.maac\"; hash=\"{}\"; }}\n\
         node s {{ type=\"core.sine/1\"; }}\n\
         track notes {{ target=&s:events; }}\n\
         place p1 {{ pattern=&common.pulse; track=&notes; at=0q; }}\n",
        sha256_digest(COMMON.as_bytes()),
    );
    fs::write(source.join("main.maac"), entry).unwrap();
    fs::write(&first_patch, duration_patch(COMMON, "pulse", "1/4")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--output-dir"),
        &base,
    ]));
    let first_spec = format!("common={}", first_patch.display());
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        &base,
        Path::new("--import"),
        Path::new(&first_spec),
        Path::new("--output-dir"),
        &grouped,
    ]));
    let before = archive_manifest(&grouped);
    assert_eq!(before["version"], 10);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &grouped,
        Path::new("--output-dir"),
        &unpacked,
    ]));
    let current = fs::read_to_string(unpacked.join("common.maac")).unwrap();
    let second = Transaction::new(
        SourceDocument::parse(&current)
            .unwrap()
            .revision()
            .to_owned(),
        vec![Operation::Set {
            object: vec!["pulse".into(), "n".into()],
            field: vec!["dur".into()],
            value: json!({"t":"quantity","n":"1","d":"8","u":"q"}),
            expect: Some(json!({"t":"quantity","n":"1","d":"4","u":"q"})),
            expect_absent: false,
        }],
    )
    .unwrap();
    fs::write(&second_patch, second.to_json().unwrap()).unwrap();
    let second_spec = format!("common.maac={}", second_patch.display());
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        &grouped,
        Path::new("--source"),
        Path::new(&second_spec),
        Path::new("--output-dir"),
        &graph,
    ]));
    let after = archive_manifest(&graph);
    assert_eq!(after["version"], 11);
    assert_eq!(
        &after["checkpoints"].as_array().unwrap()[..2],
        before["checkpoints"].as_array().unwrap()
    );
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &graph,
    ]));
}
