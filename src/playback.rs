//! Process-owned offline audition. Rendering and device playback are children
//! of one supervisor; no playback state enters a retained performance plan.

use std::path::PathBuf;

use serde::Serialize;

use crate::cli::{CliError, ProfileArg};

// The request is parsed on every platform so unsupported systems still get
// normal usage errors. Only the macOS supervisor consumes its fields.
#[cfg_attr(not(any(target_os = "macos", all(test, unix))), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct PlayRequest {
    pub input: Option<PathBuf>,
    pub plan: bool,
    pub project_root: Option<PathBuf>,
    pub disk_media: bool,
    pub profile: ProfileArg,
}

#[derive(Debug, Serialize)]
pub(crate) struct PlayResult {
    pub ok: bool,
    pub command: &'static str,
    pub input: String,
    pub backend: &'static str,
    pub output_device: &'static str,
    pub format: &'static str,
    pub status: &'static str,
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: u16,
}

pub(crate) struct PlayFailure {
    pub error: Box<CliError>,
    pub exit_code: i32,
}

impl From<CliError> for PlayFailure {
    fn from(error: CliError) -> Self {
        Self {
            error: Box::new(error),
            exit_code: 1,
        }
    }
}

pub(crate) fn run(request: &PlayRequest) -> Result<PlayResult, PlayFailure> {
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe().map_err(|error| {
            CliError::new(
                "E_PLAYBACK",
                format!("cannot locate MaaC executable: {error}"),
            )
        })?;
        managed::run_with(
            request,
            &executable,
            std::path::Path::new("/usr/bin/afplay"),
            None,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = request;
        Err(CliError::new(
            "E_CAPABILITY",
            "playback requires macOS and /usr/bin/afplay",
        )
        .into())
    }
}

#[cfg(all(unix, any(target_os = "macos", test)))]
mod managed {
    use std::fs::File;
    use std::io::{Read, Seek};
    use std::mem;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::{Mutex, MutexGuard};
    use std::thread;
    use std::time::Duration;

    use super::{CliError, PlayFailure, PlayRequest, PlayResult, ProfileArg};
    use crate::diagnostic::Span;

    const MAX_DIAGNOSTIC_BYTES: u64 = 64 * 1024;
    // One renderer diagnostic can quote a bounded source string, whose JSON
    // escaping expands each input byte to at most six bytes. Read the complete
    // envelope before bounding its message so code/path/span remain available.
    const MAX_RENDER_RESULT_BYTES: u64 =
        6 * crate::syntax::MAX_SOURCE_BYTES as u64 + MAX_DIAGNOSTIC_BYTES;
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
            // SAFETY: zeroed sigactions are initialized before installation;
            // sigaction copies them and writes the previous actions locally.
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

