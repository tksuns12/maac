use std::collections::BTreeMap;

use maac::bundle::{DependencyIdentity, SourceIdentity};
use maac::graph::{
    Control, ControlTarget, GraphNode, GraphProcessor, GraphProgram, GraphStage, InstrumentProgram,
    ProgramSource,
};
use maac::instrument_plan::InstrumentResources;
use maac::library::{LibraryMetadata, WavetableSource};
use maac::plan::{
    Automation, AutomationClock, AutomationPoint, EventKind, EventTarget, Interpolation, Node,
    OutputSettings, Plan, PortRef, Processor, ResolvedEvent, SourceMapping, TempoMap, TempoPoint,
    LEGACY_PLAN_VERSION, PLAN_VERSION,
};
use maac::wavetable::Wavetable;
use maac::Rational;

fn r(n: i64, d: i64) -> Rational {
    maac::parse_rational(&format!("{n}/{d}")).unwrap()
}

fn graph_node(id: &str, processor: GraphProcessor) -> GraphNode {
    GraphNode {
        id: id.into(),
        processor,
        params: BTreeMap::new(),
    }
}

fn program() -> InstrumentProgram {
    InstrumentProgram {
        id: "program_0".into(),
        voice: GraphProgram {
            channels: 1,
            nodes: vec![
                graph_node("amp", GraphProcessor::Adsr),
                graph_node("osc", GraphProcessor::Sine),
            ],
            connections: Vec::new(),
            modulations: Vec::new(),
            output: PortRef::new("osc", "out").unwrap(),
            amplitude: Some("amp".into()),
        },
        shared: None,
        controls: BTreeMap::from([(
            "release".into(),
            Control {
                target: ControlTarget {
                    graph: GraphStage::Voice,
                    node: "amp".into(),
                    parameter: "release".into(),
                },
                default: r(1, 10),
            },
        )]),
        source: ProgramSource {
            file: "lib/studio.maac".into(),
            object: "bell".into(),
            span: None,
        },
    }
}

fn resources() -> InstrumentResources {
    InstrumentResources {
        entry_source: "score.maac".into(),
        programs: vec![program()],
        wavetables: Vec::new(),
        wavetable_sources: Vec::new(),
        source_files: vec![
            SourceIdentity {
                path: "score.maac".into(),
                hash: format!("sha256:{}", "1".repeat(64)),
            },
            SourceIdentity {
                path: "lib/studio.maac".into(),
                hash: format!("sha256:{}", "2".repeat(64)),
            },
        ],
        dependencies: vec![DependencyIdentity {
            source: "score.maac".into(),
            alias: "studio".into(),
            path: "lib/studio.maac".into(),
            hash: format!("sha256:{}", "2".repeat(64)),
        }],
        libraries: vec![LibraryMetadata {
            file: "lib/studio.maac".into(),
            object: "studio".into(),
            version: "1.0.0".into(),
            creator: Some("Example".into()),
            license: None,
        }],
    }
}

fn base_plan(version: u32) -> Plan {
    Plan {
        version,
        output: OutputSettings {
            score_start_q: r(0, 1),
            score_end_q: r(1, 1),
            tail_seconds: r(1, 1),
            sample_rate_hz: 48_000,
            channels: 1,
            total_frames: 72_000,
            output: PortRef::new("bell", "out").unwrap(),
        },
        tempo: TempoMap {
            points: vec![TempoPoint {
                q: r(0, 1),
                bpm: r(120, 1),
                shape: Interpolation::Step,
            }],
        },
        events: vec![ResolvedEvent {
            address: "main/note".into(),
            source: SourceMapping {
                object: "note".into(),
                path: vec!["main".into(), "note".into()],
                span: None,
            },
            target: EventTarget::new("bell", "events").unwrap(),
            kind: EventKind::Note {
                pitch_expression: None,
                gain_expression: None,
                pitch_hz: 440.0,
                velocity: r(1, 1),
            },
            score_on_q: r(0, 1),
            score_off_q: Some(r(1, 2)),
            onset_offset_seconds: r(0, 1),
            release_offset_seconds: r(0, 1),
            on_seconds: r(0, 1),
            off_seconds: Some(r(1, 4)),
            release_velocity: 0.0,
            on_frame: 0,
            off_frame: Some(12_000),
            order: 0,
        }],
        nodes: vec![Node {
            id: "bell".into(),
            processor: Processor::Instrument {
                program: "program_0".into(),
                voices: 8,
                channels: 1,
            },
            params: BTreeMap::new(),
        }],
        connections: Vec::new(),
        automation: Vec::new(),
        regions: Vec::new(),
        source_mappings: Vec::new(),
        instruments: (version == PLAN_VERSION).then(resources),
        production: None,
    }
}

