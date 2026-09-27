use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::bundle::sha256_digest;
use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};
use tempfile::tempdir;

const A: &str = "maac 1;\nlibrary alpha { version=\"1\"; }\npattern part_a { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }\n";
const B: &str = "maac 1;\nlibrary beta { version=\"1\"; }\npattern part_b { length=1q; note n { at=0q; dur=1/2q; pitch=E4; } }\n";

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

fn project(root: &Path) -> String {
    fs::create_dir(root).unwrap();
    fs::write(root.join("alpha.maac"), A).unwrap();
    fs::write(root.join("beta.maac"), B).unwrap();
    let entry = format!(
        "maac 1;\nproject p {{ score=[0q,2q]; tail=0s; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}\n\
         tempo t {{ points=[(0q,120bpm,step)]; }}\n\
         meter m {{ points=[(0q,4,4)]; }}\n\
         import alpha {{ path=\"alpha.maac\"; hash=\"{}\"; }}\n\
         import beta {{ path=\"beta.maac\"; hash=\"{}\"; }}\n\
         node s {{ type=\"core.sine/1\"; }}\n\
         track notes {{ target=&s:events; }}\n\
         place first {{ pattern=&alpha.part_a; track=&notes; at=0q; }}\n\
         place second {{ pattern=&beta.part_b; track=&notes; at=1q; }}\n",
        sha256_digest(A.as_bytes()),
        sha256_digest(B.as_bytes()),
    );
    fs::write(root.join("main.maac"), &entry).unwrap();
    entry
}

fn library_patch(source: &str, duration: &str) -> Vec<u8> {
    let name = if source == A { "part_a" } else { "part_b" };
    let (n, d) = duration.split_once('/').unwrap();
    Transaction::new(
        SourceDocument::parse(source).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec![name.into(), "n".into()],
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

fn entry_patch(source: &str) -> Vec<u8> {
    Transaction::new(
        SourceDocument::parse(source).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec!["p".into()],
            field: vec!["tail".into()],
            value: json!({"t":"quantity","n":"1","d":"100","u":"s"}),
            expect: Some(json!({"t":"quantity","n":"0","d":"1","u":"s"})),
            expect_absent: false,
        }],
    )
    .unwrap()
    .to_json()
    .unwrap()
}

fn no_op_library_patch(source: &str) -> Vec<u8> {
    let name = if source == A { "part_a" } else { "part_b" };
    let half = json!({"t":"quantity","n":"1","d":"2","u":"q"});
    Transaction::new(
        SourceDocument::parse(source).unwrap().revision().to_owned(),
        vec![Operation::Set {
            object: vec![name.into(), "n".into()],
            field: vec!["dur".into()],
            value: half.clone(),
            expect: Some(half),
            expect_absent: false,
        }],
    )
    .unwrap()
    .to_json()
    .unwrap()
}

fn create(source: &Path, output: &Path) -> Value {
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        source,
        Path::new("--output-dir"),
        output,
    ]))
}

fn patch_group(
    archive: &Path,
    a: &Path,
    b: &Path,
    entry: &Path,
    output: &Path,
    reverse: bool,
) -> Output {
    let a_spec = format!("alpha={}", a.display());
    let b_spec = format!("beta={}", b.display());
    let (first, second) = if reverse {
        (&b_spec, &a_spec)
    } else {
        (&a_spec, &b_spec)
    };
    invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        archive,
        Path::new("--import"),
        Path::new(first),
        Path::new("--import"),
        Path::new(second),
        Path::new("--entry-patch"),
        entry,
        Path::new("--output-dir"),
        output,
    ])
}

fn manifest(archive: &Path) -> Value {
    serde_json::from_slice(&fs::read(archive.join("maac-archive.json")).unwrap()).unwrap()
}

