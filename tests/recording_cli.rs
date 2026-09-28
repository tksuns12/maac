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
    for option in ["--duration-seconds", "--output-dir", "--profile"] {
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
