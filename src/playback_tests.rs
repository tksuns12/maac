//! Hardware-free supervisor checks. Backend substitution exists only in this
//! unit-test module; the shipped process always uses /usr/bin/afplay.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

use super::{managed, PlayRequest, ProfileArg};

// Loading many copies of the full debug test executable at once can exceed
// the startup deadline on macOS. Each proof still runs in its own process.
static HELPER_OWNER: Mutex<()> = Mutex::new(());

struct Fixture {
    directory: TempDir,
    _helper_owner: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let helper_owner = HELPER_OWNER
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("staging")).unwrap();
        let mut writer = hound::WavWriter::create(
            directory.path().join("reference.wav"),
            hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for sample in [0.25f32, -0.25, 0.5, -0.5, 0.0, 0.125, -0.125, 0.0] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let fixture = Self {
            directory,
            _helper_owner: helper_owner,
        };
        fixture.renderer("");
        fixture.backend("exit 0");
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    fn script(&self, name: &str, body: &str) {
        let path = self.path(name);
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn renderer(&self, before_render: &str) {
        self.script(
            "renderer",
            &format!(
                "printf '%s\\n' \"$@\" > {}\nwave=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '-o' ]; then shift; wave=$1; fi\n  shift\ndone\nprintf '%s' \"$wave\" > {}\n{before_render}\ncp {} \"$wave\"\nprintf '%s\\n' '{{\"ok\":true,\"input\":\"fixture.maac\",\"frames\":4}}'",
                quoted(&self.path("arguments")),
                quoted(&self.path("wave-path")),
                quoted(&self.path("reference.wav")),
            ),
        );
    }

    fn backend(&self, body: &str) {
        self.script(
            "backend",
            &format!(
                "printf '%s' \"$$\" > {}\nprintf '%s' \"$1\" > {}\n{body}",
                quoted(&self.path("backend-pid")),
                quoted(&self.path("backend-wave")),
            ),
        );
    }

    fn spawn(&self) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "playback::tests::supervisor_helper",
                "--nocapture",
            ])
            .env("MAAC_PLAYBACK_TEST_ROOT", self.directory.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn result(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path("result.json")).unwrap()).unwrap()
    }

    fn assert_clean(&self) {
        assert_eq!(fs::read_dir(self.path("staging")).unwrap().count(), 0);
        if let Ok(wave) = fs::read_to_string(self.path("wave-path")) {
            assert!(!Path::new(&wave).exists(), "private WAV survives: {wave}");
        }
    }
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\"'\"'"))
}

fn wait_for_file(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "supervisor exited before creating {path:?}"
        );
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("supervisor did not create {path:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn finish(mut child: Child) -> Output {
    finish_with_deadline(&mut child, Duration::from_secs(20));
    child.wait_with_output().unwrap()
}

fn finish_with_deadline(child: &mut Child, allowance: Duration) {
    let deadline = Instant::now() + allowance;
    loop {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("supervisor did not finish within {allowance:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_reaped(pid_file: &Path) {
    let pid: libc::pid_t = fs::read_to_string(pid_file).unwrap().parse().unwrap();
    // SAFETY: signal 0 only probes process existence; pid came from the child.
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "child {pid} remains alive or unreaped"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[test]
fn completed_playback_receives_exact_finalized_stereo_wave_and_forwards_safe_arguments() {
    let fixture = Fixture::new();
    fixture.backend(&format!(
        "cmp {} \"$1\"\nprintf 'backend stdout is private\\n'\nprintf 'backend stderr is private\\n' >&2",
        quoted(&fixture.path("reference.wav")),
    ));
    let output = finish(fixture.spawn());
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let result = fixture.result();
    assert_eq!(result["ok"], true);
    assert_eq!(result["command"], "play");
    assert_eq!(result["backend"], "macos-afplay");
    assert_eq!(result["output_device"], "system-default");
    assert_eq!(result["format"], "float32");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["frames"], 4);
    assert_eq!(result["sample_rate"], 48_000);
    assert_eq!(result["channels"], 2);
    assert!(result.get("output").is_none());
    let arguments = fs::read_to_string(fixture.path("arguments")).unwrap();
    assert!(arguments.starts_with("--json\nbuild\n--format\nfloat32\n--profile\nsong\n"));
    assert!(
        arguments.ends_with("--project-root=project root\n--disk-media\n--\n--dash-input.maac\n")
    );
    assert_reaped(&fixture.path("backend-pid"));
    fixture.assert_clean();
}

#[test]
fn backend_failure_is_bounded_private_and_cleans_up_wave() {
    let fixture = Fixture::new();
    fixture.backend("printf 'default device unavailable\\n' >&2\nexit 7");
    let output = finish(fixture.spawn());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let result = fixture.result();
    assert_eq!(result["code"], "E_PLAYBACK");
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("default device unavailable"));
    assert_reaped(&fixture.path("backend-pid"));
    fixture.assert_clean();
}

#[test]
fn verbose_backend_failure_keeps_diagnostic_memory_bounded() {
    let fixture = Fixture::new();
    fixture.backend("/usr/bin/yes unavailable | /usr/bin/head -c 100000 >&2\nexit 7");
    let output = finish(fixture.spawn());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let result = fixture.result();
    assert_eq!(result["code"], "E_PLAYBACK");
    let message = result["message"].as_str().unwrap();
    assert!(message.len() <= 64 * 1024);
    assert!(message.ends_with(" [truncated]"));
    fixture.assert_clean();
}

#[test]
fn backend_spawn_failure_cleans_up_wave() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.path("backend")).unwrap();
    let output = finish(fixture.spawn());
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fixture.result()["code"], "E_PLAYBACK");
    assert!(fixture.result()["message"]
        .as_str()
        .unwrap()
        .contains("cannot start macOS afplay backend"));
    fixture.assert_clean();
}

