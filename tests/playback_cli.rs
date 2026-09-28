use serde_json::Value;
#[cfg(target_os = "macos")]
use std::fs;
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

#[test]
fn playback_help_and_usage_keep_source_and_plan_options_explicit() {
    let output = invoke(&["play", "--help"]);
    assert!(output.status.success(), "{output:?}");
    let help = String::from_utf8(output.stdout).unwrap();
    for option in ["--plan", "--project-root", "--disk-media", "--profile"] {
        assert!(help.contains(option), "missing {option}: {help}");
    }
    let help = invoke(&["--help"]);
    assert!(String::from_utf8(help.stdout).unwrap().contains("play"));
    for args in [
        vec!["--json", "play", "--plan"],
        vec!["--json", "play", "plan.json", "--plan", "--disk-media"],
        vec![
            "--json",
            "play",
            "plan.json",
            "--plan",
            "--project-root",
            ".",
        ],
        vec!["--json", "play", "--format", "pcm16"],
        vec!["--json", "play", "--start-frame", "0", "--end-frame", "10"],
        vec!["--json", "play", "--profile", "unknown"],
    ] {
        assert_eq!(error(invoke(&args))["code"], "E_USAGE", "{args:?}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn source_and_plan_failures_keep_renderer_diagnostics_and_remove_private_files() {
    let root = tempfile::tempdir().unwrap();
    let temporary = root.path().join("private-temporaries");
    fs::create_dir(&temporary).unwrap();
    let project = root.path().join("project with spaces");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.maac"), "maac 1; broken").unwrap();
    let plan = root.path().join("invalid-plan.json");
    fs::write(&plan, r#"{"version":999}"#).unwrap();
    for disk in [false, true] {
        let mut reference = Command::new(env!("CARGO_BIN_EXE_maac"));
        reference
            .args(["--json", "build"])
            .arg(&project)
            .arg("-o")
            .arg(root.path().join("unused.wav"));
        if disk {
            reference.arg("--disk-media");
        }
        let expected = error(reference.output().unwrap());
        let mut play = Command::new(env!("CARGO_BIN_EXE_maac"));
        play.args(["--json", "play"])
            .arg(&project)
            .env("TMPDIR", &temporary);
        if disk {
            play.arg("--disk-media");
        }
        let actual = error(play.output().unwrap());
        assert_eq!(actual["code"], expected["code"]);
        assert_eq!(actual["message"], expected["message"]);
        assert_eq!(actual["span"], expected["span"]);
        assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    }
    let dash_project = root.path().join("-project");
    fs::create_dir(&dash_project).unwrap();
    fs::write(dash_project.join("main.maac"), "maac 1; broken").unwrap();
    let expected_dash_root = error(
        Command::new(env!("CARGO_BIN_EXE_maac"))
            .current_dir(root.path())
            .args(["--json", "build", "--project-root=-project", "-o"])
            .arg(root.path().join("unused.wav"))
            .args(["--", "-project"])
            .output()
            .unwrap(),
    );
    let actual_dash_root = error(
        Command::new(env!("CARGO_BIN_EXE_maac"))
            .current_dir(root.path())
            .args([
                "--json",
                "play",
                "--project-root=-project",
                "--",
                "-project",
            ])
            .env("TMPDIR", &temporary)
            .output()
            .unwrap(),
    );
    assert_eq!(expected_dash_root["code"], "E_SYNTAX");
    assert_eq!(actual_dash_root["code"], expected_dash_root["code"]);
    assert_eq!(actual_dash_root["message"], expected_dash_root["message"]);
    fs::write(project.join("-source.maac"), "maac 1; broken").unwrap();
    let dash = error(
        Command::new(env!("CARGO_BIN_EXE_maac"))
            .current_dir(&project)
            .args(["--json", "play", "--", "-source.maac"])
            .env("TMPDIR", &temporary)
            .output()
            .unwrap(),
    );
    assert_eq!(dash["code"], "E_SYNTAX");
    let implicit = error(
        Command::new(env!("CARGO_BIN_EXE_maac"))
            .current_dir(&project)
            .args(["--json", "play", "--profile", "song"])
            .env("TMPDIR", &temporary)
            .output()
            .unwrap(),
    );
    assert_eq!(implicit["code"], "E_SYNTAX");
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    let expected = error(
        Command::new(env!("CARGO_BIN_EXE_maac"))
            .args(["--json", "render"])
            .arg(&plan)
            .arg("-o")
            .arg(root.path().join("unused.wav"))
            .output()
            .unwrap(),
    );
    let actual = error(
        Command::new(env!("CARGO_BIN_EXE_maac"))
            .args(["--json", "play"])
            .arg(&plan)
            .arg("--plan")
            .env("TMPDIR", &temporary)
            .output()
            .unwrap(),
    );
    assert_eq!(actual["code"], expected["code"]);
    assert_eq!(actual["message"], expected["message"]);
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
    assert!(!root.path().join("unused.wav").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn interrupts_stop_blocked_render_children_and_clean_staging() {
    use std::{
        ffi::CString,
        fs::OpenOptions,
        os::unix::{
            ffi::OsStrExt,
            fs::{OpenOptionsExt, PermissionsExt},
        },
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    struct RestorePermissions(std::path::PathBuf);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        }
    }
    for (signal, deny_cleanup) in [
        (libc::SIGINT, false),
        (libc::SIGTERM, false),
        (libc::SIGTERM, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let temporary = root.path().join("private-temporaries");
        fs::create_dir(&temporary).unwrap();
        let fifo = root.path().join("blocked-plan.json");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let mut child = Command::new(env!("CARGO_BIN_EXE_maac"))
            .args(["--json", "play"])
            .arg(&fifo)
            .arg("--plan")
            .env("TMPDIR", &temporary)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // A nonblocking writer only opens after the real render child has
        // opened its plan input. Keep it open without bytes to block that read.
        let deadline = Instant::now() + Duration::from_secs(15);
        let writer = loop {
            if let Ok(writer) = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
            {
                break writer;
            }
            if let Some(status) = child.try_wait().unwrap() {
                let output = child.wait_with_output().unwrap();
                panic!("play exited before renderer opened FIFO: {status}: {output:?}");
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("renderer did not open FIFO");
            }
            thread::sleep(Duration::from_millis(10));
        };
        let permission_guard = if deny_cleanup {
            let stage = fs::read_dir(&temporary)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let guard = RestorePermissions(stage);
            fs::set_permissions(&guard.0, fs::Permissions::from_mode(0o000)).unwrap();
            Some(guard)
        } else {
            None
        };
        assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("play did not stop after signal {signal}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        drop(writer);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(128 + signal), "{output:?}");
        let report = error(output);
        if let Some(guard) = permission_guard {
            let stage = guard.0.clone();
            drop(guard);
            assert!(stage.exists());
            fs::remove_dir_all(&stage).unwrap();
            let message = report["message"].as_str().unwrap();
            assert!(message.contains("clean"), "{report}");
            assert!(
                message.contains(stage.to_string_lossy().as_ref()),
                "{report}"
            );
        }
        assert_eq!(report["code"], "E_INTERRUPTED");
        assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
        assert!(fifo.exists());
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn unsupported_hosts_fail_explicitly_before_source_loading() {
    assert_eq!(
        error(invoke(&["--json", "play", "missing.maac"]))["code"],
        "E_CAPABILITY"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn long_renderer_errors_preserve_classification_path_and_span() {
    let root = tempfile::tempdir().unwrap();
    let temporary = root.path().join("private-temporaries");
    fs::create_dir(&temporary).unwrap();
    let source = root.path().join("long-error.maac");
    let original = include_str!("../example.maac");
    assert!(original.contains("requires = [];"));
    fs::write(
        &source,
        original.replace(
            "requires = [];",
            &format!("requires = [\"{}\"];", "X".repeat(70_000)),
        ),
    )
    .unwrap();
    let expected = error(
        Command::new(env!("CARGO_BIN_EXE_maac"))
            .args(["--json", "build"])
            .arg(&source)
            .arg("-o")
            .arg(root.path().join("unused.wav"))
            .output()
            .unwrap(),
    );
    assert_eq!(expected["code"], "E_CAPABILITY");
    assert!(expected["message"].as_str().unwrap().len() > 65_536);
    let actual = error(
        Command::new(env!("CARGO_BIN_EXE_maac"))
            .args(["--json", "play"])
            .arg(&source)
            .env("TMPDIR", &temporary)
            .output()
            .unwrap(),
    );
    assert_eq!(actual["code"], expected["code"]);
    assert_eq!(actual["path"], expected["path"]);
    assert_eq!(actual["span"], expected["span"]);
    assert!(actual["message"].as_str().unwrap().contains("[truncated]"));
    assert!(actual["message"].as_str().unwrap().len() < 66_000);
    assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
}