fn plan_with_dependency_chain(edge_count: usize) -> Plan {
    let mut plan = base_plan(PLAN_VERSION);
    let resources = plan.instruments.as_mut().unwrap();
    let hash = format!("sha256:{}", "1".repeat(64));
    resources.entry_source = format!("s{edge_count:02}.maac");
    resources.source_files = (0..=edge_count)
        .map(|index| SourceIdentity {
            path: format!("s{index:02}.maac"),
            hash: hash.clone(),
        })
        .collect();
    resources.dependencies = (1..=edge_count)
        .map(|index| DependencyIdentity {
            source: format!("s{index:02}.maac"),
            alias: "next".into(),
            path: format!("s{:02}.maac", index - 1),
            hash: hash.clone(),
        })
        .collect();
    resources.programs[0].source.file = "s00.maac".into();
    resources.libraries[0].file = "s00.maac".into();
    plan
}

fn plan_with_reused_dependency_suffix() -> Plan {
    let mut plan = base_plan(PLAN_VERSION);
    let resources = plan.instruments.as_mut().unwrap();
    let hash = format!("sha256:{}", "1".repeat(64));
    resources.entry_source = "a.maac".into();
    resources.source_files = std::iter::once("a.maac".to_owned())
        .chain((0..=16).map(|index| format!("s{index:02}.maac")))
        .chain((0..=16).map(|index| format!("z{index:02}.maac")))
        .map(|path| SourceIdentity {
            path,
            hash: hash.clone(),
        })
        .collect();
    resources.dependencies = (1..=16)
        .map(|index| DependencyIdentity {
            source: format!("s{index:02}.maac"),
            alias: "next".into(),
            path: format!("s{:02}.maac", index - 1),
            hash: hash.clone(),
        })
        .chain(std::iter::once(DependencyIdentity {
            source: "a.maac".into(),
            alias: "shared".into(),
            path: "s16.maac".into(),
            hash: hash.clone(),
        }))
        .chain(std::iter::once(DependencyIdentity {
            source: "z00.maac".into(),
            alias: "shared".into(),
            path: "s16.maac".into(),
            hash: hash.clone(),
        }))
        .chain((1..=16).map(|index| DependencyIdentity {
            source: format!("z{index:02}.maac"),
            alias: "next".into(),
            path: format!("z{:02}.maac", index - 1),
            hash: hash.clone(),
        }))
        .collect();
    resources.programs[0].source.file = "s00.maac".into();
    resources.libraries[0].file = "s00.maac".into();
    plan
}

