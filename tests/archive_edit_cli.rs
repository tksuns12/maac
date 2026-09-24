use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const SOURCE: &str = include_str!("fixtures/archive_v1/main.maac");

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

fn create(project: &Path, archive: &Path) -> Value {
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        project,
        Path::new("--output-dir"),
        archive,
    ]))
}

fn patch(archive: &Path, transaction: &Path, output: &Path) -> Output {
    invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch"),
        archive,
        transaction,
        Path::new("--output-dir"),
        output,
    ])
}

fn manifest(archive: &Path) -> Value {
    serde_json::from_slice(&fs::read(archive.join("maac-archive.json")).unwrap()).unwrap()
}

fn set_tail(source: &str) -> Vec<u8> {
    Transaction::new(
        SourceDocument::parse(source).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec!["p".into()],
            field: vec!["tail".into()],
            value: json!({"t":"quantity","n":"1","d":"48000","u":"s"}),
            expect: Some(json!({"t":"quantity","n":"0","d":"1","u":"s"})),
            expect_absent: false,
        }],
    )
    .unwrap()
    .to_json()
    .unwrap()
}

fn set_tail_to_same(revision: String) -> Vec<u8> {
    let zero = json!({"t":"quantity","n":"0","d":"1","u":"s"});
    Transaction::new(
        revision,
        vec![Operation::Set {
            object: vec!["p".into()],
            field: vec!["tail".into()],
            value: zero.clone(),
            expect: Some(zero),
            expect_absent: false,
        }],
    )
    .unwrap()
    .to_json()
    .unwrap()
}

#[test]
fn journaled_edit_relocates_and_reopens_both_snapshots() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let original = temp.path().join("original");
    let edited = temp.path().join("edited");
    let moved = temp.path().join("moved");
    let transaction = temp.path().join("transaction.json");
    let before = temp.path().join("before");
    let after = temp.path().join("after");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.maac"), SOURCE).unwrap();
    fs::write(&transaction, set_tail(SOURCE)).unwrap();

    create(&project, &original);
    let old_id = manifest(&original)["head"].as_str().unwrap().to_owned();
    let created = success(patch(&original, &transaction, &edited));
    assert_eq!(created["format"], "maac.editable-archive/5");
    assert!(created["edit"]["inverse"]["operations"].is_array());
    let edit_id = created["edit_digest"].as_str().unwrap();
    let history = manifest(&edited);
    assert_eq!(history["checkpoints"].as_array().unwrap().len(), 2);
    assert_eq!(history["checkpoints"][0]["id"], old_id);
    assert_eq!(history["head"], created["checkpoint"]);
    assert_eq!(history["checkpoints"][1]["edit"], edit_id);
    let edit_dir = edited.join("edits").join(&edit_id[7..]);
    for name in ["maac-edit.json", "forward.json", "inverse.json"] {
        assert!(edit_dir.join(name).is_file(), "{name}");
    }

    fs::rename(&edited, &moved).unwrap();
    fs::remove_dir_all(&project).unwrap();
    fs::remove_dir_all(&original).unwrap();
    let verified = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &moved,
        Path::new("--expect-hash"),
        Path::new(created["digest"].as_str().unwrap()),
    ]));
    assert_eq!(verified["digest"], created["digest"]);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--revision"),
        Path::new(&old_id),
        Path::new("--output-dir"),
        &before,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &moved,
        Path::new("--output-dir"),
        &after,
    ]));
    assert_eq!(
        fs::read(before.join("main.maac")).unwrap(),
        SOURCE.as_bytes()
    );
    let edited_source = fs::read_to_string(after.join("main.maac")).unwrap();
    assert!(edited_source.contains("tail=1/48000s"));
    assert_eq!(
        created["edit"]["new_revision"],
        SourceDocument::parse(edited_source).unwrap().revision()
    );
}

#[test]
fn no_op_is_recorded_and_failed_patch_never_publishes() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let original = temp.path().join("original");
    let no_op = temp.path().join("no-op");
    let refused = temp.path().join("refused");
    let no_op_file = temp.path().join("no-op.json");
    let stale_file = temp.path().join("stale.json");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.maac"), SOURCE).unwrap();
    create(&project, &original);
    fs::write(
        &no_op_file,
        set_tail_to_same(SourceDocument::parse(SOURCE).unwrap().revision().to_owned()),
    )
    .unwrap();
    let created = success(patch(&original, &no_op_file, &no_op));
    let history = manifest(&no_op);
    assert_eq!(history["checkpoints"].as_array().unwrap().len(), 2);
    assert_eq!(
        history["checkpoints"][0]["snapshot"],
        history["checkpoints"][1]["snapshot"]
    );
    assert_ne!(
        history["checkpoints"][0]["id"],
        history["checkpoints"][1]["id"]
    );
    assert_eq!(history["head"], created["checkpoint"]);
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &no_op,
    ]));

    fs::write(
        &stale_file,
        set_tail_to_same(format!("sha256:{}", "0".repeat(64))),
    )
    .unwrap();
    let failure = rejected(patch(&original, &stale_file, &refused));
    assert_eq!(failure["code"], "E_CONFLICT");
    assert!(!refused.exists());
    assert_eq!(manifest(&original)["version"], 2);
}

