use maac::diagnostic::DiagnosticCode;
use maac::graph::ParameterRate;
use maac::semantic::{
    validate_source, ParameterUnit, PortDirection, PortKind, ProcessorKind, RangePolicy,
    ReferenceTarget, SourceGraph, ValueType,
};
use maac::syntax::{parse, Unit};

fn codes(source: &str) -> Vec<DiagnosticCode> {
    let document = parse(source).expect("semantic test source must parse");
    validate_source(&document)
        .expect_err("source should fail semantic validation")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

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

#[test]
fn validates_core_source_and_exposes_object_accessors() {
    let source = base(
        r#"node synth {
  type = "core.sine/1";
  config = { voices = 8; };
  params = { attack = 5ms; release = 80ms; level = 0.2; };
}
node pan { type = "core.pan/1"; params = { pan = 0; }; }
connect synth_to_pan { from = &synth:out; to = &pan:in; }
connect pan_to_master { from = &pan:out; to = &master:in; }
"#,
    );
    let document = parse(&source).unwrap();
    let graph = validate_source(&document).expect("core source should validate");
    assert_eq!(graph.project_id(), "p");
    assert_eq!(graph.project().kind, "project");
    assert_eq!(graph.objects_of_kind("node").len(), 3);
    assert_eq!(graph.object("synth").unwrap().kind, "node");
    assert!(graph.field("synth", "params").is_some());
}

#[test]
fn validates_unused_declarations_and_does_not_discard_bad_objects() {
    let source = base(
        r#"node unused {
  type = "core.sine/1";
  config = { voices = 0; };
}
"#,
    );
    let diagnostics = codes(&source);
    assert!(diagnostics.contains(&DiagnosticCode::Range));
}

#[test]
fn reports_recognized_deferred_features_as_capability_errors() {
    let source = base(
        r#"node unsupported { type = "core.delay/1"; config = { channels = 1; }; }
pattern ptn { length = 1q; hit kick { at = 0q; key = "kick"; } }
"#,
    );
    let diagnostics = codes(&source);
    assert!(
        diagnostics
            .iter()
            .filter(|code| **code == DiagnosticCode::Capability)
            .count()
            >= 2
    );
}

#[test]
fn accepts_core_fader_channels_ports_and_signed_db_level() {
    let mono = base(
        r#"node fader {
  type = "core.fader/1";
  config = { channels = 1; };
  params = { level = -12dB; };
}
"#,
    );
    let mono_graph = validate_source(&parse(&mono).unwrap()).expect("mono fader should validate");
    let mono_node = mono_graph.node("fader").expect("fader node retained");
    assert_eq!(mono_node.processor, ProcessorKind::Fader);
    assert_eq!(
        mono_node.params.get("level"),
        Some(&ValueType::Quantity(Unit::Db))
    );
    assert!(matches!(
        mono_graph.resolve_text("&fader.params.level"),
        Some(ReferenceTarget::Parameter {
            unit: ParameterUnit::Decibels,
            range: RangePolicy::Error,
            rate: ParameterRate::Sample,
            ..
        })
    ));
    assert!(matches!(
        mono_graph.resolve_text("&fader:in"),
        Some(ReferenceTarget::Port {
            direction: PortDirection::Input,
            kind: PortKind::Audio,
            channels: 1,
            ..
        })
    ));
    assert!(matches!(
        mono_graph.resolve_text("&fader:out"),
        Some(ReferenceTarget::Port {
            direction: PortDirection::Output,
            kind: PortKind::Audio,
            channels: 1,
            ..
        })
    ));

    let stereo = base(
        r#"node fader {
  type = "core.fader/1";
  config = { channels = 2; };
}
connect fader_to_master { from = &fader:out; to = &master:in; }
"#,
    );
    let stereo_graph =
        validate_source(&parse(&stereo).unwrap()).expect("stereo fader should validate");
    assert!(matches!(
        stereo_graph.resolve_text("&fader:in"),
        Some(ReferenceTarget::Port { channels: 2, .. })
    ));
    assert!(matches!(
        stereo_graph.resolve_text("&fader:out"),
        Some(ReferenceTarget::Port { channels: 2, .. })
    ));
}

