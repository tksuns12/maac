use std::fs::File;
use std::io::Read;
use std::mem;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    output_parent, preflight, recording_error, stream, CliError, ProfileArg, RecordFailure,
    RecordRequest, RecordResult, BACKEND, CHILD_ARGUMENT, MONITOR_BACKEND, RATE,
};

const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
static INTERRUPTED: AtomicI32 = AtomicI32::new(0);
static SIGNAL_OWNER: Mutex<()> = Mutex::new(());

extern "C" fn record_signal(signal: libc::c_int) {
    let _ = INTERRUPTED.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

struct Signals {
    previous_int: libc::sigaction,
    previous_term: libc::sigaction,
    _owner: MutexGuard<'static, ()>,
}

impl Signals {
    fn install() -> Result<Self, CliError> {
        let owner = SIGNAL_OWNER
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        INTERRUPTED.store(0, Ordering::Relaxed);
        // SAFETY: fully initialized local actions are copied by sigaction.
        unsafe {
            let mut action: libc::sigaction = mem::zeroed();
            action.sa_sigaction = record_signal as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            let mut previous_int = mem::zeroed();
            let mut previous_term = mem::zeroed();
            if libc::sigaction(libc::SIGINT, &action, &mut previous_int) != 0 {
                return Err(signal_error());
            }
            if libc::sigaction(libc::SIGTERM, &action, &mut previous_term) != 0 {
                let error = signal_error();
                libc::sigaction(libc::SIGINT, &previous_int, std::ptr::null_mut());
                return Err(error);
            }
            Ok(Self {
                previous_int,
                previous_term,
                _owner: owner,
            })
        }
    }

    fn check(&self) -> Result<(), RecordFailure> {
        let signal = INTERRUPTED.load(Ordering::Relaxed);
        if signal == 0 {
            return Ok(());
        }
        Err(RecordFailure {
            error: Box::new(CliError::new(
                "E_INTERRUPTED",
                format!(
                    "recording interrupted by {}",
                    if signal == libc::SIGINT {
                        "SIGINT"
                    } else {
                        "SIGTERM"
                    }
                ),
            )),
            exit_code: 128 + signal,
        })
    }
}

fn signal_error() -> CliError {
    recording_error(format!(
        "cannot install recording signal handlers: {}",
        std::io::Error::last_os_error()
    ))
}

impl Drop for Signals {
    fn drop(&mut self) {
        // SAFETY: these actions were returned by successful installation calls.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.previous_int, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &self.previous_term, std::ptr::null_mut());
        }
    }
}

