use std::fs;

use clap::Parser;
use maac::bundle::{sha256_digest, SourceBundle};
use maac::cli::{execute_artifact, format_human_artifact, Cli, Command};
use maac::editing::{BundleEditContext, SourceDocument};

const SOURCE: &str = r#"maac 1;
// preserve materialization CLI source
project p { score=[0q,2q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
node s { type="core.sine/1"; }
track notes { target=&s:events; }
pattern riff { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }
place play { pattern=&riff; track=&notes; at=0q; count=2; }
"#;

#[test]
fn clap_parses_materialize_instance_arguments_and_requires_outputs() {
    let cli = Cli::try_parse_from([
        "maac",
        "materialize-instance",
        "song.maac",
        "play",
        "--pattern",
        "play_copy",
        "-o",
        "edited.maac",
        "--project-root",
        "project",
        "--force",
    ])
    .unwrap();
    match cli.command {
        Command::MaterializeInstance {
            input,
            placement,
            pattern,
            output,
            force,
            project_root,
        } => {
            assert_eq!(input.to_string_lossy(), "song.maac");
            assert_eq!(placement, "play");
            assert_eq!(pattern, "play_copy");
            assert_eq!(output.to_string_lossy(), "edited.maac");
            assert!(force);
            assert_eq!(project_root.unwrap().to_string_lossy(), "project");
        }
        _ => panic!("expected materialize-instance command"),
    }
    assert!(Cli::try_parse_from([
        "maac",
        "materialize-instance",
        "song.maac",
        "play",
        "-o",
        "edited.maac",
    ])
    .is_err());
}

#[test]
fn materialize_instance_writes_source_and_returns_complete_map_and_inverse() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("main.maac");
    let output = root.path().join("edited.maac");
    fs::write(&input, SOURCE).unwrap();

    let original = SourceDocument::parse(SOURCE).unwrap();
    let original_revision = original.revision().to_owned();
    let bundle = SourceBundle::new("main.maac", SOURCE);
    let context = BundleEditContext::new(&bundle).unwrap();
    let result = execute_artifact(&Command::MaterializeInstance {
        input,
        placement: "play".into(),
        pattern: "play_copy".into(),
        output: output.clone(),
        force: false,
        project_root: None,
    })
    .unwrap();

    let edited = fs::read_to_string(&output).unwrap();
    assert!(edited.contains("// preserve materialization CLI source"));
    let edited_document = SourceDocument::parse(edited.clone()).unwrap();
    assert_eq!(
        edited_document.revision(),
        result.base().digest.as_deref().unwrap()
    );
    assert!(edited.contains("pattern play_copy"));

    let edit = result.edit().expect("normal applied transaction result");
    assert_eq!(edit.new_revision, edited_document.revision());
    assert!(!edit.inverse.operations.is_empty());
    assert!(!edit.impact.affected_expanded_event_addresses.is_empty());

    let mapping = result
        .materialization()
        .expect("complete materialization result");
    assert_eq!(mapping.placement, "play");
    assert_eq!(mapping.pattern, "play_copy");
    assert_eq!(
        mapping.mappings.len(),
        2,
        "both repeated note occurrences map"
    );
    assert!(mapping
        .mappings
        .iter()
        .all(|item| item.old_event_address != item.new_event_address));
    assert!(mapping
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "W_IDENTITY_CHANGE"));

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["edit"]["new_revision"], edit.new_revision);
    let serialized_mappings = json["materialization"]["mappings"].as_array().unwrap();
    assert_eq!(serialized_mappings.len(), 2);
    assert!(serialized_mappings.iter().all(|mapping| {
        mapping["old_event_address"].is_string()
            && mapping["new_event_address"].is_string()
            && mapping["old_source_object_path"].is_array()
            && mapping["new_source_object_path"].is_array()
    }));
    assert_eq!(
        json["materialization"]["diagnostics"][0]["code"],
        "W_IDENTITY_CHANGE"
    );
    assert!(format_human_artifact(&result).contains("W_IDENTITY_CHANGE"));

    let mut reverted = SourceDocument::parse(edited).unwrap();
    reverted.apply(&edit.inverse, &context).unwrap();
    assert_eq!(reverted.revision(), original_revision);
}

#[test]
fn materialize_instance_preserves_existing_output_without_force() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("main.maac");
    let output = root.path().join("edited.maac");
    fs::write(&input, SOURCE).unwrap();
    fs::write(&output, "owner bytes").unwrap();

    let error = execute_artifact(&Command::MaterializeInstance {
        input,
        placement: "play".into(),
        pattern: "play_copy".into(),
        output: output.clone(),
        force: false,
        project_root: None,
    })
    .unwrap_err();
    assert_eq!(error.code, "E_OUTPUT_EXISTS");
    assert_eq!(fs::read_to_string(output).unwrap(), "owner bytes");
}