#[test]
fn enforces_core_fader_channel_and_db_contract() {
    let invalid_channels = base(
        r#"node fader { type = "core.fader/1"; config = { channels = 0; }; }
"#,
    );
    assert!(codes(&invalid_channels).contains(&DiagnosticCode::Range));

    let too_many_channels = base(
        r#"node fader { type = "core.fader/1"; config = { channels = 3; }; }
"#,
    );
    let diagnostics = codes(&too_many_channels);
    assert!(diagnostics.contains(&DiagnosticCode::Capability));
    assert!(!diagnostics.contains(&DiagnosticCode::Range));

    let fractional_channels = base(
        r#"node fader { type = "core.fader/1"; config = { channels = 3/2; }; }
"#,
    );
    assert!(codes(&fractional_channels).contains(&DiagnosticCode::Range));

    let missing_db = base(
        r#"node fader { type = "core.fader/1"; config = { channels = 1; }; params = { level = -12; }; }
"#,
    );
    assert!(codes(&missing_db).contains(&DiagnosticCode::Unit));

    let wrong_db_unit = base(
        r#"node fader { type = "core.fader/1"; config = { channels = 1; }; params = { level = 1Hz; }; }
"#,
    );
    assert!(codes(&wrong_db_unit).contains(&DiagnosticCode::Unit));
}

#[test]
fn fader_automation_requires_db_and_rejects_exponential_interpolation() {
    let valid = base(
        r#"node fader { type = "core.fader/1"; config = { channels = 1; }; }
curve level { clock = score; points = [(0q, -12dB, linear), (1q, 0dB, step)]; }
automation move_level { target = &fader.params.level; curve = &level; at = 0q; }
"#,
    );
    validate_source(&parse(&valid).unwrap()).expect("dB fader automation should validate");

    let wrong_unit = base(
        r#"node fader { type = "core.fader/1"; config = { channels = 1; }; }
curve level { clock = score; points = [(0q, -12, linear), (1q, 0, step)]; }
automation move_level { target = &fader.params.level; curve = &level; at = 0q; }
"#,
    );
    assert!(codes(&wrong_unit).contains(&DiagnosticCode::Unit));

    let exponential = base(
        r#"node fader { type = "core.fader/1"; config = { channels = 1; }; }
curve level { clock = score; points = [(0q, 1dB, exponential), (1q, 2dB, step)]; }
automation move_level { target = &fader.params.level; curve = &level; at = 0q; }
"#,
    );
    assert!(codes(&exponential).contains(&DiagnosticCode::Range));
}

#[test]
fn rejects_unknown_kinds_and_fields_with_stable_codes() {
    let unknown_kind = base("mystery thing { value = 1; }");
    let diagnostics = codes(&unknown_kind);
    assert!(diagnostics.contains(&DiagnosticCode::UnknownKind));

    let unknown_field = base("region r { span = [0q, 1q]; mood = \"bright\"; }");
    let diagnostics = codes(&unknown_field);
    assert!(diagnostics.contains(&DiagnosticCode::UnknownField));
}

#[test]
fn validates_units_references_curves_and_single_automation_writer() {
    let wrong_unit = base(
        r#"node filter { type = "core.onepole/1"; config = { channels = 1; }; params = { cutoff = 500ms; }; }
connect filter_to_master { from = &filter:out; to = &master:in; }
"#,
    );
    assert!(codes(&wrong_unit).contains(&DiagnosticCode::Unit));

    let bad_curve = base(
        r#"node filter { type = "core.onepole/1"; config = { channels = 1; }; params = { cutoff = 500Hz; }; }
curve c { clock = score; points = [(0q, 0Hz, exponential), (1q, 500Hz, step)]; }
automation a { target = &filter.params.cutoff; curve = &c; at = 0q; }
automation b { target = &filter.params.cutoff; curve = &c; at = 2q; }
"#,
    );
    let diagnostics = codes(&bad_curve);
    assert!(diagnostics.contains(&DiagnosticCode::Range));
    assert!(diagnostics.contains(&DiagnosticCode::AutomationWriter));
}

#[test]
fn validates_graph_nesting_without_expanding_performance() {
    let source = base(
        r#"pattern ptn { length = 1q; use nested { pattern = &other; at = 0q; } }
pattern other { length = 1q; note n { at = 0q; dur = 1/2q; pitch = C4; } }
track t { target = &synth:events; }
place x { pattern = &ptn; track = &t; at = 0q; }
"#,
    );
    let diagnostics = codes(&source);
    assert!(diagnostics.contains(&DiagnosticCode::Reference));

    // Keep the return type in this target's public contract even while plan
    // lowering is added in a later package.
    let _: Option<SourceGraph> = None;
}

#[test]
fn accepts_signed_note_offsets_global_bar_anchors_and_compatible_curve_units() {
    let source = base(
        r#"node filter { type = "core.onepole/1"; config = { channels = 1; }; params = { cutoff = 500Hz; }; }
curve c { clock = seconds; points = [(0s, 1Hz, linear), (1s, 2kHz, step)]; }
automation a { target = &filter.params.cutoff; curve = &c; at = bar(1, 1); }
pattern ptn { length = 1q; note n { at = 0q; dur = 1/2q; pitch = C4; onset_offset = -20ms; release_offset = -5ms; } }
"#,
    );
    validate_source(&parse(&source).unwrap())
        .expect("signed physical offsets and global bar anchors are valid");
}