#[test]
fn v2_round_trips_as_a_self_contained_strict_artifact() {
    let plan = base_plan(PLAN_VERSION);
    let bytes = plan.to_json().unwrap();
    let imported = Plan::from_json(&bytes).unwrap();
    assert_eq!(imported, plan);
    assert_eq!(
        imported.instrument_program("program_0").unwrap().channels(),
        1
    );

    let control = serde_json::to_string(&program().controls["release"]).unwrap();
    let needle = format!("\"controls\":{{\"release\":{control}}}");
    let replacement = format!("\"controls\":{{\"release\":{control},\"release\":{control}}}");
    let duplicate_control = String::from_utf8(bytes)
        .unwrap()
        .replacen(&needle, &replacement, 1);
    assert_eq!(
        Plan::from_json(duplicate_control.as_bytes())
            .unwrap_err()
            .code,
        "E_DUPLICATE_FIELD"
    );

    let unknown = String::from_utf8(plan.to_json().unwrap())
        .unwrap()
        .replacen(
            "\"entry_source\":\"score.maac\"",
            "\"entry_source\":\"score.maac\",\"surprise\":true",
            1,
        );
    assert_eq!(
        Plan::from_json(unknown.as_bytes()).unwrap_err().code,
        "E_UNKNOWN_FIELD"
    );
}

#[test]
fn versions_require_exactly_their_resource_and_processor_surface() {
    let mut legacy = base_plan(LEGACY_PLAN_VERSION);
    legacy.nodes[0] = Node::new("sum", Processor::sum(1)).unwrap();
    legacy.output.output = PortRef::new("sum", "out").unwrap();
    legacy.events.clear();
    legacy.instruments = None;
    legacy.validate().unwrap();
    assert!(!String::from_utf8(legacy.to_json().unwrap())
        .unwrap()
        .contains("instruments"));

    legacy.instruments = Some(resources());
    assert_eq!(legacy.validate().unwrap_err().code, "E_VERSION");

    let mut missing = base_plan(PLAN_VERSION);
    missing.instruments = None;
    assert_eq!(missing.validate().unwrap_err().code, "E_REFERENCE");
}

#[test]
fn validates_all_resource_provenance_and_wavetable_references() {
    let mut plan = base_plan(PLAN_VERSION);
    plan.instruments.as_mut().unwrap().entry_source = "./score.maac".into();
    assert_eq!(plan.validate().unwrap_err().code, "E_RANGE");

    let mut plan = base_plan(PLAN_VERSION);
    plan.instruments.as_mut().unwrap().programs[0].voice.nodes[1].processor =
        GraphProcessor::Wavetable {
            table: "missing".into(),
        };
    assert_eq!(plan.validate().unwrap_err().code, "E_REFERENCE");

    let mut plan = base_plan(PLAN_VERSION);
    plan.instruments
        .as_mut()
        .unwrap()
        .wavetables
        .push(Wavetable {
            id: "unused".into(),
            cycle_length: 8,
            samples: vec![f64::NAN; 8],
        });
    assert_eq!(plan.validate().unwrap_err().code, "E_NONFINITE");
}

