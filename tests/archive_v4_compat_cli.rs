use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::tempdir;

const V4_DIGEST: &str = "sha256:c836edfa73e44c0f4f44a16313e2761ce6def74971d42baf56a46e511bb08155";

#[test]
fn fixed_v4_frozen_archive_keeps_digest_and_unpacks_exact_source() {
    let fixture = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/archive_v4"
    ));
    let verified = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["--json", "archive", "verify"])
        .arg(fixture)
        .args(["--expect-hash", V4_DIGEST])
        .output()
        .unwrap();
    assert!(verified.status.success(), "{verified:?}");
    let result: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(result["format"], "maac.editable-archive/4");
    assert_eq!(result["digest"], V4_DIGEST);

    let temp = tempdir().unwrap();
    let unpacked = temp.path().join("unpacked");
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["archive", "unpack"])
        .arg(fixture)
        .arg("--output-dir")
        .arg(&unpacked)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read(unpacked.join("main.maac")).unwrap(),
        fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/archive_v1/main.maac"
        ))
        .unwrap()
    );
}
