use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use maac::{bundle::sha256_digest, compiler::compile_bundle_artifact, PlanArtifact, SourceBundle};
use serde::Deserialize;
use serde_json::Value;

const CORPUS_SCHEMA: &str = "maac.l3.pan-contract-corpus/1";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    schema: String,
    rate_hz: u32,
    cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    source: String,
    status: String,
    #[serde(default)]
    diagnostic: Option<String>,
    #[serde(default)]
    total_frames: Option<u64>,
    #[serde(default)]
    raw_pan: Option<String>,
    #[serde(default)]
    automation_values: Vec<String>,
    #[serde(default)]
    modulations: Vec<ExpectedModulation>,
    #[serde(default)]
    sample: Option<ExpectedSample>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ExpectedModulation {
    id: String,
    amount: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedSample {
    frame: usize,
    channels: [f64; 2],
}

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/l3")
}

fn load_corpus() -> Corpus {
    let bytes = fs::read(corpus_root().join("expected.json")).expect("read L3 expected data");
    serde_json::from_slice(&bytes).expect("parse L3 expected data")
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
    let actual_total = wire["output"]["total_frames"]
        .as_u64()
        .ok_or_else(|| format!("{}: output.total_frames is not an integer", case.id))?;
    if actual_total != expected_total {
        return Err(format!(
            "{}: expected {expected_total} total frames, found {actual_total}",
            case.id
        ));
    }

    let pan_node = wire["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|node| node["id"] == "pan"))
        .ok_or_else(|| format!("{}: retained artifact lacks pan node", case.id))?;
    let actual_pan = pan_node["params"]["pan"]
        .as_str()
        .ok_or_else(|| format!("{}: retained pan is not an exact rational", case.id))?;
    let expected_pan = case
        .raw_pan
        .as_deref()
        .ok_or_else(|| format!("{}: successful case lacks raw_pan", case.id))?;
    if actual_pan != expected_pan {
        return Err(format!(
            "{}: expected retained pan {expected_pan}, found {actual_pan}",
            case.id
        ));
    }

    let actual_automation = wire["automation"]
        .as_array()
        .ok_or_else(|| format!("{}: automation is not an array", case.id))?
        .iter()
        .flat_map(|lane| {
            lane["points"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|point| point["value"].as_str())
        })
        .collect::<Vec<_>>();
    if actual_automation != case.automation_values {
        return Err(format!(
            "{}: expected automation values {:?}, found {actual_automation:?}",
            case.id, case.automation_values
        ));
    }

    let modulation_items = match wire.get("modulations") {
        None => &[][..],
        Some(Value::Array(items)) => items.as_slice(),
        Some(_) => return Err(format!("{}: modulations is not an array", case.id)),
    };
    let actual_modulations = modulation_items
        .iter()
        .map(|modulation| {
            Ok(ExpectedModulation {
                id: modulation["id"]
                    .as_str()
                    .ok_or_else(|| format!("{}: modulation id is not a string", case.id))?
                    .into(),
                amount: modulation["amount"]
                    .as_str()
                    .ok_or_else(|| format!("{}: modulation amount is not exact", case.id))?
                    .into(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if actual_modulations != case.modulations {
        return Err(format!(
            "{}: expected modulations {:?}, found {actual_modulations:?}",
            case.id, case.modulations
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

    let expected_sample = case
        .sample
        .as_ref()
        .ok_or_else(|| format!("{}: successful case lacks sample", case.id))?;
    let actual_sample = audio
        .get(expected_sample.frame)
        .ok_or_else(|| format!("{}: sample frame is outside output", case.id))?;
    if actual_sample.len() != 2 {
        return Err(format!(
            "{}: expected stereo output, found {} channels",
            case.id,
            actual_sample.len()
        ));
    }
    for (channel, expected) in expected_sample.channels.iter().enumerate() {
        assert_close(
            actual_sample[channel],
            *expected,
            &format!(
                "{} frame {} channel {channel}",
                case.id, expected_sample.frame
            ),
        )?;
    }
    Ok(())
}

fn validate_case(case: &Case) -> Result<(), String> {
    let source_path = corpus_root().join(&case.source);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("{}: {error}", source_path.display()))?;
    let bundle = SourceBundle::new(case.source.clone(), source);
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

fn validate_inventory(corpus: &Corpus) -> Result<(), String> {
    if corpus.schema != CORPUS_SCHEMA {
        return Err(format!("unsupported corpus schema {:?}", corpus.schema));
    }
    if corpus.rate_hz != 48_000 {
        return Err(format!(
            "expected fixed 48000 Hz corpus, found {}",
            corpus.rate_hz
        ));
    }
    if corpus.cases.len() != 8 {
        return Err(format!("expected 8 cases, found {}", corpus.cases.len()));
    }
    let ids = corpus
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != corpus.cases.len() {
        return Err("case IDs must be unique".into());
    }
    let listed = corpus
        .cases
        .iter()
        .map(|case| case.source.clone())
        .collect::<BTreeSet<_>>();
    if listed.len() != corpus.cases.len() {
        return Err("case sources must be unique".into());
    }
    let actual = fs::read_dir(corpus_root().join("cases"))
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry.map_err(|error| error.to_string()).and_then(|entry| {
                entry
                    .path()
                    .strip_prefix(corpus_root())
                    .map(|path| path.to_string_lossy().into_owned())
                    .map_err(|error| error.to_string())
            })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if actual != listed {
        return Err(format!(
            "case source inventory mismatch: listed {listed:?}, found {actual:?}"
        ));
    }
    Ok(())
}

#[test]
fn fixed_core_pan_contract_corpus_matches_public_artifacts_and_renderer() {
    let corpus = load_corpus();
    validate_inventory(&corpus).unwrap();
    for case in &corpus.cases {
        validate_case(case).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn raw_retention_and_clamp_order_faults_are_detected() {
    let corpus = load_corpus();

    let mut normalized_raw = corpus
        .cases
        .iter()
        .find(|case| case.id == "raw-positive")
        .unwrap()
        .clone();
    normalized_raw.raw_pan = Some("1/1".into());
    let error = validate_case(&normalized_raw).unwrap_err();
    assert!(error.contains("expected retained pan 1/1"), "{error}");

    let mut preclamped_curve = corpus
        .cases
        .iter()
        .find(|case| case.id == "raw-linear-automation")
        .unwrap()
        .clone();
    preclamped_curve.sample.as_mut().unwrap().channels = [-0.3826834323650898, -0.9238795325112867];
    let error = validate_case(&preclamped_curve).unwrap_err();
    assert!(error.contains("frame 36000 channel 0"), "{error}");
}

#[test]
fn malformed_case_inventory_is_rejected_before_execution() {
    let mut corpus = load_corpus();
    corpus.cases.pop();
    assert_eq!(
        validate_inventory(&corpus).unwrap_err(),
        "expected 8 cases, found 7"
    );
}

const INPUT_CORPUS_SCHEMA: &str = "maac.l3.input-contract-corpus/1";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputCorpus {
    schema: String,
    rate_hz: u32,
    asset: InputAsset,
    cases: Vec<InputCase>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputAsset {
    file: String,
    bundle_path: String,
    sha256: String,
    bytes: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputCase {
    id: String,
    source: String,
    status: String,
    #[serde(default)]
    uses_asset: bool,
    #[serde(default)]
    diagnostic: Option<ExpectedDiagnostic>,
    #[serde(default)]
    total_frames: Option<u64>,
    #[serde(default)]
    incoming: BTreeMap<String, usize>,
    #[serde(default)]
    connection_ids: Vec<String>,
    #[serde(default)]
    empty_event_targets: Vec<String>,
    #[serde(default)]
    no_input_nodes: Vec<String>,
    #[serde(default)]
    sample: Option<InputSample>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedDiagnostic {
    code: String,
    object_path: Vec<String>,
    field_path: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputSample {
    frame: usize,
    channels: Vec<f64>,
}

fn input_corpus_root() -> PathBuf {
    corpus_root().join("inputs")
}

fn load_input_corpus() -> InputCorpus {
    let bytes =
        fs::read(input_corpus_root().join("expected.json")).expect("read L3 input expected data");
    serde_json::from_slice(&bytes).expect("parse L3 input expected data")
}

fn validate_input_inventory(corpus: &InputCorpus) -> Result<(), String> {
    if corpus.schema != INPUT_CORPUS_SCHEMA {
        return Err(format!(
            "unsupported input corpus schema {:?}",
            corpus.schema
        ));
    }
    if corpus.rate_hz != 48_000 {
        return Err(format!(
            "expected fixed 48000 Hz input corpus, found {}",
            corpus.rate_hz
        ));
    }
    if corpus.cases.len() != 17 {
        return Err(format!(
            "expected 17 input cases, found {}",
            corpus.cases.len()
        ));
    }
    let ids = corpus
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != corpus.cases.len() {
        return Err("input case IDs must be unique".into());
    }
    let listed = corpus
        .cases
        .iter()
        .map(|case| case.source.clone())
        .collect::<BTreeSet<_>>();
    if listed.len() != corpus.cases.len() {
        return Err("input case sources must be unique".into());
    }
    let actual = fs::read_dir(input_corpus_root().join("cases"))
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry.map_err(|error| error.to_string()).and_then(|entry| {
                entry
                    .path()
                    .strip_prefix(input_corpus_root())
                    .map(|path| path.to_string_lossy().into_owned())
                    .map_err(|error| error.to_string())
            })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if actual != listed {
        return Err(format!(
            "input case source inventory mismatch: listed {listed:?}, found {actual:?}"
        ));
    }
    let asset = fs::read(input_corpus_root().join(&corpus.asset.file))
        .map_err(|error| error.to_string())?;
    if asset.len() != corpus.asset.bytes {
        return Err(format!(
            "expected {} asset bytes, found {}",
            corpus.asset.bytes,
            asset.len()
        ));
    }
    let actual_hash = sha256_digest(&asset);
    if actual_hash.strip_prefix("sha256:") != Some(corpus.asset.sha256.as_str()) {
        return Err(format!(
            "expected asset sha256 {}, found {actual_hash}",
            corpus.asset.sha256
        ));
    }
    Ok(())
}

fn input_bundle(case: &InputCase, corpus: &InputCorpus) -> Result<SourceBundle, String> {
    let source_path = input_corpus_root().join(&case.source);
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("{}: {error}", source_path.display()))?;
    let mut bundle = SourceBundle::new(case.source.clone(), source);
    if case.uses_asset {
        let bytes = fs::read(input_corpus_root().join(&corpus.asset.file))
            .map_err(|error| error.to_string())?;
        bundle
            .assets
            .insert(corpus.asset.bundle_path.clone(), bytes);
    }
    Ok(bundle)
}

fn validate_input_success(case: &InputCase, artifact: &PlanArtifact) -> Result<(), String> {
    let encoded = artifact.to_json().map_err(|error| error.to_string())?;
    let retained = PlanArtifact::from_json(&encoded).map_err(|error| error.to_string())?;
    let retained_encoded = retained.to_json().map_err(|error| error.to_string())?;
    if encoded != retained_encoded {
        return Err(format!("{}: retained artifact JSON bytes changed", case.id));
    }
    let wire: Value = serde_json::from_slice(&encoded).map_err(|error| error.to_string())?;
    let connections = wire["connections"]
        .as_array()
        .ok_or_else(|| format!("{}: retained connections is not an array", case.id))?;
    let actual_ids = connections
        .iter()
        .map(|connection| {
            connection["id"]
                .as_str()
                .ok_or_else(|| format!("{}: connection id is not a string", case.id))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected_ids = case
        .connection_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if actual_ids != expected_ids {
        return Err(format!(
            "{}: expected connection IDs {expected_ids:?}, found {actual_ids:?}",
            case.id
        ));
    }
    for (node, expected) in &case.incoming {
        let actual = connections
            .iter()
            .filter(|connection| connection["to"]["node"] == node.as_str())
            .count();
        if actual != *expected {
            return Err(format!(
                "{}: expected {expected} incoming connections for {node}, found {actual}",
                case.id
            ));
        }
    }
    let node_ids = wire["nodes"]
        .as_array()
        .ok_or_else(|| format!("{}: retained nodes is not an array", case.id))?
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect::<BTreeSet<_>>();
    for node in &case.no_input_nodes {
        if !node_ids.contains(node.as_str()) {
            return Err(format!("{}: retained nodes lacks {node}", case.id));
        }
        if connections
            .iter()
            .any(|connection| connection["to"]["node"] == node.as_str())
        {
            return Err(format!(
                "{}: no-input node {node} retained an incoming connection",
                case.id
            ));
        }
    }
    let events = wire["events"]
        .as_array()
        .ok_or_else(|| format!("{}: retained events is not an array", case.id))?;
    for target in &case.empty_event_targets {
        if !node_ids.contains(target.as_str()) {
            return Err(format!("{}: retained nodes lacks {target}", case.id));
        }
        if events
            .iter()
            .any(|event| event["target"]["node"] == target.as_str())
        {
            return Err(format!(
                "{}: empty event target {target} retained an event",
                case.id
            ));
        }
    }
    let expected_total = case
        .total_frames
        .ok_or_else(|| format!("{}: successful input case lacks total_frames", case.id))?;
    let actual_total = wire["output"]["total_frames"]
        .as_u64()
        .ok_or_else(|| format!("{}: output.total_frames is not an integer", case.id))?;
    if actual_total != expected_total {
        return Err(format!(
            "{}: expected {expected_total} total frames, found {actual_total}",
            case.id
        ));
    }
    let audio = render(artifact)?;
    let replay = render(&retained)?;
    if audio.len() as u64 != expected_total {
        return Err(format!(
            "{}: renderer emitted {} frames, expected {expected_total}",
            case.id,
            audio.len()
        ));
    }
    if audio
        .iter()
        .flatten()
        .map(|sample| sample.to_bits())
        .ne(replay.iter().flatten().map(|sample| sample.to_bits()))
    {
        return Err(format!("{}: retained replay changed samples", case.id));
    }
    let sample = case
        .sample
        .as_ref()
        .ok_or_else(|| format!("{}: successful input case lacks sample", case.id))?;
    let actual = audio
        .get(sample.frame)
        .ok_or_else(|| format!("{}: sample frame is outside output", case.id))?;
    if actual.len() != sample.channels.len() {
        return Err(format!(
            "{}: expected {} sample channels, found {}",
            case.id,
            sample.channels.len(),
            actual.len()
        ));
    }
    for (channel, expected) in sample.channels.iter().enumerate() {
        assert_close(
            actual[channel],
            *expected,
            &format!("{} frame {} channel {channel}", case.id, sample.frame),
        )?;
    }
    Ok(())
}

fn validate_input_case(case: &InputCase, corpus: &InputCorpus) -> Result<(), String> {
    let bundle = input_bundle(case, corpus)?;
    match case.status.as_str() {
        "ok" => {
            let artifact = compile_bundle_artifact(&bundle).map_err(|diagnostics| {
                format!("{}: unexpected diagnostics: {diagnostics}", case.id)
            })?;
            validate_input_success(case, &artifact)
        }
        "error" => {
            let expected = case
                .diagnostic
                .as_ref()
                .ok_or_else(|| format!("{}: error input case lacks diagnostic", case.id))?;
            let diagnostics = match compile_bundle_artifact(&bundle) {
                Ok(_) => return Err(format!("{}: expected compilation to fail", case.id)),
                Err(diagnostics) => diagnostics,
            };
            if diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_str() == expected.code
                    && diagnostic.object_path == expected.object_path
                    && diagnostic.field_path == expected.field_path
            }) {
                Ok(())
            } else {
                Err(format!(
                    "{}: expected {} at {:?}/{:?}, found {diagnostics}",
                    case.id, expected.code, expected.object_path, expected.field_path
                ))
            }
        }
        other => Err(format!("{}: unknown input status {other:?}", case.id)),
    }
}

#[test]
fn fixed_core_input_contract_corpus_matches_public_artifacts_and_renderer() {
    let corpus = load_input_corpus();
    validate_input_inventory(&corpus).unwrap();
    for case in &corpus.cases {
        validate_input_case(case, &corpus).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn input_cardinality_faults_are_detected() {
    let corpus = load_input_corpus();
    let mut aggregate = corpus
        .cases
        .iter()
        .find(|case| case.id == "summing-and-empty-events")
        .unwrap()
        .clone();
    aggregate.incoming.insert("empty".into(), 1);
    let error = validate_input_case(&aggregate, &corpus).unwrap_err();
    assert!(
        error.contains("expected 1 incoming connections for empty"),
        "{error}"
    );

    let mut wrong_path = corpus
        .cases
        .iter()
        .find(|case| case.id == "gain-two")
        .unwrap()
        .clone();
    wrong_path.diagnostic.as_mut().unwrap().object_path.pop();
    let error = validate_input_case(&wrong_path, &corpus).unwrap_err();
    assert!(error.contains("expected E_PORT_TYPE"), "{error}");
}

#[test]
fn malformed_input_inventory_is_rejected_before_execution() {
    let mut corpus = load_input_corpus();
    corpus.cases.pop();
    assert_eq!(
        validate_input_inventory(&corpus).unwrap_err(),
        "expected 17 input cases, found 16"
    );
}
