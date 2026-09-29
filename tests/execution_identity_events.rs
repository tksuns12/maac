use maac::bundle::SourceBundle;
use maac::compiler::{compile, compile_bundle_artifact};
use maac::plan::EventKind;
use maac::production_data::SCHEMA_BYTES;
use maac::production_identity::{execution_identity, ExecutionIdentity};
use maac::{parse, parse_rational, PlanArtifact};
use serde_json::Value;

fn identity_for_source(source: &str) -> ExecutionIdentity {
    let document = parse(source).unwrap();
    let plan = compile(&document).unwrap();
    execution_identity(&document, &plan).unwrap()
}

fn production_artifact(source: &str) -> PlanArtifact {
    let mut bundle = SourceBundle::new("examples/production.maac", source);
    bundle
        .assets
        .insert("production.schema.json".into(), SCHEMA_BYTES.to_vec());
    compile_bundle_artifact(&bundle).unwrap()
}

fn production_identity(source: &str) -> ExecutionIdentity {
    let artifact = production_artifact(source);
    let json: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    serde_json::from_value(json["production"]["execution_identity"].clone()).unwrap()
}

fn production_with_message(message: &str) -> String {
    include_str!("../examples/production.maac").replace(
        "pattern bass_phrase {",
        &format!("pattern bass_phrase {{\n  {message}"),
    )
}

#[test]
fn pitch_expression_example_has_a_valid_execution_identity() {
    let source = include_str!("../examples/pitch-expression.maac");
    let document = parse(source).unwrap();
    let plan = compile(&document).unwrap();
    assert_eq!(plan.events.len(), 2);

    let identity = execution_identity(&document, &plan).unwrap();
    identity.validate().unwrap();
    let normalized: Value = serde_json::from_str(&identity.normalized_source_json).unwrap();
    assert_eq!(
        normalized["objects"]["duet"]["children"]["rising"]["children"]["bend"]["kind"],
        "expression"
    );
    assert_eq!(
        normalized["objects"]["duet"]["children"]["rising"]["children"]["bend"]["fields"]["kind"]
            ["v"],
        "pitch"
    );

    let labeled = source.replace(
        "expression bend { kind = pitch; curve = &rise; }",
        "expression bend { kind = pitch; curve = &rise; label = \"bend label\"; }",
    );
    assert_ne!(labeled, source);
    let labeled_identity = identity_for_source(&labeled);
    assert_eq!(identity.execution_hash, labeled_identity.execution_hash);
    assert_ne!(
        identity.source_input_hash,
        labeled_identity.source_input_hash
    );
}

#[test]
fn other_supported_expression_examples_have_execution_identities() {
    for source in [
        include_str!("../examples/gain-expression.maac"),
        include_str!("../examples/timbre-expression.maac"),
        include_str!("../examples/pressure-expression.maac"),
    ] {
        let document = parse(source).unwrap();
        let plan = compile(&document).unwrap();
        assert!(!plan.events.is_empty());
        execution_identity(&document, &plan)
            .unwrap()
            .validate()
            .unwrap();
    }
}

#[test]
fn expression_curve_values_and_attachment_change_execution_identity() {
    let source = include_str!("../examples/pitch-expression.maac");
    let original = identity_for_source(source);

    let changed_curve = source.replace("1200ct", "1100ct");
    assert_ne!(changed_curve, source);
    assert_ne!(
        original.execution_hash,
        identity_for_source(&changed_curve).execution_hash
    );

    let changed_attachment = source.replace("curve = &rise;", "curve = &fall;");
    assert_ne!(changed_attachment, source);
    assert_ne!(
        original.execution_hash,
        identity_for_source(&changed_attachment).execution_hash
    );
}

#[test]
fn production_message_defaults_and_payload_are_reflected_in_identity() {
    let omitted = production_with_message(
        "message cue { at = 1q; protocol = \"midi1\"; bytes = [144, 60, 127]; }",
    );
    let explicit = production_with_message(
        "message cue { at = 1q; protocol = \"midi1\"; bytes = [144, 60, 127]; onset_offset = 0ms; order = 0; }",
    );
    let first = production_identity(&omitted);
    let second = production_identity(&explicit);
    assert_eq!(first.execution_hash, second.execution_hash);
    assert_ne!(first.source_input_hash, second.source_input_hash);
    first.validate().unwrap();

    let normalized: Value = serde_json::from_str(&first.normalized_source_json).unwrap();
    let fields = &normalized["objects"]["bass_phrase"]["children"]["cue"]["fields"];
    assert_eq!(fields["onset_offset"]["u"], "s");
    assert_eq!(fields["onset_offset"]["n"], "0");
    assert_eq!(fields["order"]["n"], "0");

    let changed_bytes = omitted.replace("[144, 60, 127]", "[144, 60, 126]");
    let changed_protocol = omitted.replace("\"midi1\"", "\"raw/test\"");
    assert_ne!(
        first.execution_hash,
        production_identity(&changed_bytes).execution_hash
    );
    assert_ne!(
        first.execution_hash,
        production_identity(&changed_protocol).execution_hash
    );
}

#[test]
fn production_with_note_expression_and_message_compiles() {
    let source = production_with_message(
        "message cue { at = 1q; protocol = \"midi1\"; bytes = [1]; }",
    )
    .replace(
        "pattern bass_phrase {",
        "curve bend { clock = normalized; points = [(0, 0ct, linear), (1, 100ct, step)]; }\ncurve swell { clock = normalized; points = [(0, 1/2, linear), (1, 1, step)]; }\npattern bass_phrase {",
    )
    .replace(
        "note root { at = 0q; dur = 3/2q; pitch = C3; velocity = 0.7; }",
        "note root { at = 0q; dur = 3/2q; pitch = C3; velocity = 0.7; expression bend { kind = pitch; curve = &bend; } expression swell { kind = gain; curve = &swell; } }",
    );
    let artifact = production_artifact(&source);
    let events = artifact
        .query_events_score_window(&parse_rational("0").unwrap(), &parse_rational("8").unwrap())
        .unwrap();
    assert!(events.iter().any(|event| {
        matches!(&event.kind, EventKind::Message { protocol, bytes } if protocol == "midi1" && bytes == &[1])
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.kind,
            EventKind::Note {
                pitch_expression: Some(_),
                gain_expression: Some(_),
                ..
            }
        )
    }));
    production_identity(&source).validate().unwrap();
}