#[test]
fn every_wavetable_has_one_valid_asset_provenance_record() {
    let mut plan = base_plan(PLAN_VERSION);
    plan.instruments
        .as_mut()
        .unwrap()
        .wavetables
        .push(Wavetable {
            id: "colors".into(),
            cycle_length: 8,
            samples: vec![0.0; 8],
        });
    assert_eq!(plan.validate().unwrap_err().code, "E_REFERENCE");

    plan.instruments
        .as_mut()
        .unwrap()
        .wavetable_sources
        .push(WavetableSource {
            table: "colors".into(),
            file: "lib/studio.maac".into(),
            object: "colors".into(),
            path: "assets/colors.wav".into(),
            hash: format!("sha256:{}", "4".repeat(64)),
        });
    plan.validate().unwrap();

    let sample_limits = maac::plan::PlanLimits {
        max_embedded_samples: 7,
        ..Default::default()
    };
    assert_eq!(
        plan.validate_with_limits(&sample_limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );

    let duplicate = plan.instruments.as_ref().unwrap().wavetable_sources[0].clone();
    plan.instruments
        .as_mut()
        .unwrap()
        .wavetable_sources
        .push(duplicate);
    assert_eq!(plan.validate().unwrap_err().code, "E_DUPLICATE_ID");
}

#[test]
fn instrument_controls_are_dynamic_and_missing_values_resolve_to_defaults() {
    let plan = base_plan(PLAN_VERSION);
    let resolved = plan.resolved_node_params(&plan.nodes[0]).unwrap();
    assert_eq!(resolved.get("release"), Some(&r(1, 10)));

    let mut invalid = plan;
    invalid.nodes[0]
        .params
        .insert("release".into(), r(2_000, 1));
    assert_eq!(invalid.validate().unwrap_err().code, "E_RANGE");
}

#[test]
fn aggregate_voice_graph_state_is_bounded_before_runtime_allocation() {
    let mut plan = base_plan(PLAN_VERSION);
    if let Processor::Instrument { voices, .. } = &mut plan.nodes[0].processor {
        *voices = 4_096;
    }
    for index in 0..32 {
        plan.nodes.push(Node {
            id: format!("extra_{index}"),
            processor: Processor::Instrument {
                program: "program_0".into(),
                voices: 4_096,
                channels: 1,
            },
            params: BTreeMap::new(),
        });
    }
    assert_eq!(plan.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn shared_graph_work_counts_every_output_frame_even_without_events() {
    let mut plan = base_plan(PLAN_VERSION);
    plan.events.clear();
    plan.instruments.as_mut().unwrap().programs[0].shared = Some(GraphProgram {
        channels: 1,
        nodes: vec![graph_node("mix", GraphProcessor::Mix { channels: 1 })],
        connections: Vec::new(),
        modulations: Vec::new(),
        output: PortRef::new("mix", "out").unwrap(),
        amplitude: None,
    });
    let limits = maac::plan::PlanLimits {
        max_execution_work: plan.output.total_frames - 1,
        ..Default::default()
    };
    assert_eq!(
        plan.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn voice_work_includes_the_largest_automated_amplitude_release() {
    let mut plan = base_plan(PLAN_VERSION);
    let limits = maac::plan::PlanLimits {
        max_execution_work: 100_000,
        ..Default::default()
    };
    plan.validate_with_limits(&limits).unwrap();

    plan.automation.push(Automation {
        id: "release_lane".into(),
        target: PortRef::new("bell", "release").unwrap(),
        clock: AutomationClock::Seconds,
        at: r(0, 1),
        points: vec![AutomationPoint {
            position: r(0, 1),
            value: r(1, 1),
            shape: Interpolation::Step,
        }],
    });
    assert_eq!(
        plan.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn dependency_cycles_and_pin_mismatches_fail_without_source_resolution() {
    let mut plan = base_plan(PLAN_VERSION);
    plan.instruments.as_mut().unwrap().dependencies[0].hash = format!("sha256:{}", "3".repeat(64));
    assert_eq!(plan.validate().unwrap_err().code, "E_REFERENCE");

    let mut plan = base_plan(PLAN_VERSION);
    let resources = plan.instruments.as_mut().unwrap();
    resources.dependencies.push(DependencyIdentity {
        source: "lib/studio.maac".into(),
        alias: "score".into(),
        path: "score.maac".into(),
        hash: format!("sha256:{}", "1".repeat(64)),
    });
    assert_eq!(plan.validate().unwrap_err().code, "E_REFERENCE");
}

#[test]
fn dependency_depth_uses_the_longest_path_regardless_of_lexical_order() {
    let boundary = plan_with_dependency_chain(maac::bundle::MAX_IMPORT_DEPTH);
    boundary
        .instruments
        .as_ref()
        .unwrap()
        .validate()
        .expect("standalone resources at the bundle depth limit are valid");
    boundary
        .validate()
        .expect("a chain at the bundle depth limit is valid");

    let too_deep = plan_with_dependency_chain(maac::bundle::MAX_IMPORT_DEPTH + 1);
    assert_eq!(
        too_deep
            .instruments
            .as_ref()
            .unwrap()
            .validate()
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    let error = too_deep
        .validate()
        .expect_err("a reversed lexical chain beyond the depth limit must fail");
    assert_eq!(error.code, "E_RESOURCE_LIMIT");
    assert_eq!(error.path, "instruments.dependencies");

    let encoded = serde_json::to_vec(&too_deep).unwrap();
    let error = Plan::from_json(&encoded).expect_err("plan loading must enforce dependency depth");
    assert_eq!(error.code, "E_RESOURCE_LIMIT");
    assert_eq!(error.path, "instruments.dependencies");
}

#[test]
fn dependency_depth_composes_a_memoized_shared_suffix() {
    let error = plan_with_reused_dependency_suffix()
        .validate()
        .expect_err("an unreachable deep prefix must compose with its reused suffix");
    assert_eq!(error.code, "E_RESOURCE_LIMIT");
    assert_eq!(error.path, "instruments.dependencies");
}

#[test]
fn standalone_library_metadata_uses_the_source_boundary_contract() {
    let boundary = "x".repeat(maac::library::MAX_LIBRARY_METADATA_BYTES);
    let mut accepted = base_plan(PLAN_VERSION);
    let metadata = &mut accepted.instruments.as_mut().unwrap().libraries[0];
    metadata.version = boundary.clone();
    metadata.creator = Some(String::new());
    metadata.license = Some(boundary);
    let encoded = accepted.to_json().expect("4096-byte metadata is valid");
    Plan::from_json(&encoded).expect("boundary metadata validates after import");

    let oversized = "x".repeat(maac::library::MAX_LIBRARY_METADATA_BYTES + 1);
    for field in ["version", "creator", "license"] {
        let mut rejected = base_plan(PLAN_VERSION);
        let metadata = &mut rejected.instruments.as_mut().unwrap().libraries[0];
        match field {
            "version" => metadata.version = oversized.clone(),
            "creator" => metadata.creator = Some(oversized.clone()),
            "license" => metadata.license = Some(oversized.clone()),
            _ => unreachable!(),
        }
        assert_eq!(
            rejected.validate().unwrap_err().code,
            "E_RESOURCE_LIMIT",
            "{field}"
        );
    }

    let mut empty_version = base_plan(PLAN_VERSION);
    empty_version.instruments.as_mut().unwrap().libraries[0]
        .version
        .clear();
    assert_eq!(empty_version.validate().unwrap_err().code, "E_RANGE");
}

#[test]
fn standalone_library_metadata_requires_a_maac_object_identifier() {
    let mut plan = base_plan(PLAN_VERSION);
    plan.instruments.as_mut().unwrap().libraries[0].object = "not.valid".into();
    let error = plan.validate().unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(error.path, "instruments.libraries[0].object");
}

#[test]
fn tightened_string_and_rational_profiles_cover_embedded_programs() {
    let plan = base_plan(PLAN_VERSION);
    let string_limits = maac::plan::PlanLimits {
        max_total_string_bytes: 8,
        ..Default::default()
    };
    assert_eq!(
        plan.validate_with_limits(&string_limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );

    let mut plan = base_plan(PLAN_VERSION);
    plan.instruments.as_mut().unwrap().programs[0]
        .controls
        .get_mut("release")
        .unwrap()
        .default = r(1 << 20, 1);
    let rational_limits = maac::plan::PlanLimits {
        max_rational_bits: 8,
        ..Default::default()
    };
    assert_eq!(
        plan.validate_with_limits(&rational_limits)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn resource_collection_limits_precede_deep_program_validation() {
    let mut plan = base_plan(PLAN_VERSION);
    plan.instruments.as_mut().unwrap().programs = (0..129)
        .map(|index| {
            let mut value = program();
            value.id = format!("program_{index}");
            value
        })
        .collect();
    assert_eq!(plan.validate().unwrap_err().code, "E_RESOURCE_LIMIT");

    let plan = base_plan(PLAN_VERSION);
    let limits = maac::plan::PlanLimits {
        max_instrument_controls: 0,
        ..Default::default()
    };
    assert_eq!(
        plan.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn graph_instrument_accepts_per_note_pitch_and_gain_expression() {
    let mut plan = base_plan(PLAN_VERSION);
    plan.validate().unwrap();
    let EventKind::Note {
        pitch_expression, ..
    } = &mut plan.events[0].kind
    else {
        unreachable!()
    };
    *pitch_expression = Some(maac::plan::PitchExpression {
        clock: maac::plan::PitchExpressionClock::Seconds,
        points: vec![maac::plan::PitchExpressionPoint {
            position: r(0, 1),
            cents: r(0, 1),
            shape: Interpolation::Step,
        }],
    });
    if let EventKind::Note {
        gain_expression, ..
    } = &mut plan.events[0].kind
    {
        *gain_expression = Some(maac::plan::GainExpression {
            clock: maac::plan::ExpressionClock::Seconds,
            points: vec![maac::plan::GainExpressionPoint {
                position: r(0, 1),
                gain: r(0, 1),
                shape: Interpolation::Step,
            }],
        });
    }
    plan.validate().unwrap();
    Plan::from_json(&plan.to_json().unwrap()).unwrap();
}

#[test]
fn graph_instrument_accepts_per_note_gain_expression() {
    let mut plan = base_plan(PLAN_VERSION);
    plan.validate().unwrap();
    let EventKind::Note {
        gain_expression, ..
    } = &mut plan.events[0].kind
    else {
        unreachable!()
    };
    *gain_expression = Some(maac::plan::GainExpression {
        clock: maac::plan::ExpressionClock::Seconds,
        points: vec![maac::plan::GainExpressionPoint {
            position: r(0, 1),
            gain: r(1, 1),
            shape: Interpolation::Step,
        }],
    });
    plan.validate().unwrap();
    Plan::from_json(&plan.to_json().unwrap()).unwrap();
}

#[test]
fn gain_work_has_exact_point_and_active_release_window_cost_even_when_silent() {
    use maac::plan::{ExpressionClock, GainExpression, GainExpressionPoint, PlanLimits};
    for (points, extra) in [(0, 0), (1, 17), (2, 18), (3, 19), (65536, 33)] {
        for automated in [false, true] {
            // The 65,536-point curve already consumes the aggregate point limit.
            if points == 65536 && automated {
                continue;
            }
            let mut plan = base_plan(PLAN_VERSION);
            if automated {
                plan.automation.push(Automation {
                    id: "long_release".into(),
                    target: PortRef::new("bell", "release").unwrap(),
                    clock: AutomationClock::Seconds,
                    at: r(0, 1),
                    points: vec![AutomationPoint {
                        position: r(0, 1),
                        value: r(2, 1),
                        shape: Interpolation::Step,
                    }],
                });
            }
            if let EventKind::Note {
                gain_expression,
                velocity,
                ..
            } = &mut plan.events[0].kind
            {
                *velocity = r(0, 1);
                if points != 0 {
                    *gain_expression = Some(GainExpression {
                        clock: ExpressionClock::Seconds,
                        points: (0..points)
                            .map(|i| GainExpressionPoint {
                                position: r(i, 48000),
                                gain: r(0, 1),
                                shape: Interpolation::Step,
                            })
                            .collect(),
                    });
                }
            }
            let active = if automated { 72000 } else { 16800 };
            let mut limits = PlanLimits {
                max_execution_work: active * (2 + extra),
                ..Default::default()
            };
            plan.validate_with_limits(&limits).unwrap();
            limits.max_execution_work -= 1;
            assert_eq!(
                plan.validate_with_limits(&limits).unwrap_err().code,
                "E_RESOURCE_LIMIT"
            );
        }
    }
}
