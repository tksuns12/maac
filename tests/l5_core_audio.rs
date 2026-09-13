use std::{
    env, fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use maac::{
    bundle::sha256_digest, compiler::compile_bundle_artifact, render_artifact, PlanArtifact,
    Rational, SourceBundle,
};
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MANIFEST_FORMAT: &str = "maac.l5.core-audio-reference";
const POLICY_ID: &str = "maac.core-audio.reference-f64/1";
const REFERENCE_FORMAT: &str = "maac.l5.sample-intervals";
const MANIFEST_SHA256: &str =
    "sha256:82ac6f827ac445b082906b4a1c40d555b80656b7f4e5740dd9fd868c9733534f";
const REFERENCE_SHA256: &str =
    "sha256:cb7f8a71da9aa08529e2bcf93a53d791f2e0902123783dfa023e06c9cebd2b1d";
const ASSET_SHA256: &str =
    "sha256:66b7057e198043f4acfb86f84717e34610811559714e63c9484bde10fb7cbe06";
const EXPECTED_IDS: [&str; 6] = [
    "sine-6000",
    "pan-center",
    "pan-half",
    "one-pole-impulse",
    "delay-feedback",
    "noise-seed7",
];
const EXPECTED_SOURCES: [(&str, &str, usize); 6] = [
    (
        "sources/sine-6000.maac",
        "sha256:3bd58a04c431e726649a78b62cd08cdafe32bb10fb8a324bdcca4f4a9228d588",
        504,
    ),
    (
        "sources/pan-center.maac",
        "sha256:2db6f8c9d37e6b2376246914d25672282653324de79179af09fc7ca5d4245279",
        604,
    ),
    (
        "sources/pan-half.maac",
        "sha256:451e4170ece06acf224c76a50f503de91e992b9a9ec30aa4f6347fd600cd5575",
        606,
    ),
    (
        "sources/one-pole-impulse.maac",
        "sha256:dfc9d491ba2595b215d3cf81d8e7ea65ba96b9effb2e4a0243555bcfa17887a3",
        670,
    ),
    (
        "sources/delay-feedback.maac",
        "sha256:c8709cc5d62ff6453e150afd7194aae27687912e55ab47b37fe6aca7ddfcb98b",
        953,
    ),
    (
        "sources/noise-seed7.maac",
        "sha256:ac2c6bc0765ddd004af74c427b7f9ffaa36c67f5ca8361916bd190b8a3c5776f",
        297,
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Category {
    Inventory,
    Metadata,
    Timing,
    Shape,
    NonFinite,
    Reference,
    Numerical,
    CompilerDiagnostics,
    Artifact,
    Render,
    Io,
}

#[derive(Debug)]
struct HarnessError {
    category: Category,
    message: String,
}

impl fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.category, self.message)
    }
}

impl std::error::Error for HarnessError {}

type HResult<T> = Result<T, HarnessError>;

