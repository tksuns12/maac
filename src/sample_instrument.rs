//! Pitched sample data and key zones for `synth.sample/1` voice-graph nodes.
//!
//! A library `sample` declaration decodes one finite mono WAV, keeps its
//! sample rate, and records a 12-TET root key and an optional forward sustain
//! loop. Playback uses the core linear interpolator; no resampler, crossfade,
//! or normalization is inferred.

use std::io::Cursor;

use serde::{Deserialize, Serialize};

use crate::plan::PlanError;

/// Declarations per library set, matching the wavetable declaration limit.
pub const MAX_LIBRARY_SAMPLE_DECLARATIONS: usize = 64;
/// Zones per `synth.sample/1` node.
pub const MAX_SAMPLE_ZONES: usize = 128;
/// Encoded WAV bytes per declaration.
pub const MAX_SAMPLE_WAV_BYTES: usize = 16 * 1024 * 1024;
/// Root and zone keys use the 12-TET key numbers 0 through 127 (C-1..G9).
pub const MAX_SAMPLE_KEY: i64 = 127;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentSample {
    pub id: String,
    pub rate_hz: u32,
    /// Key at which the sample plays at its recorded rate.
    pub root_key: i64,
    /// Forward sustain loop `[start, end)` in sample frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_frames: Option<[u64; 2]>,
    pub samples: Vec<f32>,
}

/// Provenance of an embedded sample, like a wavetable source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleSource {
    pub sample: String,
    pub file: String,
    pub object: String,
    pub path: String,
    pub hash: String,
}

/// Inclusive key range `[low_key, high_key]` played by one embedded sample.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleZone {
    pub sample: String,
    pub low_key: i64,
    pub high_key: i64,
}

fn error(code: &str, path: impl Into<String>, message: impl Into<String>) -> PlanError {
    PlanError {
        code: code.into(),
        path: path.into(),
        message: message.into(),
        span: None,
    }
}

fn key_in_range(key: i64) -> bool {
    (0..=MAX_SAMPLE_KEY).contains(&key)
}

/// 12-TET frequency of a key number, with A4 = key 69 = 440 Hz.
pub fn key_hz(key: i64) -> f64 {
    440.0 * 2_f64.powf((key as f64 - 69.0) / 12.0)
}

impl InstrumentSample {
    /// Decode a finite mono PCM or binary32-float WAV. Unlike a wavetable,
    /// the WAV sample rate is the recorded playback rate.
    pub fn from_wav(
        id: impl Into<String>,
        bytes: &[u8],
        root_key: i64,
        loop_frames: Option<[u64; 2]>,
    ) -> Result<Self, PlanError> {
        if bytes.len() > MAX_SAMPLE_WAV_BYTES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "sample.wav",
                format!("WAV input exceeds {MAX_SAMPLE_WAV_BYTES} bytes"),
            ));
        }
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).map_err(wav_error)?;
        let spec = reader.spec();
        if spec.channels != 1 {
            return Err(error(
                "E_RANGE",
                "sample.wav.channels",
                format!("sample WAV must be mono, found {} channels", spec.channels),
            ));
        }
        let declared = usize::try_from(reader.duration()).map_err(|_| {
            error(
                "E_RESOURCE_LIMIT",
                "sample.samples",
                "WAV sample count cannot be represented on this platform",
            )
        })?;
        if declared > crate::graph::MAX_EMBEDDED_SAMPLES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "sample.samples",
                format!(
                    "sample has {declared} frames; the embedded limit is {}",
                    crate::graph::MAX_EMBEDDED_SAMPLES
                ),
            ));
        }
        let mut samples = Vec::with_capacity(declared);
        match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Int, bits @ (8 | 16 | 24 | 32)) => {
                let scale = 2_f64.powi(i32::from(bits) - 1);
                for decoded in reader.samples::<i32>() {
                    samples.push((f64::from(decoded.map_err(wav_error)?) / scale) as f32);
                }
            }
            (hound::SampleFormat::Float, 32) => {
                for decoded in reader.samples::<f32>() {
                    samples.push(decoded.map_err(wav_error)?);
                }
            }
            (format, bits) => {
                return Err(error(
                    "E_CAPABILITY",
                    "sample.wav.format",
                    format!("unsupported WAV sample format {format:?}/{bits}-bit"),
                ))
            }
        }
        if samples.len() != declared {
            return Err(error(
                "E_SYNTAX",
                "sample.wav",
                format!(
                    "WAV data decoded {} samples but declared {declared}",
                    samples.len()
                ),
            ));
        }
        let sample = Self {
            id: id.into(),
            rate_hz: spec.sample_rate,
            root_key,
            loop_frames,
            samples,
        };
        sample.validate()?;
        Ok(sample)
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        if self.rate_hz == 0 {
            return Err(error(
                "E_RANGE",
                "sample.rate_hz",
                "sample rate must be positive",
            ));
        }
        if self.samples.is_empty() || self.samples.len() > crate::graph::MAX_EMBEDDED_SAMPLES {
            return Err(error(
                if self.samples.is_empty() {
                    "E_RANGE"
                } else {
                    "E_RESOURCE_LIMIT"
                },
                "sample.samples",
                format!(
                    "sample must have 1 through {} frames",
                    crate::graph::MAX_EMBEDDED_SAMPLES
                ),
            ));
        }
        if let Some(index) = self.samples.iter().position(|value| !value.is_finite()) {
            return Err(error(
                "E_NONFINITE",
                format!("sample.samples[{index}]"),
                "sample values must be finite",
            ));
        }
        if !key_in_range(self.root_key) {
            return Err(error(
                "E_RANGE",
                "sample.root_key",
                format!("root key must be 0 through {MAX_SAMPLE_KEY}"),
            ));
        }
        if let Some([start, end]) = self.loop_frames {
            if start >= end || end > self.samples.len() as u64 {
                return Err(error(
                    "E_RANGE",
                    "sample.loop_frames",
                    "loop must satisfy 0 <= start < end <= frames",
                ));
            }
        }
        Ok(())
    }

    pub fn root_hz(&self) -> f64 {
        key_hz(self.root_key)
    }

    fn at(&self, index: u64) -> f64 {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.samples.get(index))
            .map_or(0.0, |value| f64::from(*value))
    }

    /// Core linear interpolation at a nonnegative source position. Inside a
    /// loop the right neighbour of the last loop frame is the loop start;
    /// otherwise values past the sample are zero.
    pub fn value(&self, position: f64) -> f64 {
        let index = position.floor();
        let fraction = position - index;
        let index = index as u64;
        let right = match self.loop_frames {
            Some([start, end]) if index + 1 == end => start,
            _ => index + 1,
        };
        (1.0 - fraction) * self.at(index) + fraction * self.at(right)
    }

    /// Advance a playback position by a positive step, wrapping inside the
    /// sustain loop once it has been reached.
    pub fn advance(&self, position: f64, step: f64) -> f64 {
        let next = position + step;
        match self.loop_frames {
            Some([start, end]) if next >= end as f64 => {
                let (start, end) = (start as f64, end as f64);
                start + (next - start).rem_euclid(end - start)
            }
            _ => next,
        }
    }
}

