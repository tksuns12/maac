use serde_json::Value;
use std::process::{Command, Output};

fn invoke(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}

fn error(output: Output) -> Value {
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    value
}

// Every invocation here fails before microphone authorization or capture.
#[test]
fn recording_help_and_usage_require_an_explicit_bounded_capture() {
    let output = invoke(&["record", "--help"]);
    assert!(output.status.success(), "{output:?}");
    let help = String::from_utf8(output.stdout).unwrap();
    for option in [
        "--duration-seconds",
        "--output-dir",
        "--profile",
        "--input-device",
    ] {
        assert!(help.contains(option), "missing {option}: {help}");
    }
    let output = invoke(&["--help"]);
    assert!(String::from_utf8(output.stdout).unwrap().contains("record"));
    for args in [
        vec!["--json", "record"],
        vec!["--json", "record", "--duration-seconds", "1"],
        vec!["--json", "record", "--output-dir", "unused"],
    ] {
        assert_eq!(error(invoke(&args))["code"], "E_USAGE", "{args:?}");
    }
    for duration in ["0", "1801", "1.5", "-1", "18446744073709551616"] {
        assert_eq!(
            error(invoke(&[
                "--json",
                "record",
                "--duration-seconds",
                duration,
                "--output-dir",
                "unused",
            ]))["code"],
            "E_USAGE",
            "duration={duration}"
        );
    }
    for (option, value) in [
        ("--profile", "unknown"),
        ("--format", "pcm16"),
        ("--channels", "2"),
        ("--device", "default"),
        ("--sample-rate", "44100"),
    ] {
        assert_eq!(
            error(invoke(&[
                "--json",
                "record",
                "--duration-seconds",
                "1",
                "--output-dir",
                "unused",
                option,
                value,
            ]))["code"],
            "E_USAGE",
            "option={option}"
        );
    }
}

#[test]
fn input_listing_help_and_usage_are_read_only_and_input_specific() {
    let output = invoke(&["inputs", "--help"]);
    assert!(output.status.success(), "{output:?}");
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.to_lowercase().contains("input"));
    let output = invoke(&["--help"]);
    assert!(String::from_utf8(output.stdout).unwrap().contains("inputs"));
    for args in [
        vec!["--json", "inputs", "extra"],
        vec!["--json", "inputs", "--output-dir", "unused"],
        vec!["--json", "inputs", "--device", "unknown"],
    ] {
        assert_eq!(error(invoke(&args))["code"], "E_USAGE", "{args:?}");
    }
}

#[test]
fn malformed_input_uids_fail_before_any_capture_can_start() {
    // An existing destination also prevents capture if selector validation ever
    // regresses. These tests must not depend on the machine's microphone state.
    let root = tempfile::tempdir().unwrap();
    let sentinel = root.path().join("sentinel");
    std::fs::write(&sentinel, b"preserve").unwrap();
    for uid in [String::new(), "x".repeat(4097), "é".repeat(2049)] {
        let output = Command::new(env!("CARGO_BIN_EXE_maac"))
            .args([
                "--json",
                "record",
                "--duration-seconds",
                "1",
                "--input-device",
            ])
            .arg(&uid)
            .arg("--output-dir")
            .arg(root.path())
            .output()
            .unwrap();
        assert_eq!(error(output)["code"], "E_USAGE", "uid bytes={}", uid.len());
    }
    assert_eq!(std::fs::read(sentinel).unwrap(), b"preserve");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(target_os = "macos")]
#[test]
fn valid_opaque_input_uids_preserve_destination_preflight() {
    let root = tempfile::tempdir().unwrap();
    for uid in [
        "a device UID with spaces",
        "-dash-prefixed-uid",
        "入力-device",
        "default",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_maac"))
            .args(["--json", "record", "--duration-seconds", "1"])
            .arg(format!("--input-device={uid}"))
            .arg("--output-dir")
            .arg(root.path())
            .output()
            .unwrap();
        assert_eq!(error(output)["code"], "E_OUTPUT_EXISTS", "uid={uid:?}");
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn input_listing_requires_macos() {
    assert_eq!(error(invoke(&["--json", "inputs"]))["code"], "E_CAPABILITY");
}

#[cfg(target_os = "macos")]
#[test]
fn recording_rejects_predictably_excessive_work_before_microphone_authorization() {
    let root = tempfile::tempdir().unwrap();
    let output_dir = root.path().join("over-budget");
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args([
            "--json",
            "record",
            "--duration-seconds",
            "232",
            "--output-dir",
        ])
        .arg(&output_dir)
        .output()
        .unwrap();
    let value = error(output);
    assert_eq!(value["code"], "E_RESOURCE_LIMIT");
    assert!(value["message"].as_str().unwrap().contains("song"));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn recording_rejects_existing_destinations_before_accessing_the_microphone() {
    use std::{fs, os::unix::fs::symlink};

    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    fs::create_dir(&private).unwrap();
    let file = root.path().join("existing file");
    fs::write(&file, b"keep these bytes").unwrap();
    let directory = root.path().join("existing directory");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("sentinel"), b"keep this member").unwrap();
    let dangling = root.path().join("dangling");
    symlink(root.path().join("absent"), &dangling).unwrap();
    for destination in [&file, &directory, &dangling] {
        let output = Command::new(env!("CARGO_BIN_EXE_maac"))
            .args([
                "--json",
                "record",
                "--duration-seconds",
                "1",
                "--output-dir",
            ])
            .arg(destination)
            .env("TMPDIR", &private)
            .output()
            .unwrap();
        assert_eq!(error(output)["code"], "E_OUTPUT_EXISTS");
        assert_eq!(fs::read_dir(&private).unwrap().count(), 0);
    }
    assert_eq!(fs::read(file).unwrap(), b"keep these bytes");
    assert_eq!(
        fs::read(directory.join("sentinel")).unwrap(),
        b"keep this member"
    );
    assert!(fs::symlink_metadata(dangling)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(target_os = "macos")]
#[test]
fn recording_rejects_missing_parent_before_accessing_the_microphone() {
    let root = tempfile::tempdir().unwrap();
    let output_dir = root.path().join("missing/new-project");
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args([
            "--json",
            "record",
            "--duration-seconds",
            "1",
            "--output-dir",
        ])
        .arg(&output_dir)
        .output()
        .unwrap();
    assert_eq!(error(output)["code"], "E_IO");
    assert!(!root.path().join("missing").exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn recording_requires_macos_and_creates_no_destination() {
    let root = tempfile::tempdir().unwrap();
    let output_dir = root.path().join("new-project");
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args([
            "--json",
            "record",
            "--duration-seconds",
            "1",
            "--output-dir",
        ])
        .arg(&output_dir)
        .output()
        .unwrap();
    assert_eq!(error(output)["code"], "E_CAPABILITY");
    assert!(!output_dir.exists());
}