#[test]
fn invalid_materialization_with_force_preserves_existing_output() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("main.maac");
    let output = root.path().join("edited.maac");
    fs::write(&input, SOURCE).unwrap();
    fs::write(&output, "owner bytes").unwrap();

    let error = execute_artifact(&Command::MaterializeInstance {
        input,
        placement: "missing".into(),
        pattern: "play_copy".into(),
        output: output.clone(),
        force: true,
        project_root: None,
    })
    .unwrap_err();
    assert_eq!(error.code, "E_REFERENCE");
    assert_eq!(fs::read_to_string(output).unwrap(), "owner bytes");
}

#[test]
fn invalid_materialization_without_force_does_not_create_output() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("main.maac");
    let output = root.path().join("edited.maac");
    fs::write(&input, SOURCE).unwrap();

    let error = execute_artifact(&Command::MaterializeInstance {
        input,
        placement: "missing".into(),
        pattern: "play_copy".into(),
        output: output.clone(),
        force: false,
        project_root: None,
    })
    .unwrap_err();
    assert_eq!(error.code, "E_REFERENCE");
    assert!(!output.exists());
}

#[test]
fn materialize_instance_resolves_imports_under_explicit_project_root() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let leaf = r#"maac 1;
library notes { version="1"; }
pattern leaf_note { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }
"#;
    let leaf_pin = sha256_digest(leaf.as_bytes());
    let library = format!(
        r#"maac 1;
library sounds {{ version="1"; }}
import notes {{ path="deps/leaf.maac"; hash="{leaf_pin}"; }}
pattern riff {{ length=1q; use nested {{ pattern=&notes.leaf_note; at=0q; }} }}
"#
    );
    let pin = sha256_digest(library.as_bytes());
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,2q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
import sounds {{ path="lib/phrases.maac"; hash="{pin}"; }}
node s {{ type="core.sine/1"; }}
track notes {{ target=&s:events; }}
place play {{ pattern=&sounds.riff; track=&notes; at=0q; }}
"#
    );
    let input = project.join("main.maac");
    let library_path = project.join("lib/phrases.maac");
    let output = project.join("edited.maac");
    let library_dir = project.join("lib");
    let dependency_dir = library_dir.join("deps");
    fs::create_dir_all(&dependency_dir).unwrap();
    fs::write(&input, &source).unwrap();
    fs::write(&library_path, &library).unwrap();
    let leaf_path = dependency_dir.join("leaf.maac");
    fs::write(&leaf_path, leaf).unwrap();

    let result = execute_artifact(&Command::MaterializeInstance {
        input,
        placement: "play".into(),
        pattern: "play_copy".into(),
        output: output.clone(),
        force: false,
        project_root: Some(project.clone()),
    })
    .unwrap();

    let edited = fs::read_to_string(&output).unwrap();
    assert!(edited.contains(&format!("hash=\"{pin}\"")));
    let map = result.materialization().unwrap();
    assert!(!map.mappings.is_empty());
    assert!(map.mappings.iter().any(|mapping| mapping
        .old_source_object_path
        .iter()
        .any(|part| part == "music" || part == "notes")));

    let edited_bundle = maac::bundle_fs::load_bundle(&output, &project).unwrap();
    let _edited_context = BundleEditContext::new(&edited_bundle).unwrap();
    let edited_document =
        SourceDocument::parse(edited_bundle.sources[&edited_bundle.entry].clone()).unwrap();
    assert_eq!(
        edited_document.authored().revision(),
        result.base().digest.as_deref().unwrap()
    );
    assert_eq!(edited_bundle.sources["lib/phrases.maac"], library);
    assert_eq!(edited_bundle.sources["lib/deps/leaf.maac"], leaf);
    assert_eq!(sha256_digest(leaf.as_bytes()), leaf_pin);
}

#[test]
fn document_only_message_materializes_without_performance_rendering() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("message.maac");
    let output = root.path().join("materialized.maac");
    let source = r#"maac 1;
project p { score=[0q,1q]; rate=48000Hz; tempo=&t; meter=&m; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
node s { type="core.sine/1"; }
track events { target=&s:events; }
pattern pat { length=1q; message msg { at=0q; protocol="midi1"; bytes=[144,60,127]; } }
place play { pattern=&pat; track=&events; at=0q; }
"#;
    fs::write(&input, source).unwrap();

    let result = execute_artifact(&Command::MaterializeInstance {
        input,
        placement: "play".into(),
        pattern: "message_copy".into(),
        output: output.clone(),
        force: false,
        project_root: None,
    })
    .unwrap();
    assert!(result
        .materialization()
        .unwrap()
        .mappings
        .iter()
        .any(|mapping| mapping.old_event_address.contains("msg")));
    assert!(fs::read_to_string(output)
        .unwrap()
        .contains("protocol = \"midi1\""));
}