/// Validate a node's zones against the embedded sample identifiers.
pub fn validate_zones(
    zones: &[SampleZone],
    path: &str,
    known: impl Fn(&str) -> bool,
) -> Result<(), PlanError> {
    if zones.is_empty() || zones.len() > MAX_SAMPLE_ZONES {
        return Err(error(
            if zones.is_empty() {
                "E_RANGE"
            } else {
                "E_RESOURCE_LIMIT"
            },
            format!("{path}.zones"),
            format!("sample nodes require 1 through {MAX_SAMPLE_ZONES} zones"),
        ));
    }
    let mut previous_high = None;
    for (index, zone) in zones.iter().enumerate() {
        let zone_path = format!("{path}.zones[{index}]");
        if !key_in_range(zone.low_key)
            || !key_in_range(zone.high_key)
            || zone.low_key > zone.high_key
        {
            return Err(error(
                "E_RANGE",
                zone_path,
                format!("zone keys must satisfy 0 <= low <= high <= {MAX_SAMPLE_KEY}"),
            ));
        }
        if previous_high.is_some_and(|high| zone.low_key <= high) {
            return Err(error(
                "E_RANGE",
                zone_path,
                "zones must be listed in ascending, non-overlapping key order",
            ));
        }
        previous_high = Some(zone.high_key);
        if !known(&zone.sample) {
            return Err(error(
                "E_REFERENCE",
                format!("{zone_path}.sample"),
                "zone refers to a missing embedded sample",
            ));
        }
    }
    Ok(())
}

/// The zone whose keys contain the nearest 12-TET key position of
/// `frequency_hz`: `low - 1/2 <= 69 + 12*log2(f/440) < high + 1/2`.
pub fn zone_for<'a>(
    zones: impl IntoIterator<Item = &'a SampleZone>,
    frequency_hz: f64,
) -> Option<usize> {
    let position = 69.0 + 12.0 * (frequency_hz / 440.0).log2();
    zones.into_iter().position(|zone| {
        position >= zone.low_key as f64 - 0.5 && position < zone.high_key as f64 + 0.5
    })
}

fn wav_error(source: hound::Error) -> PlanError {
    match source {
        unsupported @ (hound::Error::TooWide
        | hound::Error::Unsupported
        | hound::Error::InvalidSampleFormat) => error(
            "E_CAPABILITY",
            "sample.wav.format",
            format!("unsupported WAV data: {unsupported}"),
        ),
        malformed => error(
            "E_SYNTAX",
            "sample.wav",
            format!("malformed WAV: {malformed}"),
        ),
    }
}
