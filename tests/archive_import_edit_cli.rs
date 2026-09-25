use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::bundle::sha256_digest;
use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};
use tempfile::tempdir;

const LIBRARY: &str = "maac 1;\n// Keep this exact comment: 🎵 café.\nlibrary sounds { version=\"1\"; }\npattern riff { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }\n";

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

fn project(root: &Path) -> String {
    fs::create_dir(root).unwrap();
    fs::write(root.join("sounds.maac"), LIBRARY).unwrap();
    let entry = format!(
        "maac 1;\nproject p {{ score=[0q,2q]; tail=0s; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}\n\
         tempo t {{ points=[(0q,120bpm,step)]; }}\n\
         meter m {{ points=[(0q,4,4)]; }}\n\
         import sounds {{ path=\"sounds.maac\"; hash=\"{}\"; }}\n\
         node s {{ type=\"core.sine/1\"; }}\n\
         track notes {{ target=&s:events; }}\n\
         place play {{ pattern=&sounds.riff; track=&notes; at=0q; }}\n",
        sha256_digest(LIBRARY.as_bytes())
    );
    fs::write(root.join("main.maac"), &entry).unwrap();
    entry
}

fn patch_bytes() -> Vec<u8> {
    Transaction::new(
        SourceDocument::parse(LIBRARY)
            .unwrap()
            .revision()
            .to_owned(),
        vec![Operation::Set {
            object: vec!["riff".into(), "n".into()],
            field: vec!["dur".into()],
            value: json!({"t":"quantity","n":"1","d":"4","u":"q"}),
            expect: Some(json!({"t":"quantity","n":"1","d":"2","u":"q"})),
            expect_absent: false,
        }],
    )
    .unwrap()
    .to_json()
    .unwrap()
}

fn create(root: &Path, output: &Path) -> Value {
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        root,
        Path::new("--output-dir"),
        output,
    ]))
}

fn edit(archive: &Path, patch: &Path, output: &Path, alias: &str) -> Output {
    invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-import"),
        archive,
        patch,
        Path::new("--import"),
        Path::new(alias),
        Path::new("--output-dir"),
        output,
    ])
}

fn manifest(archive: &Path) -> Value {
    serde_json::from_slice(&fs::read(archive.join("maac-archive.json")).unwrap()).unwrap()
}

#[test]
fn library_edit_history_reuses_only_its_historical_current_freeze() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    let frozen = temp.path().join("frozen");
    let edited = temp.path().join("edited");
    let prior = temp.path().join("prior");
    let current = temp.path().join("current");
    let patch = temp.path().join("patch.json");
    let reused = temp.path().join("reused.wav");
    project(&root);
    fs::write(&patch, patch_bytes()).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &root,
        Path::new("--freeze-output"),
        Path::new("--output-dir"),
        &frozen,
    ]));
    let first = manifest(&frozen)["head"].as_str().unwrap().to_owned();
    let result = success(edit(&frozen, &patch, &edited, "sounds"));
    assert_eq!(result["format"], "maac.editable-archive/7");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &edited,
        Path::new("--revision"),
        Path::new(&first),
        Path::new("--output-dir"),
        &prior,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &edited,
        Path::new("--output-dir"),
        &current,
    ]));
    let historical = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &edited,
        Path::new("--revision"),
        Path::new(&first),
        Path::new("--source"),
        &prior,
        Path::new("-o"),
        &reused,
    ]));
    assert_eq!(historical["reused"], true);
    assert_eq!(historical["revision"], first);
    assert!(reused.exists());
    let stale_output = temp.path().join("stale.wav");
    let stale = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &edited,
        Path::new("--revision"),
        Path::new(&first),
        Path::new("--source"),
        &current,
        Path::new("-o"),
        &stale_output,
    ]));
    assert_eq!(stale["code"], "E_FREEZE_STALE");
    assert!(!stale_output.exists());
    let head_output = temp.path().join("head.wav");
    let unfrozen = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &edited,
        Path::new("--source"),
        &current,
        Path::new("-o"),
        &head_output,
    ]));
    assert_eq!(unfrozen["code"], "E_REFERENCE");
    assert!(!head_output.exists());
}

