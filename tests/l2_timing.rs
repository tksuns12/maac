use std::{collections::BTreeSet, fs, path::PathBuf};

use maac::{bundle::sha256_digest, compiler::compile_bundle_artifact, PlanArtifact, SourceBundle};
use serde::Deserialize;
use serde_json::Value;

const CORPUS_SCHEMA: &str = "maac.l2.timing-corpus/1";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    schema: String,
    rate_hz: u32,
    asset: Asset,
    cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    path: String,
    bytes: usize,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    kind: String,
    source: String,
    status: String,
    #[serde(default)]
    diagnostic: Option<String>,
    #[serde(default)]
    total_frames: Option<u64>,
    #[serde(default)]
    observations: Vec<Observation>,
    #[serde(default)]
    clip_frames: Option<[u64; 2]>,
    #[serde(default)]
    samples: Vec<Sample>,
    #[serde(default)]
    event_frames: Option<[u64; 2]>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    frame: usize,
    ratio: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    frame: usize,
    value: String,
}

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/l2")
}

fn load_corpus() -> Corpus {
    let bytes = fs::read(corpus_root().join("expected.json")).expect("read L2 expected data");
    serde_json::from_slice(&bytes).expect("parse L2 expected data")
}

fn parse_ratio(text: &str) -> Result<f64, String> {
    let (numerator, denominator) = text
        .split_once('/')
        .ok_or_else(|| format!("expected rational numerator/denominator, found {text:?}"))?;
    let numerator = numerator
        .parse::<i64>()
        .map_err(|_| format!("invalid rational numerator in {text:?}"))?;
    let denominator = denominator
        .parse::<i64>()
        .map_err(|_| format!("invalid rational denominator in {text:?}"))?;
    if denominator <= 0 {
        return Err(format!("rational denominator must be positive in {text:?}"));
    }
    Ok(numerator as f64 / denominator as f64)
}

fn render(artifact: &PlanArtifact) -> Result<Vec<Vec<f64>>, String> {
    let mut frames = Vec::new();
    maac::render_artifact(artifact, |frame| {
        frames.push(frame.to_vec());
        Ok(())
    })
    .map_err(|error| error.to_string())?;
    Ok(frames)
}

