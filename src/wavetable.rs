//! Embedded wavetable data and its reusable, versioned harmonic table bank.

use crate::dsp::RenderError;
use crate::plan::PlanError;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

pub const TABLE_BANK_VERSION: u32 = 1;
pub const MAX_WAVETABLE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_WAVETABLE_SAMPLES: usize = 2_048 * 32;

const MIN_CYCLE_LENGTH: u32 = 8;
const MAX_CYCLE_LENGTH: u32 = 2_048;
const MAX_FRAMES: usize = 32;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wavetable {
    pub id: String,
    pub cycle_length: u32,
    pub samples: Vec<f64>,
}

impl Wavetable {
    /// Decode complete, explicitly sized cycles from a finite mono PCM or
    /// binary32-float WAV. The WAV sample-rate field is intentionally ignored.
    pub fn from_wav(
        id: impl Into<String>,
        bytes: &[u8],
        cycle_length: u32,
    ) -> Result<Self, PlanError> {
        validate_cycle_length(cycle_length)?;
        if bytes.len() > MAX_WAVETABLE_BYTES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "wavetable.wav",
                format!(
                    "WAV input is {} bytes; the per-asset limit is {MAX_WAVETABLE_BYTES}",
                    bytes.len()
                ),
            ));
        }

        let mut reader = hound::WavReader::new(Cursor::new(bytes)).map_err(wav_error)?;
        let spec = reader.spec();
        if spec.channels != 1 {
            return Err(error(
                "E_RANGE",
                "wavetable.wav.channels",
                format!(
                    "wavetable WAV must be mono, found {} channels",
                    spec.channels
                ),
            ));
        }
        match spec.sample_format {
            hound::SampleFormat::Int if matches!(spec.bits_per_sample, 8 | 16 | 24 | 32) => {}
            hound::SampleFormat::Float if spec.bits_per_sample == 32 => {}
            _ => {
                return Err(error(
                    "E_CAPABILITY",
                    "wavetable.wav.format",
                    format!(
                        "unsupported WAV sample format {:?}/{}-bit",
                        spec.sample_format, spec.bits_per_sample
                    ),
                ))
            }
        }

        // `duration` comes from the data-chunk length. Check it before reserving
        // any sample storage, then still require the decoder to produce exactly
        // that many values so truncated chunks cannot become partial tables.
        let sample_count = usize::try_from(reader.duration()).map_err(|_| {
            error(
                "E_RESOURCE_LIMIT",
                "wavetable.samples",
                "WAV sample count cannot be represented on this platform",
            )
        })?;
        validate_sample_count(sample_count, cycle_length)?;

        // Decode once without retaining values. This validates the complete
        // declared data chunk, its concrete hound container representation,
        // and float finiteness before allocating the embedded sample vector.
        preflight_samples(&mut reader, spec, sample_count)?;
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).map_err(wav_error)?;
        let mut samples = Vec::with_capacity(sample_count);
        match spec.sample_format {
            hound::SampleFormat::Int => {
                let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
                for decoded in reader.samples::<i32>() {
                    let value = decoded.map_err(wav_error)? as f64 / scale;
                    samples.push(value);
                }
            }
            hound::SampleFormat::Float => {
                for decoded in reader.samples::<f32>() {
                    let value = f64::from(decoded.map_err(wav_error)?);
                    if !value.is_finite() {
                        return Err(error(
                            "E_NONFINITE",
                            format!("wavetable.samples[{}]", samples.len()),
                            "wavetable sample must be finite",
                        ));
                    }
                    samples.push(value);
                }
            }
        }
        if samples.len() != sample_count {
            return Err(error(
                "E_SYNTAX",
                "wavetable.wav",
                format!(
                    "WAV data decoded {} samples but declared {sample_count}",
                    samples.len()
                ),
            ));
        }

        let table = Self {
            id: id.into(),
            cycle_length,
            samples,
        };
        table.validate()?;
        Ok(table)
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        validate_cycle_length(self.cycle_length)?;
        validate_sample_count(self.samples.len(), self.cycle_length)?;
        for (index, sample) in self.samples.iter().enumerate() {
            if !sample.is_finite() {
                return Err(error(
                    "E_NONFINITE",
                    format!("wavetable.samples[{index}]"),
                    "wavetable sample must be finite",
                ));
            }
        }
        Ok(())
    }
}

fn validate_cycle_length(cycle_length: u32) -> Result<(), PlanError> {
    if !(MIN_CYCLE_LENGTH..=MAX_CYCLE_LENGTH).contains(&cycle_length)
        || !cycle_length.is_power_of_two()
    {
        return Err(error(
            "E_RANGE",
            "wavetable.cycle_length",
            format!(
                "cycle length {cycle_length} must be a power of two from {MIN_CYCLE_LENGTH} through {MAX_CYCLE_LENGTH}"
            ),
        ));
    }
    Ok(())
}