#[test]
fn tampered_nonhead_edit_blocks_whole_history() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let original = temp.path().join("original");
    let edited = temp.path().join("edited");
    let later = temp.path().join("later");
    let current = temp.path().join("current");
    let transaction = temp.path().join("transaction.json");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.maac"), SOURCE).unwrap();
    fs::write(&transaction, set_tail(SOURCE)).unwrap();
    create(&project, &original);
    success(patch(&original, &transaction, &edited));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &edited,
        Path::new("--output-dir"),
        &current,
    ]));
    let mut source = fs::read_to_string(current.join("main.maac")).unwrap();
    source.push_str("\n// later checkpoint\n");
    fs::write(current.join("main.maac"), source).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &current,
        Path::new("--previous"),
        &edited,
        Path::new("--output-dir"),
        &later,
    ]));
    let history = manifest(&later);
    assert_eq!(history["checkpoints"].as_array().unwrap().len(), 3);
    assert_eq!(history["version"], 5);
    let edit_id = history["checkpoints"][1]["edit"].as_str().unwrap();
    let inverse = later.join("edits").join(&edit_id[7..]).join("inverse.json");
    let mut bytes = fs::read(&inverse).unwrap();
    bytes[0] ^= 1;
    fs::write(&inverse, bytes).unwrap();
    let failure = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &later,
    ]));
    assert_eq!(failure["code"], "E_HASH");
}

#[test]
fn journaled_edit_accepts_song_profile_with_large_native_pcm() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let original = temp.path().join("original");
    let edited = temp.path().join("edited");
    let transaction = temp.path().join("transaction.json");
    fs::create_dir(&project).unwrap();
    let pcm = vec![0u8; 5_000_004];
    let hash = format!("sha256:{:x}", Sha256::digest(&pcm));
    fs::write(project.join("large.pcm"), &pcm).unwrap();
    let source = format!(
        "maac 1;\n\
         project p {{ score=[0q,1/3000q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&clip:out; }}\n\
         tempo clock {{ points=[(0q,120bpm,step)]; }}\n\
         meter metre {{ points=[(0q,4,4)]; }}\n\
         asset large {{ kind=audio; path=\"large.pcm\"; hash=\"{hash}\"; format=\"pcm_f32le_interleaved/1\"; rate=24000Hz; channels=1; frames=1250001; }}\n\
         audio clip {{ asset=&large; at=0q; source=[1249999frame,1250001frame]; mode=rate; }}\n"
    );
    fs::write(project.join("main.maac"), &source).unwrap();
    fs::write(&transaction, set_tail(&source)).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--profile"),
        Path::new("song"),
        Path::new("--output-dir"),
        &original,
    ]));
    let created = success(patch(&original, &transaction, &edited));
    assert_eq!(created["format"], "maac.editable-archive/5");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &edited,
    ]));
    assert_eq!(
        manifest(&edited)["checkpoints"][1]["edit"],
        created["edit_digest"]
    );
}

#[test]
fn label_only_edit_preserves_old_freeze_but_stales_it_for_new_source() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let frozen = temp.path().join("frozen");
    let edited = temp.path().join("edited");
    let current = temp.path().join("current");
    let transaction = temp.path().join("label.json");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.maac"), SOURCE).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &project,
        Path::new("--freeze-output"),
        Path::new("--output-dir"),
        &frozen,
    ]));
    let old_id = manifest(&frozen)["head"].as_str().unwrap().to_owned();
    let label = Transaction::new(
        SourceDocument::parse(SOURCE).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec!["p".into()],
            field: vec!["label".into()],
            value: json!({"t":"string","v":"display only"}),
            expect: None,
            expect_absent: true,
        }],
    )
    .unwrap();
    fs::write(&transaction, label.to_json().unwrap()).unwrap();
    let created = success(patch(&frozen, &transaction, &edited));
    assert_eq!(
        created["edit"]["impact"]["render_invalidation_scope"],
        "none"
    );
    let history = manifest(&edited);
    assert!(history["checkpoints"][0]["freeze"].is_string());
    assert!(history["checkpoints"][1]["freeze"].is_null());
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &edited,
        Path::new("--output-dir"),
        &current,
    ]));
    let stale = rejected(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-check"),
        &edited,
        Path::new("--source"),
        &current,
        Path::new("--revision"),
        Path::new(&old_id),
    ]));
    assert_eq!(stale["code"], "E_FREEZE_STALE");
}

