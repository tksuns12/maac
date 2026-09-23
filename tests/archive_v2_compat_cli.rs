use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::tempdir;

const V2_DIGEST: &str = "sha256:4d6247bdd8dee4587fdabe1f580c2d637ae56f187381918ee8a4415c46fa9f40";

#[test]
fn fixed_v2_archive_keeps_digest_and_unpacks_exact_source() {
    let fixture = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/archive_v2"
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["--json", "archive", "verify"])
        .arg(fixture)
        .args(["--expect-hash", V2_DIGEST])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["format"], "maac.editable-archive/2");
    assert_eq!(result["digest"], V2_DIGEST);

    let temp = tempdir().unwrap();
    let project = temp.path().join("unpacked");
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["archive", "unpack"])
        .arg(fixture)
        .arg("--output-dir")
        .arg(&project)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read(project.join("main.maac")).unwrap(),
        fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/archive_v1/main.maac"
        ))
        .unwrap()
    );
}
