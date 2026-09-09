//! Streaming final-artifact measurements for the production analysis profile.
//!
//! This implements the pinned estimator, not a full EBU Mode conformance claim.
use std::{fmt, path::Path};

use num_rational::BigRational;
use num_traits::ToPrimitive;
use serde::Serialize;

pub const ANALYZER_ID: &str = "maac.analysis.bs1770-5/2";
pub const TRUE_PEAK_PROFILE: &str = "maac.truepeak.bs1770-5.annex2-4x/1";

#[derive(Clone, Debug, Serialize)]
pub struct NumericalEnvironment {
    pub implementation: String,
    pub build: String,
    pub platform: String,
    pub math_library: String,
    pub arithmetic_mode: String,
    pub coefficient_identity: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LoudnessMeasurement {
    pub status: String,
    pub unit: String,
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PeakMeasurement {
    pub status: String,
    pub unit: String,
    pub amplitude: f64,
    pub value: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub analyzer: String,
    pub true_peak_profile: String,
    pub numerical_environment: NumericalEnvironment,
    pub rate: u32,
    pub channels: u8,
    pub frames: u64,
    pub true_peak_frames: u64,
    pub integrated_loudness: LoudnessMeasurement,
    pub sample_peak: PeakMeasurement,
    pub true_peak: PeakMeasurement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisError {
    pub code: &'static str,
    pub message: String,
}
impl fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for AnalysisError {}
fn error(code: &'static str, message: impl Into<String>) -> AnalysisError {
    AnalysisError {
        code,
        message: message.into(),
    }
}
fn finite(value: f64) -> Result<f64, AnalysisError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(error(
            "E_PRODUCTION_ANALYSIS_NONFINITE",
            "nonfinite analysis input or intermediate result",
        ))
    }
}

// Hosts can change the floating-point environment. Refuse a detected departure
// from the declared profile instead of reporting measurements with a false
// arithmetic identity. black_box keeps these operations at runtime.
fn validate_numerical_environment() -> Result<(), AnalysisError> {
    use std::hint::black_box;
    let one = black_box(1.0_f64);
    let half_ulp = black_box(f64::EPSILON / 2.0);
    let tiny = black_box(f64::from_bits(1));
    let normal = black_box(f64::MIN_POSITIVE);
    if one + half_ulp != 1.0
        || one + black_box(3.0 * half_ulp) != 1.0 + 2.0 * f64::EPSILON
        || -one - half_ulp != -1.0
        || tiny * one != f64::from_bits(1)
        || normal / black_box(2.0) != f64::from_bits(0x0008_0000_0000_0000)
    {
        return Err(error(
            "E_PRODUCTION_ANALYSIS_ENVIRONMENT",
            "analysis requires nearest ties-to-even and preserved subnormals",
        ));
    }
    Ok(())
}

/// Numerical identity used both in render keys and completed analysis reports.
pub fn numerical_environment() -> Result<NumericalEnvironment, AnalysisError> {
    validate_numerical_environment()?;
    Ok(NumericalEnvironment {
        implementation: concat!("maac-rust/", env!("CARGO_PKG_VERSION")).into(),
        build: option_env!("MAAC_BUILD_ID")
            .unwrap_or("build-id-unavailable")
            .into(),
        platform: format!(
            "{}-{}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS,
            std::env::consts::FAMILY
        ),
        math_library: "Rust f64::log10 (platform math implementation)".into(),
        arithmetic_mode:
            "binary64-nearest-ties-even; ordered-multiply-add; no-FMA; no-denormal-flushing".into(),
        coefficient_identity: "maac.analysis.bs1770-5/2:literal48k-rational-bilinear;annex2-table"
            .into(),
    })
}

/// Explicit host budget. The defaults cover all ordinary RIFF files while
/// bounding retained block energies to at most eight megabytes.
#[derive(Clone, Copy, Debug)]
pub struct AnalyzerLimits {
    pub max_frames: u64,
    pub max_loudness_blocks: usize,
}
impl Default for AnalyzerLimits {
    fn default() -> Self {
        Self {
            max_frames: u32::MAX as u64,
            max_loudness_blocks: 1_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
    history: [f64; 4],
}
impl Biquad {
    fn process(&mut self, x: f64) -> Result<f64, AnalysisError> {
        let [x1, x2, y1, y2] = self.history;
        let mut y = finite(self.b[0] * x)?;
        y = finite(y + finite(self.b[1] * x1)?)?;
        y = finite(y + finite(self.b[2] * x2)?)?;
        y = finite(y - finite(self.a[1] * y1)?)?;
        y = finite(y - finite(self.a[2] * y2)?)?;
        self.history = [x, x1, y, y1];
        Ok(y)
    }
}

const SHELF_B: [&str; 3] = ["1.53512485958697", "-2.69169618940638", "1.19839281085285"];
const SHELF_A: [&str; 3] = ["1", "-1.69065929318241", "0.73248077421585"];
const PASS_B: [&str; 3] = ["1", "-2", "1"];
const PASS_A: [&str; 3] = ["1", "-1.99004745483398", "0.99007225036621"];

fn transformed(c: [&str; 3], rate: u32) -> [BigRational; 3] {
    let c = c.map(|v| crate::exact::parse_rational(v).expect("constant rational"));
    let r = |v: i64| BigRational::from_integer(v.into());
    let a0 = &c[0] + &c[1] + &c[2];
    let a1 = (&c[0] - &c[2]) / r(48000);
    let a2 = (&c[0] - &c[1] + &c[2]) / r(4 * 48000_i64.pow(2));
    let f = i64::from(rate);
    [
        &a0 + r(2 * f) * &a1 + r(4 * f * f) * &a2,
        r(2) * &a0 - r(8 * f * f) * &a2,
        &a0 - r(2 * f) * &a1 + r(4 * f * f) * &a2,
    ]
}
fn biquad(b: [&str; 3], a: [&str; 3], rate: u32) -> Biquad {
    let (b, a) = if rate == 48000 {
        (
            b.map(|v| v.parse().expect("constant float")),
            a.map(|v| v.parse().expect("constant float")),
        )
    } else {
        let b = transformed(b, rate);
        let a = transformed(a, rate);
        let a0 = &a[0];
        // num-rational rounds the exact rational quotient to nearest, ties-even.
        (
            b.each_ref()
                .map(|v| (v / a0).to_f64().expect("bounded coefficient")),
            a.each_ref()
                .map(|v| (v / a0).to_f64().expect("bounded coefficient")),
        )
    };
    Biquad {
        b,
        a,
        history: [0.0; 4],
    }
}

const TRUE_PEAK_TAPS: [[f64; 4]; 12] = [
    [
        0.001708984375,
        -0.0291748046875,
        -0.0189208984375,
        -0.00830078125,
    ],
    [0.010986328125, 0.029296875, 0.0330810546875, 0.014892578125],
    [
        -0.0196533203125,
        -0.0517578125,
        -0.0582275390625,
        -0.026611328125,
    ],
    [0.033203125, 0.089111328125, 0.1015625, 0.047607421875],
    [
        -0.0594482421875,
        -0.16650390625,
        -0.2003173828125,
        -0.102294921875,
    ],
    [
        0.1373291015625,
        0.465087890625,
        0.77978515625,
        0.97216796875,
    ],
    [
        0.97216796875,
        0.77978515625,
        0.465087890625,
        0.1373291015625,
    ],
    [
        -0.102294921875,
        -0.2003173828125,
        -0.16650390625,
        -0.0594482421875,
    ],
    [0.047607421875, 0.1015625, 0.089111328125, 0.033203125],
    [
        -0.026611328125,
        -0.0582275390625,
        -0.0517578125,
        -0.0196533203125,
    ],
    [0.014892578125, 0.0330810546875, 0.029296875, 0.010986328125],
    [
        -0.00830078125,
        -0.0189208984375,
        -0.0291748046875,
        0.001708984375,
    ],
];

#[derive(Default)]
struct Interpolator {
    history: [[f64; 2]; 12],
    position: usize,
}
impl Interpolator {
    fn push(&mut self, frame: [f64; 2], channels: usize) -> Result<[[f64; 2]; 4], AnalysisError> {
        self.history[self.position] = frame;
        let mut result = [[0.0; 2]; 4];
        for (p, output) in result.iter_mut().enumerate() {
            for (c, value) in output.iter_mut().enumerate().take(channels) {
                for (k, taps) in TRUE_PEAK_TAPS.iter().enumerate() {
                    let x = self.history[(self.position + 12 - k) % 12][c];
                    *value = finite(*value + finite(taps[p] * x)?)?;
                }
            }
        }
        self.position = (self.position + 1) % 12;
        Ok(result)
    }
}

#[derive(Clone, Copy, Default)]
struct ActiveBlock {
    count: u32,
    energy: [f64; 2],
    active: bool,
}

/// Push one reconstructed frame at a time, then finish to flush the true-peak
/// interpolator. After an error discard this analyzer; no partial measurement is valid.
pub struct Analyzer {
    rate: u32,
    channels: u8,
    limits: AnalyzerLimits,
    frames: u64,
    filters: [[Biquad; 2]; 2],
    blocks: [ActiveBlock; 4],
    energies: Vec<f64>,
    interpolator: Interpolator,
    sample_peak: f64,
    true_peak: f64,
    true_peak_frames: u64,
}
impl Analyzer {
    pub fn new(rate: u32, channels: u8) -> Result<Self, AnalysisError> {
        Self::with_limits(rate, channels, AnalyzerLimits::default())
    }
    pub fn with_limits(
        rate: u32,
        channels: u8,
        limits: AnalyzerLimits,
    ) -> Result<Self, AnalysisError> {
        validate_numerical_environment()?;
        if !matches!(rate, 44100 | 48000 | 96000) {
            return Err(error(
                "E_PRODUCTION_ANALYSIS_RATE",
                "analysis requires 44100, 48000, or 96000 Hz",
            ));
        }
        if !matches!(channels, 1 | 2) {
            return Err(error(
                "E_PRODUCTION_ANALYSIS_CHANNELS",
                "analysis requires mono or stereo",
            ));
        }
        if limits.max_frames > u64::MAX / 4 - 11 {
            return Err(error(
                "E_PRODUCTION_ANALYSIS_LIMIT",
                "frame budget overflows true-peak frame count",
            ));
        }
        let filters = [biquad(SHELF_B, SHELF_A, rate), biquad(PASS_B, PASS_A, rate)];
        Ok(Self {
            rate,
            channels,
            limits,
            frames: 0,
            filters: [filters; 2],
            blocks: [ActiveBlock::default(); 4],
            energies: Vec::new(),
            interpolator: Interpolator::default(),
            sample_peak: 0.0,
            true_peak: 0.0,
            true_peak_frames: 0,
        })
    }
    /// Validate known artifact length before decoding any samples.
    pub fn preflight_frames(&self, frames: u64) -> Result<(), AnalysisError> {
        let length = u64::from(2 * self.rate / 5);
        let blocks = if frames < length {
            0
        } else {
            (frames - length) / u64::from(self.rate / 10) + 1
        };
        if frames > self.limits.max_frames || blocks > self.limits.max_loudness_blocks as u64 {
            return Err(error(
                "E_PRODUCTION_ANALYSIS_LIMIT",
                "artifact exceeds analysis frame or loudness-block budget",
            ));
        }
        Ok(())
    }
    pub fn push_frame(&mut self, frame: &[f64]) -> Result<(), AnalysisError> {
        if frame.len() != usize::from(self.channels) {
            return Err(error(
                "E_PRODUCTION_ANALYSIS_CHANNELS",
                "frame channel count does not match analyzer",
            ));
        }
        self.preflight_frames(self.frames + 1)?;
        for &sample in frame {
            finite(sample)?;
        }
        let hop = self.rate / 10;
        if self.frames.is_multiple_of(u64::from(hop)) {
            let slot = (self.frames / u64::from(hop) % 4) as usize;
            self.blocks[slot] = ActiveBlock {
                active: true,
                ..ActiveBlock::default()
            };
        }
        let mut input = [0.0; 2];
        for (c, &sample) in frame.iter().enumerate() {
            input[c] = sample;
            self.sample_peak = self.sample_peak.max(sample.abs());
            let weighted = self.filters[c][0].process(sample)?;
            let weighted = self.filters[c][1].process(weighted)?;
            let square = finite(weighted * weighted)?;
            for block in &mut self.blocks {
                if block.active {
                    block.energy[c] = finite(block.energy[c] + square)?;
                }
            }
        }
        for block in &mut self.blocks {
            if !block.active {
                continue;
            }
            block.count += 1;
            if block.count == 2 * self.rate / 5 {
                let mut energy = 0.0;
                for c in 0..usize::from(self.channels) {
                    energy = finite(energy + block.energy[c] / f64::from(block.count))?;
                }
                if self.energies.len() == self.energies.capacity() {
                    self.energies
                        .try_reserve_exact(
                            (self.limits.max_loudness_blocks - self.energies.len()).min(1024),
                        )
                        .map_err(|_| {
                            error(
                                "E_PRODUCTION_ANALYSIS_LIMIT",
                                "cannot allocate loudness block energy",
                            )
                        })?;
                }
                self.energies.push(energy);
                block.active = false;
            }
        }
        self.push_true_peak(input)?;
        self.frames += 1;
        Ok(())
    }
    fn push_true_peak(&mut self, frame: [f64; 2]) -> Result<(), AnalysisError> {
        for output in self.interpolator.push(frame, usize::from(self.channels))? {
            for value in output.iter().take(usize::from(self.channels)) {
                self.true_peak = self.true_peak.max(value.abs());
            }
            self.true_peak_frames += 1;
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<Analysis, AnalysisError> {
        for _ in 0..11 {
            self.push_true_peak([0.0; 2])?;
        }
        let integrated_loudness = integrated(&self.energies, self.sample_peak == 0.0)?;
        Ok(Analysis {
            analyzer: ANALYZER_ID.into(),
            true_peak_profile: TRUE_PEAK_PROFILE.into(),
            numerical_environment: numerical_environment()?,
            rate: self.rate,
            channels: self.channels,
            frames: self.frames,
            true_peak_frames: self.true_peak_frames,
            integrated_loudness,
            sample_peak: peak(self.sample_peak, "dBFS")?,
            true_peak: peak(self.true_peak.max(self.sample_peak), "dBTP")?,
        })
    }
}
fn peak(amplitude: f64, unit: &str) -> Result<PeakMeasurement, AnalysisError> {
    Ok(PeakMeasurement {
        status: if amplitude == 0.0 {
            "digital_silence"
        } else {
            "measured"
        }
        .into(),
        unit: unit.into(),
        amplitude,
        value: if amplitude == 0.0 {
            None
        } else {
            Some(finite(20.0 * amplitude.log10())?)
        },
    })
}
fn loudness(energy: f64) -> Result<f64, AnalysisError> {
    finite(-0.691 + 10.0 * energy.log10())
}
fn integrated(energies: &[f64], silence: bool) -> Result<LoudnessMeasurement, AnalysisError> {
    let unmeasurable = |reason: &str| LoudnessMeasurement {
        status: "unmeasurable".into(),
        unit: "LUFS".into(),
        value: None,
        reason: Some(reason.into()),
    };
    if silence {
        return Ok(unmeasurable("digital_silence"));
    }
    if energies.is_empty() {
        return Ok(unmeasurable("insufficient_duration"));
    }
    let mut sum = 0.0;
    let mut count = 0_u64;
    for &energy in energies {
        if energy > 0.0 && loudness(energy)? > -70.0 {
            sum = finite(sum + energy)?;
            count += 1;
        }
    }
    if count == 0 {
        return Ok(unmeasurable("below_absolute_gate"));
    }
    let threshold = loudness(sum / count as f64)? - 10.0;
    sum = 0.0;
    count = 0;
    for &energy in energies {
        if energy > 0.0 {
            let value = loudness(energy)?;
            if value > -70.0 && value > threshold {
                sum = finite(sum + energy)?;
                count += 1;
            }
        }
    }
    if count == 0 {
        return Ok(unmeasurable("below_relative_gate"));
    }
    Ok(LoudnessMeasurement {
        status: "measured".into(),
        unit: "LUFS".into(),
        value: Some(loudness(sum / count as f64)?),
        reason: None,
    })
}

/// Reopen the final encoded WAV and reconstruct stored PCM using WAV scaling.
pub fn analyze_wav(path: &Path) -> Result<Analysis, AnalysisError> {
    analyze_wav_with_limits(path, AnalyzerLimits::default())
}
pub fn analyze_wav_with_limits(
    path: &Path,
    limits: AnalyzerLimits,
) -> Result<Analysis, AnalysisError> {
    let mut reader = hound::WavReader::open(path).map_err(wav_error)?;
    let spec = reader.spec();
    let channels = u8::try_from(spec.channels).map_err(|_| {
        error(
            "E_PRODUCTION_ANALYSIS_CHANNELS",
            "unsupported WAV channel count",
        )
    })?;
    let mut analyzer = Analyzer::with_limits(spec.sample_rate, channels, limits)?;
    if !matches!(
        (spec.sample_format, spec.bits_per_sample),
        (hound::SampleFormat::Float, 32) | (hound::SampleFormat::Int, 16 | 24)
    ) {
        return Err(error(
            "E_PRODUCTION_ANALYSIS_ENCODING",
            "expected WAV Float32, PCM16, or PCM24",
        ));
    }
    if reader.len() % u32::from(channels) != 0 {
        return Err(error(
            "E_PRODUCTION_ANALYSIS_ENCODING",
            "incomplete WAV frame",
        ));
    }
    let frames = u64::from(reader.duration());
    analyzer.preflight_frames(frames)?;
    let mut frame = [0.0; 2];
    match spec.sample_format {
        hound::SampleFormat::Float => {
            let mut samples = reader.samples::<f32>();
            for _ in 0..frames {
                for value in frame.iter_mut().take(usize::from(channels)) {
                    *value = f64::from(
                        samples
                            .next()
                            .ok_or_else(|| {
                                error("E_PRODUCTION_ANALYSIS_ENCODING", "truncated WAV")
                            })?
                            .map_err(wav_error)?,
                    );
                }
                analyzer.push_frame(&frame[..usize::from(channels)])?;
            }
        }
        hound::SampleFormat::Int => {
            let scale = (1_u64 << (spec.bits_per_sample - 1)) as f64;
            let mut samples = reader.samples::<i32>();
            for _ in 0..frames {
                for value in frame.iter_mut().take(usize::from(channels)) {
                    *value = f64::from(
                        samples
                            .next()
                            .ok_or_else(|| {
                                error("E_PRODUCTION_ANALYSIS_ENCODING", "truncated WAV")
                            })?
                            .map_err(wav_error)?,
                    ) / scale;
                }
                analyzer.push_frame(&frame[..usize::from(channels)])?;
            }
        }
    }
    analyzer.finish()
}
fn wav_error(value: hound::Error) -> AnalysisError {
    error("E_PRODUCTION_ANALYSIS_WAV", value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_stage_impulse_preserves_original_peak_and_complete_tail() {
        let mut analyzer = Analyzer::new(48000, 1).unwrap();
        analyzer.push_frame(&[1.0]).unwrap();
        let result = analyzer.finish().unwrap();
        assert_eq!(result.true_peak.amplitude, 1.0);
        assert_eq!(result.true_peak_frames, 48);
        assert_eq!(result.analyzer, "maac.analysis.bs1770-5/2");
        assert_eq!(
            result.true_peak_profile,
            "maac.truepeak.bs1770-5.annex2-4x/1"
        );
        assert_eq!(
            result.integrated_loudness.reason.as_deref(),
            Some("insufficient_duration")
        );
    }

    #[test]
    fn silence_has_null_logarithms() {
        let result = Analyzer::new(48000, 2).unwrap().finish().unwrap();
        assert_eq!(result.sample_peak.status, "digital_silence");
        assert_eq!(result.sample_peak.value, None);
        assert_eq!(
            result.integrated_loudness.reason.as_deref(),
            Some("digital_silence")
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("NaN"));
    }
    #[test]
    fn k_coefficients_are_literal_or_once_rounded_exact_rationals() {
        let literal = biquad(SHELF_B, SHELF_A, 48000);
        assert_eq!(
            literal.b,
            [1.53512485958697, -2.69169618940638, 1.19839281085285]
        );
        // Independent Python fractions.Fraction transformation and correctly
        // rounded float conversion, compared as complete binary64 bit patterns.
        let expected = [
            (
                44100,
                SHELF_B,
                SHELF_A,
                [
                    0x3ff87e7c5de40831,
                    0xc00535f7aaa89349,
                    0x3ff2b5a214edb1cd,
                    0x3ff0000000000000,
                    0xbffa9f577e8e3201,
                    0x3fe6cf0d381d8adc,
                ],
            ),
            (
                44100,
                PASS_B,
                PASS_A,
                [
                    0x3feffc65649072c9,
                    0xbffffc65649072c9,
                    0x3feffc65649072c9,
                    0x3ff0000000000000,
                    0xbfffd3a39583a234,
                    0x3fefa784beb457e4,
                ],
            ),
            (
                96000,
                SHELF_B,
                SHELF_A,
                [
                    0x3ff8f44799d2846e,
                    0xc007687a8bd7c3d0,
                    0x3ff60af615abe86e,
                    0x3ff0000000000000,
                    0xbffd8197c120077d,
                    0x3feb5fc0b1ddd96f,
                ],
            ),
            (
                96000,
                PASS_B,
                PASS_A,
                [
                    0x3ff00a35dfe775a5,
                    0xc0000a35dfe775a5,
                    0x3ff00a35dfe775a5,
                    0x3ff0000000000000,
                    0xbfffeb9782440633,
                    0x3fefd73c0cd3d25a,
                ],
            ),
        ];
        for (rate, b, a, bits) in expected {
            let filter = biquad(b, a, rate);
            let actual: Vec<_> = filter
                .b
                .into_iter()
                .chain(filter.a)
                .map(f64::to_bits)
                .collect();
            assert_eq!(actual, bits, "rate {rate}");
        }
        for (b, a) in [(SHELF_B, SHELF_A), (PASS_B, PASS_A)] {
            let bb = transformed(b, 48000);
            let aa = transformed(a, 48000);
            for (actual, expected) in bb.iter().chain(aa.iter()).zip(b.into_iter().chain(a)) {
                assert_eq!(
                    (actual / &aa[0]).to_f64().unwrap(),
                    expected.parse::<f64>().unwrap()
                );
            }
        }
    }

    #[test]
    fn gates_are_strict_and_keep_only_absolutely_admitted_blocks() {
        let boundary = 10_f64.powf((-70.0 + 0.691) / 10.0);
        assert_eq!(loudness(boundary).unwrap(), -70.0);
        assert_eq!(
            integrated(&[boundary], false).unwrap().reason.as_deref(),
            Some("below_absolute_gate")
        );
        // Three blocks: silence is excluded; the tiny nonzero block fails the
        // absolute gate before the relative threshold is calculated.
        let result = integrated(&[0.0, 1e-12, 1.0, 0.001], false).unwrap();
        assert_eq!(result.value, Some(-0.691));
        assert_eq!(integrated(&[0.0], false).unwrap().value, None);
        assert_eq!(
            integrated(&[], false).unwrap().reason.as_deref(),
            Some("insufficient_duration")
        );
    }

    #[test]
    fn tone_loudness_and_stereo_weights_at_all_rates() {
        for rate in [44100, 48000, 96000] {
            let mut mono = Analyzer::new(rate, 1).unwrap();
            let mut stereo = Analyzer::new(rate, 2).unwrap();
            for n in 0..rate / 2 {
                let x =
                    0.1 * (std::f64::consts::TAU * 1000.0 * f64::from(n) / f64::from(rate)).sin();
                mono.push_frame(&[x]).unwrap();
                stereo.push_frame(&[x, x]).unwrap();
            }
            assert_eq!(mono.energies.len(), 2);
            let m = mono.finish().unwrap();
            let s = stereo.finish().unwrap();
            // 1 kHz mono sine with -20 dBFS peak: approximately -23 LUFS.
            assert!(
                (m.integrated_loudness.value.unwrap() + 23.0).abs() < 0.08,
                "{rate}: {:?}",
                m.integrated_loudness
            );
            let difference =
                s.integrated_loudness.value.unwrap() - m.integrated_loudness.value.unwrap();
            assert!((difference - 10.0 * 2_f64.log10()).abs() < 1e-12);
            assert_eq!(s.true_peak.amplitude, m.true_peak.amplitude);
        }
    }

    #[test]
    fn blocks_match_independent_continuous_filter_reference() {
        let rate = 44100;
        let mut analyzer = Analyzer::new(rate, 2).unwrap();
        let mut filters = [biquad(SHELF_B, SHELF_A, rate), biquad(PASS_B, PASS_A, rate)];
        let mut squares = Vec::new();
        for n in 0..rate * 3 / 5 + 29 {
            let x = if n < rate / 3 { 0.25 } else { -0.1 };
            let y = filters[0].process(x).unwrap();
            let y = filters[1].process(y).unwrap();
            squares.push(y * y);
            analyzer.push_frame(&[x, x]).unwrap();
        }
        let length = (2 * rate / 5) as usize;
        let hop = (rate / 10) as usize;
        let mut expected = Vec::new();
        for start in (0..=squares.len() - length).step_by(hop) {
            let sum = squares[start..start + length]
                .iter()
                .fold(0.0, |s, x| s + x);
            let energy = sum / length as f64;
            expected.push(energy + energy);
        }
        assert_eq!(analyzer.energies, expected);
    }

    #[test]
    fn last_frame_impulse_is_flushed_and_reset_replays_exactly() {
        let mut last = Analyzer::new(48000, 2).unwrap();
        for _ in 0..99 {
            last.push_frame(&[0.0, 0.0]).unwrap();
        }
        last.push_frame(&[0.0, 1.0]).unwrap();
        let result = last.finish().unwrap();
        assert_eq!(result.true_peak.amplitude, 1.0);
        assert_eq!(result.true_peak_frames, 4 * (100 + 11));
        for _ in 0..2 {
            let mut fresh = Analyzer::new(48000, 2).unwrap();
            fresh.push_frame(&[0.0, 1.0]).unwrap();
            assert_eq!(
                fresh.finish().unwrap().true_peak.amplitude,
                result.true_peak.amplitude
            );
        }
    }

    #[test]
    fn peak_in_flushed_tail_is_included() {
        let mut analyzer = Analyzer::new(48000, 1).unwrap();
        analyzer.push_frame(&[1.0]).unwrap();
        analyzer.push_frame(&[1.0]).unwrap();
        let result = analyzer.finish().unwrap();
        // Independent tap-pair sum at n=6, p=1: 0.465087890625+
        // 0.77978515625 = 5099/4096, strictly after both input frames.
        assert_eq!(result.true_peak.amplitude, 5099.0 / 4096.0);
        assert_eq!(result.true_peak_frames, 52);
    }

    #[test]
    fn true_peak_never_falls_below_original_peak() {
        let mut analyzer = Analyzer::new(48000, 1).unwrap();
        for n in 0..64 {
            analyzer
                .push_frame(&[if n % 2 == 0 { 1.0 } else { -1.0 }])
                .unwrap();
        }
        let result = analyzer.finish().unwrap();
        assert!(result.true_peak.amplitude >= result.sample_peak.amplitude);
    }

    #[test]
    fn decoder_measures_final_pcm_and_float_values() {
        for (format, bits, value, expected) in [
            (hound::SampleFormat::Int, 16, 32767.0, 32767.0 / 32768.0),
            (
                hound::SampleFormat::Int,
                24,
                8388607.0,
                8388607.0 / 8388608.0,
            ),
            (hound::SampleFormat::Float, 32, 0.1, f64::from(0.1_f32)),
        ] {
            let file = tempfile::NamedTempFile::new().unwrap();
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: bits,
                sample_format: format,
            };
            let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
            if format == hound::SampleFormat::Int {
                writer.write_sample(value as i32).unwrap();
            } else {
                writer.write_sample(value as f32).unwrap();
            }
            writer.finalize().unwrap();
            let result = analyze_wav(file.path()).unwrap();
            assert_eq!(result.sample_peak.amplitude, expected);
            assert_eq!(result.frames, 1);
        }
    }

    #[test]
    fn rejects_nonfinite_input_intermediate_bad_shapes_and_budgets() {
        assert_eq!(
            Analyzer::new(32000, 1).err().unwrap().code,
            "E_PRODUCTION_ANALYSIS_RATE"
        );
        assert_eq!(
            Analyzer::new(48000, 3).err().unwrap().code,
            "E_PRODUCTION_ANALYSIS_CHANNELS"
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, 1e200] {
            assert_eq!(
                Analyzer::new(48000, 1)
                    .unwrap()
                    .push_frame(&[value])
                    .unwrap_err()
                    .code,
                "E_PRODUCTION_ANALYSIS_NONFINITE"
            );
        }
        assert_eq!(
            Analyzer::new(48000, 1)
                .unwrap()
                .push_frame(&[])
                .unwrap_err()
                .code,
            "E_PRODUCTION_ANALYSIS_CHANNELS"
        );
        let limits = AnalyzerLimits {
            max_frames: 1,
            max_loudness_blocks: 0,
        };
        let mut analyzer = Analyzer::with_limits(48000, 1, limits).unwrap();
        analyzer.push_frame(&[0.0]).unwrap();
        assert_eq!(
            analyzer.push_frame(&[0.0]).unwrap_err().code,
            "E_PRODUCTION_ANALYSIS_LIMIT"
        );
        assert!(Analyzer::new(48000, 1)
            .unwrap()
            .preflight_frames(u64::MAX)
            .is_err());
        let limits = AnalyzerLimits {
            max_frames: 48000,
            max_loudness_blocks: 0,
        };
        assert!(Analyzer::with_limits(48000, 1, limits)
            .unwrap()
            .preflight_frames(19200)
            .is_err());
        assert!(Analyzer::with_limits(
            48000,
            1,
            AnalyzerLimits {
                max_frames: u64::MAX,
                max_loudness_blocks: 0
            }
        )
        .is_err());
    }

    #[test]
    fn decoder_rejects_unsupported_encoding_and_nonfinite_float() {
        for (format, bits) in [
            (hound::SampleFormat::Int, 32),
            (hound::SampleFormat::Float, 32),
        ] {
            let file = tempfile::NamedTempFile::new().unwrap();
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: bits,
                sample_format: format,
            };
            let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
            if format == hound::SampleFormat::Int {
                writer.write_sample(1_i32).unwrap();
            } else {
                writer.write_sample(f32::NAN).unwrap();
            }
            writer.finalize().unwrap();
            assert_eq!(
                analyze_wav(file.path()).unwrap_err().code,
                if format == hound::SampleFormat::Int {
                    "E_PRODUCTION_ANALYSIS_ENCODING"
                } else {
                    "E_PRODUCTION_ANALYSIS_NONFINITE"
                }
            );
        }
    }
    // Independently synthesize EBU Tech 3341 v4 (November 2023), Table 1,
    // cases 15–19. These are mathematical definitions, not redistributed EBU
    // audio fixtures: https://tech.ebu.ch/docs/tech/tech3341v4_0.pdf
    fn prescribed_true_peak_tones() -> Vec<(u32, u32, f64, f64)> {
        let mut results = Vec::new();
        for rate in [44100, 48000, 96000] {
            for (case, period, amplitude, phase, expected) in [
                (15, 4.0, 0.5, 0.0, -6.0),
                (16, 4.0, 0.5, 45.0, -6.0),
                (17, 6.0, 0.5, 60.0, -6.0),
                (18, 8.0, 0.5, 67.5, -6.0),
                (19, 4.0, 1.41, 45.0, 3.0),
            ] {
                let file = tempfile::NamedTempFile::new().unwrap();
                let spec = hound::WavSpec {
                    channels: 2,
                    sample_rate: rate,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                };
                let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
                let frames = rate / 10;
                let fade = rate / 100;
                for n in 0..frames {
                    let envelope = (f64::from(n) / f64::from(fade))
                        .min(f64::from(frames - 1 - n) / f64::from(fade))
                        .min(1.0);
                    let angle = std::f64::consts::TAU * f64::from(n) / period
                        + phase * std::f64::consts::PI / 180.0;
                    let sample = (amplitude * angle.sin() * envelope) as f32;
                    writer.write_sample(sample).unwrap();
                    writer.write_sample(sample).unwrap();
                }
                writer.finalize().unwrap();
                let value = analyze_wav(file.path()).unwrap().true_peak.value.unwrap();
                results.push((case, rate, value, expected));
            }
        }
        results
    }

    #[test]
    fn ebu_3341_prescribed_true_peak_tones_15_through_19() {
        let results = prescribed_true_peak_tones();
        for (case, rate, value, expected) in &results {
            eprintln!("EBU true-peak case {case}, {rate} Hz: {value:.9} dBTP (expected {expected} +0.2/-0.4)");
        }
        assert!(
            results
                .into_iter()
                .all(|(_, _, value, expected)| (expected - 0.4..=expected + 0.2).contains(&value)),
            "public 4x profile does not pass applicable EBU true-peak acceptance"
        );
    }

    // These official prescribed durations total 280.1 seconds of stereo PCM.
    // Keep expensive acceptance out of ordinary debug unit-test iterations.
    // cargo test --release --lib ebu_3341_prescribed_loudness -- --ignored --nocapture
    #[test]
    #[ignore = "full-duration EBU acceptance; run explicitly in release mode"]
    fn ebu_3341_prescribed_loudness_tones_1_through_5() {
        let cases: &[(&[(u32, f64)], f64)] = &[
            (&[(200, -23.0)], -23.0),
            (&[(200, -33.0)], -33.0),
            (&[(100, -36.0), (600, -23.0), (100, -36.0)], -23.0),
            (
                &[
                    (100, -72.0),
                    (100, -36.0),
                    (600, -23.0),
                    (100, -36.0),
                    (100, -72.0),
                ],
                -23.0,
            ),
            (&[(200, -26.0), (201, -20.0), (200, -26.0)], -23.0),
        ];
        for (case, (segments, expected)) in cases.iter().enumerate() {
            let file = tempfile::NamedTempFile::new().unwrap();
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
            for &(tenths, level) in *segments {
                let amplitude = 10_f64.powf(level / 20.0);
                for n in 0..tenths * 4800 {
                    let sample = (amplitude
                        * (std::f64::consts::TAU * 1000.0 * f64::from(n) / 48000.0).sin())
                        as f32;
                    writer.write_sample(sample).unwrap();
                    writer.write_sample(sample).unwrap();
                }
            }
            writer.finalize().unwrap();
            let value = analyze_wav(file.path())
                .unwrap()
                .integrated_loudness
                .value
                .unwrap();
            eprintln!(
                "EBU prescribed case {}: {value:.9} LUFS (expected {expected} +/- 0.1)",
                case + 1
            );
            assert!(
                (value - expected).abs() <= 0.1,
                "EBU case {}: {value} LUFS",
                case + 1
            );
        }
    }
    // Decode original official PCM directly into the public streaming analyzer.
    // A rate override reinterprets the same Fs-normalized sequence; no resampled
    // or rewritten file is presented as an official alternate-rate artifact.
    fn analyze_official_pcm_at_rate(path: &Path, rate: u32) -> Analysis {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        let channels = usize::from(spec.channels);
        assert!(channels <= 2);
        let mut analyzer = Analyzer::new(rate, channels as u8).unwrap();
        let frames = reader.duration();
        analyzer.preflight_frames(u64::from(frames)).unwrap();
        let scale = (1_u64 << (spec.bits_per_sample - 1)) as f64;
        let mut samples = reader.samples::<i32>();
        for _ in 0..frames {
            let mut frame = [0.0; 2];
            for value in frame.iter_mut().take(channels) {
                *value = f64::from(samples.next().unwrap().unwrap()) / scale;
            }
            analyzer.push_frame(&frame[..channels]).unwrap();
        }
        analyzer.finish().unwrap()
    }

    /// Run with official, privately downloaded EBU v05 artifacts. No official
    /// audio is stored in the repository. Reports may be retained with © EBU.
    /// MAAC_EBU_CORPUS=/tmp/ebu-v05 MAAC_METER_AUDIT_REPORT=/tmp/maac-meter-audit.json
    /// cargo test --release --lib ebu_official_corpus_audit -- --ignored --nocapture
    #[test]
    #[ignore = "requires external official EBU v05 corpus; see audit report for hash-pinned provenance"]
    fn ebu_official_corpus_audit() {
        let directory = std::path::PathBuf::from(
            std::env::var("MAAC_EBU_CORPUS")
                .expect("set MAAC_EBU_CORPUS to privately downloaded EBU v05 directory"),
        );
        let mut loudness_results = Vec::new();
        for (case, name, expected) in [
            (1, "seq-3341-1-16bit.wav", -23.0),
            (2, "seq-3341-2-16bit.wav", -33.0),
            (3, "seq-3341-3-16bit-v02.wav", -23.0),
            (4, "seq-3341-4-16bit-v02.wav", -23.0),
            (5, "seq-3341-5-16bit-v02.wav", -23.0),
            (7, "seq-3341-7_seq-3342-5-24bit.wav", -23.0),
            (8, "seq-3341-2011-8_seq-3342-6-24bit-v02.wav", -23.0),
        ] {
            let value = analyze_wav(&directory.join(name))
                .unwrap()
                .integrated_loudness
                .value
                .unwrap();
            let pass = (value - expected).abs() <= 0.1;
            eprintln!("EBU official loudness {case}: {value:.9} LUFS, pass={pass}");
            loudness_results.push(serde_json::json!({"case":case,"file":name,"value_lufs":value,"expected_lufs":expected,"tolerance":0.1,"pass":pass}));
        }
        let mut true_peak_results = Vec::new();
        for case in 15..=23 {
            let name = format!("seq-3341-{case}-24bit.wav.wav");
            let path = directory.join(&name);
            let expected = if case < 19 {
                -6.0
            } else if case == 19 {
                3.0
            } else {
                0.0
            };
            let actual = analyze_wav(&path).unwrap();
            for rate in [44100, 48000, 96000] {
                let analysis = if rate == 48000 {
                    actual.clone()
                } else {
                    analyze_official_pcm_at_rate(&path, rate)
                };
                assert_eq!(analysis.analyzer, ANALYZER_ID);
                assert_eq!(analysis.true_peak_profile, TRUE_PEAK_PROFILE);
                assert_eq!(analysis.true_peak_frames, 4 * (analysis.frames + 11));
                let value = analysis.true_peak.value.unwrap();
                let pass = (expected - 0.4..=expected + 0.2).contains(&value);
                eprintln!("EBU official TP {case}, {rate}: public4x={value:.9}, pass={pass}");
                true_peak_results.push(serde_json::json!({"case":case,"file":name,"rate":rate,"expected_dbtp":expected,"lower_tolerance":0.4,"upper_tolerance":0.2,"value_dbtp":value,"pass":pass}));
            }
        }
        let report = serde_json::json!({"corpus":"EBU Loudness test set v05 (2016-03-30)","credit":"© EBU","analyzer":ANALYZER_ID,"true_peak_profile":TRUE_PEAK_PROFILE,"loudness":loudness_results,"true_peak":true_peak_results,"note":"Measurements use the public analyzer; Fs-normalized sequence reinterpretation at alternate rates is not a claim about official alternate-rate WAV fixtures."});
        if let Ok(path) = std::env::var("MAAC_METER_AUDIT_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
        assert!(
            loudness_results.iter().all(|v| v["pass"] == true),
            "official integrated loudness acceptance failed"
        );
        assert!(
            true_peak_results.iter().all(|v| v["pass"] == true),
            "public 4x analyzer fails official minimum"
        );
    }
    #[test]
    #[ignore = "requires private ITU BS.2217 corpus; MAAC_ITU_CORPUS points to extracted WAVs"]
    fn itu_official_loudness_corpus_audit() {
        let directory = std::path::PathBuf::from(
            std::env::var("MAAC_ITU_CORPUS")
                .expect("set MAAC_ITU_CORPUS to private extracted official WAVs"),
        );
        let mut files = Vec::new();
        for level in [23, 24] {
            for frequency in [25, 100, 500, 1000, 2000, 10000] {
                files.push((
                    format!("1770-2_Comp_{level}LKFS_{frequency}Hz_2ch.wav"),
                    -f64::from(level),
                ));
            }
        }
        files.push(("1770-2_Comp_AbsGateTest.wav".into(), -69.5));
        files.push(("1770-2_Comp_RelGateTest.wav".into(), -10.0));
        files.push(("1770-2_Comp_18LKFS_FrequencySweep.wav".into(), -18.0));
        for level in [23, 24] {
            files.push((
                format!("1770-2 Conf Mono Voice+Music-{level}LKFS.wav"),
                -f64::from(level),
            ));
            files.push((
                format!("1770-2 Conf Stereo VinL+R-{level}LKFS.wav"),
                -f64::from(level),
            ));
        }
        let mut results = Vec::new();
        for (name, expected) in files {
            let value = analyze_wav(&directory.join(&name))
                .unwrap()
                .integrated_loudness
                .value
                .unwrap();
            let pass = (value - expected).abs() <= 0.1;
            eprintln!("ITU {name}: {value:.9} LUFS, expected={expected}, pass={pass}");
            results.push(serde_json::json!({"file":name,"value_lufs":value,"expected_lufs":expected,"tolerance":0.1,"pass":pass}));
        }
        if let Ok(path) = std::env::var("MAAC_ITU_AUDIT_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
        }
        assert!(
            results.iter().all(|v| v["pass"] == true),
            "official ITU loudness acceptance failed"
        );
    }
}