#[test]
fn journaled_edit_preserves_retained_original_import() {
    let temp = tempdir().unwrap();
    let fixture = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/archive_v3"
    ));
    let source = include_str!("fixtures/archive_v3/checkpoints/bdd2c0dbc3d7360f62b03958f12ebaf691e7e1b0a4f8497e6dc8c1f69b42670e/main.maac");
    let transaction = temp.path().join("label.json");
    let edited = temp.path().join("edited");
    let unpacked = temp.path().join("unpacked");
    let label = Transaction::new(
        SourceDocument::parse(source).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec!["imported_media".into()],
            field: vec!["label".into()],
            value: json!({"t":"string","v":"archived take"}),
            expect: None,
            expect_absent: true,
        }],
    )
    .unwrap();
    fs::write(&transaction, label.to_json().unwrap()).unwrap();
    success(patch(fixture, &transaction, &edited));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &edited,
    ]));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("unpack"),
        &edited,
        Path::new("--output-dir"),
        &unpacked,
    ]));
    let old = fixture
        .join("checkpoints")
        .join("bdd2c0dbc3d7360f62b03958f12ebaf691e7e1b0a4f8497e6dc8c1f69b42670e");
    for name in ["media.pcm", "import.json", "original.wav"] {
        assert_eq!(
            fs::read(unpacked.join(name)).unwrap(),
            fs::read(old.join(name)).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn dependency_changes_and_bad_pins_are_rejected_without_publication() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let original = temp.path().join("original");
    let refused = temp.path().join("refused");
    let transaction = temp.path().join("remove-import.json");
    let bad_pin_file = temp.path().join("bad-pin.json");
    fs::create_dir(&project).unwrap();
    let library = "maac 1;\nlibrary sounds { version=\"1\"; }\n";
    let hash = format!("sha256:{:x}", Sha256::digest(library.as_bytes()));
    let source = format!("{SOURCE}\nimport sounds {{ path=\"sounds.maac\"; hash=\"{hash}\"; }}\n");
    fs::write(project.join("main.maac"), &source).unwrap();
    fs::write(project.join("sounds.maac"), library).unwrap();
    create(&project, &original);
    let revision = SourceDocument::parse(&source)
        .unwrap()
        .revision()
        .to_owned();
    let bad_pin = Transaction::new(
        revision.clone(),
        vec![Operation::Set {
            object: vec!["sounds".into()],
            field: vec!["hash".into()],
            value: json!({"t":"string","v":format!("sha256:{}", "0".repeat(64))}),
            expect: Some(json!({"t":"string","v":hash})),
            expect_absent: false,
        }],
    )
    .unwrap();
    fs::write(&bad_pin_file, bad_pin.to_json().unwrap()).unwrap();
    let bad_pin_error = rejected(patch(&original, &bad_pin_file, &refused));
    assert_eq!(bad_pin_error["code"], "E_HASH");
    assert!(!refused.exists());
    let remove = Transaction::new(
        revision,
        vec![Operation::DeleteObject {
            object: vec!["sounds".into()],
            expect_object: None,
        }],
    )
    .unwrap();
    fs::write(&transaction, remove.to_json().unwrap()).unwrap();
    let error = rejected(patch(&original, &transaction, &refused));
    assert_eq!(error["code"], "E_CAPABILITY");
    assert!(!refused.exists());
}

#[test]
fn inverse_replay_near_source_limit_does_not_expand_comments() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let original = temp.path().join("original");
    let edited = temp.path().join("edited");
    let transaction = temp.path().join("same-tail.json");
    fs::create_dir(&project).unwrap();
    let limit = 4 * 1024 * 1024;
    let mut source = SOURCE.to_owned();
    source.push_str("//");
    source.push_str(&"x".repeat(limit - source.len() - 1));
    source.push('\n');
    assert_eq!(source.len(), limit);
    fs::write(project.join("main.maac"), &source).unwrap();
    fs::write(
        &transaction,
        set_tail_to_same(
            SourceDocument::parse(&source)
                .unwrap()
                .revision()
                .to_owned(),
        ),
    )
    .unwrap();
    create(&project, &original);
    success(patch(&original, &transaction, &edited));
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &edited,
    ]));
}
