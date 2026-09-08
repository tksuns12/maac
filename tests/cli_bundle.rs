use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::tempdir;

const ONE_NOTE_SOURCE: &str = r#"maac 1;
project p {
  score = [0q, 1q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &sum:out;
  tail = 0s;
}
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
pattern riff {
  length = 1q;
  note n { at = 0q; dur = 1/2q; pitch = C4; velocity = 1; }
}
track t { target = &sine:events; }
place main { pattern = &riff; track = &t; at = 0q; }
node sine {
  type = "core.sine/1";
  config = { voices = 4; };
  params = { attack = 0s; release = 0s; level = 0.5; };
}
node sum {
  type = "core.sum/1";
  config = { channels = 1; };
}
connect sine_sum { from = &sine:out; to = &sum:in; }
"#;

fn invoke(args: &[&Path]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_maac"));
    for argument in args {
        command.arg(argument);
    }
    command.output().expect("maac process starts")
}

fn invoke_in(current_dir: &Path, args: &[&Path]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_maac"));
    command.current_dir(current_dir);
    for argument in args {
        command.arg(argument);
    }
    command.output().expect("maac process starts")
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("command emits JSON")
}

#[test]
fn hash_reports_the_sha256_of_exact_file_bytes_without_modifying_the_file() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("pin.maac");
    let bytes = b"abc\r\n\0";
    fs::write(&source, bytes).unwrap();

    let human = invoke(&[Path::new("hash"), source.as_path()]);
    assert!(human.status.success(), "hash failed: {:?}", human);
    assert_eq!(
        String::from_utf8(human.stdout).unwrap(),
        "sha256:a6035160a7aa6524aa8195b60b97f9acd1828a59b78d4c14c3bc3f6cc18634a1\n"
    );
    assert!(human.stderr.is_empty());
    assert_eq!(fs::read(&source).unwrap(), bytes);

    let json = invoke(&[Path::new("--json"), Path::new("hash"), source.as_path()]);
    assert!(json.status.success(), "JSON hash failed: {:?}", json);
    let result = json_stdout(&json);
    assert_eq!(result["ok"], true);
    assert_eq!(result["command"], "hash");
    assert_eq!(
        result["digest"],
        "sha256:a6035160a7aa6524aa8195b60b97f9acd1828a59b78d4c14c3bc3f6cc18634a1"
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn check_resolves_transitive_pinned_imports_with_an_explicit_project_root() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("songs")).unwrap();
    fs::create_dir_all(root.join("libraries")).unwrap();
    fs::create_dir_all(root.join("shared")).unwrap();

    let leaf = "maac 1;\nlibrary leaf { version = \"1\"; }\n";
    let middle = format!(
        "maac 1;\nlibrary middle {{ version = \"1\"; }}\nimport leaf {{ path = \"../shared/leaf.maac\"; hash = \"{}\"; }}\n",
        maac::bundle::sha256_digest(leaf.as_bytes())
    );
    let entry = format!(
        "{ONE_NOTE_SOURCE}\nimport sounds {{ path = \"../libraries/middle.maac\"; hash = \"{}\"; }}\n",
        maac::bundle::sha256_digest(middle.as_bytes())
    );
    fs::write(root.join("shared/leaf.maac"), leaf).unwrap();
    fs::write(root.join("libraries/middle.maac"), middle).unwrap();
    fs::write(root.join("songs/main.maac"), entry).unwrap();

    let default_root = invoke(&[
        Path::new("--json"),
        Path::new("check"),
        root.join("songs/main.maac").as_path(),
    ]);
    assert!(!default_root.status.success());
    assert_eq!(json_stdout(&default_root)["code"], "E_REFERENCE");

    let explicit_root = invoke_in(
        root,
        &[
            Path::new("--json"),
            Path::new("check"),
            Path::new("songs/main.maac"),
            Path::new("--project-root"),
            Path::new("."),
        ],
    );
    assert!(
        explicit_root.status.success(),
        "check failed: {:?}",
        explicit_root
    );
    let result = json_stdout(&explicit_root);
    assert_eq!(result["ok"], true);
    assert_eq!(result["notes"], 1);
    assert_eq!(result["frames"], 24_000);
}

