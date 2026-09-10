use std::collections::BTreeMap;

use maac::graph::{
    parameter_descriptor, parameter_descriptor_for_stage, topological_order, validate_graph,
    Control, ControlTarget, GraphNode, GraphProcessor, GraphProgram, GraphStage, GraphUnit,
    InstrumentProgram, Modulation, ParameterRate, ParameterTarget, ProgramSource, MAX_GRAPH_EDGES,
    MAX_GRAPH_NODES, MAX_INSTRUMENT_CONTROLS,
};
use maac::plan::{Connection, PortRef, Rational};

fn rat(numerator: i64, denominator: i64) -> Rational {
    Rational::new(numerator.into(), denominator.into())
}

fn node(id: &str, processor: GraphProcessor) -> GraphNode {
    GraphNode {
        id: id.into(),
        processor,
        params: BTreeMap::new(),
    }
}

fn port(node: &str, port: &str) -> PortRef {
    PortRef::new(node, port).unwrap()
}

fn connection(id: &str, from: (&str, &str), to: (&str, &str)) -> Connection {
    Connection::new(id, port(from.0, from.1), port(to.0, to.1)).unwrap()
}

fn voice_graph() -> GraphProgram {
    GraphProgram {
        channels: 1,
        nodes: vec![
            node("amp", GraphProcessor::Adsr),
            node("osc", GraphProcessor::Sine),
        ],
        connections: vec![],
        modulations: vec![],
        output: port("osc", "out"),
        amplitude: Some("amp".into()),
    }
}

fn instrument() -> InstrumentProgram {
    InstrumentProgram {
        id: "lead".into(),
        voice: voice_graph(),
        shared: None,
        controls: BTreeMap::new(),
        source: ProgramSource {
            file: "sounds/lead.maac".into(),
            object: "lead".into(),
            span: None,
        },
    }
}

#[test]
fn processor_wire_kinds_are_exact_versioned_identities() {
    let gain = GraphProcessor::Gain { channels: 2 };
    assert_eq!(gain.identity(), "synth.gain/1");
    assert_eq!(
        serde_json::to_value(&gain).unwrap(),
        serde_json::json!({"kind": "synth.gain/1", "channels": 2})
    );
    assert_eq!(
        serde_json::from_value::<GraphProcessor>(serde_json::json!({
            "kind": "synth.wavetable/1",
            "table": "colors"
        }))
        .unwrap(),
        GraphProcessor::Wavetable {
            table: "colors".into()
        }
    );

    for kind in ["sine", "synth.sine", "synth.sine/2"] {
        assert!(serde_json::from_value::<GraphProcessor>(serde_json::json!({
            "kind": kind
        }))
        .is_err());
    }
    assert!(serde_json::from_value::<GraphProcessor>(serde_json::json!({
        "kind": "synth.sine/1",
        "ignored": true
    }))
    .is_err());
}

#[test]
fn rational_fields_and_source_provenance_round_trip_strictly() {
    let mut value = instrument();
    value.voice.nodes[1]
        .params
        .insert("ratio".into(), rat(3, 2));
    value.controls.insert(
        "ratio".into(),
        Control {
            target: ControlTarget {
                graph: GraphStage::Voice,
                node: "osc".into(),
                parameter: "ratio".into(),
            },
            default: rat(3, 2),
        },
    );
    value.validate().unwrap();

    let json = serde_json::to_value(&value).unwrap();
    assert_eq!(json["voice"]["nodes"][1]["params"]["ratio"], "3/2");
    assert_eq!(json["controls"]["ratio"]["default"], "3/2");
    assert_eq!(json["source"]["file"], "sounds/lead.maac");
    assert_eq!(
        serde_json::from_value::<InstrumentProgram>(json).unwrap(),
        value
    );
}

