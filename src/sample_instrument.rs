//! Pitched sample data and key zones for `synth.sample/1` voice-graph nodes.
//!
//! A library `sample` declaration either decodes one finite mono WAV into the
//! plan or names a mono core PCM file that the plan carries as an ordinary
//! (optionally disk-backed) audio asset. Either way it keeps its sample rate
//! and records a 12-TET root key and an optional forward sustain loop.
//! Playback uses the core linear interpolator; no resampler, crossfade, or
//! normalization is inferred.

use std::io::Cursor;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::exact::Rational;
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
    /// Embedded values; empty when the data is a plan audio asset.
    pub samples: Vec<f32>,
    /// Plan audio asset holding the data instead of `samples`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<SampleAsset>,
}

/// Reference from an asset-backed sample to its mono plan audio asset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleAsset {
    pub id: String,
    pub frames: u64,
}

/// Frames an asset-backed sample may declare: the disk-media byte ceiling.
pub const MAX_SAMPLE_ASSET_FRAMES: u64 = crate::disk_media::MAX_DISK_MEDIA_TOTAL_BYTES / 4;

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

/// Inclusive key range `[low_key, high_key]` played by one sample, with an
/// optional velocity layer and per-edge crossfade widths.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleZone {
    pub sample: String,
    pub low_key: i64,
    pub high_key: i64,
    /// Velocity layer `[low, high)`; `high = 1` includes velocity 1.
    /// Absent means the whole range `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<SamplePair>,
    /// Fade widths in semitones inward from the low and high key edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_fade: Option<SamplePair>,
    /// Fade widths inward from the low and high velocity edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity_fade: Option<SamplePair>,
}

/// An exact `[low, high]` pair of dimensionless rationals.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplePair {
    #[serde(with = "crate::plan::rational_serde")]
    pub low: Rational,
    #[serde(with = "crate::plan::rational_serde")]
    pub high: Rational,
}

impl SamplePair {
    fn f64s(&self) -> (f64, f64) {
        (
            self.low.to_f64().unwrap_or(f64::NAN),
            self.high.to_f64().unwrap_or(f64::NAN),
        )
    }
}

/// Crossfade curve applied to each axis gain of a sample zone.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFadeShape {
    #[default]
    Linear,
    EqualPower,
}

