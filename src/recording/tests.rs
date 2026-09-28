use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{
    managed::{self, Commands},
    stream::{self, Chunk, Ring, WaveSink, CHUNK_FRAMES},
    InputDevice, ProfileArg, RecordRequest,
};

static HELPER_OWNER: Mutex<()> = Mutex::new(());

fn device() -> InputDevice {
    InputDevice {
        selection: "system-default".into(),
        uid: "synthetic-test-input".into(),
        hardware_rate_hz: Some(44_100.0),
    }
}
fn chunk(time: f64, frames: usize) -> Chunk {
    Chunk {
        time,
        frames,
        samples: [0.25; CHUNK_FRAMES],
    }
}
fn reference(path: &Path) {
    let mut sink = WaveSink::create(path, 1).unwrap();
    let mut time = 9123.0;
    while !sink.complete() {
        sink.accept(&chunk(time, CHUNK_FRAMES)).unwrap();
        time += CHUNK_FRAMES as f64;
    }
    sink.finish(1, device()).unwrap();
}

#[test]
fn callback_storage_is_bounded_and_overflow_is_fatal() {
    let ring = Ring::new();
    for n in 0..16 {
        assert!(ring.push(n as f64, &[n as f32]));
    }
    assert!(!ring.push(16.0, &[16.0]));
    assert!(ring.check().unwrap_err().message.contains("overflow"));
    for n in 0..16 {
        let chunk = ring.pop().unwrap();
        assert_eq!(chunk.time, n as f64);
        assert_eq!(chunk.samples[0], n as f32);
    }
    assert!(ring.pop().is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn callback_enqueue_racing_shutdown_does_not_reject_a_complete_take() {
    use std::sync::{atomic::Ordering, Arc, Barrier};
    let ring = Arc::new(Ring::new());
    let transition = Arc::new(Barrier::new(2));
    let callback_ring = Arc::clone(&ring);
    let callback_transition = Arc::clone(&transition);
    let callback = thread::spawn(move || {
        assert!(!callback_ring.stopping.load(Ordering::Acquire));
        callback_transition.wait();
        callback_transition.wait();
        super::native::enqueue_result(&callback_ring, -66632);
    });
    transition.wait();
    ring.stopping.store(true, Ordering::Release);
    transition.wait();
    callback.join().unwrap();
    ring.check().unwrap();
    super::native::enqueue_result(&ring, -50);
    assert!(ring.check().is_err());
    let recording = Ring::new();
    super::native::enqueue_result(&recording, -66632);
    assert!(recording.check().is_err());
}

#[test]
fn frame_gap_nonfinite_and_invalid_callback_data_abort() {
    let temp = tempfile::tempdir().unwrap();
    let mut sink = WaveSink::create(&temp.path().join("gap.wav"), 1).unwrap();
    sink.accept(&chunk(200.0, 10)).unwrap();
    assert!(sink
        .accept(&chunk(211.0, 10))
        .unwrap_err()
        .message
        .contains("discontinuity"));
    assert!(sink.finish(1, device()).is_err());
    let mut sink = WaveSink::create(&temp.path().join("nonfinite.wav"), 1).unwrap();
    let mut bad = chunk(0.0, 1);
    bad.samples[0] = f32::NAN;
    assert!(sink.accept(&bad).unwrap_err().message.contains("nonfinite"));
    for (time, samples) in [(f64::NAN, &[1.0][..]), (0.0, &[][..])] {
        let ring = Ring::new();
        assert!(!ring.push(time, samples));
        assert!(ring.check().is_err());
    }
}

#[test]
fn exact_final_frames_and_strict_provenance_validate_without_native_device() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("reference.wav");
    reference(&path);
    let metadata = stream::validate_wave(&path, 1, || Ok(())).unwrap();
    assert_eq!(metadata.delivered.frames, 48_000);
    assert_eq!(metadata.origin.source_frame, 0);
    assert!(metadata.latency.measured_input_frames.is_none());
    assert_eq!(metadata.input_device.hardware_rate_hz, Some(44_100.0));
    let mut reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.duration(), 48_000);
    assert!(reader
        .samples::<f32>()
        .all(|sample| sample.unwrap() == 0.25));
    let clean = fs::read(&path).unwrap();
    let mut corrupt = clean.clone();
    corrupt[44] ^= 1;
    fs::write(&path, corrupt).unwrap();
    assert!(stream::validate_wave(&path, 1, || Ok(()))
        .unwrap_err()
        .message
        .contains("provenance differs"));
    fs::write(&path, &clean).unwrap();
    assert!(stream::validate_wave(&path, 2, || Ok(())).is_err());
    let mut trailing = clean;
    trailing.extend_from_slice(b"maac\0\0\0\0");
    let extent = (trailing.len() - 8) as u32;
    trailing[4..8].copy_from_slice(&extent.to_le_bytes());
    fs::write(&path, trailing).unwrap();
    assert!(stream::validate_wave(&path, 1, || Ok(())).is_err());
}

