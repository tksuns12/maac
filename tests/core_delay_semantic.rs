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
    let document = parse(source).expect("delay semantic source must parse");
    validate_source(&document)
        .expect_err("source should fail delay semantic validation")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn accepts_delay_and_exposes_mono_stereo_audio_ports() {
    for (channels, frames) in [(1, 1), (2, 3)] {
        let source = base(&format!(
            "node delay {{ type = \"core.delay/1\"; config = {{ channels = {channels}; frames = {frames}; }}; }}\n"
        ));
        let graph = validate_source(&parse(&source).unwrap()).expect("delay should validate");
        let node = graph.node("delay").expect("delay node retained");
        assert_eq!(node.processor, ProcessorKind::Delay);
        assert_eq!(
            node.config["channels"].to_integer().to_string(),
            channels.to_string()
        );
        assert_eq!(
            node.config["frames"].to_integer().to_string(),
            frames.to_string()
        );
        assert!(matches!(
            graph.resolve_text("&delay:in"),
            Some(ReferenceTarget::Port {
                direction: PortDirection::Input,
                kind: PortKind::Audio,
                channels: actual,
                ..
            }) if *actual == channels
        ));
        assert!(matches!(
            graph.resolve_text("&delay:out"),
            Some(ReferenceTarget::Port {
                direction: PortDirection::Output,
                kind: PortKind::Audio,
                channels: actual,
                ..
            }) if *actual == channels
        ));
    }
}

#[test]
fn requires_positive_integer_delay_config_and_rejects_bad_types() {
    let missing_config = base(
        r#"node delay { type = "core.delay/1"; }
"#,
    );
    assert!(codes(&missing_config).contains(&DiagnosticCode::Range));

    let missing_frames = base(
        r#"node delay { type = "core.delay/1"; config = { channels = 1; }; }
"#,
    );
    assert!(codes(&missing_frames).contains(&DiagnosticCode::Range));

    let invalid = base(
        r#"node delay { type = "core.delay/1"; config = { channels = 0; frames = 3/2; }; }
"#,
    );
    assert!(codes(&invalid).contains(&DiagnosticCode::Range));

    let wrong_unit = base(
        r#"node delay { type = "core.delay/1"; config = { channels = 1Hz; frames = 1s; }; }
"#,
    );
    assert!(codes(&wrong_unit).contains(&DiagnosticCode::Unit));

    let unsupported_channels = base(
        r#"node delay { type = "core.delay/1"; config = { channels = 3; frames = 1; }; }
"#,
    );
    assert!(codes(&unsupported_channels).contains(&DiagnosticCode::Capability));
}

#[test]
fn delay_has_no_parameters() {
    let params = base(
        r#"node delay { type = "core.delay/1"; config = { channels = 1; frames = 1; }; params = { level = 0; }; }
"#,
    );
    assert!(codes(&params).contains(&DiagnosticCode::UnknownField));

    let empty_params = base(
        r#"node delay { type = "core.delay/1"; config = { channels = 1; frames = 1; }; params = {}; }
"#,
    );
    validate_source(&parse(&empty_params).unwrap()).expect("empty delay params should validate");
}
