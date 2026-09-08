use std::{fs, process::Command};
use tempfile::tempdir;

#[test]
fn filesystem_and_cli_load_builtin_without_a_std_directory() {
    let directory = tempdir().unwrap();
    let source = "maac 1; library local { version = \"1\"; } import basic { builtin = \"std/basic/1.0.0\"; }";
    fs::write(directory.path().join("main.maac"), source).unwrap();
    let bundle =
        maac::bundle_fs::load_bundle(std::path::Path::new("main.maac"), directory.path()).unwrap();
    assert_eq!(bundle.sources.len(), 1);
    assert_eq!(bundle.resolve().unwrap().documents.len(), 2);
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(directory.path())
        .args(["--json", "check", "main.maac"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
}

#[test]
fn cli_resolves_transitive_builtin_and_renders_retained_plan() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    let library = "maac 1; library local { version = \"1\"; } import basic { builtin = \"std/basic/1.0.0\"; }";
    let entry = format!(
        r#"maac 1;
import local {{ path = "local.maac"; hash = "{}"; }}
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &tone:out; tail = 0s; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
pattern riff {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = C4; velocity = 1; }} }}
track t {{ target = &tone:events; }}
place main {{ pattern = &riff; track = &t; at = 0q; }}
node tone {{ type = "core.sine/1"; config = {{ voices = 4; }}; params = {{ attack = 0s; release = 0s; level = 0.5; }}; }}
"#,
        maac::bundle::sha256_digest(library.as_bytes())
    );
    fs::write(root.join("main.maac"), entry).unwrap();
    fs::write(root.join("local.maac"), library).unwrap();
    let invoke = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_maac"))
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    invoke(&["--json", "compile", "main.maac", "-o", "retained.plan.json"]);
    invoke(&["--json", "build", "main.maac", "-o", "built.wav"]);
    let mut plan: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("retained.plan.json")).unwrap()).unwrap();
    assert_eq!(plan["version"], 2);
    let embedded_path = maac::stdlib::BASIC_SOURCE_PATH;
    let identities = plan["instruments"]["source_files"].as_array_mut().unwrap();
    let embedded = identities
        .iter_mut()
        .find(|source| source["path"] == embedded_path)
        .unwrap();
    assert_eq!(
        embedded["hash"],
        maac::bundle::sha256_digest(maac::stdlib::BASIC_SOURCE.as_bytes())
    );
    // A retained plan may describe a different historical embedded source.
    // Loading checks its internal identities, without consulting this registry.
    let retained_hash = maac::bundle::sha256_digest(b"historical embedded source");
    embedded["hash"] = retained_hash.clone().into();
    for dependency in plan["instruments"]["dependencies"].as_array_mut().unwrap() {
        if dependency["path"] == embedded_path {
            dependency["hash"] = retained_hash.clone().into();
        }
    }
    let retained_bytes = serde_json::to_vec(&plan).unwrap();
    maac::load_plan(&retained_bytes).unwrap();
    fs::write(root.join("retained.plan.json"), retained_bytes).unwrap();
    fs::remove_file(root.join("main.maac")).unwrap();
    fs::remove_file(root.join("local.maac")).unwrap();
    invoke(&[
        "--json",
        "render",
        "retained.plan.json",
        "-o",
        "rendered.wav",
    ]);
    assert_eq!(
        fs::read(root.join("built.wav")).unwrap(),
        fs::read(root.join("rendered.wav")).unwrap()
    );
}

#[test]
fn cli_reports_unknown_builtin_and_rejects_reserved_local_entry() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("main.maac"),
        "maac 1; import x { builtin = \"std/basic/9.0.0\"; }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(directory.path())
        .args(["--json", "check", "main.maac"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["code"], "E_REFERENCE");
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("unknown built-in"));

    fs::create_dir(directory.path().join("@builtin")).unwrap();
    fs::write(
        directory.path().join("@builtin/forged.maac"),
        "maac 1; library forged { version = \"1\"; }",
    )
    .unwrap();
    let result = maac::bundle_fs::load_bundle(
        std::path::Path::new("@builtin/forged.maac"),
        directory.path(),
    );
    assert_eq!(
        result.unwrap_err().first().unwrap().code,
        maac::DiagnosticCode::Reference
    );
}