fn validate_sample_count(sample_count: usize, cycle_length: u32) -> Result<(), PlanError> {
    let cycle_length = cycle_length as usize;
    if sample_count > MAX_WAVETABLE_SAMPLES {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "wavetable.samples",
            format!("wavetable has {sample_count} samples; the limit is {MAX_WAVETABLE_SAMPLES}"),
        ));
    }
    if sample_count == 0 || !sample_count.is_multiple_of(cycle_length) {
        return Err(error(
            "E_RANGE",
            "wavetable.samples",
            format!("sample count {sample_count} is not a positive number of complete cycles"),
        ));
    }
    let frames = sample_count / cycle_length;
    if !(1..=MAX_FRAMES).contains(&frames) {
        return Err(error(
            "E_RANGE",
            "wavetable.samples",
            format!("wavetable has {frames} frames; expected 1 through {MAX_FRAMES}"),
        ));
    }
    Ok(())
}

fn wav_error(source: hound::Error) -> PlanError {
    match source {
        unsupported @ (hound::Error::TooWide
        | hound::Error::Unsupported
        | hound::Error::InvalidSampleFormat) => error(
            "E_CAPABILITY",
            "wavetable.wav.format",
            format!("unsupported WAV data: {unsupported}"),
        ),
        malformed => error(
            "E_SYNTAX",
            "wavetable.wav",
            format!("invalid WAV data: {malformed}"),
        ),
    }
}

fn preflight_samples(
    reader: &mut hound::WavReader<Cursor<&[u8]>>,
    spec: hound::WavSpec,
    expected: usize,
) -> Result<(), PlanError> {
    let mut decoded = 0_usize;
    match spec.sample_format {
        hound::SampleFormat::Int => {
            for sample in reader.samples::<i32>() {
                sample.map_err(wav_error)?;
                decoded += 1;
            }
        }
        hound::SampleFormat::Float => {
            for sample in reader.samples::<f32>() {
                let value = sample.map_err(wav_error)?;
                if !value.is_finite() {
                    return Err(error(
                        "E_NONFINITE",
                        format!("wavetable.samples[{decoded}]"),
                        "wavetable sample must be finite",
                    ));
                }
                decoded += 1;
            }
        }
    }
    if decoded != expected {
        return Err(error(
            "E_SYNTAX",
            "wavetable.wav",
            format!("WAV data decoded {decoded} samples but declared {expected}"),
        ));
    }
    Ok(())
}

fn error(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> PlanError {
    PlanError {
        code: code.into(),
        path: path.into(),
        message: message.into(),
        span: None,
    }
}

/// Precomputed band-limited cycles. Version 1 uses power-of-two harmonic
/// ceilings from `cycle_length / 2` down through one. Every level retains DC.
/// The `cycle_length / 2` Nyquist bin is a single real FFT bin and is retained
/// only in the richest level; there is no peak normalization. Zero frequency
/// selects that richest level.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBank {
    cycle_length: usize,
    frame_count: usize,
    harmonic_limits: Vec<usize>,
    tables: Vec<f64>,
}

