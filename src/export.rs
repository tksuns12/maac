//! Bounded streaming WAV export for validated performance plans.
//!
//! Export is deliberately a separate boundary from DSP. The renderer emits
//! binary64 interleaved frames; this module performs the explicit float32 or
//! PCM16 conversion and publishes completed files atomically.

use std::fmt;
use std::fs;
use std::io::{self, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};
use tempfile::NamedTempFile;

use crate::dsp::{self, RenderError};
use crate::plan::Plan;

pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;

/// The two explicit WAV encodings supported by the foundation renderer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WavFormat {
    /// IEEE binary32 samples. Finite binary64 values that overflow binary32
    /// are rejected; no gain adjustment or normalization is applied.
    Float32,
    /// Signed little-endian PCM16. Raw binary64 samples must be finite and in
    /// `[-1, 1]`; quantization is nearest integer of `sample * 32767.0`.
    /// Thus `1.0` maps to `32767` and `-1.0` maps to `-32767`. No clipping,
    /// normalization, or dithering occurs.
    Pcm16,
}

impl WavFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Float32 => "float32",
            Self::Pcm16 => "pcm16",
        }
    }

    pub const fn sample_format(self) -> SampleFormat {
        match self {
            Self::Float32 => SampleFormat::Float,
            Self::Pcm16 => SampleFormat::Int,
        }
    }
}