#[test]
fn library_edit_repins_exact_bytes_and_reopens_both_versions() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    let base = temp.path().join("base");
    let edited = temp.path().join("edited");
    let moved = temp.path().join("moved");
    let prior = temp.path().join("prior");
    let current = temp.path().join("current");
    let before_wav = temp.path().join("before.wav");
    let after_wav = temp.path().join("after.wav");
    let patch = temp.path().join("patch.json");
    let entry = project(&root);
    fs::write(&patch, patch_bytes()).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &root,
        Path::new("-o"),
        &before_wav,
    ]));
    create(&root, &base);
    let original_id = manifest(&base)["head"].as_str().unwrap().to_owned();
    let result = success(edit(&base, &patch, &edited, "sounds"));
    assert_eq!(result["format"], "maac.editable-archive/7");
    assert_eq!(result["import_alias"], "sounds");
    assert_eq!(
        result["edit"]["impact"]["render_invalidation_scope"],
        "full"
    );
    assert_eq!(result["edit"]["impact"]["full_render_invalidated"], true);
    fs::rename(&edited, &moved).unwrap();
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&base).unwrap();
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
    assert_eq!(fs::read_to_string(prior.join("main.maac")).unwrap(), entry);
    assert_eq!(
        fs::read_to_string(prior.join("sounds.maac")).unwrap(),
        LIBRARY
    );
    let changed_library = fs::read(current.join("sounds.maac")).unwrap();
    assert!(
        String::from_utf8_lossy(&changed_library).contains("// Keep this exact comment: 🎵 café.")
    );
    let new_pin = sha256_digest(&changed_library);
    assert_eq!(result["pin"], new_pin);
    assert_eq!(
        fs::read_to_string(current.join("main.maac")).unwrap(),
        entry.replace(&sha256_digest(LIBRARY.as_bytes()), &new_pin)
    );
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &current,
        Path::new("-o"),
        &after_wav,
    ]));
    assert_ne!(fs::read(before_wav).unwrap(), fs::read(after_wav).unwrap());
}

#[test]
fn invalid_alias_and_stale_revision_never_publish() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    let base = temp.path().join("base");
    let bad_alias = temp.path().join("bad-alias");
    let bad_revision = temp.path().join("bad-revision");
    let patch = temp.path().join("patch.json");
    project(&root);
    create(&root, &base);
    fs::write(&patch, patch_bytes()).unwrap();
    assert_eq!(
        failure(edit(&base, &patch, &bad_alias, "missing"))["ok"],
        false
    );
    assert!(!bad_alias.exists());
    let bytes = Transaction::new(
        format!("sha256:{}", "0".repeat(64)),
        vec![Operation::Set {
            object: vec!["riff".into(), "n".into()],
            field: vec!["dur".into()],
            value: json!({"t":"quantity","n":"1","d":"4","u":"q"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap()
    .to_json()
    .unwrap();
    fs::write(&patch, bytes).unwrap();
    assert_eq!(
        failure(edit(&base, &patch, &bad_revision, "sounds"))["code"],
        "E_CONFLICT"
    );
    assert!(!bad_revision.exists());
}

#[test]
fn authored_noop_still_records_a_two_source_transition() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("project");
    let base = temp.path().join("base");
    let edited = temp.path().join("edited");
    let patch = temp.path().join("noop.json");
    project(&root);
    create(&root, &base);
    let before = manifest(&base);
    let half = json!({"t":"quantity","n":"1","d":"2","u":"q"});
    let transaction = Transaction::new(
        SourceDocument::parse(LIBRARY)
            .unwrap()
            .revision()
            .to_owned(),
        vec![Operation::Set {
            object: vec!["riff".into(), "n".into()],
            field: vec!["dur".into()],
            value: half.clone(),
            expect: Some(half),
            expect_absent: false,
        }],
    )
    .unwrap();
    fs::write(&patch, transaction.to_json().unwrap()).unwrap();
    let result = success(edit(&base, &patch, &edited, "sounds"));
    let after = manifest(&edited);
    assert_eq!(after["version"], 7);
    assert_eq!(after["checkpoints"].as_array().unwrap().len(), 2);
    assert_ne!(after["head"], before["head"]);
    assert_eq!(result["pin"], sha256_digest(LIBRARY.as_bytes()));
    let checkpoint = edited
        .join("checkpoints")
        .join(&after["head"].as_str().unwrap()[7..]);
    assert_eq!(
        fs::read_to_string(checkpoint.join("sounds.maac")).unwrap(),
        LIBRARY
    );
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &edited,
    ]));
}
