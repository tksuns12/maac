use std::collections::BTreeMap;

use maac::bundle::{sha256_digest, SourceIdentity};
use maac::graph::{GraphNode, GraphProcessor, GraphProgram, InstrumentProgram, ProgramSource};
use maac::instrument_plan::InstrumentResources;
use maac::plan::{
    Connection, EqMode, Interpolation, Node, OutputSettings, Plan, PlanError, PlanLimits, PortRef,
    Processor, TempoMap, TempoPoint, PLAN_VERSION,
};
use maac::{DiagnosticCode, PlanArtifact, Rational, VersionedPlan};
use num_traits::Zero;
use serde_json::{json, Value};

fn limits(max_channels: u8) -> PlanLimits {
    PlanLimits {
        max_channels,
        ..PlanLimits::default()
    }
}

fn base_plan() -> Plan {
    Plan {
        version: 1,
        output: OutputSettings {
            score_start_q: Rational::zero(),
            score_end_q: Rational::new(1.into(), 24_000.into()),
            tail_seconds: Rational::zero(),
            sample_rate_hz: 48_000,
            channels: 1,
            total_frames: 2,
            output: PortRef::new("source", "out").unwrap(),
        },
        tempo: TempoMap {
            points: vec![TempoPoint {
                q: Rational::zero(),
                bpm: Rational::from_integer(60.into()),
                shape: Interpolation::Step,
            }],
        },
        events: Vec::new(),
        nodes: vec![Node::new("source", Processor::sine(1)).unwrap()],
        connections: Vec::new(),
        automation: Vec::new(),
        regions: Vec::new(),
        source_mappings: Vec::new(),
        instruments: None,
        production: None,
    }
}

fn processor_plan(processor: Processor) -> Plan {
    let mut plan = base_plan();
    let input_channels = match &processor {
        Processor::OnePole { channels }
        | Processor::Gain { channels }
        | Processor::Fader { channels }
        | Processor::Delay { channels, .. }
        | Processor::Eq { channels, .. }
        | Processor::Compressor { channels, .. }
        | Processor::Reverb { channels, .. }
        | Processor::Sum { channels } => Some(*channels),
        Processor::Matrix { inputs, .. } => Some(*inputs),
        _ => None,
    };
    plan.nodes.push(Node::new("subject", processor).unwrap());
    if input_channels == Some(1) {
        plan.connections.push(
            Connection::new(
                "subject_input",
                PortRef::new("source", "out").unwrap(),
                PortRef::new("subject", "in").unwrap(),
            )
            .unwrap(),
        );
    } else if input_channels == Some(2) {
        plan.nodes.push(Node::new("pan", Processor::pan()).unwrap());
        plan.connections.extend([
            Connection::new(
                "pan_input",
                PortRef::new("source", "out").unwrap(),
                PortRef::new("pan", "in").unwrap(),
            )
            .unwrap(),
            Connection::new(
                "subject_input",
                PortRef::new("pan", "out").unwrap(),
                PortRef::new("subject", "in").unwrap(),
            )
            .unwrap(),
        ]);
    }
    plan
}

fn output_plan(channels: u8) -> Plan {
    let mut plan = base_plan();
    plan.nodes[0].processor = if channels == 2 {
        Processor::pan()
    } else {
        Processor::sine(1)
    };
    plan.output.channels = channels;
    if channels == 2 {
        plan.nodes
            .push(Node::new("osc", Processor::sine(1)).unwrap());
        plan.connections.push(
            Connection::new(
                "output_input",
                PortRef::new("osc", "out").unwrap(),
                PortRef::new("source", "in").unwrap(),
            )
            .unwrap(),
        );
    }
    plan
}

fn record_error(
    failures: &mut Vec<String>,
    label: &str,
    result: Result<(), PlanError>,
    expected_code: &str,
    expected_path: &str,
) {
    match result {
        Err(error) if error.code == expected_code && error.path == expected_path => {}
        Err(error) => failures.push(format!(
            "{label}: expected {expected_code} at {expected_path}, got {} at {}",
            error.code, error.path
        )),
        Ok(()) => failures.push(format!(
            "{label}: expected {expected_code} at {expected_path}, got success"
        )),
    }
}

