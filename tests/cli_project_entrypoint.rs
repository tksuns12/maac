use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

const SOURCE: &str = r#"maac 1;
project p { score = [0q, 1/50q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &tone:out; tail = 0s; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node tone { type = "core.sine/1"; config = { voices = 2; }; params = { attack = 0s; release = 0s; level = 0.2; }; }
pattern phrase { length = 1/50q; note n { at = 0q; dur = 1/100q; pitch = A4; velocity = 0.5; } }
track t { target = &tone:events; }
place play { pattern = &phrase; track = &t; at = 0q; }
"#;

fn invoke(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(cwd)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}
fn json(output: &Output) -> Value {
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}
fn ok(cwd: &Path, args: &[&str]) -> Value {
    let output = invoke(cwd, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    let value = json(&output);
    assert_eq!(value["ok"], true);
    value
}
fn error(cwd: &Path, args: &[&str], code: &str) {
    let output = invoke(cwd, args);
    assert!(!output.status.success(), "{args:?}: {output:?}");
    assert_eq!(json(&output)["code"], code, "{args:?}");
}

#[test]
fn omitted_input_uses_main_for_check_compile_and_build_without_changing_bytes() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("main.maac"), SOURCE).unwrap();
    assert_eq!(ok(root, &["check"])["input"], "main.maac");
    ok(root, &["compile", "-o", "implicit.json"]);
    ok(root, &["compile", "main.maac", "-o", "explicit.json"]);
    assert_eq!(
        fs::read(root.join("implicit.json")).unwrap(),
        fs::read(root.join("explicit.json")).unwrap()
    );
    ok(root, &["build", "-o", "implicit.wav"]);
    ok(root, &["build", "main.maac", "-o", "explicit.wav"]);
    assert_eq!(
        fs::read(root.join("implicit.wav")).unwrap(),
        fs::read(root.join("explicit.wav")).unwrap()
    );
}

#[test]
fn directory_inputs_select_their_main_and_keep_relative_outputs_in_cwd() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let project = root.join("song with spaces");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.maac"), SOURCE).unwrap();
    let expected = project.join("main.maac");
    assert_eq!(
        ok(root, &["check", project.to_str().unwrap()])["input"],
        expected.to_str().unwrap()
    );
    assert_eq!(
        ok(root, &["check", "song with spaces"])["input"],
        "song with spaces/main.maac"
    );
    ok(root, &["compile", "song with spaces", "-o", "song.json"]);
    ok(root, &["build", "song with spaces", "-o", "song.wav"]);
    assert!(root.join("song.json").is_file() && root.join("song.wav").is_file());
    assert!(!project.join("song.json").exists() && !project.join("song.wav").exists());
    ok(&project, &["check", "."]);
}

#[test]
fn no_main_does_not_search_neighbors_ancestors_or_descendants() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("main.maac"), SOURCE).unwrap();
    let child = root.join("child");
    fs::create_dir_all(child.join("nested")).unwrap();
    fs::write(child.join("other.maac"), SOURCE).unwrap();
    fs::write(child.join("nested/main.maac"), SOURCE).unwrap();
    error(&child, &["check"], "E_IO");
    error(root, &["check", "child"], "E_IO");
    error(&child, &["compile", "-o", "missing.json"], "E_IO");
    assert!(!child.join("missing.json").exists());
}

#[test]
fn explicit_files_and_library_checks_keep_their_existing_contract() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("main.maac"), "not source").unwrap();
    fs::write(root.join("alternative.maac"), SOURCE).unwrap();
    ok(root, &["check", "alternative.maac"]);
    ok(
        root,
        &["compile", "alternative.maac", "-o", "alternative.json"],
    );
    fs::write(
        root.join("main.maac"),
        "maac 1; library main { version = \"1\"; }",
    )
    .unwrap();
    assert_eq!(ok(root, &["check"])["exports"], 0);
    error(root, &["compile", "-o", "library.json"], "E_CONFLICT");
    error(root, &["build", ".", "-o", "library.wav"], "E_CONFLICT");
    assert!(!root.join("library.json").exists() && !root.join("library.wav").exists());
}

