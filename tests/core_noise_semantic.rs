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
    let document = parse(source).expect("noise semantic source must parse");
    validate_source(&document)
        .expect_err("source should fail noise semantic validation")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn accepts_mono_stereo_noise_and_optional_unsigned_seed() {
    let max_seed = u64::MAX.to_string();
    for (channels, seed) in [(1, "0".to_owned()), (2, max_seed)] {
        let source = base(&format!(
            "node noise {{ type = \"core.noise/1\"; config = {{ channels = {channels}; seed = {seed}; }}; }}\n"
        ));
        let graph = validate_source(&parse(&source).unwrap()).expect("noise should validate");
        let node = graph.node("noise").expect("noise node retained");
        assert_eq!(node.processor, ProcessorKind::Noise);
        assert_eq!(
            node.config["channels"].to_integer().to_string(),
            channels.to_string()
        );
        assert_eq!(node.config["seed"].to_integer().to_string(), seed);
        assert!(matches!(
            graph.resolve_text("&noise:out"),
            Some(ReferenceTarget::Port {
                direction: PortDirection::Output,
                kind: PortKind::Audio,
                channels: actual,
                ..
            }) if *actual == channels
        ));
        assert!(graph.resolve_text("&noise:in").is_none());
    }

    let default_seed = base(
        r#"node noise { type = "core.noise/1"; config = { channels = 1; }; }
"#,
    );
    let graph =
        validate_source(&parse(&default_seed).unwrap()).expect("default seed should validate");
    assert!(!graph.node("noise").unwrap().config.contains_key("seed"));
}

#[test]
fn requires_channels_and_rejects_invalid_noise_config() {
    let missing_config = base(
        r#"node noise { type = "core.noise/1"; }
"#,
    );
    assert!(codes(&missing_config).contains(&DiagnosticCode::Range));

    let missing_channels = base(
        r#"node noise { type = "core.noise/1"; config = { seed = 0; }; }
"#,
    );
    assert!(codes(&missing_channels).contains(&DiagnosticCode::Range));

    let invalid_channels = base(
        r#"node noise { type = "core.noise/1"; config = { channels = 0; }; }
"#,
    );
    assert!(codes(&invalid_channels).contains(&DiagnosticCode::Range));

    let fractional_channels = base(
        r#"node noise { type = "core.noise/1"; config = { channels = 3/2; }; }
"#,
    );
    assert!(codes(&fractional_channels).contains(&DiagnosticCode::Range));

    let unsupported_channels = base(
        r#"node noise { type = "core.noise/1"; config = { channels = 3; }; }
"#,
    );
    assert!(codes(&unsupported_channels).contains(&DiagnosticCode::Capability));
}

#[test]
fn validates_unsigned_seed_and_rejects_noise_inputs_and_parameters() {
    for seed in ["-1", "3/2", "18446744073709551616"] {
        let source = base(&format!(
            "node noise {{ type = \"core.noise/1\"; config = {{ channels = 1; seed = {seed}; }}; }}\n"
        ));
        assert!(codes(&source).contains(&DiagnosticCode::Range));
    }

    let wrong_unit = base(
        r#"node noise { type = "core.noise/1"; config = { channels = 1; seed = 1Hz; }; }
"#,
    );
    assert!(codes(&wrong_unit).contains(&DiagnosticCode::Unit));

    let params = base(
        r#"node noise { type = "core.noise/1"; config = { channels = 1; }; params = { level = 0; }; }
"#,
    );
    assert!(codes(&params).contains(&DiagnosticCode::UnknownField));
}