impl fmt::Display for WavFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for WavFormat {
    type Err = ExportError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "float32" => Ok(Self::Float32),
            "pcm16" => Ok(Self::Pcm16),
            _ => Err(ExportError::InvalidFormat(format!(
                "unsupported WAV format `{value}`"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WavStats {
    pub frames: u64,
    pub channels: u8,
    pub format: WavFormat,
}

/// Export failures have stable codes for CLI and library callers.
#[derive(Debug)]
pub enum ExportError {
    Io {
        message: String,
    },
    OutputExists {
        path: PathBuf,
    },
    InvalidFormat(String),
    Nonfinite {
        frame: u64,
        channel: usize,
    },
    Pcm16Overload {
        frame: u64,
        channel: usize,
        sample: f64,
    },
    FrameShape {
        expected: usize,
        actual: usize,
    },
    Wav {
        message: String,
    },
    Render(RenderError),
}

impl ExportError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "E_IO",
            Self::OutputExists { .. } => "E_OUTPUT_EXISTS",
            Self::InvalidFormat(_) => "E_FORMAT",
            Self::Nonfinite { .. } => "E_NONFINITE",
            Self::Pcm16Overload { .. } => "E_PCM16_RANGE",
            Self::FrameShape { .. } => "E_RENDER_STATE",
            Self::Wav { .. } => "E_WAV",
            Self::Render(error) => error.code(),
        }
    }

    fn from_hound(error: hound::Error) -> Self {
        match error {
            hound::Error::IoError(error) => Self::Io {
                message: error.to_string(),
            },
            other => Self::Wav {
                message: other.to_string(),
            },
        }
    }
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { message } => write!(f, "E_IO: {message}"),
            Self::OutputExists { path } => {
                write!(f, "E_OUTPUT_EXISTS: destination already exists: {}", path.display())
            }
            Self::InvalidFormat(message) => write!(f, "E_FORMAT: {message}"),
            Self::Nonfinite { frame, channel } => {
                write!(f, "E_NONFINITE: sample at frame {frame}, channel {channel} is nonfinite")
            }
            Self::Pcm16Overload {
                frame,
                channel,
                sample,
            } => write!(
                f,
                "E_PCM16_RANGE: sample {sample} at frame {frame}, channel {channel} is outside [-1, 1]"
            ),
            Self::FrameShape { expected, actual } => write!(
                f,
                "E_RENDER_STATE: renderer emitted {actual} channels; expected {expected}"
            ),
            Self::Wav { message } => write!(f, "E_WAV: {message}"),
            Self::Render(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<RenderError> for ExportError {
    fn from(value: RenderError) -> Self {
        Self::Render(value)
    }
}

impl From<io::Error> for ExportError {
    fn from(value: io::Error) -> Self {
        Self::Io {
            message: value.to_string(),
        }
    }
}

/// Stream a validated plan into a WAV writer. The writer must be seekable
/// because Hound patches the RIFF data length on `finalize`.
pub fn write_wav<W>(sink: W, plan: &Plan, format: WavFormat) -> Result<WavStats, ExportError>
where
    W: Write + Seek,
{
    // Validate before Hound writes its RIFF header.  This keeps the generic
    // writer boundary transactional for callers that supply their own sink,
    // and prevents invalid channel/rate settings from reaching the encoder.
    plan.validate()
        .map_err(RenderError::Plan)
        .map_err(ExportError::Render)?;
    let channels = plan.output.channels;
    let spec = WavSpec {
        channels: u16::from(channels),
        sample_rate: plan.output.sample_rate_hz,
        bits_per_sample: match format {
            WavFormat::Float32 => 32,
            WavFormat::Pcm16 => 16,
        },
        sample_format: format.sample_format(),
    };
    // Hound's generic constructor intentionally does not buffer.  Keep the
    // public sink generic while avoiding one syscall per interleaved sample;
    // `finalize` flushes this buffer before it returns.
    let mut writer = WavWriter::new(BufWriter::new(sink), spec).map_err(ExportError::from_hound)?;
    let mut frames = 0u64;
    let mut write_error: Option<ExportError> = None;
    let render_result = dsp::render(plan, |frame| {
        if frame.len() != usize::from(channels) {
            let error = ExportError::FrameShape {
                expected: usize::from(channels),
                actual: frame.len(),
            };
            write_error = Some(error);
            return Err(RenderError::Callback(
                "WAV frame channel count mismatch".into(),
            ));
        }
        for (channel, sample) in frame.iter().copied().enumerate() {
            let result = match format {
                WavFormat::Float32 => {
                    let value = sample as f32;
                    if sample.is_finite() && value.is_finite() {
                        writer.write_sample(value).map_err(ExportError::from_hound)
                    } else {
                        Err(ExportError::Nonfinite {
                            frame: frames,
                            channel,
                        })
                    }
                }
                WavFormat::Pcm16 => {
                    if !sample.is_finite() {
                        Err(ExportError::Nonfinite {
                            frame: frames,
                            channel,
                        })
                    } else if !(-1.0..=1.0).contains(&sample) {
                        Err(ExportError::Pcm16Overload {
                            frame: frames,
                            channel,
                            sample,
                        })
                    } else {
                        let value = quantize_pcm16(sample);
                        writer.write_sample(value).map_err(ExportError::from_hound)
                    }
                }
            };
            if let Err(error) = result {
                write_error = Some(error);
                return Err(RenderError::Callback("WAV sample write failed".into()));
            }
        }
        frames += 1;
        Ok(())
    });
    if let Some(error) = write_error {
        return Err(error);
    }
    render_result.map_err(ExportError::Render)?;
    writer.finalize().map_err(ExportError::from_hound)?;
    Ok(WavStats {
        frames,
        channels,
        format,
    })
}

/// Quantize a validated PCM16 sample without clipping or dithering.  The
/// representable MaaC range maps `-1` to `-32767` and `+1` to `+32767`;
/// this symmetric policy leaves the extra two's-complement code unused.
fn quantize_pcm16(sample: f64) -> i16 {
    (sample * 32_767.0).round() as i16
}

/// Render and atomically publish a WAV file in the destination directory.
/// Existing destinations are protected unless `force` is true, and all
/// failures before publication leave the destination unchanged.
pub fn render_wav_to_path(
    plan: &Plan,
    path: impl AsRef<Path>,
    format: WavFormat,
    force: bool,
) -> Result<WavStats, ExportError> {
    let path = path.as_ref();
    if !force && path_exists(path)? {
        return Err(ExportError::OutputExists {
            path: path.to_owned(),
        });
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent).map_err(ExportError::from)?;
    let stats = write_wav(temporary.as_file_mut(), plan, format)?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(ExportError::from)?;
    if force {
        temporary.persist(path).map_err(|error| ExportError::Io {
            message: error.error.to_string(),
        })?;
    } else {
        temporary.persist_noclobber(path).map_err(|error| {
            if path_exists(path).unwrap_or(false) {
                ExportError::OutputExists {
                    path: path.to_owned(),
                }
            } else {
                ExportError::Io {
                    message: error.error.to_string(),
                }
            }
        })?;
    }
    Ok(stats)
}

/// Atomically write a completed plan or other bounded artifact. This helper
/// shares the same no-overwrite and same-directory publication policy as WAV.
pub fn atomic_write(path: impl AsRef<Path>, bytes: &[u8], force: bool) -> Result<(), ExportError> {
    let path = path.as_ref();
    if !force && path_exists(path)? {
        return Err(ExportError::OutputExists {
            path: path.to_owned(),
        });
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent).map_err(ExportError::from)?;
    temporary.write_all(bytes).map_err(ExportError::from)?;
    temporary.as_file_mut().flush().map_err(ExportError::from)?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(ExportError::from)?;
    if force {
        temporary.persist(path).map_err(|error| ExportError::Io {
            message: error.error.to_string(),
        })?;
    } else {
        temporary.persist_noclobber(path).map_err(|error| {
            if path_exists(path).unwrap_or(false) {
                ExportError::OutputExists {
                    path: path.to_owned(),
                }
            } else {
                ExportError::Io {
                    message: error.error.to_string(),
                }
            }
        })?;
    }
    Ok(())
}

pub fn path_exists(path: &Path) -> Result<bool, ExportError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ExportError::from(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Node, OutputSettings, PortRef, Processor, TempoMap, TempoPoint};
    use num_rational::BigRational;

    fn rat(numerator: i64, denominator: i64) -> BigRational {
        BigRational::new(numerator.into(), denominator.into())
    }

    fn one_note_plan() -> Plan {
        Plan {
            version: 1,
            output: OutputSettings {
                score_start_q: rat(0, 1),
                score_end_q: rat(1, 1),
                tail_seconds: rat(0, 1),
                sample_rate_hz: 48_000,
                channels: 1,
                total_frames: 24_000,
                output: PortRef::new("sine", "out").unwrap(),
            },
            tempo: TempoMap {
                points: vec![TempoPoint {
                    q: rat(0, 1),
                    bpm: rat(120, 1),
                    shape: crate::plan::Interpolation::Step,
                }],
            },
            events: vec![crate::plan::ResolvedEvent {
                address: "main/0/n1".into(),
                source: crate::plan::SourceMapping {
                    object: "n1".into(),
                    path: vec!["main".into(), "n1".into()],
                    span: None,
                },
                target: PortRef::new("sine", "events").unwrap(),
                kind: crate::plan::EventKind::Note {
                    pitch_hz: 440.0,
                    velocity: rat(1, 1),
                },
                score_on_q: rat(0, 1),
                score_off_q: Some(rat(1, 2)),
                onset_offset_seconds: rat(0, 1),
                release_offset_seconds: rat(0, 1),
                on_seconds: rat(0, 1),
                off_seconds: Some(rat(1, 4)),
                release_velocity: 0.0,
                on_frame: 0,
                off_frame: Some(12_000),
                order: 0,
            }],
            nodes: vec![Node::new("sine", Processor::sine(4)).unwrap()],
            connections: Vec::new(),
            automation: Vec::new(),
            regions: Vec::new(),
            source_mappings: Vec::new(),
            instruments: None,
        }
    }

    #[test]
    fn streams_a_finite_float_wav_with_a_valid_header() {
        let plan = one_note_plan();
        let mut sink = io::Cursor::new(Vec::<u8>::new());
        let stats = write_wav(&mut sink, &plan, WavFormat::Float32).unwrap();
        assert_eq!(stats.frames, 24_000);
        assert_eq!(stats.channels, 1);
        let bytes = sink.into_inner();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert!(bytes[44..].iter().any(|byte| *byte != 0));
    }
}
