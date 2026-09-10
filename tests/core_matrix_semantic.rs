use maac::diagnostic::DiagnosticCode;
use maac::semantic::{validate_source, PortDirection, PortKind, ProcessorKind, ReferenceTarget};
use maac::syntax::parse;

fn base(extra: &str) -> String {
    format!(
        r#"maac 1;
project p {{
  score = [0q, 4q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &master:out;
}}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node master {{ type = "core.sum/1"; config = {{ channels = 2; }}; }}
{extra}
"#
    )
}

fn codes(source: &str) -> Vec<DiagnosticCode> {
    let document = parse(source).expect("matrix semantic source must parse");
    validate_source(&document)
        .expect_err("source should fail matrix semantic validation")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn accepts_matrix_and_exposes_distinct_audio_port_dimensions() {
    let source = base(
        r#"node matrix {
  type = "core.matrix/1";
  config = { inputs = 2; outputs = 1; coefficients = [[1, -1]]; };
}
"#,
    );
    let graph = validate_source(&parse(&source).unwrap()).expect("matrix should validate");
    assert_eq!(
        graph.node("matrix").unwrap().processor,
        ProcessorKind::Matrix
    );
    assert!(matches!(
        graph.resolve_text("&matrix:in"),
        Some(ReferenceTarget::Port {
            direction: PortDirection::Input,
            kind: PortKind::Audio,
            channels: 2,
            ..
        })
    ));
    assert!(matches!(
        graph.resolve_text("&matrix:out"),
        Some(ReferenceTarget::Port {
            direction: PortDirection::Output,
            kind: PortKind::Audio,
            channels: 1,
            ..
        })
    ));
}

#[test]
fn rejects_matrix_shape_units_dimensions_and_parameters() {
    let missing_config = base(
        r#"node matrix { type = "core.matrix/1"; }
"#,
    );
    assert!(codes(&missing_config).contains(&DiagnosticCode::Range));

    let bad_shape = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 2; outputs = 2; coefficients = [[1, 0]]; }; }
"#,
    );
    assert!(codes(&bad_shape).contains(&DiagnosticCode::Range));

    let bad_row = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 2; outputs = 2; coefficients = [[1, 0], [1]]; }; }
"#,
    );
    assert!(codes(&bad_row).contains(&DiagnosticCode::Range));

    let dimension_unit = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 1Hz; outputs = 1; coefficients = [[1]]; }; }
"#,
    );
    assert!(codes(&dimension_unit).contains(&DiagnosticCode::Unit));

    let coefficient_unit = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 1; outputs = 1; coefficients = [[1dB]]; }; }
"#,
    );
    assert!(codes(&coefficient_unit).contains(&DiagnosticCode::Unit));

    let wrong_type = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 1; outputs = 1; coefficients = [1]; }; }
"#,
    );
    assert!(codes(&wrong_type).contains(&DiagnosticCode::Unit));

    let invalid_dimension = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 0; outputs = 3/2; coefficients = []; }; }
"#,
    );
    assert!(codes(&invalid_dimension).contains(&DiagnosticCode::Range));

    let unsupported_dimension = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 3; outputs = 1; coefficients = [[1, 0, 0]]; }; }
"#,
    );
    assert!(codes(&unsupported_dimension).contains(&DiagnosticCode::Capability));

    let params = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 1; outputs = 1; coefficients = [[1]]; }; params = { level = 0; }; }
"#,
    );
    assert!(codes(&params).contains(&DiagnosticCode::UnknownField));
}

#[test]
fn accepts_signed_and_zero_coefficients_but_rejects_nonfinite_values() {
    let source = base(
        r#"node matrix { type = "core.matrix/1"; config = { inputs = 1; outputs = 2; coefficients = [[0], [-1/2]]; }; }
"#,
    );
    validate_source(&parse(&source).unwrap()).expect("signed zero matrix should validate");

    let huge = format!(
        r#"node matrix {{ type = "core.matrix/1"; config = {{ inputs = 1; outputs = 1; coefficients = [[1{}]]; }}; }}
"#,
        "0".repeat(400)
    );
    assert!(codes(&base(&huge)).contains(&DiagnosticCode::Nonfinite));
}