#[test]
fn required_outputs_hash_and_render_do_not_gain_entrypoint_discovery() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("main.maac"), SOURCE).unwrap();
    for args in [
        vec!["compile"],
        vec!["build"],
        vec!["render"],
        vec!["hash"],
        vec!["render", "missing.json"],
    ] {
        error(root, &args, "E_USAGE");
    }
    error(root, &["hash", "."], "E_IO");
    error(root, &["render", ".", "-o", "bad.wav"], "E_IO");
    assert!(!root.join("bad.wav").exists());
    fs::write(root.join("protected.wav"), b"existing output").unwrap();
    error(root, &["build", "-o", "protected.wav"], "E_OUTPUT_EXISTS");
    fs::remove_file(root.join("main.maac")).unwrap();
    error(root, &["build", "--force", "-o", "protected.wav"], "E_IO");
    assert_eq!(
        fs::read(root.join("protected.wav")).unwrap(),
        b"existing output"
    );
    fs::create_dir(root.join("main.maac")).unwrap();
    error(root, &["check"], "E_REFERENCE");
}

#[test]
fn explicit_project_root_changes_containment_without_reselecting_main() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("child")).unwrap();
    fs::write(root.join("main.maac"), SOURCE).unwrap();
    fs::write(root.join("child/main.maac"), SOURCE).unwrap();
    error(root, &["check", "--project-root", "child"], "E_REFERENCE");
    ok(root, &["check", "child", "--project-root", "."]);
    assert_eq!(
        ok(&root.join("child"), &["check", "--project-root", ".."])["input"],
        "main.maac"
    );
}

#[cfg(unix)]
#[test]
fn default_and_directory_symlink_escape_is_rejected_but_explicit_file_behavior_is_preserved() {
    use std::os::unix::fs::symlink;
    let temp = tempdir().unwrap();
    let root = temp.path();
    let project = root.join("project");
    fs::create_dir_all(project.join("narrow")).unwrap();
    fs::write(root.join("outside.maac"), SOURCE).unwrap();
    symlink("../outside.maac", project.join("main.maac")).unwrap();
    error(&project, &["check"], "E_REFERENCE");
    error(&project, &["compile", "-o", "escaped.json"], "E_REFERENCE");
    error(
        &project,
        &["build", ".", "-o", "escaped.wav"],
        "E_REFERENCE",
    );
    error(root, &["check", "project"], "E_REFERENCE");
    assert!(!project.join("escaped.json").exists() && !project.join("escaped.wav").exists());
    ok(&project, &["check", "main.maac"]); // Historical canonical-file-parent root.
    ok(&project, &["check", "--project-root", ".."]);
    error(
        &project,
        &["check", "--project-root", "narrow"],
        "E_REFERENCE",
    );
}

#[cfg(unix)]
#[test]
fn contained_main_and_directory_symlinks_preserve_canonical_target_import_resolution() {
    use std::os::unix::fs::symlink;
    let temp = tempdir().unwrap();
    let root = temp.path();
    let project = root.join("project");
    fs::create_dir_all(project.join("nested")).unwrap();
    let leaf = "maac 1; library leaf { version = \"1\"; }";
    let source = format!(
        "{SOURCE}\nimport leaf {{ path = \"leaf.maac\"; hash = \"{}\"; }}",
        maac::bundle::sha256_digest(leaf.as_bytes())
    );
    fs::write(project.join("nested/song.maac"), source).unwrap();
    fs::write(project.join("nested/leaf.maac"), leaf).unwrap();
    fs::write(project.join("leaf.maac"), "wrong neighboring source").unwrap();
    symlink("nested/song.maac", project.join("main.maac")).unwrap();
    symlink("project", root.join("project-link")).unwrap();
    ok(&project, &["check"]);
    ok(root, &["check", "project-link"]);
    ok(&project, &["compile", "-o", "plan.json"]);
    let plan: Value =
        serde_json::from_slice(&fs::read(project.join("plan.json")).unwrap()).unwrap();
    assert_eq!(plan["instruments"]["entry_source"], "nested/song.maac");
    assert!(plan["instruments"]["source_files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "nested/leaf.maac"));
}
