use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::tempdir;

fn invoke_in(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(root)
        .args(args)
        .output()
        .expect("maac process starts")
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("JSON result")
}

fn write_library(root: &Path) {
    fs::create_dir_all(root.join("library/deps")).unwrap();
    let leaf = r#"maac 1;
library leaf { version = "1"; }
tuning just { period = 1200ct; steps = [0ct, 702ct]; reference_index = 0; reference_frequency = 220Hz; }
pattern note { length = 1q; note n { at = 0q; dur = 1/2q; pitch = degree(1, &just); } }
"#;
    fs::write(root.join("library/deps/leaf.maac"), leaf).unwrap();
    let pin = maac::bundle::sha256_digest(leaf.as_bytes());
    fs::write(
        root.join("library/root.maac"),
        format!(
            "maac 1; library root {{ version = \"1\"; }} import leaf {{ path = \"deps/leaf.maac\"; hash = \"{pin}\"; }} curve move {{ clock = score; points = [(0q, 0, linear), (1q, 1, step)]; }} pattern phrase {{ length = 1q; use nested {{ pattern = &leaf.note; at = 0q; }} }}"
        ),
    )
    .unwrap();
}

#[test]
fn module_export_check_and_unpack_are_atomic_and_restore_exact_sources() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    write_library(root);
    let module = root.join("phrases.module.json");
    let export = invoke_in(
        root,
        &[
            "--json",
            "module",
            "export",
            "library/root.maac",
            "-o",
            module.to_str().unwrap(),
            "--project-root",
            ".",
        ],
    );
    assert!(export.status.success(), "export failed: {export:?}");
    let digest = json_stdout(&export)["digest"].as_str().unwrap().to_owned();

    let check = invoke_in(
        root,
        &[
            "--json",
            "module",
            "check",
            module.to_str().unwrap(),
            "--expect-hash",
            &digest,
        ],
    );
    assert!(check.status.success(), "check failed: {check:?}");
    assert_eq!(json_stdout(&check)["exports"], 2);

    let wrong = format!("sha256:{}", "0".repeat(64));
    let wrong_hash = invoke_in(
        root,
        &[
            "--json",
            "module",
            "check",
            module.to_str().unwrap(),
            "--expect-hash",
            &wrong,
        ],
    );
    assert!(!wrong_hash.status.success());
    assert_eq!(json_stdout(&wrong_hash)["code"], "E_HASH");

    let root_source = fs::read(root.join("library/root.maac")).unwrap();
    let leaf_source = fs::read(root.join("library/deps/leaf.maac")).unwrap();
    fs::remove_dir_all(root.join("library")).unwrap();
    let restored = root.join("restored");
    let unpack = invoke_in(
        root,
        &[
            "--json",
            "module",
            "unpack",
            module.to_str().unwrap(),
            "--output-dir",
            restored.to_str().unwrap(),
            "--expect-hash",
            &digest,
        ],
    );
    assert!(unpack.status.success(), "unpack failed: {unpack:?}");
    assert_eq!(
        fs::read(restored.join("library/root.maac")).unwrap(),
        root_source
    );
    assert_eq!(
        fs::read(restored.join("library/deps/leaf.maac")).unwrap(),
        leaf_source
    );

    fs::write(restored.join("sentinel"), b"keep").unwrap();
    let second_unpack = invoke_in(
        root,
        &[
            "--json",
            "module",
            "unpack",
            module.to_str().unwrap(),
            "--output-dir",
            restored.to_str().unwrap(),
        ],
    );
    assert!(!second_unpack.status.success());
    assert_eq!(json_stdout(&second_unpack)["code"], "E_OUTPUT_EXISTS");
    assert_eq!(fs::read(restored.join("sentinel")).unwrap(), b"keep");

    let preserved = b"preserve previous module";
    fs::write(&module, preserved).unwrap();
    fs::write(
        root.join("invalid.maac"),
        "maac 1; library bad { version = \"1\"; }",
    )
    .unwrap();
    let failed_force = invoke_in(
        root,
        &[
            "--json",
            "module",
            "export",
            "invalid.maac",
            "-o",
            module.to_str().unwrap(),
            "--force",
        ],
    );
    assert!(!failed_force.status.success());
    assert_eq!(fs::read(&module).unwrap(), preserved);
}