#[test]
fn descriptors_publish_ranges_units_and_stage_specific_rates() {
    let cutoff = parameter_descriptor(&GraphProcessor::OnePole { channels: 1 }, "cutoff")
        .expect("cutoff descriptor");
    assert_eq!(cutoff.unit, GraphUnit::Hertz);
    assert_eq!(cutoff.rate, ParameterRate::Sample);
    assert!(cutoff.min_open);
    assert!(cutoff.max_open);
    assert_eq!(cutoff.default, rat(1000, 1));
    assert!(cutoff.validate(&rat(1, 1)).is_ok());
    assert_eq!(cutoff.validate(&rat(0, 1)).unwrap_err().code, "E_RANGE");

    let lfo_phase = parameter_descriptor(&GraphProcessor::Lfo, "phase").unwrap();
    assert_eq!(lfo_phase.rate, ParameterRate::NoteOn);
    assert_eq!(
        parameter_descriptor_for_stage(&GraphProcessor::Lfo, "phase", GraphStage::Shared)
            .unwrap()
            .rate,
        ParameterRate::Reset
    );
    assert!(parameter_descriptor(&GraphProcessor::Mix { channels: 1 }, "level").is_none());
}

#[test]
fn duplicate_control_keys_are_rejected_during_deserialization() {
    let json = r#"{
        "id":"lead",
        "voice":{
            "channels":1,
            "nodes":[
                {"id":"amp","processor":{"kind":"synth.adsr/1"},"params":{}},
                {"id":"osc","processor":{"kind":"synth.sine/1"},"params":{}}
            ],
            "connections":[],
            "modulations":[],
            "output":{"node":"osc","port":"out"},
            "amplitude":"amp"
        },
        "controls":{
            "ratio":{"target":{"graph":"voice","node":"osc","parameter":"ratio"},"default":"1/1"},
            "ratio":{"target":{"graph":"voice","node":"osc","parameter":"ratio"},"default":"2/1"}
        },
        "source":{"file":"lead.maac","object":"lead"}
    }"#;
    let error = serde_json::from_str::<InstrumentProgram>(json).unwrap_err();
    assert!(error.to_string().contains("E_DUPLICATE_FIELD"));
}

#[test]
fn source_span_is_bounded_by_the_per_source_limit() {
    let mut value = instrument();
    value.source.span = Some(maac::plan::SourceSpan {
        start: 0,
        end: 4 * 1024 * 1024 + 1,
    });
    assert_eq!(value.validate().unwrap_err().code, "E_RANGE");
}

#[test]
fn validates_voice_shared_io_controls_and_channels() {
    let mut value = instrument();
    value.shared = Some(GraphProgram {
        channels: 2,
        nodes: vec![node("pan", GraphProcessor::Pan)],
        connections: vec![connection("input_pan", ("input", "out"), ("pan", "in"))],
        modulations: vec![],
        output: port("pan", "out"),
        amplitude: None,
    });
    value.controls.insert(
        "ratio".into(),
        Control {
            target: ControlTarget {
                graph: GraphStage::Voice,
                node: "osc".into(),
                parameter: "ratio".into(),
            },
            default: rat(2, 1),
        },
    );
    value.validate().unwrap();
    assert_eq!(value.channels(), 2);
    assert_eq!(
        value.control_spec("ratio").unwrap().unit,
        GraphUnit::Dimensionless
    );

    value.controls.insert(
        "other_ratio".into(),
        Control {
            target: value.controls["ratio"].target.clone(),
            default: rat(1, 1),
        },
    );
    let err = value.validate().unwrap_err();
    assert_eq!(err.code, "E_AUTOMATION_WRITER");
}

#[test]
fn rejects_invalid_references_ports_cardinality_and_output_channels() {
    let mut graph = voice_graph();
    graph.connections.push(connection(
        "missing_gain",
        ("osc", "out"),
        ("missing", "in"),
    ));
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_REFERENCE"
    );

    let mut graph = voice_graph();
    graph
        .nodes
        .push(node("gain", GraphProcessor::Gain { channels: 2 }));
    graph
        .connections
        .push(connection("wrong_channels", ("osc", "out"), ("gain", "in")));
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_PORT_TYPE"
    );

    let mut graph = voice_graph();
    graph
        .nodes
        .push(node("gain", GraphProcessor::Gain { channels: 1 }));
    graph.output = port("gain", "out");
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_PORT_TYPE"
    );

    let mut graph = voice_graph();
    graph.output = port("osc", "in");
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_PORT_TYPE"
    );
}