impl TableBank {
    /// Validate embedded source data and compute all reusable bank levels once.
    pub fn new(wavetable: &Wavetable) -> Result<Self, PlanError> {
        wavetable.validate()?;
        let cycle_length = wavetable.cycle_length as usize;
        let frame_count = wavetable.samples.len() / cycle_length;

        let mut harmonic_limits = Vec::new();
        let mut limit = cycle_length / 2;
        while limit != 0 {
            harmonic_limits.push(limit);
            limit /= 2;
        }

        let table_values = wavetable
            .samples
            .len()
            .checked_mul(harmonic_limits.len())
            .ok_or_else(|| {
                error(
                    "E_RESOURCE_LIMIT",
                    "wavetable.table_bank",
                    "harmonic table-bank size overflows",
                )
            })?;
        // At the maximum cycle size there are 11 levels, so this is a fixed
        // upper bound of 720,896 f64 values regardless of input byte metadata.
        const MAX_TABLE_VALUES: usize = MAX_WAVETABLE_SAMPLES * 11;
        if table_values > MAX_TABLE_VALUES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "wavetable.table_bank",
                "harmonic table bank exceeds its preprocessing limit",
            ));
        }
        let mut tables = vec![0.0; table_values];
        let level_stride = wavetable.samples.len();

        for frame in 0..frame_count {
            let source_start = frame * cycle_length;
            let source = &wavetable.samples[source_start..source_start + cycle_length];

            // Preserve the richest legal table byte-for-value. It is exactly
            // the inverse DFT containing every real-signal bin, while avoiding
            // needless roundoff in the common low-frequency path.
            tables[source_start..source_start + cycle_length].copy_from_slice(source);

            let mut spectrum: Vec<Complex> = source
                .iter()
                .copied()
                .map(|re| Complex { re, im: 0.0 })
                .collect();
            fft(&mut spectrum, false);
            if spectrum.iter().any(|value| !value.is_finite()) {
                return Err(error(
                    "E_NONFINITE",
                    "wavetable.table_bank",
                    "wavetable magnitude overflows harmonic preprocessing",
                ));
            }

            for (level_index, &harmonic_limit) in harmonic_limits.iter().enumerate().skip(1) {
                let mut filtered = vec![Complex::default(); cycle_length];
                filtered[0] = spectrum[0];
                for harmonic in 1..=harmonic_limit {
                    filtered[harmonic] = spectrum[harmonic];
                    if harmonic != cycle_length - harmonic {
                        filtered[cycle_length - harmonic] = spectrum[cycle_length - harmonic];
                    }
                }
                fft(&mut filtered, true);
                let destination = level_index * level_stride + source_start;
                for (offset, value) in filtered.into_iter().enumerate() {
                    let sample = value.re / cycle_length as f64;
                    if !sample.is_finite() {
                        return Err(error(
                            "E_NONFINITE",
                            "wavetable.table_bank",
                            "wavetable magnitude overflows harmonic preprocessing",
                        ));
                    }
                    tables[destination + offset] = sample;
                }
            }
        }

        Ok(Self {
            cycle_length,
            frame_count,
            harmonic_limits,
            tables,
        })
    }

    pub fn version(&self) -> u32 {
        TABLE_BANK_VERSION
    }

    pub fn sample(
        &self,
        phase_cycles: f64,
        position: f64,
        frequency_hz: f64,
        rate: f64,
    ) -> Result<f64, RenderError> {
        if !phase_cycles.is_finite() {
            return Err(RenderError::Nonfinite(
                "wavetable phase must be finite".into(),
            ));
        }
        if !position.is_finite() || !(0.0..=1.0).contains(&position) {
            return Err(RenderError::Nonfinite(format!(
                "wavetable position {position} must be finite and within 0..=1"
            )));
        }
        if !frequency_hz.is_finite() || !(-24_000.0..=24_000.0).contains(&frequency_hz) {
            return Err(RenderError::Nonfinite(format!(
                "wavetable frequency {frequency_hz} must be finite and within -24000..=24000 Hz"
            )));
        }
        if !rate.is_finite() || rate != 48_000.0 {
            return Err(RenderError::Nonfinite(format!(
                "wavetable sample rate {rate} must be 48000 Hz"
            )));
        }

        let absolute_frequency = frequency_hz.abs();
        let nyquist = rate * 0.5;
        let level = self
            .harmonic_limits
            .iter()
            .position(|&limit| limit as f64 * absolute_frequency <= nyquist)
            .unwrap_or(self.harmonic_limits.len() - 1);

        let phase = phase_cycles.rem_euclid(1.0) * self.cycle_length as f64;
        let phase_floor = phase.floor();
        // Floating rem_euclid may round a tiny negative input to exactly 1.0.
        // Canonicalize the resulting endpoint before indexing the cyclic table.
        let sample_index = phase_floor as usize % self.cycle_length;
        let phase_mix = phase - phase_floor;
        let next_sample = (sample_index + 1) % self.cycle_length;

        let frame_position = position * (self.frame_count - 1) as f64;
        let first_frame = frame_position.floor() as usize;
        let second_frame = (first_frame + 1).min(self.frame_count - 1);
        let frame_mix = frame_position - first_frame as f64;

        let first = self.sample_at(level, first_frame, sample_index, next_sample, phase_mix);
        let second = self.sample_at(level, second_frame, sample_index, next_sample, phase_mix);
        let sample = first + (second - first) * frame_mix;
        if !sample.is_finite() {
            return Err(RenderError::Nonfinite(
                "wavetable interpolation produced a nonfinite sample".into(),
            ));
        }
        Ok(sample)
    }

    fn sample_at(
        &self,
        level: usize,
        frame: usize,
        sample: usize,
        next_sample: usize,
        mix: f64,
    ) -> f64 {
        let base = (level * self.frame_count + frame) * self.cycle_length;
        let first = self.tables[base + sample];
        first + (self.tables[base + next_sample] - first) * mix
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

/// In-place radix-2 FFT. Wavetable validation guarantees power-of-two lengths,
/// keeping preprocessing O(frames * levels * N log N) under fixed bounds.
fn fft(values: &mut [Complex], inverse: bool) {
    let len = values.len();
    let mut reversed = 0;
    for index in 1..len {
        let mut bit = len >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            values.swap(index, reversed);
        }
    }

    let mut span = 2;
    while span <= len {
        let angle = if inverse { 1.0 } else { -1.0 } * std::f64::consts::TAU / span as f64;
        let step = Complex {
            re: angle.cos(),
            im: angle.sin(),
        };
        for block in (0..len).step_by(span) {
            let mut twiddle = Complex { re: 1.0, im: 0.0 };
            for offset in 0..span / 2 {
                let even = values[block + offset];
                let odd = multiply(values[block + offset + span / 2], twiddle);
                values[block + offset] = Complex {
                    re: even.re + odd.re,
                    im: even.im + odd.im,
                };
                values[block + offset + span / 2] = Complex {
                    re: even.re - odd.re,
                    im: even.im - odd.im,
                };
                twiddle = multiply(twiddle, step);
            }
        }
        span *= 2;
    }
}

fn multiply(left: Complex, right: Complex) -> Complex {
    Complex {
        re: left.re * right.re - left.im * right.im,
        im: left.re * right.im + left.im * right.re,
    }
}
