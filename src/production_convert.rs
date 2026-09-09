//! Certified, bounded production SRC and sample encoding. No full-PCM allocation.
//!
//! The caller supplies random access to the 48 kHz graph spool; this converter
//! does not own graph execution or publication. The coefficient certificate and
//! reproducible interval generator are in `production_src/` and
//! `scripts/production_src_coefficients.py`. Rust's ordinary floating operations
//! retain multiply/add order (no explicit `mul_add`, fast-math or reassociation).

use std::fmt;

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive};
use sha2::{Digest, Sha256};

use crate::exact::{Rational, MAX_RATIONAL_BITS};

pub const CONVERTER_ID: &str = "maac.src.kaiser/1";
pub const COEFFICIENT_GENERATOR_ID: &str = "production_src_coefficients.py/1";
pub const COEFFICIENT_CERTIFICATE_JSON: &str = include_str!("production_src/certificate.json");
pub const ARITHMETIC_ID: &str = "ieee754-binary64-nearest-ties-even-ordered-no-fma";
pub const DITHER_ID: &str = "maac.dither.sha256-tpdf/1";
const TABLE_44100: &[u8] = include_bytes!("production_src/44100.f64le");
const TABLE_96000: &[u8] = include_bytes!("production_src/96000.f64le");
const DIGEST_44100: &str = "0e1eb42b6d2c0bdb486f812fa61378da64cfb55fdb574417c52067aa3c71adde";
const DIGEST_96000: &str = "423d049d3e87af8c3cb3db01d8923cb189248b6113dfe1b10e22763188d7e9af";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversionError {
    Rate,
    Channels,
    Duration,
    ResourceLimit,
    Frame,
    Nonfinite,
    Overload,
    Dither,
    Format,
    CoefficientTable,
    Io(String),
}

impl ConversionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Rate | Self::Channels => "E_CAPABILITY",
            Self::Duration | Self::Frame => "E_RANGE",
            Self::ResourceLimit => "E_RESOURCE_LIMIT",
            Self::Nonfinite => "E_NONFINITE",
            Self::Overload => "E_OVERLOAD",
            Self::Dither => "E_DITHER",
            Self::Format => "E_FORMAT",
            Self::CoefficientTable => "E_HASH",
            Self::Io(_) => "E_IO",
        }
    }
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?}", self.code(), self)
    }
}

impl std::error::Error for ConversionError {}

/// Caller-tightenable conversion allowances. Work is conservatively charged per
/// channel and supported input position, including zero-extension positions.
#[derive(Clone, Copy, Debug)]
pub struct ConversionLimits {
    pub max_output_frames: u64,
    pub max_work: u64,
    pub max_coefficient_bytes: usize,
}

impl Default for ConversionLimits {
    fn default() -> Self {
        Self {
            max_output_frames: 96_000 * 30 * 60,
            max_work: 500_000_000,
            max_coefficient_bytes: 327_688,
        }
    }
}

/// Exact ceil(rate * duration), independent of rounded engine-frame count.
pub fn delivery_frames(rate: u32, duration: &Rational) -> Result<u64, ConversionError> {
    if !matches!(rate, 44_100 | 48_000 | 96_000) {
        return Err(ConversionError::Rate);
    }
    if duration.is_negative() {
        return Err(ConversionError::Duration);
    }
    if duration.numer().bits() > MAX_RATIONAL_BITS || duration.denom().bits() > MAX_RATIONAL_BITS {
        return Err(ConversionError::ResourceLimit);
    }
    (duration.numer() * BigInt::from(rate))
        .div_ceil(duration.denom())
        .to_u64()
        .ok_or(ConversionError::ResourceLimit)
}

/// Immutable converter; one sample call uses O(1) additional storage. Spool
/// readers may cache a bounded window without changing accumulation order.
#[derive(Debug)]
pub struct Converter {
    rate: u32,
    channels: u8,
    engine_frames: u64,
    output_frames: u64,
    up: i128,
    down: i128,
    half_length: i128,
    table: &'static [u8],
    digest: Option<&'static str>,
    work: u64,
}