#[test]
fn validates_every_node_and_rejects_mixed_audio_modulation_cycles() {
    let mut graph = voice_graph();
    let mut unused = node("unused", GraphProcessor::Gain { channels: 1 });
    unused.params.insert("bogus".into(), rat(1, 1));
    graph.nodes.push(unused);
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_RANGE"
    );

    let mut graph = voice_graph();
    graph
        .nodes
        .push(node("gain", GraphProcessor::Gain { channels: 1 }));
    graph
        .connections
        .push(connection("osc_gain", ("osc", "out"), ("gain", "in")));
    graph.modulations.push(Modulation {
        id: "gain_osc".into(),
        from: port("gain", "out"),
        to: ParameterTarget {
            node: "osc".into(),
            parameter: "frequency".into(),
        },
        depth: rat(1, 1),
    });
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_ALGEBRAIC_LOOP"
    );
}

#[test]
fn modulation_requires_mono_source_sample_target_and_bounded_depth() {
    let mut graph = voice_graph();
    graph.modulations.push(Modulation {
        id: "phase_mod".into(),
        from: port("amp", "out"),
        to: ParameterTarget {
            node: "osc".into(),
            parameter: "phase".into(),
        },
        depth: rat(1, 1),
    });
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_PORT_TYPE"
    );

    graph.modulations[0].to.parameter = "frequency".into();
    graph.modulations[0].depth = rat(48_001, 1);
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_RANGE"
    );
}

#[test]
fn voice_adsr_event_rate_modulations_allow_signed_depths_only() {
    let mut graph = voice_graph();
    graph.nodes.push(node("source", GraphProcessor::Sine));
    graph.modulations = vec![
        Modulation {
            id: "attack_zero".into(),
            from: port("source", "out"),
            to: ParameterTarget {
                node: "amp".into(),
                parameter: "attack".into(),
            },
            depth: rat(0, 1),
        },
        Modulation {
            id: "decay_signed".into(),
            from: port("source", "out"),
            to: ParameterTarget {
                node: "amp".into(),
                parameter: "decay".into(),
            },
            depth: rat(-1, 2),
        },
        Modulation {
            id: "sustain_zero".into(),
            from: port("source", "out"),
            to: ParameterTarget {
                node: "amp".into(),
                parameter: "sustain".into(),
            },
            depth: rat(0, 1),
        },
        Modulation {
            id: "release_signed".into(),
            from: port("source", "out"),
            to: ParameterTarget {
                node: "amp".into(),
                parameter: "release".into(),
            },
            depth: rat(-1, 2),
        },
    ];
    validate_graph(&graph, true, None).unwrap();

    let mut phase = graph.clone();
    phase.modulations = vec![Modulation {
        id: "phase".into(),
        from: port("source", "out"),
        to: ParameterTarget {
            node: "osc".into(),
            parameter: "phase".into(),
        },
        depth: rat(0, 1),
    }];
    assert_eq!(
        validate_graph(&phase, true, None).unwrap_err().code,
        "E_PORT_TYPE"
    );

    let mut wrong_port = graph.clone();
    wrong_port.modulations[0].from = port("source", "in");
    assert_eq!(
        validate_graph(&wrong_port, true, None).unwrap_err().code,
        "E_PORT_TYPE"
    );

    let mut missing_source = graph.clone();
    missing_source.modulations[0].from = port("missing", "out");
    assert_eq!(
        validate_graph(&missing_source, true, None)
            .unwrap_err()
            .code,
        "E_REFERENCE"
    );

    let mut cycle = graph.clone();
    cycle.modulations = vec![
        Modulation {
            id: "source_amp".into(),
            from: port("source", "out"),
            to: ParameterTarget {
                node: "amp".into(),
                parameter: "attack".into(),
            },
            depth: rat(0, 1),
        },
        Modulation {
            id: "amp_source".into(),
            from: port("amp", "out"),
            to: ParameterTarget {
                node: "source".into(),
                parameter: "frequency".into(),
            },
            depth: rat(1, 1),
        },
    ];
    assert_eq!(
        validate_graph(&cycle, true, None).unwrap_err().code,
        "E_ALGEBRAIC_LOOP"
    );

    let mut shared = GraphProgram {
        channels: 1,
        nodes: vec![
            node("lfo", GraphProcessor::Lfo),
            node("gain", GraphProcessor::Gain { channels: 1 }),
        ],
        connections: vec![connection("input_gain", ("input", "out"), ("gain", "in"))],
        modulations: vec![Modulation {
            id: "reset".into(),
            from: port("input", "out"),
            to: ParameterTarget {
                node: "lfo".into(),
                parameter: "phase".into(),
            },
            depth: rat(0, 1),
        }],
        output: port("gain", "out"),
        amplitude: None,
    };
    assert_eq!(
        validate_graph(&shared, false, Some(1)).unwrap_err().code,
        "E_PORT_TYPE"
    );
    shared.modulations.clear();
    validate_graph(&shared, false, Some(1)).unwrap();
}