#[test]
fn renderer_diagnostic_preserves_code_message_path_and_span() {
    let fixture = Fixture::new();
    fixture.script("renderer", "printf '%s\\n' '{\"ok\":false,\"code\":\"E_RANGE\",\"message\":\"bad range\",\"path\":\"project.score\",\"span\":{\"start\":7,\"end\":9}}'\nexit 1");
    let output = finish(fixture.spawn());
    assert_eq!(output.status.code(), Some(1));
    let result = fixture.result();
    assert_eq!(result["code"], "E_RANGE");
    assert_eq!(result["message"], "bad range");
    assert_eq!(result["path"], "project.score");
    assert_eq!(result["span"], serde_json::json!({"start":7,"end":9}));
    assert!(!fixture.path("backend-pid").exists());
    fixture.assert_clean();
}

#[test]
fn oversized_valid_renderer_json_preserves_error_identity_and_bounds_utf8_message() {
    let fixture = Fixture::new();
    let diagnostic = serde_json::json!({
        "ok": false,
        "code": "E_CAPABILITY",
        "message": "unsupported required capability ".to_owned() + &"가".repeat(30_000),
        "path": "project.required",
        "span": { "start": 11, "end": 90011 },
    });
    let result_path = fixture.path("renderer-result.json");
    fs::write(&result_path, serde_json::to_vec(&diagnostic).unwrap()).unwrap();
    fixture.script("renderer", &format!("cat {}\nexit 1", quoted(&result_path)));
    let output = finish(fixture.spawn());
    assert_eq!(output.status.code(), Some(1));
    let result = fixture.result();
    assert_eq!(result["code"], "E_CAPABILITY");
    assert_eq!(result["path"], "project.required");
    assert_eq!(result["span"], diagnostic["span"]);
    let message = result["message"].as_str().unwrap();
    assert!(message.starts_with("unsupported required capability "));
    assert!(message.ends_with(" [truncated]"));
    assert!(message.len() <= 64 * 1024);
    assert!(!fixture.path("backend-pid").exists());
    fixture.assert_clean();
}

#[test]
fn invalid_rendered_wave_never_starts_backend() {
    let fixture = Fixture::new();
    fs::write(fixture.path("reference.wav"), b"not a WAV").unwrap();
    let output = finish(fixture.spawn());
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fixture.result()["code"], "E_PLAYBACK");
    assert!(!fixture.path("backend-pid").exists());
    fixture.assert_clean();
}

#[test]
fn incomplete_rendered_wave_never_starts_backend() {
    let fixture = Fixture::new();
    let reference = fixture.path("reference.wav");
    let bytes = fs::read(&reference).unwrap();
    fs::write(reference, &bytes[..bytes.len() - 4]).unwrap();
    let output = finish(fixture.spawn());
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fixture.result()["code"], "E_PLAYBACK");
    assert!(!fixture.path("backend-pid").exists());
    fixture.assert_clean();
}

#[test]
fn sigint_during_playback_kills_and_reaps_backend_before_cleanup() {
    interrupt_player(libc::SIGINT);
}

#[test]
fn sigterm_during_playback_kills_and_reaps_backend_before_cleanup() {
    interrupt_player(libc::SIGTERM);
}

#[test]
fn failed_cleanup_after_success_reports_io_error_and_surviving_directory() {
    cleanup_failure_after_backend("exit 0", "E_IO");
}

#[test]
fn failed_cleanup_preserves_backend_error_and_reports_surviving_directory() {
    cleanup_failure_after_backend("printf 'device unavailable\\n' >&2\nexit 7", "E_PLAYBACK");
}