#[test]
fn module_reader_is_separate_from_the_four_megabyte_plan_reader() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    let padding = "\n".repeat(2_200_000);
    fs::write(
        root.join("large.maac"),
        format!(
            "maac 1; library large {{ version = \"1\"; }} curve sweep {{{padding}clock = normalized; points = [(0, 0, linear), (1, 1, step)]; }}"
        ),
    )
    .unwrap();
    let module = root.join("large.module.json");
    let export = invoke_in(
        root,
        &[
            "--json",
            "module",
            "export",
            "large.maac",
            "-o",
            module.to_str().unwrap(),
        ],
    );
    assert!(export.status.success(), "large export failed: {export:?}");
    assert!(fs::metadata(&module).unwrap().len() > 4 * 1024 * 1024);
    let check = invoke_in(
        root,
        &["--json", "module", "check", module.to_str().unwrap()],
    );
    assert!(check.status.success(), "large check failed: {check:?}");
}

#[test]
fn unpack_rejects_distinct_logical_members_that_collide_on_the_host_filesystem() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("CaseProbe"), b"probe").unwrap();
    let case_insensitive = root.join("caseprobe").exists();
    fs::remove_file(root.join("CaseProbe")).unwrap();

    fs::create_dir(root.join("library")).unwrap();
    let upper = "maac 1; library upper { version = \"1\"; }";
    let lower = "maac 1; library lower { version = \"1\"; }";
    fs::write(root.join("library/A.maac"), upper).unwrap();
    fs::write(root.join("library/a.maac"), lower).unwrap();
    fs::write(
        root.join("library/root.maac"),
        format!(
            "maac 1; library root {{ version = \"1\"; }} pattern p {{ length = 1q; }} import upper {{ path = \"A.maac\"; hash = \"{}\"; }} import lower {{ path = \"a.maac\"; hash = \"{}\"; }}",
            maac::bundle::sha256_digest(upper.as_bytes()),
            maac::bundle::sha256_digest(lower.as_bytes()),
        ),
    )
    .unwrap();
    if case_insensitive {
        // The host cannot create this fixture directly with distinct bytes, so
        // construct the already-validated logical bundle through the public API.
        let source = fs::read_to_string(root.join("library/root.maac")).unwrap();
        let artifact = maac::ModuleArtifact::from_source_bundle(&maac::SourceBundle {
            entry: "library/root.maac".into(),
            sources: std::collections::BTreeMap::from([
                ("library/root.maac".into(), source),
                ("library/A.maac".into(), upper.into()),
                ("library/a.maac".into(), lower.into()),
            ]),
            assets: std::collections::BTreeMap::new(),
        })
        .unwrap();
        fs::write(
            root.join("collision.module.json"),
            artifact.to_json().unwrap(),
        )
        .unwrap();
    } else {
        let export = invoke_in(
            root,
            &[
                "--json",
                "module",
                "export",
                "library/root.maac",
                "-o",
                "collision.module.json",
                "--project-root",
                ".",
            ],
        );
        assert!(export.status.success(), "export failed: {export:?}");
    }

    let unpack = invoke_in(
        root,
        &[
            "--json",
            "module",
            "unpack",
            "collision.module.json",
            "--output-dir",
            "restored",
        ],
    );
    if case_insensitive {
        assert!(!unpack.status.success());
        assert_eq!(json_stdout(&unpack)["code"], "E_CONFLICT");
        assert!(!root.join("restored").exists());
    } else {
        assert!(unpack.status.success(), "unpack failed: {unpack:?}");
        assert_eq!(
            fs::read(root.join("restored/library/A.maac")).unwrap(),
            upper.as_bytes()
        );
        assert_eq!(
            fs::read(root.join("restored/library/a.maac")).unwrap(),
            lower.as_bytes()
        );
    }
}