#[test]
fn shared_graph_restrictions_and_reserved_input_are_enforced() {
    let shared = GraphProgram {
        channels: 1,
        nodes: vec![node("osc", GraphProcessor::Sine)],
        connections: vec![],
        modulations: vec![],
        output: port("osc", "out"),
        amplitude: None,
    };
    assert_eq!(
        validate_graph(&shared, false, Some(1)).unwrap_err().code,
        "E_CAPABILITY"
    );

    let mut voice = voice_graph();
    voice
        .connections
        .push(connection("reserved", ("input", "out"), ("osc", "in")));
    assert_eq!(
        validate_graph(&voice, true, None).unwrap_err().code,
        "E_REFERENCE"
    );

    let mut value = instrument();
    value.shared = Some(GraphProgram {
        channels: 1,
        nodes: vec![
            node("lfo", GraphProcessor::Lfo),
            node("gain", GraphProcessor::Gain { channels: 1 }),
        ],
        connections: vec![connection("input_gain", ("input", "out"), ("gain", "in"))],
        modulations: vec![],
        output: port("gain", "out"),
        amplitude: None,
    });
    value.controls.insert(
        "phase".into(),
        Control {
            target: ControlTarget {
                graph: GraphStage::Shared,
                node: "lfo".into(),
                parameter: "phase".into(),
            },
            default: rat(0, 1),
        },
    );
    value.validate().unwrap();
    assert_eq!(
        value.control_spec("phase").unwrap().rate,
        ParameterRate::Reset
    );
}

#[test]
fn local_resource_limits_fail_before_deep_graph_validation() {
    let mut graph = voice_graph();
    graph.nodes = (0..=MAX_GRAPH_NODES)
        .map(|index| node(&format!("n{index}"), GraphProcessor::Sine))
        .collect();
    graph.output = port("does_not_exist", "out");
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );

    let mut graph = voice_graph();
    graph.connections = (0..=MAX_GRAPH_EDGES)
        .map(|index| connection(&format!("c{index}"), ("missing", "out"), ("missing", "in")))
        .collect();
    assert_eq!(
        validate_graph(&graph, true, None).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );

    let mut value = instrument();
    value.controls = (0..=MAX_INSTRUMENT_CONTROLS)
        .map(|index| {
            (
                format!("c{index}"),
                Control {
                    target: ControlTarget {
                        graph: GraphStage::Voice,
                        node: "missing".into(),
                        parameter: "missing".into(),
                    },
                    default: rat(0, 1),
                },
            )
        })
        .collect();
    assert_eq!(value.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn topology_uses_all_edge_types_and_node_ids_for_stable_ties() {
    let graph = GraphProgram {
        channels: 1,
        nodes: vec![
            node("z_source", GraphProcessor::Sine),
            node("output", GraphProcessor::Gain { channels: 1 }),
            node("a_source", GraphProcessor::Lfo),
            node("amp", GraphProcessor::Adsr),
        ],
        connections: vec![connection("audio", ("z_source", "out"), ("output", "in"))],
        modulations: vec![Modulation {
            id: "mod".into(),
            from: port("a_source", "out"),
            to: ParameterTarget {
                node: "output".into(),
                parameter: "level".into(),
            },
            depth: rat(1, 1),
        }],
        output: port("output", "out"),
        amplitude: Some("amp".into()),
    };
    validate_graph(&graph, true, None).unwrap();
    let ordered_ids: Vec<_> = topological_order(&graph)
        .unwrap()
        .into_iter()
        .map(|index| graph.nodes[index].id.as_str())
        .collect();
    assert_eq!(ordered_ids, ["a_source", "amp", "z_source", "output"]);
}
