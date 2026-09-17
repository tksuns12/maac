use std::fs;

use clap::Parser;
use maac::cli::{execute_artifact, Cli, Command};
use maac::editing::{Operation, SourceDocument, Transaction};
use serde_json::json;

const SOURCE: &str = r#"maac 1;
// preserve cli comment
project p { score=[0q,4q]; rate=48000Hz; tempo=&clock; meter=&metre; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
"#;

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

fn patch_bytes(source: &str, tail: i64) -> Vec<u8> {
    let document = SourceDocument::parse(source).unwrap();
    Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: path(&["p"]),
            field: path(&["tail"]),
            value: json!({"t":"quantity","n":tail.to_string(),"d":"1","u":"s"}),
            expect: Some(json!({"t":"quantity","n":"0","d":"1","u":"s"})),
            expect_absent: false,
        }],
    )
    .unwrap()
    .to_json()
    .unwrap()
}
#[test]
fn patch_command_writes_source_and_returns_full_edit_result() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.maac");
    let patch = root.path().join("patch.json");
    let output = root.path().join("output.maac");
    fs::write(&input, SOURCE).unwrap();
    fs::write(&patch, patch_bytes(SOURCE, 1)).unwrap();

    let result = execute_artifact(&Command::Patch {
        input: input.clone(),
        patch,
        output: output.clone(),
        force: false,
        project_root: None,
    })
    .unwrap();
    let edited = fs::read_to_string(output).unwrap();
    assert!(edited.contains("// preserve cli comment"));
    assert!(edited.contains("tail = 1s;"));
    assert!(result
        .base()
        .digest
        .as_deref()
        .unwrap()
        .starts_with("sha256:"));
    let edit = result.edit().expect("patch result");
    assert_eq!(edit.new_revision, result.base().digest.as_deref().unwrap());
    assert!(!edit.inverse.operations.is_empty());
    assert!(edit.impact.full_render_invalidated);
    let wire = serde_json::to_value(&result).unwrap();
    assert_eq!(wire["edit"]["new_revision"], edit.new_revision);
    assert!(wire["edit"]["inverse"]["operations"].is_array());
}
#[test]
fn patch_command_preserves_existing_output_without_force() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.maac");
    let patch = root.path().join("patch.json");
    let output = root.path().join("output.maac");
    fs::write(&input, SOURCE).unwrap();
    fs::write(&patch, patch_bytes(SOURCE, 1)).unwrap();
    fs::write(&output, "owner").unwrap();

    let error = execute_artifact(&Command::Patch {
        input,
        patch,
        output: output.clone(),
        force: false,
        project_root: None,
    })
    .unwrap_err();
    assert_eq!(error.code, "E_OUTPUT_EXISTS");
    assert_eq!(fs::read_to_string(output).unwrap(), "owner");
}

#[test]
fn stale_patch_never_publishes_output() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.maac");
    let patch = root.path().join("patch.json");
    let output = root.path().join("output.maac");
    fs::write(&input, SOURCE.replace("4q", "8q")).unwrap();
    fs::write(&patch, patch_bytes(SOURCE, 1)).unwrap();

    let error = execute_artifact(&Command::Patch {
        input,
        patch,
        output: output.clone(),
        force: false,
        project_root: None,
    })
    .unwrap_err();
    assert_eq!(error.code, "E_CONFLICT");
    assert!(!output.exists());
}

#[test]
fn clap_parses_patch_subcommand_and_required_output() {
    let cli = Cli::try_parse_from([
        "maac",
        "patch",
        "song.maac",
        "edit.json",
        "-o",
        "edited.maac",
        "--force",
    ])
    .unwrap();
    match cli.command {
        Command::Patch {
            input,
            patch,
            output,
            force,
            project_root,
        } => {
            assert_eq!(input.to_string_lossy(), "song.maac");
            assert_eq!(patch.to_string_lossy(), "edit.json");
            assert_eq!(output.to_string_lossy(), "edited.maac");
            assert!(force);
            assert!(project_root.is_none());
        }
        _ => panic!("expected patch command"),
    }
    assert!(Cli::try_parse_from(["maac", "patch", "song.maac", "edit.json"]).is_err());
}

#[test]
fn patch_command_resolves_hash_pinned_imports_under_project_root() {
    let root = tempfile::tempdir().unwrap();
    let library = r#"maac 1;
library sounds { version="1"; }
pattern riff { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }
"#;
    let pin = maac::bundle::sha256_digest(library.as_bytes());
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,2q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
import sounds {{ path="sounds.maac"; hash="{pin}"; }}
node s {{ type="core.sine/1"; }}
track notes {{ target=&s:events; }}
place play {{ pattern=&sounds.riff; track=&notes; at=0q; }}
"#
    );
    let input = root.path().join("main.maac");
    let library_path = root.path().join("sounds.maac");
    let patch = root.path().join("patch.json");
    let output = root.path().join("edited.maac");
    fs::write(&input, &source).unwrap();
    fs::write(&library_path, library).unwrap();
    let document = SourceDocument::parse(source.clone()).unwrap();
    let tx = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: path(&["play"]),
            field: path(&["count"]),
            value: json!({"t":"number","n":"2","d":"1"}),
            expect: Some(json!({"t":"number","n":"1","d":"1"})),
            expect_absent: false,
        }],
    )
    .unwrap();
    fs::write(&patch, tx.to_json().unwrap()).unwrap();

    execute_artifact(&Command::Patch {
        input,
        patch,
        output: output.clone(),
        force: false,
        project_root: Some(root.path().to_path_buf()),
    })
    .unwrap();
    assert!(fs::read_to_string(output).unwrap().contains("count = 2;"));
}