#[test]
fn two_libraries_and_entry_edit_reopen_after_relocation() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let base = temp.path().join("base");
    let edited = temp.path().join("edited");
    let reverse = temp.path().join("reverse");
    let moved = temp.path().join("moved");
    let prior = temp.path().join("prior");
    let current = temp.path().join("current");
    let a_patch = temp.path().join("alpha.json");
    let b_patch = temp.path().join("beta.json");
    let main_patch = temp.path().join("entry.json");
    let original_entry = project(&source);
    fs::write(&a_patch, library_patch(A, "1/4")).unwrap();
    fs::write(&b_patch, library_patch(B, "1/8")).unwrap();
    fs::write(&main_patch, entry_patch(&original_entry)).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &source,
        Path::new("--freeze-output"),
        Path::new("--output-dir"),
        &base,
    ]));
    let original_id = manifest(&base)["head"].as_str().unwrap().to_owned();
    let result = success(patch_group(
        &base,
        &a_patch,
        &b_patch,
        &main_patch,
        &edited,
        true,
    ));
    assert_eq!(result["format"], "maac.editable-archive/10");
    assert_eq!(result["imports"].as_array().unwrap().len(), 2);
    assert_eq!(result["imports"][0]["alias"], "alpha");
    assert_eq!(result["imports"][1]["alias"], "beta");
    assert!(result["entry_edit"].is_object());
    let ordered = success(patch_group(
        &base,
        &a_patch,
        &b_patch,
        &main_patch,
        &reverse,
        false,
    ));
    assert_eq!(ordered["digest"], result["digest"]);
    fs::rename(&edited, &moved).unwrap();
    fs::remove_dir_all(&source).unwrap();
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
    assert_eq!(
        fs::read_to_string(prior.join("main.maac")).unwrap(),
        original_entry
    );
    assert_eq!(fs::read_to_string(prior.join("alpha.maac")).unwrap(), A);
    assert_eq!(fs::read_to_string(prior.join("beta.maac")).unwrap(), B);
    let historical_wav = temp.path().join("historical.wav");
    let historical = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &moved,
        Path::new("--revision"),
        Path::new(&original_id),
        Path::new("--source"),
        &prior,
        Path::new("-o"),
        &historical_wav,
    ]));
    assert_eq!(historical["reused"], true);
    let stale_wav = temp.path().join("stale.wav");
    let stale = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("freeze-render"),
        &moved,
        Path::new("--revision"),
        Path::new(&original_id),
        Path::new("--source"),
        &current,
        Path::new("-o"),
        &stale_wav,
    ]));
    assert_eq!(stale["code"], "E_FREEZE_STALE");
    assert!(!stale_wav.exists());
    let current_entry = fs::read_to_string(current.join("main.maac")).unwrap();
    assert!(current_entry.contains(&sha256_digest(
        &fs::read(current.join("alpha.maac")).unwrap()
    )));
    assert!(current_entry.contains(&sha256_digest(
        &fs::read(current.join("beta.maac")).unwrap()
    )));
    assert!(current_entry.contains("tail=1/100s"));
    let wav = temp.path().join("current.wav");
    success(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &current,
        Path::new("-o"),
        &wav,
    ]));
    assert!(wav.exists());
    fs::write(
        current.join("main.maac"),
        current_entry.replace("tail=1/100s", "tail=1/50s"),
    )
    .unwrap();
    let continued = temp.path().join("continued");
    let appended = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("create"),
        &current,
        Path::new("--previous"),
        &moved,
        Path::new("--output-dir"),
        &continued,
    ]));
    assert_eq!(appended["format"], "maac.editable-archive/10");
    success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &continued,
    ]));
}

#[test]
fn invalid_group_never_publishes_and_existing_output_is_preserved() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let base = temp.path().join("base");
    let output = temp.path().join("output");
    let a_patch = temp.path().join("alpha.json");
    let b_patch = temp.path().join("beta.json");
    let main_patch = temp.path().join("entry.json");
    let original_entry = project(&source);
    fs::write(&a_patch, library_patch(A, "1/4")).unwrap();
    fs::write(&b_patch, library_patch(B, "1/8")).unwrap();
    fs::write(&main_patch, entry_patch(&original_entry)).unwrap();
    create(&source, &base);
    let duplicate = format!("alpha={}", a_patch.display());
    let failed = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        &base,
        Path::new("--import"),
        Path::new(&duplicate),
        Path::new("--import"),
        Path::new(&duplicate),
        Path::new("--output-dir"),
        &output,
    ]));
    assert_eq!(failed["ok"], false);
    assert!(!output.exists());
    fs::create_dir(&output).unwrap();
    fs::write(output.join("sentinel"), b"keep").unwrap();
    let no_clobber = failure(patch_group(
        &base,
        &a_patch,
        &b_patch,
        &main_patch,
        &output,
        false,
    ));
    assert_eq!(no_clobber["code"], "E_OUTPUT_EXISTS");
    assert_eq!(fs::read(output.join("sentinel")).unwrap(), b"keep");
    let valid = temp.path().join("valid");
    let result = success(patch_group(
        &base,
        &a_patch,
        &b_patch,
        &main_patch,
        &valid,
        false,
    ));
    let edit_digest = result["edit_digest"].as_str().unwrap();
    fs::write(
        valid
            .join("edits")
            .join(&edit_digest[7..])
            .join("pin-forward.json"),
        b"tampered",
    )
    .unwrap();
    let rejected = failure(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("verify"),
        &valid,
    ]));
    assert_eq!(rejected["ok"], false);
}

#[test]
fn authored_noop_group_still_records_one_checkpoint() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    let base = temp.path().join("base");
    let edited = temp.path().join("edited");
    let unpacked = temp.path().join("unpacked");
    let a_patch = temp.path().join("alpha.json");
    let b_patch = temp.path().join("beta.json");
    let entry = project(&source);
    fs::write(&a_patch, no_op_library_patch(A)).unwrap();
    fs::write(&b_patch, no_op_library_patch(B)).unwrap();
    create(&source, &base);
    let prior = manifest(&base)["head"].as_str().unwrap().to_owned();
    let a_spec = format!("alpha={}", a_patch.display());
    let b_spec = format!("beta={}", b_patch.display());
    let result = success(invoke(&[
        Path::new("--json"),
        Path::new("archive"),
        Path::new("patch-group"),
        &base,
        Path::new("--import"),
        Path::new(&a_spec),
        Path::new("--import"),
        Path::new(&b_spec),
        Path::new("--output-dir"),
        &edited,
    ]));
    assert_eq!(result["format"], "maac.editable-archive/10");
    assert_ne!(manifest(&edited)["head"], prior);
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
    assert_eq!(
        fs::read_to_string(unpacked.join("main.maac")).unwrap(),
        entry
    );
    assert_eq!(fs::read_to_string(unpacked.join("alpha.maac")).unwrap(), A);
    assert_eq!(fs::read_to_string(unpacked.join("beta.maac")).unwrap(), B);
}
