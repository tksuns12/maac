//! Grammar-driven surface-syntax corpus (evidence row D01, spec §2).
//!
//! `scripts/lexical_corpus.py` generates every case from the token classes of
//! `grammar.lark` and records the independent oracle's verdict: Lark's parse
//! plus the `check_spec.py` syntax transformer. `maac::parse` must agree with
//! each one, either on the exact typed syntax tree or on the error code.

use serde_json::Value;

#[test]
fn rust_parser_agrees_with_the_grammar_oracle_on_every_case() {
    let corpus: Value = serde_json::from_str(include_str!("fixtures/lexical/corpus.json")).unwrap();
    assert_eq!(corpus["schema"], "maac.lexical-corpus/1");
    let cases = corpus["cases"].as_array().unwrap();
    assert!(cases.len() >= 300, "{} cases", cases.len());
    let mut disagreements = Vec::new();
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let source = case["source"].as_str().unwrap();
        let expected = &case["expected"];
        match (maac::parse(source), expected.get("syntax")) {
            (Ok(document), Some(syntax)) => {
                let actual = document.to_syntax_json_value();
                if &actual != syntax {
                    disagreements.push(format!("{id}: tree {actual} != {syntax}"));
                }
            }
            (Ok(_), None) => {
                disagreements.push(format!("{id}: accepted, expected {}", expected["error"]));
            }
            (Err(diagnostics), Some(_)) => disagreements.push(format!(
                "{id}: refused with {:?}, expected acceptance",
                diagnostics
                    .iter()
                    .map(|d| d.code.as_str())
                    .collect::<Vec<_>>()
            )),
            (Err(diagnostics), None) => {
                let code = expected["error"].as_str().unwrap();
                if !diagnostics.iter().any(|d| d.code.as_str() == code) {
                    disagreements.push(format!(
                        "{id}: refused with {:?}, expected {code}",
                        diagnostics
                            .iter()
                            .map(|d| d.code.as_str())
                            .collect::<Vec<_>>()
                    ));
                }
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "{} of {} cases disagree:\n{}",
        disagreements.len(),
        cases.len(),
        disagreements.join("\n")
    );
}
