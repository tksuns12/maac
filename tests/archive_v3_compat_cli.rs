use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::tempdir;

const V3_DIGEST: &str = "sha256:e20f7ca361cd3877b943e61c4f51aac0e1287543627357f4987527bdb56c269a";

#[test]
fn fixed_v3_archive_keeps_digest_and_retained_original() {
    let fixture = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/archive_v3"
    ));
    let verified = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["--json", "archive", "verify"])
        .arg(fixture)
        .args(["--expect-hash", V3_DIGEST])
        .output()
        .unwrap();
    assert!(verified.status.success(), "{verified:?}");
    let result: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(result["format"], "maac.editable-archive/3");
    assert_eq!(result["digest"], V3_DIGEST);

    let temp = tempdir().unwrap();
    let project = temp.path().join("unpacked");
    let unpacked = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["archive", "unpack"])
        .arg(fixture)
        .arg("--output-dir")
        .arg(&project)
        .output()
        .unwrap();
    assert!(unpacked.status.success(), "{unpacked:?}");
    let checkpoint = fixture
        .join("checkpoints")
        .join("bdd2c0dbc3d7360f62b03958f12ebaf691e7e1b0a4f8497e6dc8c1f69b42670e");
    for name in ["main.maac", "import.json", "media.pcm", "original.wav"] {
        assert_eq!(
            fs::read(project.join(name)).unwrap(),
            fs::read(checkpoint.join(name)).unwrap(),
            "{name}"
        );
    }
}
