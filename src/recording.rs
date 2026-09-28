//! Process-owned, bounded microphone capture. Only a fully checked retained
//! disk import can cross the final atomic publication boundary.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cli::{CliError, ProfileArg};

const RATE: u32 = 48_000;
#[cfg(any(target_os = "macos", all(test, unix)))]
const MAX_METADATA_BYTES: usize = 16 * 1024;
#[cfg(any(target_os = "macos", all(test, unix)))]
const BACKEND: &str = "macos-audioqueue/1";
const CHILD_ARGUMENT: &str = "--maac-private-recording-child";

#[derive(Debug)]
pub(crate) struct RecordRequest {
    pub duration_seconds: u32,
    pub output_dir: PathBuf,
    pub profile: ProfileArg,
}

#[derive(Debug, Serialize)]
pub(crate) struct RecordResult {
    pub ok: bool,
    pub command: &'static str,
    pub status: &'static str,
    pub backend: &'static str,
    pub input_device: InputDevice,
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub format: &'static str,
    pub output: String,
    pub recording_format: &'static str,
    pub digest: String,
}

pub(crate) struct RecordFailure {
    pub error: Box<CliError>,
    pub exit_code: i32,
}

impl From<CliError> for RecordFailure {
    fn from(error: CliError) -> Self {
        Self {
            error: Box::new(error),
            exit_code: 1,
        }
    }
}

#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecordingMetadata {
    format: String,
    version: u32,
    backend: String,
    requested_duration_seconds: u32,
    delivered: Delivered,
    input_device: InputDevice,
    origin: Origin,
    latency: Latency,
    continuity: Continuity,
    data_sha256: String,
}

#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Delivered {
    encoding: String,
    rate_hz: u32,
    channels: u16,
    frames: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputDevice {
    selection: String,
    uid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hardware_rate_hz: Option<f64>,
}

#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    kind: String,
    source_frame: u64,
}
#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Latency {
    #[serde(deserialize_with = "required_nullable_frames")]
    measured_input_frames: Option<u64>,
    compensation_frames: u64,
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn required_nullable_frames<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Option::<u64>::deserialize(deserializer)
}
#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Continuity {
    policy: String,
    detected_discontinuities: u64,
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn recording_error(message: impl Into<String>) -> CliError {
    CliError::new("E_RECORDING", message)
}

fn preflight(request: &RecordRequest) -> Result<(), CliError> {
    if !(1..=1800).contains(&request.duration_seconds) {
        return Err(CliError::new(
            "E_USAGE",
            "recording duration must be whole seconds in 1..1800",
        ));
    }
    if crate::export::path_exists(&request.output_dir).map_err(CliError::from_export)? {
        return Err(CliError::from_export(
            crate::export::ExportError::OutputExists {
                path: request.output_dir.clone(),
            },
        ));
    }
    let parent = output_parent(&request.output_dir);
    if !std::fs::metadata(parent)
        .map_err(|error| {
            CliError::new(
                "E_IO",
                format!(
                    "cannot inspect recording output parent {}: {error}",
                    parent.display()
                ),
            )
        })?
        .is_dir()
    {
        return Err(CliError::new(
            "E_IO",
            "recording output parent is not a directory",
        ));
    }
    // The generated import is one full-length mono forward rate clip. The
    // authoritative plan audio_execution_work charges (4+1)+(32+8)=45/frame.
    // Reject this predictable budget failure before requesting microphone access;
    // the ordinary importer still performs the authoritative validation later.
    let work = u64::from(request.duration_seconds) * u64::from(RATE) * 45;
    if work > request.profile.limits().max_execution_work {
        return Err(CliError::new("E_RESOURCE_LIMIT", "recorded project exceeds this execution-work profile; use --profile song for longer recordings"));
    }
    Ok(())
}

fn output_parent(output: &Path) -> &Path {
    output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub(crate) fn run(request: &RecordRequest) -> Result<RecordResult, RecordFailure> {
    preflight(request)?;
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe()
            .map_err(|error| recording_error(format!("cannot locate MaaC executable: {error}")))?;
        managed::run_with(request, &executable, None)
    }
    #[cfg(not(target_os = "macos"))]
    Err(CliError::new("E_CAPABILITY", "microphone recording requires macOS").into())
}

/// An unadvertised same-executable child protocol. It accepts no backend,
/// source, device, or format override and never publishes a user project.
pub(crate) fn private_child(args: &[OsString]) -> Option<i32> {
    if args.get(1).is_none_or(|arg| arg != CHILD_ARGUMENT) {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        let result = (|| {
            if args.len() != 4 {
                return Err(recording_error("invalid private capture request"));
            }
            let duration = args[2]
                .to_str()
                .and_then(|arg| arg.parse::<u32>().ok())
                .filter(|duration| (1..=1800).contains(duration))
                .ok_or_else(|| recording_error("invalid private capture duration"))?;
            native::capture(duration, Path::new(&args[3]))
        })();
        match result {
            Ok(()) => {
                println!("{{\"ok\":true}}");
                Some(0)
            }
            Err(error) => {
                println!(
                    "{}",
                    serde_json::to_string(&error).expect("capture error is serializable")
                );
                Some(1)
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(1)
    }
}

#[cfg(any(target_os = "macos", all(test, unix)))]
mod managed;
#[cfg(target_os = "macos")]
mod native;
#[cfg(any(target_os = "macos", all(test, unix)))]
mod stream;
#[cfg(all(test, unix))]
mod tests;