        fn check(&self) -> Result<(), PlayFailure> {
            let signal = INTERRUPTED.load(Ordering::Relaxed);
            if signal == 0 {
                return Ok(());
            }
            Err(PlayFailure {
                error: Box::new(CliError::new(
                    "E_INTERRUPTED",
                    format!(
                        "playback interrupted by {}",
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
        CliError::new(
            "E_PLAYBACK",
            format!(
                "cannot install playback signal handlers: {}",
                std::io::Error::last_os_error()
            ),
        )
    }

    impl Drop for Signals {
        fn drop(&mut self) {
            // SAFETY: saved actions were returned by successful sigaction
            // calls and remain alive for both restoration calls.
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
                .map_err(|error| {
                    CliError::new("E_PLAYBACK", format!("cannot start {stage}: {error}"))
                })?;
            Ok(Self { child: Some(child) })
        }

        fn wait(&mut self, signals: &Signals) -> Result<ExitStatus, PlayFailure> {
            loop {
                signals.check()?;
                match self.child.as_mut().expect("child is active").try_wait() {
                    Ok(Some(status)) => {
                        // try_wait has reaped the child. Disarm before any
                        // subsequent fallible work so a reused pid is safe.
                        self.child.take();
                        signals.check()?;
                        return Ok(status);
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                    Err(error) => {
                        return Err(CliError::new(
                            "E_PLAYBACK",
                            format!("cannot wait for playback child: {error}"),
                        )
                        .into());
                    }
                }
            }
        }
    }

    impl Drop for ManagedChild {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                // SAFETY: each child owns a new process group named by its
                // unreaped pid. The parent is outside that group. Kill the
                // group before reaping and before the private directory drops.
                unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    pub(super) fn run_with(
        request: &PlayRequest,
        renderer: &Path,
        backend: &Path,
        temporary_parent: Option<&Path>,
    ) -> Result<PlayResult, PlayFailure> {
        let signals = Signals::install()?;
        signals.check()?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("maac-play-");
        let temporary = match temporary_parent {
            Some(parent) => builder.tempdir_in(parent),
            None => builder.tempdir(),
        }
        .map_err(|error| CliError::new("E_IO", format!("cannot stage playback: {error}")))?;
        let result = audition(request, renderer, backend, temporary.path(), &signals);
        // audition has dropped (and reaped) every managed child before close.
        let temporary_path = temporary.path().to_owned();
        let cleanup = temporary.close().map_err(|error| {
            let mut failure = CliError::new(
                "E_IO",
                format!(
                    "private playback cleanup failed; files may remain at {}: {error}",
                    temporary_path.display()
                ),
            );
            failure.path = Some(temporary_path.display().to_string());
            failure
        });
        let result = match signals.check() {
            Ok(()) => result,
            Err(interrupted) => Err(interrupted),
        };
        match (result, cleanup) {
            (Err(mut failure), Err(cleanup)) => {
                let detail =
                    bounded_message_to(&cleanup.message, MAX_DIAGNOSTIC_BYTES as usize - 2);
                let maximum = MAX_DIAGNOSTIC_BYTES as usize - detail.len() - 2;
                failure.error.message = format!(
                    "{}; {detail}",
                    bounded_message_to(&failure.error.message, maximum)
                );
                Err(failure)
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
            (Ok(result), Ok(())) => Ok(result),
        }
    }

    fn audition(
        request: &PlayRequest,
        renderer: &Path,
        backend: &Path,
        temporary: &Path,
        signals: &Signals,
    ) -> Result<PlayResult, PlayFailure> {
        let wave = temporary.join("audition.wav");
        let render_stdout = temporary.join("render.stdout");
        let render_stderr = temporary.join("render.stderr");
        let mut render = Command::new(renderer);
        render
            .arg("--json")
            .arg(if request.plan { "render" } else { "build" });
        render.args(["--format", "float32", "--profile"]);
        render.arg(match request.profile {
            ProfileArg::Default => "default",
            ProfileArg::Song => "song",
        });
        render.arg("-o").arg(&wave);
        if let Some(root) = &request.project_root {
            let mut root_argument = std::ffi::OsString::from("--project-root=");
            root_argument.push(root.as_os_str());
            render.arg(root_argument);
        }
        if request.disk_media {
            render.arg("--disk-media");
        }
        if let Some(input) = &request.input {
            render.arg("--").arg(input);
        }
        redirect(&mut render, &render_stdout, &render_stderr)?;
        signals.check()?;
        let status = ManagedChild::spawn(&mut render, "MaaC renderer")?.wait(signals)?;
        let rendered = read_json(&render_stdout)?;
        if !status.success() || rendered["ok"] != true {
            return Err(render_error(&rendered, status).into());
        }
        let file = File::open(&wave).map_err(|error| {
            CliError::new(
                "E_PLAYBACK",
                format!("cannot open rendered playback WAV: {error}"),
            )
        })?;
        let file_bytes = file.metadata().map_err(private_io_error)?.len();
        let reader = hound::WavReader::new(file).map_err(|error| {
            CliError::new(
                "E_PLAYBACK",
                format!("cannot read rendered playback WAV: {error}"),
            )
        })?;
        let spec = reader.spec();
        let frames = u64::from(reader.duration());
        let samples = u64::from(reader.len());
        let data_start = reader
            .into_inner()
            .stream_position()
            .map_err(private_io_error)?;
        if rendered["frames"].as_u64() != Some(frames)
            || spec.sample_format != hound::SampleFormat::Float
            || spec.bits_per_sample != 32
            || spec.channels == 0
            || spec.sample_rate == 0
            || samples % u64::from(spec.channels) != 0
            || data_start.checked_add(samples * 4) != Some(file_bytes)
        {
            return Err(CliError::new(
                "E_PLAYBACK",
                "renderer returned inconsistent playback WAV metadata",
            )
            .into());
        }
        signals.check()?;
        let backend_stdout = temporary.join("backend.stdout");
        let backend_stderr = temporary.join("backend.stderr");
        let mut player = Command::new(backend);
        player.arg(&wave);
        redirect(&mut player, &backend_stdout, &backend_stderr)?;
        signals.check()?;
        let status = ManagedChild::spawn(&mut player, "macOS afplay backend")?.wait(signals)?;
        if !status.success() {
            let detail = match read_diagnostic(&backend_stderr) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).trim().to_owned(),
                Err(error) => error.to_string(),
            };
            return Err(CliError::new(
                "E_PLAYBACK",
                bounded_message(&format!(
                    "macOS afplay backend failed ({status}){}",
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    }
                )),
            )
            .into());
        }
        signals.check()?;
        Ok(PlayResult {
            ok: true,
            command: "play",
            input: rendered["input"].as_str().unwrap_or("main.maac").to_owned(),
            backend: "macos-afplay",
            output_device: "system-default",
            format: "float32",
            status: "completed",
            frames,
            sample_rate: spec.sample_rate,
            channels: spec.channels,
        })
    }

    fn redirect(command: &mut Command, stdout: &Path, stderr: &Path) -> Result<(), CliError> {
        let stdout = File::create(stdout).map_err(private_io_error)?;
        let stderr = File::create(stderr).map_err(private_io_error)?;
        command
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        Ok(())
    }

    fn private_io_error(error: std::io::Error) -> CliError {
        CliError::new(
            "E_IO",
            format!("cannot access private playback diagnostics: {error}"),
        )
    }

    fn read_diagnostic(path: &Path) -> Result<Vec<u8>, CliError> {
        let file = File::open(path).map_err(private_io_error)?;
        let mut bytes = Vec::new();
        file.take(MAX_DIAGNOSTIC_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(private_io_error)?;
        Ok(bytes)
    }

    fn read_json(path: &Path) -> Result<serde_json::Value, CliError> {
        let file = File::open(path).map_err(private_io_error)?;
        let mut bytes = Vec::new();
        file.take(MAX_RENDER_RESULT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(private_io_error)?;
        if bytes.len() as u64 > MAX_RENDER_RESULT_BYTES {
            return Err(CliError::new(
                "E_PLAYBACK",
                "renderer diagnostics exceed the bounded playback envelope",
            ));
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            CliError::new(
                "E_PLAYBACK",
                format!("renderer returned invalid playback diagnostics: {error}"),
            )
        })
    }

    fn bounded_message(message: &str) -> String {
        bounded_message_to(message, MAX_DIAGNOSTIC_BYTES as usize)
    }

    fn bounded_message_to(message: &str, maximum: usize) -> String {
        if message.len() <= maximum {
            return message.to_owned();
        }
        let suffix = " [truncated]";
        if maximum < suffix.len() {
            return suffix[..maximum].to_owned();
        }
        let mut end = maximum - suffix.len();
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{suffix}", &message[..end])
    }

    fn render_error(value: &serde_json::Value, status: ExitStatus) -> CliError {
        let mut error = CliError::new(
            value["code"].as_str().unwrap_or("E_PLAYBACK"),
            value["message"]
                .as_str()
                .map(bounded_message)
                .unwrap_or_else(|| format!("MaaC renderer failed ({status})")),
        );
        error.path = value["path"].as_str().map(str::to_owned);
        error.span = value["span"]["start"].as_u64().and_then(|start| {
            let end = value["span"]["end"].as_u64()?;
            Some(Span::new(
                usize::try_from(start).ok()?,
                usize::try_from(end).ok()?,
            ))
        });
        error
    }
}

#[cfg(all(test, unix))]
#[path = "playback_tests.rs"]
mod tests;