fn artifact_json(artifact: &PlanArtifact) -> Result<Value, String> {
    serde_json::from_slice(&artifact.to_json().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn case_bundle(case: &Case, asset: &[u8]) -> Result<SourceBundle, String> {
    let source_path = corpus_root().join(&case.source);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("{}: {error}", source_path.display()))?;
    let mut bundle = SourceBundle::new(case.source.clone(), source);
    if case.kind == "audio" {
        bundle
            .assets
            .insert("assets/two-frame.pcm".into(), asset.to_vec());
    }
    Ok(bundle)
}

fn require_u64(value: &Value, path: &str) -> Result<u64, String> {
    value
        .as_u64()
        .ok_or_else(|| format!("expected unsigned integer at {path}"))
}

fn assert_close(actual: f64, expected: f64, context: &str) -> Result<(), String> {
    if !actual.is_finite() || !expected.is_finite() || (actual - expected).abs() > 1e-12 {
        return Err(format!(
            "{context}: expected {expected:.17}, found {actual:.17}"
        ));
    }
    Ok(())
}

fn validate_success(case: &Case, artifact: &PlanArtifact) -> Result<(), String> {
    let wire = artifact_json(artifact)?;
    let expected_total = case
        .total_frames
        .ok_or_else(|| format!("{}: successful case lacks total_frames", case.id))?;
    let actual_total = require_u64(&wire["output"]["total_frames"], "output.total_frames")?;
    if actual_total != expected_total {
        return Err(format!(
            "{}: expected {expected_total} total frames, found {actual_total}",
            case.id
        ));
    }

    let audio = render(artifact)?;
    if audio.len() as u64 != expected_total {
        return Err(format!(
            "{}: renderer emitted {} frames, expected {expected_total}",
            case.id,
            audio.len()
        ));
    }
    let retained = PlanArtifact::from_json(&artifact.to_json().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let replay = render(&retained)?;
    if audio
        .iter()
        .flatten()
        .map(|sample| sample.to_bits())
        .ne(replay.iter().flatten().map(|sample| sample.to_bits()))
    {
        return Err(format!(
            "{}: retained artifact changed rendered samples",
            case.id
        ));
    }

    match case.kind.as_str() {
        "automation" => {
            for observation in &case.observations {
                let frame = audio
                    .get(observation.frame)
                    .ok_or_else(|| format!("{}: observation frame is outside output", case.id))?;
                if frame.len() != 2 || frame[1].abs() < 0.01 {
                    return Err(format!(
                        "{}: frame {} lacks an observable stereo reference sample",
                        case.id, observation.frame
                    ));
                }
                let expected = parse_ratio(&observation.ratio)?;
                assert_close(
                    frame[0] / frame[1],
                    expected,
                    &format!("{} frame {} left/right ratio", case.id, observation.frame),
                )?;
            }
        }
        "audio" => {
            let expected_clip = case
                .clip_frames
                .ok_or_else(|| format!("{}: audio case lacks clip_frames", case.id))?;
            let clip = wire["nodes"]
                .as_array()
                .and_then(|nodes| {
                    nodes
                        .iter()
                        .find(|node| node["processor"]["kind"] == "audio")
                })
                .map(|node| &node["processor"]["clip"])
                .ok_or_else(|| format!("{}: compiled artifact lacks audio clip", case.id))?;
            let actual_clip = [
                require_u64(&clip["start_frame"], "clip.start_frame")?,
                require_u64(&clip["end_frame"], "clip.end_frame")?,
            ];
            if actual_clip != expected_clip {
                return Err(format!(
                    "{}: expected clip frames {expected_clip:?}, found {actual_clip:?}",
                    case.id
                ));
            }
            for sample in &case.samples {
                let actual = audio
                    .get(sample.frame)
                    .and_then(|frame| frame.first())
                    .copied()
                    .ok_or_else(|| format!("{}: sample frame is outside output", case.id))?;
                assert_close(
                    actual,
                    parse_ratio(&sample.value)?,
                    &format!("{} frame {} sample", case.id, sample.frame),
                )?;
            }
        }
        "note" => {
            let expected = case
                .event_frames
                .ok_or_else(|| format!("{}: note case lacks event_frames", case.id))?;
            let events = wire["events"]
                .as_array()
                .ok_or_else(|| format!("{}: artifact events are not an array", case.id))?;
            if events.len() != 1 {
                return Err(format!(
                    "{}: expected one event, found {}",
                    case.id,
                    events.len()
                ));
            }
            let actual = [
                require_u64(&events[0]["on_frame"], "events[0].on_frame")?,
                require_u64(&events[0]["off_frame"], "events[0].off_frame")?,
            ];
            if actual != expected {
                return Err(format!(
                    "{}: expected event frames {expected:?}, found {actual:?}",
                    case.id
                ));
            }
        }
        other => return Err(format!("{}: unknown case kind {other:?}", case.id)),
    }
    Ok(())
}

fn validate_case(case: &Case, asset: &[u8]) -> Result<(), String> {
    let bundle = case_bundle(case, asset)?;
    match case.status.as_str() {
        "ok" => {
            let artifact = compile_bundle_artifact(&bundle).map_err(|diagnostics| {
                format!("{}: unexpected diagnostics: {diagnostics}", case.id)
            })?;
            validate_success(case, &artifact)
        }
        "error" => {
            let expected = case
                .diagnostic
                .as_deref()
                .ok_or_else(|| format!("{}: error case lacks diagnostic", case.id))?;
            let diagnostics = match compile_bundle_artifact(&bundle) {
                Ok(_) => return Err(format!("{}: expected compilation to fail", case.id)),
                Err(diagnostics) => diagnostics,
            };
            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_str() == expected)
            {
                Ok(())
            } else {
                Err(format!(
                    "{}: expected {expected}, found {diagnostics}",
                    case.id
                ))
            }
        }
        other => Err(format!("{}: unknown status {other:?}", case.id)),
    }
}

fn validate_inventory(corpus: &Corpus, asset: &[u8]) -> Result<(), String> {
    if corpus.schema != CORPUS_SCHEMA {
        return Err(format!("unsupported corpus schema {:?}", corpus.schema));
    }
    if corpus.rate_hz != 48_000 {
        return Err(format!(
            "expected fixed 48000 Hz corpus, found {}",
            corpus.rate_hz
        ));
    }
    if corpus.cases.len() != 16 {
        return Err(format!("expected 16 cases, found {}", corpus.cases.len()));
    }
    let ids = corpus
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != corpus.cases.len() {
        return Err("case IDs must be unique".into());
    }
    let kind_counts = ["automation", "audio", "note"]
        .map(|kind| corpus.cases.iter().filter(|case| case.kind == kind).count());
    if kind_counts != [7, 4, 5] {
        return Err(format!(
            "expected automation/audio/note counts [7, 4, 5], found {kind_counts:?}"
        ));
    }
    if corpus.asset.bytes != asset.len() {
        return Err(format!(
            "expected {} asset bytes, found {}",
            corpus.asset.bytes,
            asset.len()
        ));
    }
    let digest = sha256_digest(asset);
    let digest = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("unexpected digest encoding {digest:?}"))?;
    if digest != corpus.asset.sha256 {
        return Err(format!(
            "asset digest mismatch: expected {}, found {digest}",
            corpus.asset.sha256
        ));
    }
    Ok(())
}

#[test]
fn fixed_l2_timing_corpus_matches_public_compiler_and_renderer() {
    let corpus = load_corpus();
    let asset = fs::read(corpus_root().join(&corpus.asset.path)).expect("read L2 PCM asset");
    validate_inventory(&corpus, &asset).unwrap();
    for case in &corpus.cases {
        validate_case(case, &asset).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn wrong_origin_and_unclamped_tail_expectations_are_detected() {
    let corpus = load_corpus();
    let asset = fs::read(corpus_root().join(&corpus.asset.path)).expect("read L2 PCM asset");

    let mut wrong_origin = corpus
        .cases
        .iter()
        .find(|case| case.id == "automation-score")
        .unwrap()
        .clone();
    wrong_origin.observations[1].ratio = "48001/96000".into();
    let error = validate_case(&wrong_origin, &asset).unwrap_err();
    assert!(error.contains("frame 72001 left/right ratio"), "{error}");

    let mut unclamped_tail = corpus
        .cases
        .iter()
        .find(|case| case.id == "automation-seconds-q-anchor")
        .unwrap()
        .clone();
    unclamped_tail.observations[2].ratio = "72001/96000".into();
    let error = validate_case(&unclamped_tail, &asset).unwrap_err();
    assert!(error.contains("frame 96001 left/right ratio"), "{error}");
}

#[test]
fn malformed_expected_inventory_is_rejected_before_execution() {
    let mut corpus = load_corpus();
    let asset = fs::read(corpus_root().join(&corpus.asset.path)).expect("read L2 PCM asset");
    corpus.cases.pop();
    assert_eq!(
        validate_inventory(&corpus, &asset).unwrap_err(),
        "expected 16 cases, found 15"
    );
}