#[test]
fn check_accepts_libraries_and_validates_unused_exports_without_render_stats() {
    let directory = tempdir().unwrap();
    let library = directory.path().join("studio.maac");
    let valid = r#"maac 1;
library studio { version = "1"; }
instrument lead {
  channels = 1;
  voice v {
    channels = 1;
    amplitude = &amp;
    output = &osc:out;
    node amp { type = "synth.adsr/1"; }
    node osc { type = "synth.sine/1"; }
  }
}
"#;
    fs::write(&library, valid).unwrap();

    let check = invoke_in(
        directory.path(),
        &[
            Path::new("--json"),
            Path::new("check"),
            Path::new("studio.maac"),
        ],
    );
    assert!(check.status.success(), "library check failed: {:?}", check);
    let result = json_stdout(&check);
    assert_eq!(result["ok"], true);
    assert_eq!(result["exports"], 1);
    assert!(result.get("notes").is_none());
    assert!(result.get("frames").is_none());

    let plan = directory.path().join("library.plan.json");
    let compile = invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        library.as_path(),
        Path::new("-o"),
        plan.as_path(),
    ]);
    assert!(!compile.status.success());
    assert_eq!(json_stdout(&compile)["code"], "E_CONFLICT");
    assert!(!plan.exists());

    let wave = directory.path().join("library.wav");
    let build = invoke(&[
        Path::new("--json"),
        Path::new("build"),
        library.as_path(),
        Path::new("-o"),
        wave.as_path(),
    ]);
    assert!(!build.status.success());
    assert_eq!(json_stdout(&build)["code"], "E_CONFLICT");
    assert!(!wave.exists());

    let invalid = valid.replace(
        "node osc { type = \"synth.sine/1\"; }",
        "node osc { type = \"synth.sine/1\"; params = { typo = 1; }; }",
    );
    fs::write(&library, invalid).unwrap();
    let invalid_check = invoke(&[Path::new("--json"), Path::new("check"), library.as_path()]);
    assert!(!invalid_check.status.success());
    assert_eq!(json_stdout(&invalid_check)["code"], "E_UNKNOWN_FIELD");
}

#[test]
fn source_commands_reject_wrong_pins_and_entries_outside_the_explicit_root() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("project");
    fs::create_dir(&root).unwrap();
    let dependency = "maac 1;\nlibrary dependency { version = \"1\"; }\n";
    fs::write(root.join("dependency.maac"), dependency).unwrap();
    fs::write(
        root.join("main.maac"),
        format!(
            "{ONE_NOTE_SOURCE}\nimport dependency {{ path = \"dependency.maac\"; hash = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"; }}\n"
        ),
    )
    .unwrap();

    let wrong_pin = invoke(&[
        Path::new("--json"),
        Path::new("check"),
        root.join("main.maac").as_path(),
    ]);
    assert!(!wrong_pin.status.success());
    assert_eq!(json_stdout(&wrong_pin)["code"], "E_HASH");

    let outside = directory.path().join("outside.maac");
    fs::write(&outside, ONE_NOTE_SOURCE).unwrap();
    let outside_root = invoke(&[
        Path::new("--json"),
        Path::new("check"),
        outside.as_path(),
        Path::new("--project-root"),
        root.as_path(),
    ]);
    assert!(!outside_root.status.success());
    assert_eq!(json_stdout(&outside_root)["code"], "E_REFERENCE");
}