struct ManagedChild {
    child: Option<Child>,
}
impl ManagedChild {
    fn spawn(command: &mut Command, stage: &str) -> Result<Self, CliError> {
        let child = command
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|error| recording_error(format!("cannot start {stage}: {error}")))?;
        Ok(Self { child: Some(child) })
    }
    fn wait(
        &mut self,
        signals: &Signals,
        capture: Option<(&Path, u32)>,
    ) -> Result<ExitStatus, RecordFailure> {
        let mut watchdog = CaptureWatch::default();
        loop {
            signals.check()?;
            match self.child.as_mut().expect("active child").try_wait() {
                Ok(Some(status)) => {
                    self.child.take();
                    signals.check()?;
                    return Ok(status);
                }
                Ok(None) => {
                    if let Some((marker, duration)) = capture {
                        if let Some(phase) = read_phase(marker)? {
                            watchdog.observe(&phase, Instant::now())?;
                        }
                        watchdog.check(Instant::now(), duration)?;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(recording_error(format!(
                        "cannot wait for recording child: {error}"
                    ))
                    .into())
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct CaptureWatch {
    authorized: Option<Instant>,
    started: Option<Instant>,
}

impl CaptureWatch {
    pub(super) fn observe(&mut self, phase: &str, now: Instant) -> Result<(), CliError> {
        match phase {
            "authorized" => {
                self.authorized.get_or_insert(now);
            }
            "capturing" => {
                self.authorized.get_or_insert(now);
                self.started.get_or_insert(now);
            }
            // A truncate/write transition may briefly expose an empty marker.
            "" => {}
            _ => {
                return Err(recording_error(
                    "invalid private recording lifecycle marker",
                ))
            }
        }
        Ok(())
    }

    pub(super) fn check(&self, now: Instant, duration: u32) -> Result<(), CliError> {
        let expired = match (self.authorized, self.started) {
            (_, Some(started)) => {
                now.duration_since(started) > Duration::from_secs(u64::from(duration) + 10)
            }
            (Some(authorized), None) => now.duration_since(authorized) > Duration::from_secs(10),
            (None, None) => false,
        };
        if expired {
            return Err(recording_error(
                "microphone child exceeded its bounded setup/capture deadline",
            ));
        }
        Ok(())
    }
}

fn read_phase(path: &Path) -> Result<Option<String>, CliError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(private_io_error(error)),
    };
    let mut bytes = Vec::new();
    file.take(32)
        .read_to_end(&mut bytes)
        .map_err(private_io_error)?;
    String::from_utf8(bytes)
        .map(|phase| Some(phase.trim().to_owned()))
        .map_err(|_| recording_error("invalid recording lifecycle marker encoding"))
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // SAFETY: the unreaped child owns its new process group; the
            // supervisor is outside it. Kill and reap before private cleanup.
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// Command overrides are a private, compile-time test seam. Public command
// parsing and the production supervisor have no backend/source override.
#[derive(Default)]
pub(super) struct Commands {
    pub capture: Option<Command>,
    pub import: Option<Command>,
}

pub(super) fn run_with(
    request: &RecordRequest,
    executable: &Path,
    commands: Option<Commands>,
) -> Result<RecordResult, RecordFailure> {
    preflight(request)?;
    let signals = Signals::install()?;
    signals.check()?;
    let temporary = tempfile::Builder::new()
        .prefix("maac-record-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(output_parent(&request.output_dir))
        .map_err(|error| CliError::new("E_IO", format!("cannot stage recording: {error}")))?;
    let stage = temporary.path().canonicalize().map_err(private_io_error)?;
    let result = prepare_and_publish(
        request,
        executable,
        &stage,
        &signals,
        commands.unwrap_or_default(),
    );
    // prepare_and_publish has dropped and reaped all children. Once publication
    // commits successfully, subsequent signals do not turn it into an abort.
    let result = if result.is_err() {
        signals.check().and(result)
    } else {
        result
    };
    if result.is_ok() {
        // Publication moved this directory into the requested output. All
        // scratch and diagnostics were removed before that atomic commit;
        // disarm the old TempDir path instead of attempting to delete it.
        let _ = temporary.keep();
        return result;
    }
    let cleanup = temporary.close().map_err(|error| {
        let mut error = CliError::new(
            "E_IO",
            format!(
                "private recording cleanup failed; files may remain at {}: {error}",
                stage.display()
            ),
        );
        error.path = Some(stage.display().to_string());
        error
    });
    // This path has no committed output. Preserve interruption even when the
    // signal arrived while private files were being removed.
    let result = signals.check().and(result);
    match (result, cleanup) {
        (Err(mut failure), Err(cleanup)) => {
            failure.error.message =
                bounded_message(&format!("{}; {}", failure.error.message, cleanup.message));
            Err(failure)
        }
        (Err(failure), Ok(())) => Err(failure),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(result), Ok(())) => Ok(result),
    }
}

fn prepare_and_publish(
    request: &RecordRequest,
    executable: &Path,
    stage: &Path,
    signals: &Signals,
    mut commands: Commands,
) -> Result<RecordResult, RecordFailure> {
    let wave = stage.join("capture.wav");
    let project = stage.join("project");
    let scratch = stage.join("scratch");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&scratch)
        .map_err(private_io_error)?;
    let capture_stdout = stage.join("capture.stdout");
    let capture_stderr = stage.join("capture.stderr");
    let mut capture = commands
        .capture
        .take()
        .unwrap_or_else(|| Command::new(executable));
    contain_temporaries(&mut capture, &scratch);
    capture
        .arg(CHILD_ARGUMENT)
        .arg(request.duration_seconds.to_string())
        .arg(&wave);
    if let Some(uid) = &request.input_device {
        capture.arg("--input-device").arg(uid);
    }
    if request.monitor {
        capture.arg("--monitor");
    }
    redirect(&mut capture, &capture_stdout, &capture_stderr)?;
    signals.check()?;
    let phase = wave.with_extension("status");
    let status = ManagedChild::spawn(&mut capture, "macOS microphone capture")?
        .wait(signals, Some((&phase, request.duration_seconds)))?;
    check_child(&capture_stdout, status, "capture")?;
    signals.check()?;
    let metadata = stream::validate_wave_request(
        &wave,
        request.duration_seconds,
        request.input_device.as_deref(),
        request.monitor,
        || signals.check().map_err(|failure| *failure.error),
    )?;
    signals.check()?;
    let import_stdout = stage.join("import.stdout");
    let import_stderr = stage.join("import.stderr");
    let mut import = commands
        .import
        .take()
        .unwrap_or_else(|| Command::new(executable));
    contain_temporaries(&mut import, &scratch);
    import
        .args(["--json", "import-wav"])
        .arg(&wave)
        .args([
            "--disk-media",
            "--retain-original",
            "--profile",
            match request.profile {
                ProfileArg::Default => "default",
                ProfileArg::Song => "song",
            },
            "--output-dir",
        ])
        .arg(&project);
    redirect(&mut import, &import_stdout, &import_stderr)?;
    signals.check()?;
    let status =
        ManagedChild::spawn(&mut import, "retained disk-media import")?.wait(signals, None)?;
    let imported = check_child(&import_stdout, status, "import")?;
    if imported["frames"].as_u64() != Some(metadata.delivered.frames)
        || imported["format"] != "maac.media-import/2"
        || !imported["digest"]
            .as_str()
            .is_some_and(|digest| digest.len() == 71 && digest.starts_with("sha256:"))
    {
        return Err(recording_error(
            "import child returned inconsistent retained project metadata",
        )
        .into());
    }
    let result = RecordResult {
        ok: true,
        command: "record",
        status: "completed",
        backend: if request.monitor {
            MONITOR_BACKEND
        } else {
            BACKEND
        },
        input_device: metadata.input_device,
        monitoring: metadata.monitoring,
        frames: metadata.delivered.frames,
        sample_rate: RATE,
        channels: 1,
        format: "float32",
        output: request.output_dir.display().to_string(),
        recording_format: if metadata.version == 3 {
            "maac.recording/3"
        } else if metadata.version == 2 {
            "maac.recording/2"
        } else {
            "maac.recording/1"
        },
        digest: imported["digest"]
            .as_str()
            .expect("checked digest")
            .to_owned(),
    };
    // Remove all audio and diagnostics outside the retained project before the
    // commit point. Failure here leaves no user output and is reported by cleanup.
    for path in [
        &wave,
        &capture_stdout,
        &capture_stderr,
        &import_stdout,
        &import_stderr,
    ] {
        std::fs::remove_file(path).map_err(private_io_error)?;
    }
    match std::fs::remove_file(&phase) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(private_io_error(error).into()),
    }
    std::fs::remove_dir_all(&scratch).map_err(private_io_error)?;
    // Publish the outer owner-only directory itself. It has no leftover private
    // work after this move. The caller disarms TempDir after successful commit
    // instead of introducing a fallible post-commit cleanup operation.
    for member in ["main.maac", "media.pcm", "import.json", "original.wav"] {
        std::fs::rename(project.join(member), stage.join(member)).map_err(private_io_error)?;
    }
    std::fs::remove_dir(&project).map_err(private_io_error)?;
    signals.check()?;
    crate::cli::publish_staged_directory_noclobber(
        stage,
        &request.output_dir,
        "recorded media project",
    )?;
    Ok(result)
}

fn contain_temporaries(command: &mut Command, scratch: &Path) {
    for variable in ["TMPDIR", "TMP", "TEMP"] {
        command.env(variable, scratch);
    }
}

fn redirect(command: &mut Command, stdout: &Path, stderr: &Path) -> Result<(), CliError> {
    command
        .stdout(Stdio::from(File::create(stdout).map_err(private_io_error)?))
        .stderr(Stdio::from(File::create(stderr).map_err(private_io_error)?));
    Ok(())
}
fn private_io_error(error: std::io::Error) -> CliError {
    CliError::new(
        "E_IO",
        format!("cannot access private recording staging: {error}"),
    )
}
fn check_child(
    path: &Path,
    status: ExitStatus,
    stage: &str,
) -> Result<serde_json::Value, CliError> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(private_io_error)?
        .take(MAX_DIAGNOSTIC_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(private_io_error)?;
    if bytes.len() > MAX_DIAGNOSTIC_BYTES {
        return Err(recording_error(format!(
            "{stage} child diagnostics exceed the bounded envelope"
        )));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        recording_error(format!(
            "{stage} child returned invalid terminal diagnostics ({status})"
        ))
    })?;
    if !status.success() || value["ok"] != true {
        let mut error = CliError::new(
            value["code"].as_str().unwrap_or("E_RECORDING"),
            bounded_message(
                value["message"]
                    .as_str()
                    .unwrap_or("recording child failed"),
            ),
        );
        error.path = value["path"].as_str().map(str::to_owned);
        return Err(error);
    }
    Ok(value)
}
fn bounded_message(message: &str) -> String {
    if message.len() <= MAX_DIAGNOSTIC_BYTES {
        return message.into();
    }
    let mut end = MAX_DIAGNOSTIC_BYTES - 12;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [truncated]", &message[..end])
}
