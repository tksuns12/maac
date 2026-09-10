use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::diagnostic::{Diagnostic, DiagnosticCode};
use maac::semantic::validate_source;
use maac::syntax::parse;

fn source(extra: &str) -> String {
    format!(
        r#"maac 1;
project p {{
  score = [0q, 1q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &master:out;
}}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node master {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
{extra}
"#
    )
}

fn diagnostics(text: &str) -> Vec<Diagnostic> {
    let document = parse(text).expect("processor config source should parse");
    validate_source(&document)
        .expect_err("processor config source should fail validation")
        .into_iter()
        .collect()
}

fn compile_diagnostics(text: String) -> Vec<Diagnostic> {
    compile_bundle_artifact(&SourceBundle::new("required-config.maac", text))
        .expect_err("invalid processor config should not compile")
        .into_iter()
        .collect()
}

fn has_diagnostic(
    diagnostics: &[Diagnostic],
    code: DiagnosticCode,
    object: &str,
    fields: &[&str],
) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.code == code
            && diagnostic.object_path == [object.to_owned()]
            && diagnostic
                .field_path
                .iter()
                .map(String::as_str)
                .eq(fields.iter().copied())
    })
}

fn compile_valid(extra: &str) {
    let text = source(extra);
    let document = parse(&text).unwrap();
    validate_source(&document).expect("valid processor config should validate");
    compile_bundle_artifact(&SourceBundle::new("required-config.maac", text))
        .expect("valid processor config should compile");
}

#[test]
fn omitted_required_config_reports_range_at_source_and_compile_boundaries() {
    let cases = [
        ("core.sum/1", Some(&["config", "channels"][..])),
        ("core.onepole/1", Some(&["config", "channels"][..])),
        ("core.gain/1", Some(&["config", "channels"][..])),
        ("core.fader/1", Some(&["config", "channels"][..])),
        ("core.matrix/1", None),
        ("core.delay/1", None),
        ("core.noise/1", None),
    ];
    for (processor, expected_fields) in cases {
        let text = source(&format!("node target {{ type = \"{processor}\"; }}\n"));
        let diagnostics = diagnostics(&text);
        assert!(
            expected_fields.is_none_or(|fields| {
                has_diagnostic(&diagnostics, DiagnosticCode::Range, "target", fields)
            }) && diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::Range),
            "{processor} should require config.channels: {diagnostics:?}"
        );
        let compile_errors = compile_diagnostics(text);
        assert!(
            compile_errors
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::Range),
            "{processor} without config should fail with E_RANGE: {compile_errors:?}"
        );
    }
}

#[test]
fn empty_required_configs_keep_their_field_specific_diagnostics() {
    let cases = [
        ("core.sum/1", "config.channels", &["config", "channels"][..]),
        (
            "core.onepole/1",
            "config.channels",
            &["config", "channels"][..],
        ),
        (
            "core.gain/1",
            "config.channels",
            &["config", "channels"][..],
        ),
        (
            "core.fader/1",
            "config.channels",
            &["config", "channels"][..],
        ),
        ("core.matrix/1", "config.inputs", &["config", "inputs"][..]),
        (
            "core.delay/1",
            "config.channels",
            &["config", "channels"][..],
        ),
        (
            "core.noise/1",
            "config.channels",
            &["config", "channels"][..],
        ),
    ];
    for (processor, expected, fields) in cases {
        let text = source(&format!(
            "node target {{ type = \"{processor}\"; config = {{}}; }}\n"
        ));
        let diagnostics = diagnostics(&text);
        assert!(
            has_diagnostic(&diagnostics, DiagnosticCode::Range, "target", fields),
            "{processor} should report {expected}: {diagnostics:?}"
        );
        let compile_errors = compile_diagnostics(text);
        assert!(
            compile_errors
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::Range),
            "{processor} empty config should fail with E_RANGE: {compile_errors:?}"
        );
    }
}

#[test]
fn required_processors_accept_valid_mono_and_stereo_configs_and_compile() {
    for (processor, config, input_channels) in [
        ("core.sum/1", "channels = 1;", Some(1u8)),
        ("core.onepole/1", "channels = 1;", Some(1u8)),
        ("core.gain/1", "channels = 1;", Some(1u8)),
        ("core.fader/1", "channels = 1;", Some(1u8)),
        ("core.delay/1", "channels = 1; frames = 1;", Some(1u8)),
        ("core.noise/1", "channels = 1;", None),
        (
            "core.matrix/1",
            "inputs = 1; outputs = 1; coefficients = [[1]];",
            Some(1u8),
        ),
        ("core.sum/1", "channels = 2;", Some(2u8)),
        ("core.onepole/1", "channels = 2;", Some(2u8)),
        ("core.gain/1", "channels = 2;", Some(2u8)),
        ("core.fader/1", "channels = 2;", Some(2u8)),
        ("core.delay/1", "channels = 2; frames = 1;", Some(2u8)),
        ("core.noise/1", "channels = 2;", None),
        (
            "core.matrix/1",
            "inputs = 2; outputs = 2; coefficients = [[1, 0], [0, 1]];",
            Some(2u8),
        ),
    ] {
        let input = match input_channels {
            Some(1) => {
                "node input { type = \"core.sine/1\"; }\nconnect input_target { from = &input:out; to = &target:in; }\n"
            }
            Some(2) => {
                "node input { type = \"core.sine/1\"; }\nnode stereo_input { type = \"core.pan/1\"; }\nconnect input_pan { from = &input:out; to = &stereo_input:in; }\nconnect input_target { from = &stereo_input:out; to = &target:in; }\n"
            }
            None => "",
            Some(_) => unreachable!("test fixture only uses mono or stereo inputs"),
        };
        compile_valid(&format!(
            "{input}node target {{ type = \"{processor}\"; config = {{ {config} }}; }}\n"
        ));
    }
}

#[test]
fn optional_sine_and_pan_configs_remain_accepted() {
    compile_valid(
        r#"node sine { type = "core.sine/1"; }
node pan { type = "core.pan/1"; }
connect sine_pan { from = &sine:out; to = &pan:in; }
"#,
    );
}