#[cfg(unix)]
#[test]
fn source_commands_reject_import_symlinks_that_escape_the_project_root() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let root = directory.path().join("project");
    fs::create_dir(&root).unwrap();
    let outside = directory.path().join("outside.maac");
    let dependency = "maac 1;\nlibrary dependency { version = \"1\"; }\n";
    fs::write(&outside, dependency).unwrap();
    symlink(&outside, root.join("linked.maac")).unwrap();
    fs::write(
        root.join("main.maac"),
        format!(
            "{ONE_NOTE_SOURCE}\nimport dependency {{ path = \"linked.maac\"; hash = \"{}\"; }}\n",
            maac::bundle::sha256_digest(dependency.as_bytes())
        ),
    )
    .unwrap();

    let output = invoke(&[
        Path::new("--json"),
        Path::new("check"),
        root.join("main.maac").as_path(),
        Path::new("--project-root"),
        root.as_path(),
    ]);
    assert!(!output.status.success());
    assert_eq!(json_stdout(&output)["code"], "E_REFERENCE");
}

#[test]
fn compiled_bundle_plan_renders_after_sources_are_removed_and_rejects_malformed_v2() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    let dependency = "maac 1;\nlibrary dependency { version = \"1\"; }\n";
    let entry = format!(
        "{ONE_NOTE_SOURCE}\nimport dependency {{ path = \"dependency.maac\"; hash = \"{}\"; }}\n",
        maac::bundle::sha256_digest(dependency.as_bytes())
    );
    let source = root.join("main.maac");
    let imported = root.join("dependency.maac");
    let plan = root.join("standalone.plan.json");
    let built = root.join("built.wav");
    fs::write(&source, entry).unwrap();
    fs::write(&imported, dependency).unwrap();

    let compile = invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        source.as_path(),
        Path::new("-o"),
        plan.as_path(),
    ]);
    assert!(compile.status.success(), "compile failed: {:?}", compile);
    let plan_json: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    assert_eq!(plan_json["version"], 2);
    assert_eq!(
        plan_json["instruments"]["dependencies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let build = invoke(&[
        Path::new("--json"),
        Path::new("build"),
        source.as_path(),
        Path::new("-o"),
        built.as_path(),
    ]);
    assert!(build.status.success(), "build failed: {:?}", build);

    fs::remove_file(&source).unwrap();
    fs::remove_file(&imported).unwrap();

    let first_wav = root.join("first.wav");
    let second_wav = root.join("second.wav");
    for output in [&first_wav, &second_wav] {
        let render = invoke(&[
            Path::new("--json"),
            Path::new("render"),
            plan.as_path(),
            Path::new("-o"),
            output.as_path(),
        ]);
        assert!(render.status.success(), "render failed: {:?}", render);
    }
    assert_eq!(
        fs::read(&first_wav).unwrap(),
        fs::read(&second_wav).unwrap()
    );
    assert_eq!(fs::read(&built).unwrap(), fs::read(&first_wav).unwrap());

    let mut malformed = plan_json;
    malformed["instruments"]["entry_source"] = Value::String("missing.maac".into());
    let malformed_plan = root.join("malformed.plan.json");
    let rejected_wav = root.join("rejected.wav");
    fs::write(&malformed_plan, serde_json::to_vec(&malformed).unwrap()).unwrap();
    let rejected = invoke(&[
        Path::new("--json"),
        Path::new("render"),
        malformed_plan.as_path(),
        Path::new("-o"),
        rejected_wav.as_path(),
    ]);
    assert!(!rejected.status.success());
    assert_eq!(json_stdout(&rejected)["code"], "E_REFERENCE");
    assert!(!rejected_wav.exists());
}

#[test]
fn hash_rejects_inputs_larger_than_the_source_and_asset_file_limit() {
    let directory = tempdir().unwrap();
    let oversized = directory.path().join("oversized.bin");
    fs::write(&oversized, vec![0_u8; 4 * 1024 * 1024 + 1]).unwrap();

    let output = invoke(&[Path::new("--json"), Path::new("hash"), oversized.as_path()]);
    assert!(!output.status.success());
    assert_eq!(json_stdout(&output)["code"], "E_RESOURCE_LIMIT");
}