fn cleanup_failure_after_backend(ending: &str, expected_code: &str) {
    let fixture = Fixture::new();
    fixture.backend(&format!("chmod 000 \"$(dirname \"$1\")\"\n{ending}"));
    let output = finish(fixture.spawn());
    let stage = restore_stage_permissions(&fixture);
    assert_eq!(output.status.code(), Some(1));
    let result = fixture.result();
    assert_eq!(result["code"], expected_code);
    assert_cleanup_failure_detail(&result, &stage);
    if expected_code == "E_PLAYBACK" {
        assert!(result["message"]
            .as_str()
            .unwrap()
            .contains("macOS afplay backend failed"));
    } else {
        assert_eq!(result["path"], stage.to_str().unwrap());
    }
    assert_reaped(&fixture.path("backend-pid"));
    fs::remove_dir_all(stage).unwrap();
    fixture.assert_clean();
}

#[test]
fn failed_cleanup_preserves_sigterm_exit_and_reports_surviving_directory() {
    let fixture = Fixture::new();
    fixture.backend(&format!(
        "chmod 000 \"$(dirname \"$1\")\"\nprintf ready > {}\nexec /bin/sleep 30",
        quoted(&fixture.path("permissions-removed")),
    ));
    let mut child = fixture.spawn();
    wait_for_file(&fixture.path("permissions-removed"), &mut child);
    // SAFETY: signal targets only the live helper process owned by this test.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    finish_with_deadline(&mut child, Duration::from_secs(5));
    let output = child.wait_with_output().unwrap();
    let stage = restore_stage_permissions(&fixture);
    assert_eq!(output.status.code(), Some(143));
    let result = fixture.result();
    assert_eq!(result["code"], "E_INTERRUPTED");
    assert!(result["message"].as_str().unwrap().contains("SIGTERM"));
    assert_cleanup_failure_detail(&result, &stage);
    assert_reaped(&fixture.path("backend-pid"));
    fs::remove_dir_all(stage).unwrap();
    fixture.assert_clean();
}

fn restore_stage_permissions(fixture: &Fixture) -> PathBuf {
    let wave = fs::read_to_string(fixture.path("wave-path")).unwrap();
    let stage = Path::new(&wave).parent().unwrap().to_owned();
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        fs::read_dir(&stage).unwrap().count() > 0,
        "cleanup unexpectedly removed every private file"
    );
    stage
}

fn assert_cleanup_failure_detail(result: &Value, stage: &Path) {
    let message = result["message"].as_str().unwrap();
    assert!(message.contains("private playback cleanup failed"));
    assert!(message.contains(stage.to_str().unwrap()));
    assert!(message.len() <= 64 * 1024);
}

fn interrupt_player(signal: libc::c_int) {
    let fixture = Fixture::new();
    fixture.backend("exec /bin/sleep 30");
    let mut child = fixture.spawn();
    wait_for_file(&fixture.path("backend-pid"), &mut child);
    // SAFETY: signal targets only the live helper process owned by this test.
    assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
    finish_with_deadline(&mut child, Duration::from_secs(5));
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(128 + signal), "{output:?}");
    assert_eq!(fixture.result()["code"], "E_INTERRUPTED");
    assert_reaped(&fixture.path("backend-pid"));
    fixture.assert_clean();
}

#[test]
fn supervisor_helper() {
    let Some(root) = std::env::var_os("MAAC_PLAYBACK_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    // Snapshot process signal actions around every stage, including failures.
    let before = signal_actions();
    let request = PlayRequest {
        input: Some(PathBuf::from("--dash-input.maac")),
        plan: false,
        project_root: Some(PathBuf::from("project root")),
        disk_media: true,
        profile: ProfileArg::Song,
    };
    let result = managed::run_with(
        &request,
        &root.join("renderer"),
        &root.join("backend"),
        Some(&root.join("staging")),
    );
    let after = signal_actions();
    for (old, restored) in before.into_iter().zip(after) {
        assert_eq!(old.sa_sigaction, restored.sa_sigaction);
        assert_eq!(old.sa_flags, restored.sa_flags);
        for signal in [libc::SIGINT, libc::SIGTERM] {
            // SAFETY: both masks were initialized by successful sigaction.
            assert_eq!(unsafe { libc::sigismember(&old.sa_mask, signal) }, unsafe {
                libc::sigismember(&restored.sa_mask, signal)
            });
        }
    }
    let (value, code) = match result {
        Ok(result) => (serde_json::to_value(result).unwrap(), 0),
        Err(failure) => (
            serde_json::to_value(failure.error).unwrap(),
            failure.exit_code,
        ),
    };
    fs::write(
        root.join("result.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    std::process::exit(code);
}

fn signal_actions() -> [libc::sigaction; 2] {
    // SAFETY: sigaction reads the current disposition into valid local slots.
    unsafe {
        let mut actions = [std::mem::zeroed(), std::mem::zeroed()];
        for (signal, action) in [libc::SIGINT, libc::SIGTERM].into_iter().zip(&mut actions) {
            assert_eq!(libc::sigaction(signal, std::ptr::null(), action), 0);
        }
        actions
    }
}
