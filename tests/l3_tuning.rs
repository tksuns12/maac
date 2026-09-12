use std::{collections::BTreeSet, fs, path::PathBuf};

use maac::{
    compiler::compile_bundle_artifact, Diagnostic, DiagnosticCode, Diagnostics, PlanArtifact,
    SourceBundle,
};
use serde::Deserialize;
use serde_json::Value;

const NEGATIVE_REFERENCE_SOURCE: &str = r#"maac 1;
project p {
  score = [0q, 1q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &sine:out;
}
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
tuning t {
  period = 1200ct;
  steps = [0ct, 700ct];
  reference_index = -1;
  reference_frequency = 440Hz;
}
node sine { type = "core.sine/1"; config = { voices = 1; }; }
track tr { target = &sine:events; }
pattern pat {
  length = 1q;
  note n {
    at = 0q;
    dur = 1/4q;
    pitch = degree(-3, &t);
    velocity = 1;
  }
}
place pl { pattern = &pat; track = &tr; at = 0q; }
"#;

const CORPUS_SCHEMA: &str = "maac.l3.tuning-contract-corpus/1";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    schema: String,
    rate_hz: u32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    source: String,
    status: String,
    #[serde(default)]
    expected_pitch_hz: Vec<f64>,
    #[serde(default)]
    diagnostic: Option<String>,
    #[serde(default)]
    object_path: Vec<String>,
    #[serde(default)]
    field_path: Vec<String>,
}

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/l3/tuning")
}

fn load_corpus() -> Corpus {
    let bytes = fs::read(corpus_root().join("expected.json")).expect("read tuning corpus");
    serde_json::from_slice(&bytes).expect("parse tuning corpus")
}

fn artifact_json(artifact: &PlanArtifact) -> Value {
    serde_json::from_slice(&artifact.to_json().expect("serialize tuning artifact"))
        .expect("parse tuning artifact")
}

fn pitch_bits(wire: &Value) -> Vec<u64> {
    wire["events"]
        .as_array()
        .expect("artifact events array")
        .iter()
        .map(|event| {
            assert_eq!(event["kind"]["kind"], "note");
            event["kind"]["pitch_hz"]
                .as_f64()
                .expect("resolved note pitch_hz")
                .to_bits()
        })
        .collect()
}

fn rendered_bits(artifact: &PlanArtifact) -> Result<Vec<u64>, String> {
    let mut bits = Vec::new();
    maac::render_artifact(artifact, |frame| {
        bits.extend(frame.iter().map(|sample| sample.to_bits()));
        Ok(())
    })
    .map_err(|error| error.to_string())?;
    Ok(bits)
}

fn assert_expected_error(
    id: &str,
    diagnostics: &Diagnostics,
    expected_code: &str,
    expected_object_path: &[String],
    expected_field_path: &[String],
) -> Result<(), String> {
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code_str() == expected_code)
        .ok_or_else(|| format!("{id}: expected {expected_code}, found {diagnostics}"))?;
    if diagnostic.object_path.as_slice() != expected_object_path {
        return Err(format!(
            "{id}: expected object path {expected_object_path:?}, found {:?}",
            diagnostic.object_path
        ));
    }
    if diagnostic.field_path.as_slice() != expected_field_path {
        return Err(format!(
            "{id}: expected field path {expected_field_path:?}, found {:?}",
            diagnostic.field_path
        ));
    }
    Ok(())
}

fn assert_exact_source_inventory(
    expected: &BTreeSet<String>,
    actual: &BTreeSet<String>,
) -> Result<(), String> {
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "source inventory mismatch: expected {expected:?}, found {actual:?}"
        ))
    }
}

fn source_inventory(corpus: &Corpus) -> Result<(), String> {
    let expected = corpus
        .cases
        .iter()
        .map(|case| case.source.clone())
        .collect::<BTreeSet<_>>();
    if expected.len() != corpus.cases.len() {
        return Err("source paths must be unique".into());
    }
    if corpus
        .cases
        .iter()
        .any(|case| !case.source.starts_with("cases/") || !case.source.ends_with(".maac"))
    {
        return Err("source paths must be cases/*.maac".into());
    }

    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(corpus_root().join("cases")).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err(format!(
                "fixture inventory contains non-file {}",
                path.display()
            ));
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("maac") {
            return Err(format!(
                "fixture inventory contains non-MaaC file {}",
                path.display()
            ));
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("fixture path is not UTF-8: {}", path.display()))?;
        actual.insert(format!("cases/{name}"));
    }
    assert_exact_source_inventory(&expected, &actual)
}

