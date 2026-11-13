//! `maac patch --check-compile`: a patch that leaves a document which does
//! not compile is refused with the compile's first error, and nothing is
//! written. Without the flag the same patch applies.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::{json, Value};

const SOURCE: &str = r#"maac 1;
project p { score = [0q, 16q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &synth:out; tail = 100ms; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node synth { type = "core.sine/1"; params = { attack = 5ms; release = 50ms; level = 0.05; }; }
track t { target = &synth:events; }
pattern bar { length = 4q; note a { at = 0q; dur = 1q; pitch = C4; } }
place main { pattern = &bar; track = &t; at = 0q; }
"#;

fn invoke(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(cwd)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}

fn value(output: &Output) -> Value {
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn success(cwd: &Path, args: &[&str]) -> Value {
    let output = invoke(cwd, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    value(&output)
}

fn failure(cwd: &Path, args: &[&str]) -> Value {
    let output = invoke(cwd, args);
    assert!(!output.status.success(), "{args:?}: {output:?}");
    value(&output)
}

/// Write `main.maac` and a patch `edit.json` that sets the onset offset of
/// note `a` to `offset_ms`.
fn project(root: &Path, offset_ms: i64) {
    fs::write(root.join("main.maac"), SOURCE).unwrap();
    let document = SourceDocument::parse(SOURCE).unwrap();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["bar".into(), "a".into()],
            field: vec!["onset_offset".into()],
            value: json!({"t":"quantity","n":offset_ms.to_string(),"d":"1","u":"ms"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    fs::write(root.join("edit.json"), transaction.to_json().unwrap()).unwrap();
}

#[test]
fn a_patch_that_leaves_a_document_which_does_not_compile_is_refused_with_the_flag() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // The offset moves the first onset before the score.
    project(root, -10);

    success(
        root,
        &["patch", "main.maac", "edit.json", "-o", "plain.maac"],
    );
    let compile = failure(root, &["check", "plain.maac"]);
    assert_eq!(compile["code"], "E_INTERVAL");

    let refused = failure(
        root,
        &[
            "patch",
            "main.maac",
            "edit.json",
            "-o",
            "checked.maac",
            "--check-compile",
        ],
    );
    assert_eq!(refused["code"], compile["code"]);
    assert_eq!(refused["message"], compile["message"]);
    assert!(!root.join("checked.maac").exists());
}

#[test]
fn a_patch_that_leaves_a_document_which_compiles_applies_with_the_flag() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    project(root, 10);

    let plain = success(
        root,
        &["patch", "main.maac", "edit.json", "-o", "plain.maac"],
    );
    let checked = success(
        root,
        &[
            "patch",
            "main.maac",
            "edit.json",
            "-o",
            "checked.maac",
            "--check-compile",
        ],
    );
    assert_eq!(
        fs::read(root.join("checked.maac")).unwrap(),
        fs::read(root.join("plain.maac")).unwrap()
    );
    // The same result but for the path each wrote.
    assert_eq!(checked["command"], "patch");
    assert_eq!(checked["digest"], plain["digest"]);
    assert_eq!(checked["edit"], plain["edit"]);
    success(root, &["check", "checked.maac"]);
}

#[test]
fn the_flag_belongs_to_patch_and_lets_it_take_a_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    project(root, 10);

    let other = failure(root, &["check", "main.maac", "--check-compile"]);
    assert_eq!(other["code"], "E_USAGE");

    // A patch compiles nothing without the flag, so it takes no profile.
    let unused = failure(
        root,
        &[
            "patch",
            "main.maac",
            "edit.json",
            "-o",
            "unused.maac",
            "--profile",
            "song",
        ],
    );
    assert_eq!(unused["code"], "E_USAGE");
    assert!(!root.join("unused.maac").exists());

    success(
        root,
        &[
            "patch",
            "main.maac",
            "edit.json",
            "-o",
            "song.maac",
            "--check-compile",
            "--profile",
            "song",
        ],
    );
    assert!(root.join("song.maac").exists());
}

/// A disk-media patch compiles what it leaves already; the flag adds nothing
/// to it and is accepted.
#[test]
fn a_disk_media_patch_accepts_the_flag() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    project(root, -10);
    for flags in [
        vec!["--disk-media"],
        vec!["--disk-media", "--check-compile"],
    ] {
        let mut args = vec!["patch", "main.maac", "edit.json", "-o", "out.maac"];
        args.extend(&flags);
        let refused = failure(root, &args);
        assert_eq!(refused["code"], "E_INTERVAL", "{flags:?}");
        assert!(!root.join("out.maac").exists(), "{flags:?}");
    }
}