impl Converter {
    pub fn new(
        rate: u32,
        duration: &Rational,
        channels: u8,
        limits: ConversionLimits,
    ) -> Result<Self, ConversionError> {
        if !matches!(channels, 1 | 2) {
            return Err(ConversionError::Channels);
        }
        let output_frames = delivery_frames(rate, duration)?;
        let engine_frames = delivery_frames(48_000, duration)?;
        let (up, down, half_length, table, digest) = match rate {
            48_000 => (1, 1, 0, &[][..], None),
            44_100 => (147, 160, 40_960, TABLE_44100, Some(DIGEST_44100)),
            96_000 => (2, 1, 512, TABLE_96000, Some(DIGEST_96000)),
            _ => return Err(ConversionError::Rate),
        };
        let support = if rate == 48_000 {
            1
        } else {
            2 * half_length / up + 2
        };
        let work = output_frames
            .checked_mul(u64::from(channels))
            .and_then(|frames| frames.checked_mul(support as u64))
            .ok_or(ConversionError::ResourceLimit)?;
        if output_frames > limits.max_output_frames
            || work > limits.max_work
            || table.len() > limits.max_coefficient_bytes
        {
            return Err(ConversionError::ResourceLimit);
        }
        if let Some(expected) = digest {
            if format!("{:x}", Sha256::digest(table)) != expected
                || table.len() != (half_length as usize + 1) * 8
            {
                return Err(ConversionError::CoefficientTable);
            }
        }
        Ok(Self {
            rate,
            channels,
            engine_frames,
            output_frames,
            up,
            down,
            half_length,
            table,
            digest,
            work,
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }
    pub fn channels(&self) -> u8 {
        self.channels
    }
    pub fn engine_frames(&self) -> u64 {
        self.engine_frames
    }
    pub fn output_frames(&self) -> u64 {
        self.output_frames
    }
    pub fn work(&self) -> u64 {
        self.work
    }
    /// None identifies exact 48 kHz passthrough, which has no coefficients.
    pub fn coefficient_digest(&self) -> Option<&'static str> {
        self.digest
    }

    fn coefficient(&self, q: i128) -> f64 {
        let offset = q.unsigned_abs() as usize * 8;
        f64::from_le_bytes(
            self.table[offset..offset + 8]
                .try_into()
                .expect("bounded coefficient"),
        )
    }

    pub fn sample<F>(
        &self,
        frame: u64,
        channel: usize,
        read: &mut F,
    ) -> Result<f64, ConversionError>
    where
        F: FnMut(u64, usize) -> Result<f64, ConversionError>,
    {
        if frame >= self.output_frames || channel >= usize::from(self.channels) {
            return Err(ConversionError::Frame);
        }
        if self.rate == 48_000 {
            let value = read(frame, channel)?;
            return if value.is_finite() {
                Ok(value)
            } else {
                Err(ConversionError::Nonfinite)
            };
        }
        // i128 safely contains u64 frame indices multiplied by the fixed Ds.
        let center = i128::from(frame) * self.down;
        let first = -(-(center - self.half_length)).div_euclid(self.up);
        let last = (center + self.half_length).div_euclid(self.up);
        let mut sum = 0.0;
        for k in first..=last {
            let x = if k < 0 || k >= i128::from(self.engine_frames) {
                0.0
            } else {
                read(k as u64, channel)?
            };
            let product = x * self.coefficient(center - k * self.up);
            sum += product;
            if !x.is_finite() || !product.is_finite() || !sum.is_finite() {
                return Err(ConversionError::Nonfinite);
            }
        }
        Ok(sum)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Float32,
    Pcm16,
    Pcm24,
}

impl Encoding {
    pub fn identity(self) -> &'static str {
        match self {
            Self::Float32 => "wav_f32le",
            Self::Pcm16 => "wav_pcm16le",
            Self::Pcm24 => "wav_pcm24le",
        }
    }
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::Float32 => 4,
            Self::Pcm16 => 2,
            Self::Pcm24 => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dither {
    None,
    Tpdf {
        seed: u64,
        delivery_id: String,
        target_id: String,
    },
}

impl Dither {
    pub fn identity(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Tpdf { .. } => DITHER_ID,
        }
    }
    pub fn seed(&self) -> Option<u64> {
        match self {
            Self::None => None,
            Self::Tpdf { seed, .. } => Some(*seed),
        }
    }
    pub fn validate(&self, encoding: Encoding) -> Result<(), ConversionError> {
        if let Self::Tpdf {
            delivery_id,
            target_id,
            ..
        } = self
        {
            if encoding == Encoding::Float32 || !valid_id(delivery_id) || !valid_id(target_id) {
                return Err(ConversionError::Dither);
            }
        }
        Ok(())
    }
    pub fn counts(&self, frame: u64, channel: usize) -> Result<f64, ConversionError> {
        match self {
            Self::None => Ok(0.0),
            Self::Tpdf {
                seed,
                delivery_id,
                target_id,
            } => {
                if !valid_id(delivery_id) || !valid_id(target_id) || channel > 1 {
                    return Err(ConversionError::Dither);
                }
                let uniform = |draw: u8| {
                    let input = format!("{DITHER_ID}\n{seed}\n{delivery_id}\n{target_id}\n{frame}\n{channel}\n{draw}\n");
                    let hash = Sha256::digest(input.as_bytes());
                    let bits =
                        u64::from_be_bytes(hash[..8].try_into().expect("digest length")) >> 11;
                    bits as f64 / 9_007_199_254_740_992.0
                };
                Ok(uniform(0) - uniform(1))
            }
        }
    }
}

fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// Quantize integer PCM counts. .round() uses halfway-away semantics without
/// the erroneous intermediate addition of 0.5 at representable neighbors.
pub fn quantize(sample: f64, bits: u8, dither_counts: f64) -> Result<i32, ConversionError> {
    if !matches!(bits, 16 | 24) {
        return Err(ConversionError::Format);
    }
    if !sample.is_finite() || !dither_counts.is_finite() {
        return Err(ConversionError::Nonfinite);
    }
    if !(-1.0..=1.0).contains(&sample) {
        return Err(ConversionError::Overload);
    }
    let maximum = ((1_u32 << (bits - 1)) - 1) as f64;
    let scaled = sample * maximum;
    let value = scaled + dither_counts;
    if !value.is_finite() {
        return Err(ConversionError::Nonfinite);
    }
    if value.abs() > maximum {
        return Err(ConversionError::Overload);
    }
    Ok(value.round() as i32)
}

#[derive(Clone, Copy, Debug)]
pub struct EncodedSample {
    pub bytes: [u8; 4],
    pub len: usize,
    /// Exact WAV reconstruction, usable by an artifact-byte decoder. This is
    /// not a substitute for authoritative analysis of the published artifact.
    pub reconstructed: f64,
}

pub fn encode_sample(
    sample: f64,
    encoding: Encoding,
    dither: &Dither,
    frame: u64,
    channel: usize,
) -> Result<EncodedSample, ConversionError> {
    dither.validate(encoding)?;
    if channel > 1 {
        return Err(ConversionError::Channels);
    }
    if !sample.is_finite() {
        return Err(ConversionError::Nonfinite);
    }
    let (bytes, reconstructed) = match encoding {
        Encoding::Float32 => {
            let v = sample as f32;
            if !v.is_finite() {
                return Err(ConversionError::Nonfinite);
            }
            (v.to_le_bytes(), f64::from(v))
        }
        Encoding::Pcm16 | Encoding::Pcm24 => {
            let bits = if encoding == Encoding::Pcm16 { 16 } else { 24 };
            let q = quantize(sample, bits, dither.counts(frame, channel)?)?;
            (
                q.to_le_bytes(),
                f64::from(q) / f64::from(1_u32 << (bits - 1)),
            )
        }
    };
    Ok(EncodedSample {
        bytes,
        len: encoding.bytes_per_sample(),
        reconstructed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exact::parse_rational;
    use serde_json::Value;

    fn fixtures() -> Value {
        serde_json::from_str(include_str!("../production-conformance.json")).unwrap()
    }
    fn real(value: &Value) -> f64 {
        parse_rational(value.as_str().unwrap())
            .unwrap()
            .to_f64()
            .unwrap()
    }
    fn converter(rate: u32) -> Converter {
        Converter::new(
            rate,
            &parse_rational("1").unwrap(),
            1,
            ConversionLimits::default(),
        )
        .unwrap()
    }

    #[test]
    fn independent_quantization_vectors_and_half_neighbors() {
        for case in fixtures()["quantization"].as_array().unwrap() {
            let d = case.get("dither_counts").map_or(0.0, real);
            let result = quantize(
                real(&case["input"]),
                case["bits"].as_u64().unwrap() as u8,
                d,
            );
            if case.get("error").is_some() {
                assert_eq!(result, Err(ConversionError::Overload));
            } else {
                assert_eq!(
                    result.unwrap(),
                    case["expected_integer"].as_i64().unwrap() as i32
                );
            }
        }
        let below = f64::from_bits(0.5_f64.to_bits() - 1);
        let above = f64::from_bits(0.5_f64.to_bits() + 1);
        for bits in [16, 24] {
            assert_eq!(quantize(0.0, bits, below).unwrap(), 0);
            assert_eq!(quantize(0.0, bits, -below).unwrap(), 0);
            assert_eq!(quantize(0.0, bits, 0.5).unwrap(), 1);
            assert_eq!(quantize(0.0, bits, -0.5).unwrap(), -1);
            assert_eq!(quantize(0.0, bits, above).unwrap(), 1);
            assert_eq!(
                quantize(f64::NAN, bits, 0.0),
                Err(ConversionError::Nonfinite)
            );
        }
    }

    #[test]
    fn pinned_dither_hashes_and_stateless_draws() {
        for case in fixtures()["dither"].as_array().unwrap() {
            let seed = case["seed"].as_u64().unwrap();
            let delivery = case["delivery"].as_str().unwrap();
            let target = case["target"].as_str().unwrap();
            let frame = case["frame"].as_u64().unwrap();
            let channel = case["channel"].as_u64().unwrap() as usize;
            let mut uniform = [0.0; 2];
            for (j, u) in uniform.iter_mut().enumerate() {
                let data =
                    format!("{DITHER_ID}\n{seed}\n{delivery}\n{target}\n{frame}\n{channel}\n{j}\n");
                let hash = Sha256::digest(data.as_bytes());
                assert_eq!(
                    format!("{hash:x}"),
                    case[format!("sha256_j{j}")].as_str().unwrap()
                );
                *u = (u64::from_be_bytes(hash[..8].try_into().unwrap()) >> 11) as f64
                    / 9_007_199_254_740_992.0;
            }
            let dither = Dither::Tpdf {
                seed,
                delivery_id: delivery.into(),
                target_id: target.into(),
            };
            assert_eq!(
                dither.counts(frame, channel).unwrap().to_bits(),
                (uniform[0] - uniform[1]).to_bits()
            );
            let _other = dither.counts(frame + 10, 1).unwrap();
            assert_eq!(
                dither.counts(frame, channel).unwrap().to_bits(),
                (uniform[0] - uniform[1]).to_bits()
            );
        }
        assert!(Dither::Tpdf {
            seed: u64::MAX,
            delivery_id: "a\nb".into(),
            target_id: "m".into()
        }
        .counts(0, 0)
        .is_err());
    }

    #[test]
    fn encoded_bytes_reconstruct_wav_scaling_and_reject_overload() {
        let pcm16 = encode_sample(1.0, Encoding::Pcm16, &Dither::None, 0, 0).unwrap();
        assert_eq!(&pcm16.bytes[..pcm16.len], &[255, 127]);
        assert_eq!(pcm16.reconstructed, 32767.0 / 32768.0);
        let pcm24 = encode_sample(-1.0, Encoding::Pcm24, &Dither::None, 0, 0).unwrap();
        assert_eq!(&pcm24.bytes[..pcm24.len], &[1, 0, 128]);
        assert_eq!(pcm24.reconstructed, -8388607.0 / 8388608.0);
        assert_eq!(
            encode_sample(4.0, Encoding::Float32, &Dither::None, 0, 0)
                .unwrap()
                .reconstructed,
            4.0
        );
        assert!(encode_sample(f64::MAX, Encoding::Float32, &Dither::None, 0, 0).is_err());
        let d = Dither::Tpdf {
            seed: 1,
            delivery_id: "release_cd".into(),
            target_id: "master".into(),
        };
        assert!(encode_sample(-1.0, Encoding::Pcm24, &d, 0, 0).is_err());
        assert!(encode_sample(0.0, Encoding::Float32, &d, 0, 0).is_err());
        let halfway = 1.0 + 2_f64.powi(-24);
        assert_eq!(
            encode_sample(halfway, Encoding::Float32, &Dither::None, 0, 0)
                .unwrap()
                .reconstructed,
            1.0
        );
    }

    #[test]
    fn exact_frame_counts_and_preflight_limits() {
        for case in fixtures()["frame_counts"].as_array().unwrap() {
            let duration = parse_rational(case["duration_seconds"].as_str().unwrap()).unwrap();
            assert_eq!(
                delivery_frames(case["rate"].as_u64().unwrap() as u32, &duration).unwrap(),
                case["expected_frames"].as_u64().unwrap()
            );
        }
        let d = parse_rational("1/100000").unwrap();
        let c = Converter::new(96000, &d, 1, ConversionLimits::default()).unwrap();
        assert_eq!((c.engine_frames(), c.output_frames()), (1, 1));
        assert!(Converter::new(
            96000,
            &d,
            1,
            ConversionLimits {
                max_work: 0,
                ..ConversionLimits::default()
            }
        )
        .is_err());
        assert!(Converter::new(
            44100,
            &d,
            1,
            ConversionLimits {
                max_coefficient_bytes: 327687,
                ..ConversionLimits::default()
            }
        )
        .is_err());
        assert!(Converter::new(48000, &d, 3, ConversionLimits::default()).is_err());
        assert!(delivery_frames(48000, &parse_rational("-1").unwrap()).is_err());
    }

    #[test]
    fn exact_passthrough_retains_bits_and_callback_bounds() {
        let c = converter(48000);
        let values = [-0.0, f64::MIN_POSITIVE, f64::from_bits(1), -1.25, 1.0];
        for (n, value) in values.iter().enumerate() {
            let mut calls = 0;
            let actual = c
                .sample(n as u64, 0, &mut |frame, channel| {
                    assert_eq!((frame, channel), (n as u64, 0));
                    calls += 1;
                    Ok(*value)
                })
                .unwrap();
            assert_eq!(actual.to_bits(), value.to_bits());
            assert_eq!(calls, 1);
        }
        assert_eq!(c.coefficient_digest(), None);
        assert_eq!(
            c.sample(48000, 0, &mut |_, _| panic!("outside interval")),
            Err(ConversionError::Frame)
        );
        assert_eq!(
            c.sample(0, 0, &mut |_, _| Ok(f64::INFINITY)),
            Err(ConversionError::Nonfinite)
        );
    }

    #[test]
    fn every_phase_has_unit_dc_gain_and_centered_impulse() {
        for rate in [44100, 96000] {
            let c = converter(rate);
            for phase in 0..c.up as u64 {
                let actual = c
                    .sample(rate as u64 / 2 + phase, 0, &mut |_, _| Ok(1.0))
                    .unwrap();
                assert!(
                    // Up to 558 ordered additions introduce accumulated
                    // rounding beyond the single coefficient rounding.
                    (actual - 1.0).abs() < 1e-13,
                    "{rate} phase {phase}: {actual}"
                );
            }
            // Input k=640 aligns exactly at n=588 (44.1k) or n=1280 (96k).
            let center = 640 * c.up as u64 / c.down as u64;
            let mut impulse = |k, _| Ok(if k == 640 { 1.0 } else { 0.0 });
            assert_eq!(c.sample(center, 0, &mut impulse).unwrap(), c.coefficient(0));
            for offset in [1, 2, 10, 100] {
                let a = c.sample(center - offset, 0, &mut impulse).unwrap();
                let b = c.sample(center + offset, 0, &mut impulse).unwrap();
                assert_eq!(a.to_bits(), b.to_bits());
            }
        }
    }

    #[test]
    fn zero_extension_never_reads_outside_spool_and_propagates_failure() {
        for rate in [44100, 96000] {
            let c = Converter::new(
                rate,
                &parse_rational("1/100000").unwrap(),
                1,
                ConversionLimits::default(),
            )
            .unwrap();
            assert_eq!(
                c.sample(0, 0, &mut |k, _| {
                    assert_eq!(k, 0);
                    Ok(1.0)
                })
                .unwrap(),
                c.coefficient(0)
            );
            assert_eq!(
                c.sample(0, 0, &mut |_, _| Err(ConversionError::Io("read".into()))),
                Err(ConversionError::Io("read".into()))
            );
            assert_eq!(
                c.sample(0, 0, &mut |_, _| Ok(f64::NAN)),
                Err(ConversionError::Nonfinite)
            );
        }
    }

    #[test]
    fn independently_sampled_kernel_passband_and_stopband() {
        for rate in [44100, 96000] {
            let c = converter(rate);
            let m = c.up.max(c.down) as f64;
            // Independent symmetric Fourier evaluation, not the sample loop.
            let response = |frequency: f64| {
                let mut sum = c.coefficient(0);
                for q in 1..=c.half_length {
                    sum += 2.0
                        * c.coefficient(q)
                        * (std::f64::consts::TAU * frequency * q as f64).cos();
                }
                sum / c.up as f64
            };
            for f in [0.0, 0.1, 0.2, 0.3, 0.4, 0.44] {
                assert!((response(f / m) - 1.0).abs() < 1e-6, "passband {rate} {f}");
            }
            for f in [0.5, 0.55, 0.7, 1.0] {
                assert!(
                    response(f / m).abs() < 1e-6,
                    "stopband {rate} {f}: {}",
                    response(f / m)
                );
            }
            for f in [0.1, 0.25, 0.499] {
                if f > 0.5 / m {
                    assert!(response(f).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn independent_tone_phase_and_alias_rejection() {
        for rate in [44100, 96000] {
            let c = converter(rate);
            let mut source =
                |k, _| Ok((std::f64::consts::TAU * 10000.0 * k as f64 / 48000.0).sin());
            for n in 1000..1200 {
                let expected = (std::f64::consts::TAU * 10000.0 * n as f64 / rate as f64).sin();
                assert!((c.sample(n, 0, &mut source).unwrap() - expected).abs() < 1e-6);
            }
        }
        let c = converter(44100);
        let mut source = |k, _| Ok((std::f64::consts::TAU * 23000.0 * k as f64 / 48000.0).sin());
        for n in 1000..1200 {
            assert!(c.sample(n, 0, &mut source).unwrap().abs() < 1e-6);
        }
    }
}