fn assert_success(case: &Case, artifact: &PlanArtifact) -> Result<(), String> {
    let wire = artifact_json(artifact);
    if wire.get("tuning").is_some() || wire.get("tunings").is_some() {
        return Err(format!(
            "{}: tuning object leaked into retained plan",
            case.id
        ));
    }
    let actual = pitch_bits(&wire);
    let expected = case
        .expected_pitch_hz
        .iter()
        .map(|pitch| pitch.to_bits())
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(format!(
            "{}: expected pitch bits {expected:016x?}, found {actual:016x?}",
            case.id
        ));
    }

    let encoded = artifact.to_json().map_err(|error| error.to_string())?;
    let retained = PlanArtifact::from_json(&encoded).map_err(|error| error.to_string())?;
    let retained_wire = artifact_json(&retained);
    if pitch_bits(&retained_wire) != actual {
        return Err(format!("{}: retained artifact changed pitch bits", case.id));
    }
    let direct_audio = rendered_bits(artifact)?;
    let retained_audio = rendered_bits(&retained)?;
    if direct_audio != retained_audio {
        return Err(format!(
            "{}: retained artifact changed rendered samples",
            case.id
        ));
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
            assert_success(case, &artifact)
        }
        "error" => {
            let expected = case
                .diagnostic
                .as_deref()
                .ok_or_else(|| format!("{}: missing expected diagnostic", case.id))?;
            let diagnostics = compile_bundle_artifact(&bundle)
                .expect_err(&format!("{}: source should be rejected", case.id));
            assert_expected_error(
                &case.id,
                &diagnostics,
                expected,
                &case.object_path,
                &case.field_path,
            )
        }
        other => Err(format!("{}: unknown status {other:?}", case.id)),
    }
}

#[test]
fn negative_reference_index_is_accepted_by_public_source_compile() {
    let bundle = SourceBundle::new("negative-reference.maac", NEGATIVE_REFERENCE_SOURCE);
    let artifact =
        compile_bundle_artifact(&bundle).expect("negative reference index should compile");
    assert_eq!(
        pitch_bits(&artifact_json(&artifact)),
        vec![220.0_f64.to_bits()]
    );
}

#[test]
fn tuning_corpus_matches_literal_pitch_and_diagnostic_contract() {
    let corpus = load_corpus();
    assert_eq!(corpus.schema, CORPUS_SCHEMA);
    assert_eq!(corpus.rate_hz, 48_000);
    assert_eq!(corpus.cases.len(), 16);
    source_inventory(&corpus).expect("corpus source inventory");
    let ids = corpus
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), corpus.cases.len(), "case IDs must be unique");
    for case in &corpus.cases {
        validate_case(case).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn diagnostic_guard_rejects_wrong_literal_path() {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(
        Diagnostic::error(DiagnosticCode::Range, "fixture", None)
            .object_path(["wrong"])
            .field_path(["reference_index"]),
    );
    let expected_object_path = vec!["t".to_owned()];
    let expected_field_path = vec!["reference_index".to_owned()];
    assert!(assert_expected_error(
        "wrong-path",
        &diagnostics,
        "E_RANGE",
        &expected_object_path,
        &expected_field_path,
    )
    .is_err());
}

#[test]
fn source_inventory_guard_rejects_unlisted_or_missing_fixture() {
    let expected = ["cases/one.maac", "cases/two.maac"]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let mut extra = expected.clone();
    extra.insert("cases/three.maac".to_owned());
    assert!(assert_exact_source_inventory(&expected, &extra).is_err());
    let mut missing = expected.clone();
    missing.remove("cases/two.maac");
    assert!(assert_exact_source_inventory(&expected, &missing).is_err());
}