#[test]
fn budget_preflight_matches_generated_mono_clip_boundary() {
    let root = tempfile::tempdir().unwrap();
    let mut request = RecordRequest {
        duration_seconds: 231,
        output_dir: root.path().join("out"),
        profile: ProfileArg::Default,
    };
    super::preflight(&request).unwrap();
    request.duration_seconds = 232;
    assert_eq!(
        super::preflight(&request).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
    request.duration_seconds = 1800;
    request.profile = ProfileArg::Song;
    super::preflight(&request).unwrap();
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn setup_and_capture_watchdogs_exclude_permission_decision_time() {
    let mut watchdog = managed::CaptureWatch::default();
    let start = Instant::now();
    watchdog
        .check(start + Duration::from_secs(3600), 1)
        .unwrap();
    watchdog.observe("authorized", start).unwrap();
    assert!(watchdog.check(start + Duration::from_secs(11), 1).is_err());
    watchdog
        .observe("capturing", start + Duration::from_secs(1))
        .unwrap();
    watchdog.check(start + Duration::from_secs(11), 1).unwrap();
    assert!(watchdog.check(start + Duration::from_secs(13), 1).is_err());
}

struct Fixture {
    root: tempfile::TempDir,
    _owner: MutexGuard<'static, ()>,
}
impl Fixture {
    fn new() -> Self {
        let owner = HELPER_OWNER
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
            _owner: owner,
        };
        reference(&fixture.path("reference.wav"));
        fixture.capture("");
        let helper = std::env::current_exe().unwrap();
        fixture.script("import", &format!("MAAC_RECORD_IMPORT_WAVE=\"$3\" MAAC_RECORD_IMPORT_OUTPUT=\"$9\" MAAC_RECORD_IMPORT_RESULT={} {} --exact recording::tests::import_helper --nocapture > {}\ncat {}", quoted(&fixture.path("import-result.json")), quoted(&helper), quoted(&fixture.path("import-harness.stdout")), quoted(&fixture.path("import-result.json"))));
        fixture
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
    fn script(&self, name: &str, body: &str) {
        let path = self.path(name);
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn capture(&self, extra: &str) {
        self.script("capture", &format!("printf '%s' \"$$\" > {}\nprintf '%s' \"$3\" > {}\ncp {} \"$3\"\n{extra}\nprintf '%s\\n' '{{\"ok\":true}}'", quoted(&self.path("capture-pid")), quoted(&self.path("wave-path")), quoted(&self.path("reference.wav"))));
    }
    fn spawn(&self) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "recording::tests::supervisor_helper",
                "--nocapture",
            ])
            .env("MAAC_RECORD_TEST_ROOT", self.root.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
    fn result(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path("result.json")).unwrap()).unwrap()
    }
    fn clean(&self) {
        for member in fs::read_dir(self.root.path()).unwrap() {
            assert!(!member
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("maac-record-"));
        }
        if let Ok(wave) = fs::read_to_string(self.path("wave-path")) {
            assert!(!Path::new(&wave).exists());
        }
    }
}
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\"'\"'"))
}
fn wait_for(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "helper exited before creating {path:?}"
        );
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("helper did not create {path:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("helper exceeded deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
fn reaped(path: &Path) {
    let pid: libc::pid_t = fs::read_to_string(path).unwrap().parse().unwrap();
    // SAFETY: signal zero only checks whether the previously owned child exists.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[test]
fn synthetic_supervisor_uses_real_retained_import_and_archive_roundtrip() {
    let fixture = Fixture::new();
    let output = finish(fixture.spawn());
    assert!(output.status.success(), "{output:?}");
    let result = fixture.result();
    assert_eq!(result["command"], "record");
    assert_eq!(result["frames"], 48_000);
    assert_eq!(result["input_device"]["uid"], "synthetic-test-input");
    let project = fixture.path("output");
    assert_eq!(fs::read_dir(&project).unwrap().count(), 4);
    let original = fs::read(project.join("original.wav")).unwrap();
    assert_eq!(original, fs::read(fixture.path("reference.wav")).unwrap());
    crate::media_import::verify_retained_import_disk(&project).unwrap();
    let archive = fixture.path("archive");
    let created = crate::cli::execute_artifact(&crate::cli::Command::Archive {
        command: crate::cli::ArchiveCommand::Create {
            input: project.clone(),
            output_dir: archive.clone(),
            project_root: None,
            profile: ProfileArg::Default,
            previous: None,
            expect_previous_hash: None,
            freeze_output: false,
            freeze_node: vec![],
            retain_imports: vec![],
            processor_context: false,
        },
    })
    .unwrap();
    assert!(created.base().ok);
    let moved = fixture.path("moved");
    fs::rename(&archive, &moved).unwrap();
    fs::remove_dir_all(&project).unwrap();
    crate::cli::execute_artifact(&crate::cli::Command::Archive {
        command: crate::cli::ArchiveCommand::Verify {
            archive: moved.clone(),
            expect_hash: created.base().digest.clone(),
        },
    })
    .unwrap();
    let restored = fixture.path("restored");
    crate::cli::execute_artifact(&crate::cli::Command::Archive {
        command: crate::cli::ArchiveCommand::Unpack {
            archive: moved,
            output_dir: restored.clone(),
            expect_hash: created.base().digest.clone(),
            revision: None,
        },
    })
    .unwrap();
    assert_eq!(fs::read(restored.join("original.wav")).unwrap(), original);
    crate::media_import::verify_retained_import_disk(&restored).unwrap();
    stream::validate_wave(&restored.join("original.wav"), 1, || Ok(())).unwrap();
    fixture.clean();
}

#[test]
fn capture_denial_and_invalid_diagnostics_publish_nothing() {
    for body in [
        "printf '%s\\n' '{\"ok\":false,\"code\":\"E_PERMISSION\",\"message\":\"denied\"}'; exit 1",
        "printf not-json; exit 1",
    ] {
        let fixture = Fixture::new();
        fixture.script("capture", body);
        let output = finish(fixture.spawn());
        assert!(!output.status.success());
        let expected = if body.contains("E_PERMISSION") {
            "E_PERMISSION"
        } else {
            "E_RECORDING"
        };
        assert_eq!(fixture.result()["code"], expected);
        assert!(!fixture.path("output").exists());
        fixture.clean();
    }
}

#[test]
fn publication_race_preserves_existing_destination() {
    let fixture = Fixture::new();
    fixture.capture(&format!(
        "mkdir {}\nprintf sentinel > {}",
        quoted(&fixture.path("output")),
        quoted(&fixture.path("output/sentinel"))
    ));
    let output = finish(fixture.spawn());
    assert!(!output.status.success());
    assert_eq!(fixture.result()["code"], "E_OUTPUT_EXISTS");
    assert_eq!(
        fs::read(fixture.path("output/sentinel")).unwrap(),
        b"sentinel"
    );
    fixture.clean();
}

fn interrupt(stage: &str, signal: libc::c_int) {
    let fixture = Fixture::new();
    let pid = fixture.path("blocked-pid");
    let blocked = format!("printf '%s' \"$$\" > {}\nexec /bin/sleep 30", quoted(&pid));
    if stage == "capture" {
        fixture.script("capture", &blocked);
    } else {
        fixture.script("import", &blocked);
    }
    let mut child = fixture.spawn();
    wait_for(&pid, &mut child);
    // SAFETY: target is the live supervisor process owned by this fixture.
    assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
    let output = finish(child);
    assert_eq!(output.status.code(), Some(128 + signal));
    assert_eq!(fixture.result()["code"], "E_INTERRUPTED");
    reaped(&pid);
    assert!(!fixture.path("output").exists());
    fixture.clean();
}
#[test]
fn sigint_during_capture_aborts_and_reaps() {
    interrupt("capture", libc::SIGINT);
}
#[test]
fn sigterm_during_capture_aborts_and_reaps() {
    interrupt("capture", libc::SIGTERM);
}
#[test]
fn sigint_during_import_aborts_and_reaps() {
    interrupt("import", libc::SIGINT);
}
#[test]
fn sigterm_during_import_aborts_and_reaps() {
    interrupt("import", libc::SIGTERM);
}

#[test]
fn real_import_snapshots_are_contained_and_removed_after_cancellation() {
    let fixture = Fixture::new();
    fixture.script("import", &format!(
        "exec env MAAC_RECORD_IMPORT_WAVE=\"$3\" MAAC_RECORD_IMPORT_OUTPUT=\"$9\" MAAC_RECORD_IMPORT_RESULT={} MAAC_RECORD_IMPORT_BLOCK={} {} --exact recording::tests::import_helper --nocapture",
        quoted(&fixture.path("import-result.json")), quoted(&fixture.path("snapshots-ready.json")), quoted(&std::env::current_exe().unwrap()),
    ));
    let mut child = fixture.spawn();
    wait_for(&fixture.path("snapshots-ready.json"), &mut child);
    let marker: Value =
        serde_json::from_slice(&fs::read(fixture.path("snapshots-ready.json")).unwrap()).unwrap();
    let scratch = PathBuf::from(marker["scratch"].as_str().unwrap());
    assert!(scratch.is_absolute());
    assert_eq!(
        fs::metadata(&scratch).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(scratch.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(scratch
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("maac-record-"));
    let snapshots = fs::read_dir(&scratch)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(
        snapshots.len(),
        2,
        "retained source and decoded PCM are both live"
    );
    let original = fs::read(fixture.path("reference.wav")).unwrap();
    assert!(snapshots
        .iter()
        .any(|path| fs::read(path).unwrap() == original));
    // SAFETY: signal targets only the supervisor owned by this fixture.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let output = finish(child);
    assert_eq!(output.status.code(), Some(130));
    assert_eq!(fixture.result()["code"], "E_INTERRUPTED");
    // The import helper is exec'd directly as the supervised child.
    let pid = marker["pid"].as_u64().unwrap() as libc::pid_t;
    // SAFETY: signal zero only checks the now reaped child process.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert!(!scratch.exists());
    assert!(!fixture.path("output").exists());
    fixture.clean();
}

#[test]
fn cleanup_failure_preserves_interruption_and_identifies_private_residue() {
    let fixture = Fixture::new();
    fixture.capture(&format!(
        "mkdir \"$(dirname \"$3\")/locked\"\nprintf residual > \"$(dirname \"$3\")/locked/private\"\nchmod 000 \"$(dirname \"$3\")/locked\"\nprintf ready > {}\nexec /bin/sleep 30",
        quoted(&fixture.path("blocked"))
    ));
    let mut child = fixture.spawn();
    wait_for(&fixture.path("blocked"), &mut child);
    // SAFETY: signal only the live supervisor owned by this test.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let output = finish(child);
    assert_eq!(output.status.code(), Some(143));
    let stage = PathBuf::from(fs::read_to_string(fixture.path("wave-path")).unwrap())
        .parent()
        .unwrap()
        .to_owned();
    fs::set_permissions(stage.join("locked"), fs::Permissions::from_mode(0o700)).unwrap();
    let result = fixture.result();
    assert_eq!(result["code"], "E_INTERRUPTED");
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("private recording cleanup failed"));
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains(stage.to_str().unwrap()));
    assert!(fs::read_dir(&stage).unwrap().next().is_some());
    reaped(&fixture.path("capture-pid"));
    fs::remove_dir_all(stage).unwrap();
    fixture.clean();
}

#[test]
fn import_helper() {
    let Some(wave) = std::env::var_os("MAAC_RECORD_IMPORT_WAVE") else {
        return;
    };
    let output = std::env::var_os("MAAC_RECORD_IMPORT_OUTPUT").unwrap();
    let result_file = std::env::var_os("MAAC_RECORD_IMPORT_RESULT").unwrap();
    if let Some(marker) = std::env::var_os("MAAC_RECORD_IMPORT_BLOCK") {
        let imported =
            crate::media_import::import_wav_file_disk_with_retention(Path::new(&wave), None, true)
                .unwrap();
        fs::write(
            marker,
            serde_json::to_vec(
                &serde_json::json!({"pid": std::process::id(), "scratch": std::env::temp_dir()}),
            )
            .unwrap(),
        )
        .unwrap();
        thread::sleep(Duration::from_secs(30));
        drop(imported);
        return;
    }
    let result = crate::cli::execute_disk_media_import(
        &crate::cli::Command::ImportWav {
            input: wave.into(),
            output_dir: output.into(),
            retain_original: true,
            start_frame: None,
            end_frame: None,
        },
        ProfileArg::Default,
    );
    let (value, code) = match result {
        Ok(result) => (serde_json::to_value(result).unwrap(), 0),
        Err(error) => (serde_json::to_value(error).unwrap(), 1),
    };
    fs::write(result_file, serde_json::to_vec(&value).unwrap()).unwrap();
    std::process::exit(code);
}

#[test]
fn supervisor_helper() {
    let Some(root) = std::env::var_os("MAAC_RECORD_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    // SAFETY: this isolated helper exits at the end of the test. Prove the
    // private stage permissions do not depend on a restrictive caller umask.
    unsafe {
        libc::umask(0);
    }
    let request = RecordRequest {
        duration_seconds: 1,
        output_dir: root.join("output"),
        profile: ProfileArg::Default,
    };
    let before = signal_actions();
    let result = managed::run_with(
        &request,
        &std::env::current_exe().unwrap(),
        Some(Commands {
            capture: Some(Command::new(root.join("capture"))),
            import: Some(Command::new(root.join("import"))),
        }),
    );
    for (old, new) in before.into_iter().zip(signal_actions()) {
        assert_eq!(old.sa_sigaction, new.sa_sigaction);
        assert_eq!(old.sa_flags, new.sa_flags);
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
    // SAFETY: initialized local storage receives the process's current actions.
    unsafe {
        let mut actions = [std::mem::zeroed(), std::mem::zeroed()];
        for (signal, action) in [libc::SIGINT, libc::SIGTERM].into_iter().zip(&mut actions) {
            assert_eq!(libc::sigaction(signal, std::ptr::null(), action), 0);
        }
        actions
    }
}