impl SampleFadeShape {
    pub fn is_linear(&self) -> bool {
        *self == Self::Linear
    }
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
            asset: None,
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
        match &self.asset {
            None if self.samples.is_empty()
                || self.samples.len() > crate::graph::MAX_EMBEDDED_SAMPLES =>
            {
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
            None => {}
            Some(asset) => {
                crate::plan::validate_identifier(&asset.id, "sample.asset.id")?;
                if !self.samples.is_empty()
                    || asset.frames == 0
                    || asset.frames > MAX_SAMPLE_ASSET_FRAMES
                {
                    return Err(error(
                        "E_RANGE",
                        "sample.asset",
                        "an asset-backed sample has no embedded values and 1 or more frames",
                    ));
                }
            }
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
            if start >= end || end > self.frames() {
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

    /// Recorded frames, embedded or in the referenced asset.
    pub fn frames(&self) -> u64 {
        self.asset
            .as_ref()
            .map_or(self.samples.len() as u64, |asset| asset.frames)
    }
}

/// Runtime sample data: embedded values or a shared, possibly disk-backed,
/// mono audio buffer.
#[derive(Debug)]
pub(crate) struct SamplePlayback {
    sample: InstrumentSample,
    buffer: Option<Arc<crate::audio_buffer::AudioBuffer>>,
}

impl SamplePlayback {
    pub(crate) fn embedded(sample: InstrumentSample) -> Self {
        Self {
            sample,
            buffer: None,
        }
    }

    /// Bind an asset-backed sample to its validated buffer.
    pub(crate) fn from_buffer(
        sample: InstrumentSample,
        buffer: Arc<crate::audio_buffer::AudioBuffer>,
    ) -> Result<Self, PlanError> {
        if buffer.channels() != 1
            || buffer.rate_hz() != sample.rate_hz
            || buffer.frames() != sample.frames()
        {
            return Err(error(
                "E_ASSET",
                "instruments.samples.asset",
                "sample asset must be mono with the declared rate and frames",
            ));
        }
        Ok(Self {
            sample,
            buffer: Some(buffer),
        })
    }

    pub(crate) fn sample(&self) -> &InstrumentSample {
        &self.sample
    }

    fn at(&self, index: u64) -> Result<f64, PlanError> {
        if index >= self.sample.frames() {
            return Ok(0.0);
        }
        match &self.buffer {
            Some(buffer) => buffer.interpolate(index, 0.0, 0),
            None => Ok(f64::from(self.sample.samples[index as usize])),
        }
    }

    /// Core linear interpolation at a nonnegative source position. Inside a
    /// loop the right neighbour of the last loop frame is the loop start;
    /// otherwise values past the sample are zero.
    pub(crate) fn value(&self, position: f64) -> Result<f64, PlanError> {
        let index = position.floor();
        let fraction = position - index;
        let index = index as u64;
        let right = match self.sample.loop_frames {
            Some([start, end]) if index + 1 == end => start,
            _ => index + 1,
        };
        Ok((1.0 - fraction) * self.at(index)? + fraction * self.at(right)?)
    }

    /// Advance a playback position by a positive step, wrapping inside the
    /// sustain loop once it has been reached.
    pub(crate) fn advance(&self, position: f64, step: f64) -> f64 {
        let next = position + step;
        match self.sample.loop_frames {
            Some([start, end]) if next >= end as f64 => {
                let (start, end) = (start as f64, end as f64);
                start + (next - start).rem_euclid(end - start)
            }
            _ => next,
        }
    }
}

/// Validate a node's zones against the embedded sample identifiers. Zones
/// may overlap: every zone containing a note plays at its crossfade gain.
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
        let (v0, v1) = velocity_bounds(zone);
        if v0.is_negative() || v0 >= v1 || v1 > Rational::one() {
            return Err(error(
                "E_RANGE",
                format!("{zone_path}.velocity"),
                "zone velocity must satisfy 0 <= low < high <= 1",
            ));
        }
        let key_span = Rational::from_integer((zone.high_key - zone.low_key + 1).into());
        for (fade, span, name) in [
            (&zone.key_fade, key_span, "key_fade"),
            (&zone.velocity_fade, &v1 - &v0, "velocity_fade"),
        ] {
            if let Some(fade) = fade {
                if fade.low.is_negative()
                    || fade.high.is_negative()
                    || &fade.low + &fade.high > span
                {
                    return Err(error(
                        "E_RANGE",
                        format!("{zone_path}.{name}"),
                        format!("{name} widths must be nonnegative and fit inside the zone"),
                    ));
                }
            }
        }
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

fn velocity_bounds(zone: &SampleZone) -> (Rational, Rational) {
    zone.velocity.as_ref().map_or_else(
        || (Rational::zero(), Rational::one()),
        |pair| (pair.low.clone(), pair.high.clone()),
    )
}

/// Gain of one axis: zero outside `[a, b)`, rising over `fade_low` from `a`
/// and falling over `fade_high` towards `b`, the lower of the two ramps.
fn axis_gain(
    x: f64,
    a: f64,
    b: f64,
    inside: bool,
    fades: (f64, f64),
    shape: SampleFadeShape,
) -> f64 {
    if !inside {
        return 0.0;
    }
    let (fade_low, fade_high) = fades;
    let mut gain: f64 = 1.0;
    if fade_low > 0.0 {
        gain = gain.min((x - a) / fade_low);
    }
    if fade_high > 0.0 {
        gain = gain.min((b - x) / fade_high);
    }
    let gain = gain.clamp(0.0, 1.0);
    match shape {
        SampleFadeShape::Linear => gain,
        SampleFadeShape::EqualPower => (std::f64::consts::FRAC_PI_2 * gain).sin(),
    }
}

/// Every zone that plays a note, with its gain. The key position is the
/// nearest 12-TET position `69 + 12*log2(f/440)`; a zone's key axis covers
/// `[low - 1/2, high + 1/2)` and its velocity axis `[v0, v1)`, including
/// velocity 1 when `v1 = 1`. The gain is the product of both axis gains.
pub fn zone_gains(
    zones: &[SampleZone],
    frequency_hz: f64,
    velocity: f64,
    shape: SampleFadeShape,
) -> Vec<(usize, f64)> {
    let position = 69.0 + 12.0 * (frequency_hz / 440.0).log2();
    zones
        .iter()
        .enumerate()
        .filter_map(|(index, zone)| {
            let (a, b) = (zone.low_key as f64 - 0.5, zone.high_key as f64 + 0.5);
            let key_fades = zone.key_fade.as_ref().map_or((0.0, 0.0), SamplePair::f64s);
            let key = axis_gain(
                position,
                a,
                b,
                position >= a && position < b,
                key_fades,
                shape,
            );
            let (v0, v1) = velocity_bounds(zone);
            let (v0, top) = (v0.to_f64().unwrap_or(f64::NAN), v1.is_one());
            let v1 = v1.to_f64().unwrap_or(f64::NAN);
            let inside = velocity >= v0 && (velocity < v1 || (top && velocity == v1));
            let velocity_fades = zone
                .velocity_fade
                .as_ref()
                .map_or((0.0, 0.0), SamplePair::f64s);
            let gain = key * axis_gain(velocity, v0, v1, inside, velocity_fades, shape);
            (gain > 0.0).then_some((index, gain))
        })
        .collect()
}

/// Expand `synth.sample/1` config defaults in a normalized typed record:
/// `fade_shape = linear` and, per zone, `velocity = [0, 1]` and zero
/// `key_fade`/`velocity_fade`. Shared by the editing and execution views.
pub(crate) fn expand_config_defaults(config: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::json;
    let number = |n: i64| json!({"t": "number", "n": n.to_string(), "d": "1"});
    let pair = |low: i64, high: i64| json!({"t": "list", "items": [number(low), number(high)]});
    config
        .entry("fade_shape")
        .or_insert_with(|| json!({"t": "symbol", "v": "linear"}));
    if let Some(items) = config
        .get_mut("zones")
        .and_then(|zones| zones.get_mut("items"))
        .and_then(serde_json::Value::as_array_mut)
    {
        for zone in items {
            if let Some(fields) = zone
                .get_mut("fields")
                .and_then(serde_json::Value::as_object_mut)
            {
                fields.entry("velocity").or_insert_with(|| pair(0, 1));
                fields.entry("key_fade").or_insert_with(|| pair(0, 0));
                fields.entry("velocity_fade").or_insert_with(|| pair(0, 0));
            }
        }
    }
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
