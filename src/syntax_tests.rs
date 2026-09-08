use std::fs;

use crate::{parse, DiagnosticCode, ParseOptions, ValueKind};

#[test]
fn accepts_canonical_maac_header() {
    let document = parse("maac 1; project p {}").expect("maac 1 should be the canonical header");
    assert_eq!(document.version, 1);
}

#[test]
fn rejects_legacy_scoreir_header() {
    let diagnostics = parse("scoreir 1; project p {}").expect_err("scoreir 1 is not an alias");
    assert!(diagnostics.iter().any(|d| d.code == DiagnosticCode::Syntax));
}

#[test]
fn parses_example_and_preserves_source_spans() {
    let source = fs::read_to_string("example.maac").unwrap();
    let document = parse(&source).expect("example should be valid surface syntax");
    assert_eq!(document.version, 1);
    assert_eq!(document.source(), source);
    let project = document.object("demo").unwrap();
    assert_eq!(project.kind, "project");
    assert_eq!(
        document.slice(project.span),
        &source[project.span.start..project.span.end]
    );
    let score = project.field("score").unwrap();
    assert_eq!(document.slice(score.span), "score = [0q, 32q];");
    assert!(matches!(score.value.kind, ValueKind::List(_)));
}

#[test]
fn emits_the_typed_syntax_tree_shape() {
    let source = fs::read_to_string("example.maac").unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(&fs::read_to_string("example.syntax.json").unwrap()).unwrap();
    let actual = parse(&source).unwrap().to_syntax_json_value();
    assert_eq!(actual, expected);
}

#[test]
fn parses_evening_window() {
    let source = fs::read_to_string("evening-window.maac").unwrap();
    let document = parse(&source).expect("evening-window should be valid surface syntax");
    assert_eq!(document.version, 1);
    assert_eq!(document.object("evening_window").unwrap().kind, "project");
    assert_eq!(document.objects.len(), 45);
}

#[test]
fn reports_duplicate_fields_and_ids_with_stable_codes() {
    let source = "maac 1; project p { x = 1; x = 2; note x { at = 0q; } }";
    let diagnostics = parse(source).expect_err("duplicates must fail");
    assert!(diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::DuplicateField));
    assert!(diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::DuplicateId));
    assert!(diagnostics.iter().all(|d| d.span.is_some()));
}

#[test]
fn enforces_source_and_nesting_limits() {
    let oversized = format!("maac 1; // {}", "x".repeat(4 * 1024 * 1024));
    let diagnostics = parse(&oversized).expect_err("source limit");
    assert!(diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::ResourceLimit));

    let mut nested = String::from("maac 1; project p { value = ");
    for _ in 0..65 {
        nested.push('[');
    }
    nested.push('0');
    for _ in 0..65 {
        nested.push(']');
    }
    nested.push_str("; }");
    let diagnostics = parse(&nested).expect_err("nesting limit");
    assert!(diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::ResourceLimit));
}

#[test]
fn accepts_exact_values_and_rejects_bad_grammar() {
    let source = r#"maac 1;
        project p { a = 0.10; b = 1/3q; c = &node:out; d = key(60); e = { nested = [true, "hi\n"]; }; }
        node node { }"#;
    let document = parse(source).unwrap();
    let fields = &document.object("p").unwrap().fields;
    assert_eq!(fields["a"].value.as_rational().unwrap().to_string(), "1/10");
    assert_eq!(fields["b"].value.unit().unwrap().as_str(), "q");
    assert_eq!(
        fields["c"].value.reference().unwrap().port.as_deref(),
        Some("out")
    );
    assert!(matches!(fields["d"].value.kind, ValueKind::Call { .. }));

    let diagnostics = parse("maac 1; project p { x = [; }").unwrap_err();
    assert!(diagnostics.iter().any(|d| d.code == DiagnosticCode::Syntax));
}

#[test]
fn recognizes_signed_pitch_tokens_only_in_value_positions() {
    let source = "maac 1; pattern p { note n { at = 0q; dur = 1q; pitch = C-1; } note m { at = 1q; dur = 1q; pitch = F##-1; } }";
    let document = parse(source).unwrap();
    let pattern = document.object("p").unwrap();
    assert_eq!(
        pattern
            .child("n")
            .unwrap()
            .field("pitch")
            .unwrap()
            .value
            .as_symbol(),
        Some("C-1")
    );
    assert_eq!(
        pattern
            .child("m")
            .unwrap()
            .field("pitch")
            .unwrap()
            .value
            .as_symbol(),
        Some("F##-1")
    );

    let invalid_id = "maac 1; pattern p { note C#4 { at = 0q; dur = 1q; pitch = C4; } }";
    let diagnostics = parse(invalid_id).unwrap_err();
    assert!(diagnostics.iter().any(|d| d.code == DiagnosticCode::Syntax));
}

#[test]
fn rejects_invalid_json_strings_and_unpaired_surrogates() {
    for source in [
        r#"maac 1; project p { label = "bad\q"; }"#,
        r#"maac 1; project p { label = "\uD800"; }"#,
    ] {
        let diagnostics = parse(source).unwrap_err();
        assert!(diagnostics.iter().any(|d| d.code == DiagnosticCode::Syntax));
    }
}

#[test]
fn deeply_nested_objects_fail_with_a_bounded_diagnostic() {
    let mut source = String::from("maac 1; ");
    for index in 0..=64 {
        source.push_str(&format!("kind k{index} {{ "));
    }
    source.push_str("value = 1; ");
    for _ in 0..=64 {
        source.push_str("} ");
    }
    let diagnostics = parse(&source).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::ResourceLimit));
}

#[test]
fn limits_rational_component_bits() {
    let too_large = format!(
        "maac 1; project p {{ x = {big}; }}",
        big = "1".to_owned() + &"0".repeat(1300)
    );
    let diagnostics = parse(&too_large).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::ResourceLimit));

    let opts = ParseOptions {
        max_rational_bits: 8,
        ..ParseOptions::default()
    };
    let diagnostics =
        crate::syntax::parse_with_options("maac 1; project p { x = 256; }", opts).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::ResourceLimit));
}