fn error(category: Category, message: impl Into<String>) -> HarnessError {
    HarnessError {
        category,
        message: message.into(),
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    policy: Policy,
    provenance: Value,
    reference: FileRecord,
    assets: Vec<AssetRecord>,
    cases: Vec<CaseRecord>,
    claims: Claims,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    id: String,
    sample_rate_hz: String,
    reset_frame: String,
    frame_window: [String; 2],
    observed_samples: String,
    reference_samples: String,
    metric: String,
    error_upper: String,
    certification: String,
    epsilon: FractionRecord,
    maximum_interval_width: FractionRecord,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FractionRecord {
    numerator: String,
    denominator: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    path: String,
    sha256: String,
    bytes: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetRecord {
    path: String,
    sha256: String,
    bytes: String,
    format: String,
    sample_rate_hz: String,
    channels: String,
    frames: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseRecord {
    id: String,
    source: FileRecord,
    assets: Vec<String>,
    render: RenderRecord,
    timing: TimingRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RenderRecord {
    sample_rate_hz: String,
    reset_frame: String,
    frame_start: String,
    frame_end: String,
    score_frames: String,
    tail_frames: String,
    output_port: Vec<String>,
    channels: String,
    channel_order: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TimingRecord {
    NoteGate {
        on_frame: String,
        off_frame: String,
    },
    AudioTransport {
        start_frame: String,
        end_frame: String,
    },
    EmptyEvents {
        events: Vec<Value>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    static_reference_verified: bool,
    runtime_render_executed_by_this_checker: bool,
    cross_platform_bound_proven: bool,
    full_core_audio_profile_covered: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceDocument {
    format: String,
    version: u32,
    cases: Vec<ReferenceCase>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceCase {
    id: String,
    frames: Vec<Vec<Interval>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Interval {
    lo: String,
    hi: String,
}

#[derive(Clone, Debug, Serialize)]
struct ObservedCase {
    id: String,
    render: RenderRecord,
    timing: TimingRecord,
    frames: Vec<Vec<f64>>,
    replay_bit_identical: bool,
}

#[derive(Debug)]
struct SuiteStats {
    comparisons: usize,
    maximum_error_upper: Rational,
    worst_case: String,
    worst_frame: usize,
    worst_channel: usize,
}

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/l5")
}

fn read(path: &Path) -> HResult<Vec<u8>> {
    fs::read(path).map_err(|failure| error(Category::Io, format!("{}: {failure}", path.display())))
}

fn parse_u64(text: &str, context: &str) -> HResult<u64> {
    if text.is_empty()
        || !text.bytes().all(|byte| byte.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return Err(error(
            Category::Metadata,
            format!("{context}: noncanonical unsigned integer {text:?}"),
        ));
    }
    text.parse::<u64>().map_err(|_| {
        error(
            Category::Metadata,
            format!("{context}: unsigned integer exceeds u64"),
        )
    })
}

fn parse_fraction(text: &str) -> HResult<Rational> {
    let (numerator_text, denominator_text) = text.split_once('/').ok_or_else(|| {
        error(
            Category::Reference,
            format!("expected reduced rational, found {text:?}"),
        )
    })?;
    let canonical_numerator = numerator_text == "0"
        || numerator_text
            .strip_prefix('-')
            .unwrap_or(numerator_text)
            .as_bytes()
            .first()
            .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
            && numerator_text
                .strip_prefix('-')
                .unwrap_or(numerator_text)
                .bytes()
                .all(|byte| byte.is_ascii_digit());
    let canonical_denominator = denominator_text
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
        && denominator_text.bytes().all(|byte| byte.is_ascii_digit());
    if !canonical_numerator || !canonical_denominator || numerator_text == "-0" {
        return Err(error(
            Category::Reference,
            format!("noncanonical rational {text:?}"),
        ));
    }
    let numerator = BigInt::from_str(numerator_text).map_err(|_| {
        error(
            Category::Reference,
            format!("invalid numerator in {text:?}"),
        )
    })?;
    let denominator = BigInt::from_str(denominator_text).map_err(|_| {
        error(
            Category::Reference,
            format!("invalid denominator in {text:?}"),
        )
    })?;
    let rational = Rational::new(numerator.clone(), denominator.clone());
    if rational.numer() != &numerator || rational.denom() != &denominator {
        return Err(error(
            Category::Reference,
            format!("rational is not reduced: {text:?}"),
        ));
    }
    Ok(rational)
}

fn fraction_record(value: &FractionRecord, context: &str) -> HResult<Rational> {
    parse_fraction(&format!("{}/{}", value.numerator, value.denominator))
        .map_err(|failure| error(failure.category, format!("{context}: {}", failure.message)))
}

fn fraction_text(value: &Rational) -> String {
    format!("{}/{}", value.numer(), value.denom())
}

fn exact_f64(value: f64) -> HResult<Rational> {
    if !value.is_finite() {
        return Err(error(
            Category::NonFinite,
            format!("observed sample is not finite: {value:?}"),
        ));
    }
    let bits = value.to_bits();
    let negative = bits >> 63 != 0;
    let exponent_bits = ((bits >> 52) & 0x7ff) as i32;
    let fraction_bits = bits & ((1_u64 << 52) - 1);
    if exponent_bits == 0 && fraction_bits == 0 {
        return Ok(Rational::zero());
    }
    let significand = if exponent_bits == 0 {
        fraction_bits
    } else {
        (1_u64 << 52) | fraction_bits
    };
    let exponent = if exponent_bits == 0 {
        1 - 1023 - 52
    } else {
        exponent_bits - 1023 - 52
    };
    let mut numerator = BigInt::from(significand);
    if negative {
        numerator = -numerator;
    }
    if exponent >= 0 {
        Ok(Rational::from_integer(numerator << exponent as usize))
    } else {
        Ok(Rational::new(
            numerator,
            BigInt::from(1_u8) << (-exponent) as usize,
        ))
    }
}

fn compare_sample(
    actual: f64,
    interval: &Interval,
    epsilon: &Rational,
    maximum_width: &Rational,
) -> HResult<Rational> {
    let actual = exact_f64(actual)?;
    let lo = parse_fraction(&interval.lo)?;
    let hi = parse_fraction(&interval.hi)?;
    if lo > hi {
        return Err(error(Category::Reference, "reference interval is reversed"));
    }
    if &hi - &lo > *maximum_width {
        return Err(error(
            Category::Reference,
            "reference interval exceeds the fixed width bound",
        ));
    }
    let upper = (&actual - lo).abs().max((&actual - hi).abs());
    if upper > *epsilon {
        return Err(error(
            Category::Numerical,
            format!(
                "sample error upper {} exceeds epsilon {}",
                fraction_text(&upper),
                fraction_text(epsilon)
            ),
        ));
    }
    Ok(upper)
}

fn load_corpus() -> HResult<(Manifest, ReferenceDocument)> {
    let root = corpus_root();
    let manifest_raw = read(&root.join("manifest.json"))?;
    if sha256_digest(&manifest_raw) != MANIFEST_SHA256 {
        return Err(error(Category::Inventory, "manifest hash mismatch"));
    }
    let manifest: Manifest = serde_json::from_slice(&manifest_raw)
        .map_err(|failure| error(Category::Inventory, format!("manifest JSON: {failure}")))?;
    let reference_raw = read(&root.join("references/sample-intervals.json"))?;
    if sha256_digest(&reference_raw) != REFERENCE_SHA256 {
        return Err(error(Category::Inventory, "reference hash mismatch"));
    }
    let reference: ReferenceDocument = serde_json::from_slice(&reference_raw)
        .map_err(|failure| error(Category::Reference, format!("reference JSON: {failure}")))?;
    validate_manifest(&manifest, &manifest_raw, &reference_raw)?;
    Ok((manifest, reference))
}

fn validate_manifest(
    manifest: &Manifest,
    manifest_raw: &[u8],
    reference_raw: &[u8],
) -> HResult<()> {
    if manifest.format != MANIFEST_FORMAT || manifest.version != 1 {
        return Err(error(
            Category::Inventory,
            "unsupported corpus discriminator",
        ));
    }
    if sha256_digest(manifest_raw) != MANIFEST_SHA256 {
        return Err(error(
            Category::Inventory,
            "manifest byte identity mismatch",
        ));
    }
    let policy = &manifest.policy;
    if policy.id != POLICY_ID
        || policy.sample_rate_hz != "48000"
        || policy.reset_frame != "0"
        || policy.frame_window != ["0", "8"]
        || policy.observed_samples
            != "finite pre-encoding binary64 decoded exactly as dyadic rationals"
        || policy.reference_samples != "real values enclosed by reduced rational endpoints"
        || policy.metric != "maximum absolute sample error"
        || policy.error_upper != "max_samples(max(abs(actual-lo),abs(actual-hi)))"
        || policy.certification != "pass only when error_upper <= epsilon"
    {
        return Err(error(
            Category::Metadata,
            "numeric policy metadata mismatch",
        ));
    }
    if fraction_record(&policy.epsilon, "policy.epsilon")?
        != Rational::new(1.into(), 100_000_000_000_000_i64.into())
        || fraction_record(
            &policy.maximum_interval_width,
            "policy.maximum_interval_width",
        )? != Rational::new(1.into(), BigInt::from(10_u8).pow(80))
    {
        return Err(error(
            Category::Metadata,
            "numeric policy rational mismatch",
        ));
    }
    if manifest.reference.path != "references/sample-intervals.json"
        || manifest.reference.sha256 != REFERENCE_SHA256
        || parse_u64(&manifest.reference.bytes, "reference.bytes")? as usize != reference_raw.len()
        || sha256_digest(reference_raw) != REFERENCE_SHA256
    {
        return Err(error(Category::Inventory, "reference record mismatch"));
    }
    if manifest.assets.len() != 1 {
        return Err(error(Category::Inventory, "expected exactly one asset"));
    }
    let asset = &manifest.assets[0];
    if asset.path != "assets/impulse.pcm"
        || asset.sha256 != ASSET_SHA256
        || asset.bytes != "32"
        || asset.format != "pcm_f32le_interleaved/1"
        || asset.sample_rate_hz != "48000"
        || asset.channels != "1"
        || asset.frames != "8"
    {
        return Err(error(
            Category::Inventory,
            "fixed impulse asset record mismatch",
        ));
    }
    let asset_raw = read(&corpus_root().join(&asset.path))?;
    if asset_raw.len() != 32 || sha256_digest(&asset_raw) != ASSET_SHA256 {
        return Err(error(
            Category::Inventory,
            "fixed impulse asset bytes mismatch",
        ));
    }
    if manifest.cases.len() != EXPECTED_IDS.len() {
        return Err(error(Category::Inventory, "expected exactly six cases"));
    }
    for (index, case) in manifest.cases.iter().enumerate() {
        let (expected_path, expected_hash, expected_bytes) = EXPECTED_SOURCES[index];
        if case.id != EXPECTED_IDS[index]
            || case.source.path != expected_path
            || case.source.sha256 != expected_hash
            || parse_u64(&case.source.bytes, "case.source.bytes")? as usize != expected_bytes
        {
            return Err(error(
                Category::Inventory,
                format!("case {index} fixed identity mismatch"),
            ));
        }
        let source_raw = read(&corpus_root().join(&case.source.path))?;
        if source_raw.len() != expected_bytes || sha256_digest(&source_raw) != expected_hash {
            return Err(error(
                Category::Inventory,
                format!("{} source bytes mismatch", case.id),
            ));
        }
        let expected_assets = if case.id == "one-pole-impulse" {
            &["assets/impulse.pcm".to_owned()][..]
        } else {
            &[][..]
        };
        if case.assets != expected_assets {
            return Err(error(
                Category::Inventory,
                format!("{} asset inventory mismatch", case.id),
            ));
        }
    }
    if !manifest.provenance.is_object() {
        return Err(error(Category::Metadata, "provenance is not an object"));
    }
    if !manifest.claims.static_reference_verified
        || manifest.claims.runtime_render_executed_by_this_checker
        || manifest.claims.cross_platform_bound_proven
        || manifest.claims.full_core_audio_profile_covered
    {
        return Err(error(Category::Metadata, "corpus claim boundary mismatch"));
    }
    Ok(())
}

fn render(artifact: &PlanArtifact) -> HResult<Vec<Vec<f64>>> {
    let mut frames = Vec::new();
    render_artifact(artifact, |frame| {
        frames.push(frame.to_vec());
        Ok(())
    })
    .map_err(|failure| error(Category::Render, failure.to_string()))?;
    Ok(frames)
}

fn required_u64(value: &Value, context: &str) -> HResult<u64> {
    value.as_u64().ok_or_else(|| {
        error(
            Category::Timing,
            format!("{context} is not an unsigned integer"),
        )
    })
}

fn actual_timing(case: &CaseRecord, wire: &Value) -> HResult<TimingRecord> {
    let events = wire["events"]
        .as_array()
        .ok_or_else(|| error(Category::Timing, "artifact events are not an array"))?;
    match &case.timing {
        TimingRecord::NoteGate { .. } => {
            if events.len() != 1 {
                return Err(error(Category::Timing, "expected exactly one note event"));
            }
            Ok(TimingRecord::NoteGate {
                on_frame: required_u64(&events[0]["on_frame"], "event.on_frame")?.to_string(),
                off_frame: required_u64(&events[0]["off_frame"], "event.off_frame")?.to_string(),
            })
        }
        TimingRecord::AudioTransport { .. } => {
            if !events.is_empty() {
                return Err(error(
                    Category::Timing,
                    "audio case unexpectedly has events",
                ));
            }
            let clip = wire["nodes"]
                .as_array()
                .and_then(|nodes| {
                    nodes
                        .iter()
                        .find(|node| node["processor"]["kind"] == "audio")
                })
                .map(|node| &node["processor"]["clip"])
                .ok_or_else(|| error(Category::Timing, "artifact lacks audio clip"))?;
            Ok(TimingRecord::AudioTransport {
                start_frame: required_u64(&clip["start_frame"], "clip.start_frame")?.to_string(),
                end_frame: required_u64(&clip["end_frame"], "clip.end_frame")?.to_string(),
            })
        }
        TimingRecord::EmptyEvents { .. } => {
            if !events.is_empty() {
                return Err(error(Category::Timing, "expected empty event list"));
            }
            Ok(TimingRecord::EmptyEvents { events: Vec::new() })
        }
    }
}

fn compile_case(case: &CaseRecord, manifest: &Manifest) -> HResult<ObservedCase> {
    let root = corpus_root();
    let source_raw = read(&root.join(&case.source.path))?;
    let source = String::from_utf8(source_raw)
        .map_err(|failure| error(Category::Inventory, failure.to_string()))?;
    let mut bundle = SourceBundle::new(case.source.path.clone(), source);
    for asset_path in &case.assets {
        let asset = manifest
            .assets
            .iter()
            .find(|asset| &asset.path == asset_path)
            .ok_or_else(|| error(Category::Inventory, "case asset is absent from manifest"))?;
        let key = Path::new(&asset.path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| error(Category::Inventory, "asset path lacks UTF-8 file name"))?;
        bundle
            .assets
            .insert(key.to_owned(), read(&root.join(&asset.path))?);
    }
    let artifact = compile_bundle_artifact(&bundle).map_err(|diagnostics| {
        error(
            Category::CompilerDiagnostics,
            format!("{}: {diagnostics}", case.id),
        )
    })?;
    let artifact_raw = artifact
        .to_json()
        .map_err(|failure| error(Category::Artifact, failure.to_string()))?;
    let retained = PlanArtifact::from_json(&artifact_raw)
        .map_err(|failure| error(Category::Artifact, failure.to_string()))?;
    let frames = render(&artifact)?;
    let replay = render(&retained)?;
    let replay_bit_identical = frames
        .iter()
        .flatten()
        .map(|sample| sample.to_bits())
        .eq(replay.iter().flatten().map(|sample| sample.to_bits()));
    if !replay_bit_identical {
        return Err(error(
            Category::Artifact,
            format!("{} replay samples differ", case.id),
        ));
    }
    let wire: Value = serde_json::from_slice(&artifact_raw)
        .map_err(|failure| error(Category::Artifact, failure.to_string()))?;
    let output = artifact.output();
    if !output.score_origin_q().is_zero() {
        return Err(error(Category::Timing, "score/reset origin is not zero"));
    }
    let tail_frames_rational =
        &output.tail_seconds * Rational::from_integer(BigInt::from(output.sample_rate_hz));
    if !tail_frames_rational.is_integer() {
        return Err(error(Category::Timing, "tail is not an exact frame count"));
    }
    let tail_frames = tail_frames_rational
        .to_integer()
        .to_u64()
        .ok_or_else(|| error(Category::Timing, "tail frame count exceeds u64"))?;
    let score_frames = output
        .total_frames
        .checked_sub(tail_frames)
        .ok_or_else(|| error(Category::Timing, "tail exceeds total frames"))?;
    let render = RenderRecord {
        sample_rate_hz: output.sample_rate_hz.to_string(),
        reset_frame: "0".to_owned(),
        frame_start: "0".to_owned(),
        frame_end: output.total_frames.to_string(),
        score_frames: score_frames.to_string(),
        tail_frames: tail_frames.to_string(),
        output_port: vec![output.output.node.clone(), output.output.port.clone()],
        channels: output.channels.to_string(),
        channel_order: (0..output.channels)
            .map(|channel| channel.to_string())
            .collect(),
    };
    Ok(ObservedCase {
        id: case.id.clone(),
        render,
        timing: actual_timing(case, &wire)?,
        frames,
        replay_bit_identical,
    })
}

fn validate_suite(
    manifest: &Manifest,
    references: &ReferenceDocument,
    observed: &[ObservedCase],
) -> HResult<SuiteStats> {
    if references.format != REFERENCE_FORMAT || references.version != 1 {
        return Err(error(
            Category::Inventory,
            "reference discriminator mismatch",
        ));
    }
    if manifest.cases.len() != 6 || references.cases.len() != 6 || observed.len() != 6 {
        return Err(error(
            Category::Inventory,
            "suite must contain exactly six cases",
        ));
    }
    let epsilon = fraction_record(&manifest.policy.epsilon, "policy.epsilon")?;
    let maximum_width = fraction_record(
        &manifest.policy.maximum_interval_width,
        "policy.maximum_interval_width",
    )?;
    let mut comparisons = 0;
    let mut maximum_error_upper = Rational::zero();
    let mut worst_case = String::new();
    let mut worst_frame = 0;
    let mut worst_channel = 0;
    for index in 0..6 {
        let case = &manifest.cases[index];
        let reference = &references.cases[index];
        let actual = &observed[index];
        if case.id != EXPECTED_IDS[index] || reference.id != case.id || actual.id != case.id {
            return Err(error(Category::Inventory, "case ID/order mismatch"));
        }
        if actual.render.sample_rate_hz != case.render.sample_rate_hz
            || actual.render.reset_frame != case.render.reset_frame
            || actual.render.frame_start != case.render.frame_start
            || actual.render.frame_end != case.render.frame_end
            || actual.render.output_port != case.render.output_port
            || actual.render.channels != case.render.channels
            || actual.render.channel_order != case.render.channel_order
        {
            return Err(error(
                Category::Metadata,
                format!("{} render metadata mismatch", case.id),
            ));
        }
        if actual.render.score_frames != case.render.score_frames
            || actual.render.tail_frames != case.render.tail_frames
            || actual.timing != case.timing
        {
            return Err(error(
                Category::Timing,
                format!("{} timing metadata mismatch", case.id),
            ));
        }
        let channels = parse_u64(&case.render.channels, "case.render.channels")? as usize;
        if actual.frames.len() != 8 {
            return Err(error(
                Category::Shape,
                format!("{} actual frame count is not 8", case.id),
            ));
        }
        if reference.frames.len() != 8 {
            return Err(error(
                Category::Reference,
                format!("{} reference frame count is not 8", case.id),
            ));
        }
        for frame_index in 0..8 {
            if actual.frames[frame_index].len() != channels {
                return Err(error(
                    Category::Shape,
                    format!("{} frame {frame_index} channel shape mismatch", case.id),
                ));
            }
            if reference.frames[frame_index].len() != channels {
                return Err(error(
                    Category::Reference,
                    format!(
                        "{} frame {frame_index} reference channel shape mismatch",
                        case.id
                    ),
                ));
            }
            for channel in 0..channels {
                let upper = compare_sample(
                    actual.frames[frame_index][channel],
                    &reference.frames[frame_index][channel],
                    &epsilon,
                    &maximum_width,
                )?;
                comparisons += 1;
                if upper > maximum_error_upper {
                    maximum_error_upper = upper;
                    worst_case.clone_from(&case.id);
                    worst_frame = frame_index;
                    worst_channel = channel;
                }
            }
        }
        if !actual.replay_bit_identical {
            return Err(error(Category::Artifact, "replay identity flag is false"));
        }
    }
    if comparisons != 64 {
        return Err(error(
            Category::Shape,
            format!("expected 64 comparisons, found {comparisons}"),
        ));
    }
    Ok(SuiteStats {
        comparisons,
        maximum_error_upper,
        worst_case,
        worst_frame,
        worst_channel,
    })
}

fn render_suite(manifest: &Manifest) -> HResult<Vec<ObservedCase>> {
    manifest
        .cases
        .iter()
        .map(|case| compile_case(case, manifest))
        .collect()
}

fn maybe_write_observations(observed: &[ObservedCase], stats: &SuiteStats) -> HResult<()> {
    let Some(path) = env::var_os("MAAC_L5_OBSERVATIONS") else {
        return Ok(());
    };
    let cases = observed
        .iter()
        .map(|case| {
            json!({
                "id": case.id,
                "render": case.render,
                "timing": case.timing,
                "replay_bit_identical": case.replay_bit_identical,
                "sample_bits": case.frames.iter().map(|frame| frame.iter().map(|sample| format!("{:016x}", sample.to_bits())).collect::<Vec<_>>()).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let report = json!({
        "format": "maac.l5.local-runtime-observation",
        "version": 1,
        "candidate": env::var("MAAC_L5_CANDIDATE").unwrap_or_else(|_| "unrecorded".into()),
        "policy": POLICY_ID,
        "case_count": observed.len(),
        "comparison_count": stats.comparisons,
        "maximum_error_upper": fraction_text(&stats.maximum_error_upper),
        "worst": {"case": stats.worst_case, "frame": stats.worst_frame, "channel": stats.worst_channel},
        "cases": cases,
    });
    let mut bytes = serde_json::to_vec_pretty(&report)
        .map_err(|failure| error(Category::Io, failure.to_string()))?;
    bytes.push(b'\n');
    fs::write(PathBuf::from(path), bytes)
        .map_err(|failure| error(Category::Io, failure.to_string()))
}

fn interval(value: &Rational) -> Interval {
    let text = fraction_text(value);
    Interval {
        lo: text.clone(),
        hi: text,
    }
}

#[test]
fn public_render_matches_all_fixed_reference_intervals() -> HResult<()> {
    let (manifest, references) = load_corpus()?;
    let observed = render_suite(&manifest)?;
    let stats = validate_suite(&manifest, &references, &observed)?;
    if stats.comparisons != 64 {
        return Err(error(Category::Shape, "comparison count was not 64"));
    }
    maybe_write_observations(&observed, &stats)
}

#[test]
fn comparator_is_inclusive_and_uses_the_farthest_endpoint() {
    let epsilon = Rational::new(1.into(), 100_000_000_000_000_i64.into());
    let delta = Rational::new(1.into(), BigInt::from(10_u8).pow(81));
    let maximum_width = Rational::new(1.into(), BigInt::from(10_u8).pow(80));
    assert!(compare_sample(
        0.0,
        &interval(&(&epsilon - &delta)),
        &epsilon,
        &maximum_width
    )
    .is_ok());
    assert!(compare_sample(0.0, &interval(&epsilon), &epsilon, &maximum_width).is_ok());
    let above = compare_sample(
        0.0,
        &interval(&(&epsilon + &delta)),
        &epsilon,
        &maximum_width,
    )
    .unwrap_err();
    assert_eq!(above.category, Category::Numerical);
    let crossing = Interval {
        lo: fraction_text(&(&epsilon - &delta)),
        hi: fraction_text(&(&epsilon + &delta)),
    };
    let crossing_error = compare_sample(0.0, &crossing, &epsilon, &maximum_width).unwrap_err();
    assert_eq!(crossing_error.category, Category::Numerical);
}

#[test]
fn comparator_rejects_every_nonfinite_binary64_value() {
    let epsilon = Rational::new(1.into(), 100_000_000_000_000_i64.into());
    let maximum_width = Rational::new(1.into(), BigInt::from(10_u8).pow(80));
    let point = Interval {
        lo: "0/1".into(),
        hi: "0/1".into(),
    };
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            compare_sample(value, &point, &epsilon, &maximum_width)
                .unwrap_err()
                .category,
            Category::NonFinite
        );
    }
}

#[test]
fn comparator_rejects_malformed_reversed_unreduced_and_wide_intervals() {
    let epsilon = Rational::new(1.into(), 100_000_000_000_000_i64.into());
    let maximum_width = Rational::new(1.into(), BigInt::from(10_u8).pow(80));
    for bad in [
        Interval {
            lo: "zero".into(),
            hi: "0/1".into(),
        },
        Interval {
            lo: "1/1".into(),
            hi: "0/1".into(),
        },
        Interval {
            lo: "2/2".into(),
            hi: "1/1".into(),
        },
        Interval {
            lo: "0/1".into(),
            hi: format!("1/{}", BigInt::from(10_u8).pow(79)),
        },
    ] {
        assert_eq!(
            compare_sample(0.0, &bad, &epsilon, &maximum_width)
                .unwrap_err()
                .category,
            Category::Reference
        );
    }
}

#[test]
fn suite_rejects_missing_extra_frame_channel_case_and_output_metadata() -> HResult<()> {
    let (manifest, references) = load_corpus()?;
    let mut zeroed = manifest
        .cases
        .iter()
        .map(|case| ObservedCase {
            id: case.id.clone(),
            render: case.render.clone(),
            timing: case.timing.clone(),
            frames: vec![
                vec![0.0; parse_u64(&case.render.channels, "channels").unwrap() as usize];
                8
            ],
            replay_bit_identical: true,
        })
        .collect::<Vec<_>>();

    let mut missing_case = zeroed.clone();
    missing_case.pop();
    assert_eq!(
        validate_suite(&manifest, &references, &missing_case)
            .unwrap_err()
            .category,
        Category::Inventory
    );
    let mut extra_case = zeroed.clone();
    extra_case.push(zeroed[0].clone());
    assert_eq!(
        validate_suite(&manifest, &references, &extra_case)
            .unwrap_err()
            .category,
        Category::Inventory
    );
    let mut missing_frame = zeroed.clone();
    missing_frame[0].frames.pop();
    assert_eq!(
        validate_suite(&manifest, &references, &missing_frame)
            .unwrap_err()
            .category,
        Category::Shape
    );
    let mut extra_frame = zeroed.clone();
    extra_frame[0].frames.push(vec![0.0]);
    assert_eq!(
        validate_suite(&manifest, &references, &extra_frame)
            .unwrap_err()
            .category,
        Category::Shape
    );
    let mut missing_channel = zeroed.clone();
    missing_channel[0].frames[0].pop();
    assert_eq!(
        validate_suite(&manifest, &references, &missing_channel)
            .unwrap_err()
            .category,
        Category::Shape
    );
    let mut extra_channel = zeroed.clone();
    extra_channel[0].frames[0].push(0.0);
    assert_eq!(
        validate_suite(&manifest, &references, &extra_channel)
            .unwrap_err()
            .category,
        Category::Shape
    );
    zeroed[0].render.output_port = vec!["wrong".into(), "out".into()];
    assert_eq!(
        validate_suite(&manifest, &references, &zeroed)
            .unwrap_err()
            .category,
        Category::Metadata
    );
    Ok(())
}

#[test]
fn isolated_last_sample_corruption_is_not_vacuously_accepted() -> HResult<()> {
    let (manifest, references) = load_corpus()?;
    let mut observed = render_suite(&manifest)?;
    let last_case = observed.last_mut().expect("six fixed cases");
    last_case.frames[7][0] += 1.0e-6;
    assert_eq!(
        validate_suite(&manifest, &references, &observed)
            .unwrap_err()
            .category,
        Category::Numerical
    );
    Ok(())
}

#[test]
fn delay_tail_timing_and_pre_tail_history_are_both_checked() -> HResult<()> {
    let (manifest, references) = load_corpus()?;
    let mut observed = render_suite(&manifest)?;
    let delay_index = EXPECTED_IDS
        .iter()
        .position(|id| *id == "delay-feedback")
        .expect("fixed delay case");
    observed[delay_index].render.tail_frames = "3".into();
    assert_eq!(
        validate_suite(&manifest, &references, &observed)
            .unwrap_err()
            .category,
        Category::Timing
    );

    let mut observed = render_suite(&manifest)?;
    observed[delay_index].frames[4][0] = 0.0;
    assert_eq!(
        validate_suite(&manifest, &references, &observed)
            .unwrap_err()
            .category,
        Category::Numerical
    );
    Ok(())
}