fn assert_dimension_contract(name: &str, path: &str, make_plan: impl Fn(u8) -> Plan) {
    make_plan(1).validate().unwrap();
    make_plan(2).validate().unwrap();
    make_plan(1).validate_with_limits(&limits(1)).unwrap();
    make_plan(2).validate_with_limits(&limits(2)).unwrap();
    make_plan(1).validate_with_limits(&limits(4)).unwrap();
    make_plan(2).validate_with_limits(&limits(4)).unwrap();

    let mut failures = Vec::new();
    for caller_limit in [0, 2, 4] {
        record_error(
            &mut failures,
            &format!("{name} zero with max_channels={caller_limit}"),
            make_plan(0).validate_with_limits(&limits(caller_limit)),
            "E_RANGE",
            path,
        );
    }
    for caller_limit in [1, 2, 4] {
        record_error(
            &mut failures,
            &format!("{name} unsupported three with max_channels={caller_limit}"),
            make_plan(3).validate_with_limits(&limits(caller_limit)),
            "E_CAPABILITY",
            path,
        );
    }
    record_error(
        &mut failures,
        &format!("{name} mono with max_channels=0"),
        make_plan(1).validate_with_limits(&limits(0)),
        "E_RESOURCE_LIMIT",
        path,
    );
    record_error(
        &mut failures,
        &format!("{name} stereo with max_channels=1"),
        make_plan(2).validate_with_limits(&limits(1)),
        "E_RESOURCE_LIMIT",
        path,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn eq(channels: u8) -> Processor {
    Processor::Eq {
        channels,
        mode: EqMode::LowPass,
    }
}

fn compressor(channels: u8) -> Processor {
    Processor::Compressor {
        channels,
        sidechain_channels: None,
    }
}

fn reverb(channels: u8) -> Processor {
    Processor::Reverb {
        channels,
        predelay_frames: 0,
        damping: Rational::zero(),
    }
}

type ProcessorFactory = fn(u8) -> Processor;

#[test]
fn output_channel_diagnostics_distinguish_range_capability_and_caller_limit() {
    assert_dimension_contract("output", "output.channels", output_plan);
}

#[test]
fn common_processor_channel_diagnostics_cover_every_variant() {
    let variants: [(&str, ProcessorFactory); 6] = [
        ("one_pole", Processor::one_pole),
        ("gain", Processor::gain),
        ("eq", eq),
        ("compressor", compressor),
        ("reverb", reverb),
        ("sum", Processor::sum),
    ];
    for (name, processor) in variants {
        assert_dimension_contract(name, "nodes.subject.processor", |channels| {
            processor_plan(processor(channels))
        });
    }
}

#[test]
fn explicit_processor_channel_diagnostics_cover_fader_noise_and_delay() {
    let variants: [(&str, ProcessorFactory); 3] = [
        ("fader", Processor::fader),
        ("noise", |channels| Processor::noise(channels, 7)),
        ("delay", |channels| Processor::delay(channels, 1)),
    ];
    for (name, processor) in variants {
        assert_dimension_contract(name, "nodes.subject.processor.channels", |channels| {
            processor_plan(processor(channels))
        });
    }
}

fn matrix(inputs: u8, outputs: u8) -> Processor {
    Processor::matrix(
        inputs,
        outputs,
        vec![vec![Rational::zero(); usize::from(inputs)]; usize::from(outputs)],
    )
}

#[test]
fn matrix_input_and_output_diagnostics_are_classified_independently() {
    assert_dimension_contract(
        "matrix inputs",
        "nodes.subject.processor.inputs",
        |inputs| processor_plan(matrix(inputs, 1)),
    );

    let output_plan = |outputs| processor_plan(matrix(1, outputs));
    output_plan(1).validate().unwrap();
    output_plan(2).validate().unwrap();
    output_plan(1).validate_with_limits(&limits(1)).unwrap();
    output_plan(2).validate_with_limits(&limits(2)).unwrap();
    output_plan(2).validate_with_limits(&limits(4)).unwrap();
    let path = "nodes.subject.processor.outputs";
    let mut failures = Vec::new();
    for caller_limit in [1, 2, 4] {
        record_error(
            &mut failures,
            &format!("matrix outputs zero with max_channels={caller_limit}"),
            output_plan(0).validate_with_limits(&limits(caller_limit)),
            "E_RANGE",
            path,
        );
        record_error(
            &mut failures,
            &format!("matrix outputs three with max_channels={caller_limit}"),
            output_plan(3).validate_with_limits(&limits(caller_limit)),
            "E_CAPABILITY",
            path,
        );
    }
    record_error(
        &mut failures,
        "matrix outputs stereo with max_channels=1",
        output_plan(2).validate_with_limits(&limits(1)),
        "E_RESOURCE_LIMIT",
        path,
    );
    record_error(
        &mut failures,
        "matrix inputs take precedence when max_channels=0",
        output_plan(1).validate_with_limits(&limits(0)),
        "E_RESOURCE_LIMIT",
        "nodes.subject.processor.inputs",
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn sidechain_plan(channels: u8) -> Plan {
    let mut plan = base_plan();
    plan.nodes.push(
        Node::new(
            "subject",
            Processor::Compressor {
                channels: 1,
                sidechain_channels: Some(channels),
            },
        )
        .unwrap(),
    );
    if channels == 1 {
        plan.connections.extend([
            Connection::new(
                "subject_input",
                PortRef::new("source", "out").unwrap(),
                PortRef::new("subject", "in").unwrap(),
            )
            .unwrap(),
            Connection::new(
                "sidechain",
                PortRef::new("source", "out").unwrap(),
                PortRef::new("subject", "sidechain").unwrap(),
            )
            .unwrap(),
        ]);
    } else if channels == 2 {
        plan.nodes.push(Node::new("pan", Processor::pan()).unwrap());
        plan.connections.extend([
            Connection::new(
                "subject_input",
                PortRef::new("source", "out").unwrap(),
                PortRef::new("subject", "in").unwrap(),
            )
            .unwrap(),
            Connection::new(
                "pan_input",
                PortRef::new("source", "out").unwrap(),
                PortRef::new("pan", "in").unwrap(),
            )
            .unwrap(),
            Connection::new(
                "sidechain",
                PortRef::new("pan", "out").unwrap(),
                PortRef::new("subject", "sidechain").unwrap(),
            )
            .unwrap(),
        ]);
    }
    plan
}

#[test]
fn compressor_sidechain_channel_diagnostics_follow_the_same_contract() {
    sidechain_plan(1).validate().unwrap();
    sidechain_plan(2).validate().unwrap();
    sidechain_plan(1).validate_with_limits(&limits(1)).unwrap();
    sidechain_plan(2).validate_with_limits(&limits(2)).unwrap();
    sidechain_plan(2).validate_with_limits(&limits(4)).unwrap();

    let path = "nodes.subject.processor.sidechain_channels";
    let mut failures = Vec::new();
    for caller_limit in [1, 2, 4] {
        record_error(
            &mut failures,
            &format!("compressor sidechain zero with max_channels={caller_limit}"),
            sidechain_plan(0).validate_with_limits(&limits(caller_limit)),
            "E_RANGE",
            path,
        );
        record_error(
            &mut failures,
            &format!("compressor sidechain three with max_channels={caller_limit}"),
            sidechain_plan(3).validate_with_limits(&limits(caller_limit)),
            "E_CAPABILITY",
            path,
        );
    }
    record_error(
        &mut failures,
        "compressor sidechain stereo with max_channels=1",
        sidechain_plan(2).validate_with_limits(&limits(1)),
        "E_RESOURCE_LIMIT",
        path,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn instrument_resources(channels: u8) -> InstrumentResources {
    let shared = (channels == 2).then(|| GraphProgram {
        channels: 2,
        nodes: vec![GraphNode {
            id: "pan".into(),
            processor: GraphProcessor::Pan,
            params: BTreeMap::new(),
        }],
        connections: vec![Connection::new(
            "input_pan",
            PortRef::new("input", "out").unwrap(),
            PortRef::new("pan", "in").unwrap(),
        )
        .unwrap()],
        modulations: Vec::new(),
        output: PortRef::new("pan", "out").unwrap(),
        amplitude: None,
    });
    InstrumentResources {
        entry_source: "score.maac".into(),
        programs: vec![InstrumentProgram {
            id: "program".into(),
            voice: GraphProgram {
                channels: 1,
                nodes: vec![
                    GraphNode {
                        id: "amp".into(),
                        processor: GraphProcessor::Adsr,
                        params: BTreeMap::new(),
                    },
                    GraphNode {
                        id: "osc".into(),
                        processor: GraphProcessor::Sine,
                        params: BTreeMap::new(),
                    },
                ],
                connections: Vec::new(),
                modulations: Vec::new(),
                output: PortRef::new("osc", "out").unwrap(),
                amplitude: Some("amp".into()),
            },
            shared,
            controls: BTreeMap::new(),
            source: ProgramSource {
                file: "score.maac".into(),
                object: "program".into(),
                span: None,
            },
        }],
        wavetables: Vec::new(),
        wavetable_sources: Vec::new(),
        source_files: vec![SourceIdentity {
            path: "score.maac".into(),
            hash: format!("sha256:{}", "1".repeat(64)),
        }],
        dependencies: Vec::new(),
        libraries: Vec::new(),
    }
}

fn instrument_plan(channels: u8) -> Plan {
    let resource_channels = channels.clamp(1, PlanLimits::MAX_CHANNELS);
    let mut plan = base_plan();
    plan.version = PLAN_VERSION;
    plan.nodes.push(
        Node::new(
            "subject",
            Processor::Instrument {
                program: "program".into(),
                voices: 1,
                channels,
            },
        )
        .unwrap(),
    );
    plan.instruments = Some(instrument_resources(resource_channels));
    plan
}

#[test]
fn instrument_channel_diagnostics_follow_the_same_contract() {
    assert_dimension_contract(
        "instrument",
        "nodes.subject.processor.channels",
        instrument_plan,
    );
}

fn asset(channels: u8) -> Value {
    let bytes = vec![0; usize::from(channels) * 4];
    json!({
        "id": "sample",
        "format": "pcm_f32le_interleaved/1",
        "rate_hz": 48_000,
        "channels": channels,
        "frames": 1,
        "hash": sha256_digest(&bytes),
        "bytes": bytes,
    })
}

fn artifact_base(version: u32, asset_channels: u8) -> Value {
    json!({
        "version": version,
        "output": {
            "score_start_q": "0/1",
            "score_end_q": "1/24000",
            "tail_seconds": "0/1",
            "sample_rate_hz": 48_000,
            "channels": 1,
            "total_frames": 2,
            "output": {"node": "source", "port": "out"},
        },
        "tempo": {"points": [{"q": "0/1", "bpm": "60/1", "shape": "step"}]},
        "nodes": [{
            "id": "source",
            "processor": {"kind": "core", "processor": {"kind": "sine", "voices": 1}},
        }],
        "audio_assets": [asset(asset_channels)],
    })
}

fn load_artifact(value: &Value, limits: &PlanLimits) -> Result<(), PlanError> {
    PlanArtifact::from_json_with_limits(&serde_json::to_vec(value).unwrap(), limits).map(|_| ())
}

fn kit_artifact(channels: u8) -> Value {
    let asset_channels = channels.clamp(1, PlanLimits::MAX_CHANNELS);
    let mut value = artifact_base(4, asset_channels);
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id": "kit",
        "processor": {
            "kind": "kit",
            "channels": channels,
            "voices": 1,
            "samples": [{"key": "hit", "asset": "sample"}],
        },
    }));
    value
}

fn audio_artifact(channels: u8) -> Value {
    let asset_channels = channels.clamp(1, PlanLimits::MAX_CHANNELS);
    let mut value = artifact_base(5, asset_channels);
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id": "audio",
        "processor": {"kind": "audio", "clip": {
            "asset": "sample",
            "channels": channels,
            "at": {"kind": "score", "q": "0/1"},
            "source_start_frame": 0,
            "source_end_frame": 1,
            "speed": "1/1",
            "reverse": false,
            "gain": "1/1",
            "fade_in_seconds": "0/1",
            "fade_out_seconds": "0/1",
            "fade_shape": "linear",
            "source": {"object": "audio", "path": ["audio"]},
            "start_frame": 0,
            "end_frame": 1,
        }},
    }));
    value
}

fn assert_artifact_dimension_contract(name: &str, path: &str, make_value: impl Fn(u8) -> Value) {
    load_artifact(&make_value(1), &PlanLimits::default()).unwrap();
    load_artifact(&make_value(2), &PlanLimits::default()).unwrap();
    load_artifact(&make_value(1), &limits(1)).unwrap();
    load_artifact(&make_value(2), &limits(2)).unwrap();
    load_artifact(&make_value(2), &limits(4)).unwrap();

    let mut failures = Vec::new();
    for caller_limit in [0, 2, 4] {
        record_error(
            &mut failures,
            &format!("{name} zero with max_channels={caller_limit}"),
            load_artifact(&make_value(0), &limits(caller_limit)),
            "E_RANGE",
            path,
        );
    }
    for caller_limit in [1, 2, 4] {
        record_error(
            &mut failures,
            &format!("{name} unsupported three with max_channels={caller_limit}"),
            load_artifact(&make_value(3), &limits(caller_limit)),
            "E_CAPABILITY",
            path,
        );
    }
    record_error(
        &mut failures,
        &format!("{name} mono with max_channels=0"),
        load_artifact(&make_value(1), &limits(0)),
        "E_RESOURCE_LIMIT",
        path,
    );
    record_error(
        &mut failures,
        &format!("{name} stereo with max_channels=1"),
        load_artifact(&make_value(2), &limits(1)),
        "E_RESOURCE_LIMIT",
        path,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn kit_wrapper_propagates_channel_diagnostics_from_retained_json() {
    assert_artifact_dimension_contract("kit", "nodes.kit.processor.channels", kit_artifact);
}

#[test]
fn audio_wrapper_propagates_channel_diagnostics_from_retained_json() {
    assert_artifact_dimension_contract("audio", "nodes.audio.channels", audio_artifact);
}

#[test]
fn legacy_retained_json_loaders_propagate_the_resource_diagnostic() {
    let plan = processor_plan(Processor::gain(2));
    plan.validate().unwrap();
    let bytes = serde_json::to_vec(&plan).unwrap();
    Plan::from_json(&bytes).unwrap();
    VersionedPlan::from_json(&bytes).unwrap();
    PlanArtifact::from_json(&bytes).unwrap();

    for (name, result) in [
        (
            "Plan",
            Plan::from_json_with_limits(&bytes, &limits(1)).map(|_| ()),
        ),
        (
            "VersionedPlan",
            VersionedPlan::from_json_with_limits(&bytes, &limits(1)).map(|_| ()),
        ),
        (
            "PlanArtifact",
            PlanArtifact::from_json_with_limits(&bytes, &limits(1)).map(|_| ()),
        ),
    ] {
        let error = result.unwrap_err();
        assert_eq!(error.code, "E_RESOURCE_LIMIT", "{name}");
        assert_eq!(error.path, "nodes.subject.processor", "{name}");
    }
}

#[test]
fn source_compilation_propagates_the_resource_diagnostic() {
    let source = r#"maac 1;
project p { score = [0q, 1/24000q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &gain:out; }
tempo clock { points = [(0q, 60bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node sine { type = "core.sine/1"; }
node pan { type = "core.pan/1"; }
node gain { type = "core.gain/1"; config = { channels = 2; }; }
connect sine_pan { from = &sine:out; to = &pan:in; }
connect pan_gain { from = &pan:out; to = &gain:in; }
"#;
    let document = maac::parse(source).unwrap();
    maac::compile_with_limits(&document, &limits(2)).unwrap();
    let diagnostics = maac::compile_with_limits(&document, &limits(1)).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::ResourceLimit),
        "{diagnostics:?}"
    );
}
