use maac::graph::{
    Control, ControlTarget, GraphNode, GraphProcessor, GraphProgram, GraphStage, InstrumentProgram,
    Modulation, ParameterTarget, ProgramSource,
};
use maac::instrument_plan::InstrumentResources;
use maac::plan::{
    Automation, AutomationClock, AutomationPoint, Connection, EventKind, EventTarget,
    Interpolation, Node, OutputSettings, Plan, PortRef, Processor, ResolvedEvent, SourceMapping,
    TempoMap, TempoPoint, PLAN_VERSION,
};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use maac::wavetable::{TableBank, Wavetable};
use std::collections::BTreeMap;
use std::sync::Arc;

const RATE: f64 = 48_000.0;

fn r(numerator: i64, denominator: i64) -> maac::Rational {
    maac::parse_rational(&format!("{numerator}/{denominator}")).unwrap()
}

fn oscillator_program() -> InstrumentProgram {
    InstrumentProgram {
        id: "plain".into(),
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
                    params: [("phase".into(), maac::parse_rational("1/4").unwrap())]
                        .into_iter()
                        .collect(),
                },
            ],
            connections: Vec::new(),
            modulations: Vec::new(),
            output: PortRef::new("osc", "out").unwrap(),
            amplitude: Some("amp".into()),
        },
        shared: None,
        controls: BTreeMap::new(),
        source: ProgramSource {
            file: "test.maac".into(),
            object: "plain".into(),
            span: None,
        },
    }
}

#[test]
fn overlapping_notes_have_independent_voice_state_and_sum_by_address() {
    let program = oscillator_program();
    program.validate().unwrap();
    let tables: BTreeMap<String, Arc<TableBank>> = BTreeMap::new();
    let compiled = CompiledInstrument::compile(&program, &tables).unwrap();
    let mut runtime =
        InstrumentRuntime::new(Arc::new(compiled), 2, RATE, &BTreeMap::new()).unwrap();

    runtime.note_on("voice/b", 480.0, 0.5, 0).unwrap();
    runtime.note_on("voice/a", 480.0, 0.25, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap(), &[0.75]);
}

fn runtime(program: &InstrumentProgram, capacity: u32) -> InstrumentRuntime {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new()).unwrap();
    InstrumentRuntime::new(Arc::new(compiled), capacity, RATE, &BTreeMap::new()).unwrap()
}

#[test]
fn release_tails_hold_capacity_until_zero_and_zero_release_frees_same_frame() {
    let mut tail_program = oscillator_program();
    tail_program.voice.nodes[0]
        .params
        .insert("release".into(), r(1, 24_000));
    let mut tail = runtime(&tail_program, 1);
    tail.note_on("held", 480.0, 1.0, 0).unwrap();
    let _ = tail.render(0).unwrap();
    tail.note_off("held", 1).unwrap();
    tail.prune_finished(1).unwrap();
    assert_eq!(tail.active_voice_count(), 1);
    assert_eq!(
        tail.note_on("overflow", 480.0, 1.0, 1).unwrap_err().code(),
        "E_VOICE_LIMIT"
    );
    tail.prune_finished(3).unwrap();
    assert_eq!(tail.active_voice_count(), 0);
    tail.note_on("replacement", 480.0, 1.0, 3).unwrap();

    let mut instant = runtime(&oscillator_program(), 1);
    instant.note_on("first", 480.0, 1.0, 0).unwrap();
    instant.note_off("first", 1).unwrap();
    instant.prune_finished(1).unwrap();
    instant.note_on("same_frame", 480.0, 1.0, 1).unwrap();
    assert_eq!(instant.active_voice_count(), 1);

    let mut silent_program = oscillator_program();
    silent_program.voice.nodes[0]
        .params
        .insert("sustain".into(), r(0, 1));
    silent_program.voice.nodes[0]
        .params
        .insert("release".into(), r(1, 24_000));
    let mut silent = runtime(&silent_program, 1);
    silent.note_on("silent", 480.0, 1.0, 0).unwrap();
    assert_eq!(silent.render(0).unwrap(), &[0.0]);
    silent.prune_finished(100).unwrap();
    assert_eq!(silent.active_voice_count(), 1);
    silent.note_off("silent", 100).unwrap();
    silent.prune_finished(100).unwrap();
    assert_eq!(silent.active_voice_count(), 0);
}

fn controlled_program() -> InstrumentProgram {
    let mut program = oscillator_program();
    for (name, node, parameter, default) in [
        ("attack", "amp", "attack", r(1, 24_000)),
        ("decay", "amp", "decay", r(1, 24_000)),
        ("sustain", "amp", "sustain", r(1, 4)),
        ("release", "amp", "release", r(1, 12_000)),
        ("phase", "osc", "phase", r(1, 4)),
        ("frequency", "osc", "frequency", r(-480, 1)),
        ("level", "osc", "level", r(1, 1)),
    ] {
        program.controls.insert(
            name.into(),
            Control {
                target: ControlTarget {
                    graph: GraphStage::Voice,
                    node: node.into(),
                    parameter: parameter.into(),
                },
                default,
            },
        );
    }
    program
}

#[test]
fn controls_capture_event_rates_apply_sample_rates_and_keep_instances_independent() {
    let program = controlled_program();
    let compiled = Arc::new(CompiledInstrument::compile(&program, &BTreeMap::new()).unwrap());
    let mut first = InstrumentRuntime::new(compiled.clone(), 2, RATE, &BTreeMap::new()).unwrap();
    first.note_on("held", 480.0, 1.0, 0).unwrap();

    let changed: BTreeMap<String, f64> = [
        ("attack".into(), 0.0),
        ("decay".into(), 0.0),
        ("sustain".into(), 1.0),
        ("phase".into(), 0.0),
        ("frequency".into(), -480.0),
        ("level".into(), 1.0),
        ("release".into(), 0.0),
    ]
    .into_iter()
    .collect();
    first.update_controls(&changed).unwrap();
    assert_eq!(first.render(0).unwrap(), &[0.0]);
    assert!((first.render(1).unwrap()[0] - 0.5).abs() < 1.0e-12);
    assert!((first.render(2).unwrap()[0] - 1.0).abs() < 1.0e-12);
    assert!((first.render(3).unwrap()[0] - 0.625).abs() < 1.0e-12);
    assert!((first.render(4).unwrap()[0] - 0.25).abs() < 1.0e-12);

    // Sample-rate level and frequency change immediately. The initial -480 Hz
    // offset held the 480 Hz note at zero; -960 Hz reverses it through zero.
    let mut reversed = changed.clone();
    reversed.insert("level".into(), 0.5);
    reversed.insert("frequency".into(), -960.0);
    reversed.insert("release".into(), 4.0 / RATE);
    first.update_controls(&reversed).unwrap();
    assert!((first.render(5).unwrap()[0] - 0.125).abs() < 1.0e-12);
    assert!(first.render(6).unwrap()[0] < 0.125);

    // Release is captured now; a later zero control cannot shorten this tail.
    first.note_off("held", 6).unwrap();
    first.update_controls(&changed).unwrap();
    first.prune_finished(8).unwrap();
    assert_eq!(first.active_voice_count(), 1);
    first.prune_finished(10).unwrap();
    assert_eq!(first.active_voice_count(), 0);

    let mut second = InstrumentRuntime::new(compiled.clone(), 1, RATE, &changed).unwrap();
    second.note_on("independent", 480.0, 1.0, 0).unwrap();
    assert_eq!(second.render(0).unwrap(), &[0.0]);
    assert!(Arc::strong_count(&compiled) >= 3);
}

fn connection(id: &str, from: &str, to: &str) -> Connection {
    Connection::new(
        id,
        PortRef::new(from, "out").unwrap(),
        PortRef::new(to, "in").unwrap(),
    )
    .unwrap()
}

#[test]
fn shared_palette_processes_filter_tails_without_active_voices_and_resets_exactly() {
    let mut program = oscillator_program();
    program.shared = Some(GraphProgram {
        channels: 2,
        nodes: vec![
            GraphNode {
                id: "gain".into(),
                processor: GraphProcessor::Gain { channels: 1 },
                params: [("level".into(), r(1, 2))].into_iter().collect(),
            },
            GraphNode {
                id: "lfo".into(),
                processor: GraphProcessor::Lfo,
                params: [("phase".into(), r(1, 4))].into_iter().collect(),
            },
            GraphNode {
                id: "filter".into(),
                processor: GraphProcessor::OnePole { channels: 1 },
                params: [("cutoff".into(), r(1_000, 1))].into_iter().collect(),
            },
            GraphNode {
                id: "mix".into(),
                processor: GraphProcessor::Mix { channels: 1 },
                params: BTreeMap::new(),
            },
            GraphNode {
                id: "pan".into(),
                processor: GraphProcessor::Pan,
                params: BTreeMap::new(),
            },
        ],
        connections: vec![
            connection("a_input", "input", "gain"),
            connection("b_gain", "gain", "filter"),
            connection("c_filter", "filter", "mix"),
            connection("d_mix", "mix", "pan"),
        ],
        modulations: vec![Modulation {
            id: "cutoff_lfo".into(),
            from: PortRef::new("lfo", "out").unwrap(),
            to: ParameterTarget {
                node: "filter".into(),
                parameter: "cutoff".into(),
            },
            depth: r(100, 1),
        }],
        output: PortRef::new("pan", "out").unwrap(),
        amplitude: None,
    });
    let mut runtime = runtime(&program, 1);
    runtime.note_on("note", 480.0, 1.0, 0).unwrap();
    let first = runtime.render(0).unwrap().to_vec();
    assert_eq!(first.len(), 2);
    assert!(first[0] > 0.0);
    assert!((first[0] - first[1]).abs() < 1.0e-15);

    runtime.note_off("note", 1).unwrap();
    runtime.prune_finished(1).unwrap();
    assert_eq!(runtime.active_voice_count(), 0);
    let tail = runtime.render(1).unwrap();
    assert!(tail[0] > 0.0 && tail[0] < first[0]);
    assert!((tail[0] - tail[1]).abs() < 1.0e-15);

    runtime.reset(&BTreeMap::new()).unwrap();
    runtime.note_on("note", 480.0, 1.0, 0).unwrap();
    let repeated = runtime.render(0).unwrap();
    assert_eq!(repeated[0].to_bits(), first[0].to_bits());
    assert_eq!(repeated[1].to_bits(), first[1].to_bits());
}

#[test]
fn graph_dispatches_every_basic_waveform_and_reuses_prebuilt_wavetable_banks() {
    for waveform in [
        GraphProcessor::Sine,
        GraphProcessor::Saw,
        GraphProcessor::Square,
        GraphProcessor::Triangle,
    ] {
        let mut program = oscillator_program();
        program.voice.nodes[1].processor = waveform;
        let mut runtime = runtime(&program, 1);
        runtime.note_on("note", 6_000.0, 1.0, 0).unwrap();
        assert!(runtime.render(0).unwrap()[0].is_finite());
    }

    let cycle: Vec<f64> = (0..8)
        .map(|index| (std::f64::consts::TAU * index as f64 / 8.0).sin())
        .collect();
    let samples = cycle
        .iter()
        .copied()
        .chain(cycle.iter().map(|sample| -*sample))
        .collect();
    let table = Wavetable {
        id: "colors".into(),
        cycle_length: 8,
        samples,
    };
    let bank = Arc::new(TableBank::new(&table).unwrap());
    let tables = [("colors".into(), bank.clone())].into_iter().collect();
    let mut program = oscillator_program();
    program.voice.nodes[1].processor = GraphProcessor::Wavetable {
        table: "colors".into(),
    };
    program.voice.nodes[1]
        .params
        .insert("position".into(), r(1, 4));
    program.voice.nodes[1]
        .params
        .insert("level".into(), r(2, 1));
    let compiled = Arc::new(CompiledInstrument::compile(&program, &tables).unwrap());
    assert_eq!(Arc::strong_count(&bank), 3);
    let mut runtime = InstrumentRuntime::new(compiled, 1, RATE, &BTreeMap::new()).unwrap();
    runtime.note_on("wavetable", 480.0, 1.0, 0).unwrap();
    assert!((runtime.render(0).unwrap()[0] - 1.0).abs() < 1.0e-12);
}

fn note(address: &str, target: &str, on: u64, off: u64) -> ResolvedEvent {
    ResolvedEvent {
        address: address.into(),
        source: SourceMapping {
            object: address.replace('/', "_"),
            path: address.split('/').map(str::to_owned).collect(),
            span: None,
        },
        target: EventTarget::new(target, "events").unwrap(),
        kind: EventKind::Note {
            pitch_expression: None,
            pitch_hz: 12_000.0,
            velocity: r(1, 1),
        },
        score_on_q: r(on as i64, 24_000),
        score_off_q: Some(r(off as i64, 24_000)),
        onset_offset_seconds: r(0, 1),
        release_offset_seconds: r(0, 1),
        on_seconds: r(on as i64, 48_000),
        off_seconds: Some(r(off as i64, 48_000)),
        release_velocity: 0.0,
        on_frame: on,
        off_frame: Some(off),
        order: 0,
    }
}

fn instrument_plan(program: InstrumentProgram, frames: u64) -> Plan {
    Plan {
        version: PLAN_VERSION,
        output: OutputSettings {
            score_start_q: r(0, 1),
            score_end_q: r(frames as i64, 24_000),
            tail_seconds: r(0, 1),
            sample_rate_hz: 48_000,
            channels: program.channels(),
            total_frames: frames,
            output: PortRef::new("instrument", "out").unwrap(),
        },
        tempo: TempoMap {
            points: vec![TempoPoint {
                q: r(0, 1),
                bpm: r(120, 1),
                shape: Interpolation::Step,
            }],
        },
        events: Vec::new(),
        nodes: vec![Node {
            id: "instrument".into(),
            processor: Processor::Instrument {
                program: program.id.clone(),
                voices: 2,
                channels: program.channels(),
            },
            params: BTreeMap::new(),
        }],
        connections: Vec::new(),
        automation: Vec::new(),
        regions: Vec::new(),
        source_mappings: Vec::new(),
        production: None,
        instruments: Some(InstrumentResources {
            entry_source: "test.maac".into(),
            programs: vec![program],
            wavetables: Vec::new(),
            wavetable_sources: Vec::new(),
            source_files: vec![SourceIdentity {
                path: "test.maac".into(),
                hash: format!("sha256:{}", "a".repeat(64)),
            }],
            dependencies: Vec::new(),
            libraries: Vec::new(),
        }),
    }
}

fn collect(engine: &mut DspEngine<'_>) -> Vec<Vec<f64>> {
    let mut output = Vec::new();
    engine
        .render(|frame| {
            output.push(frame.to_vec());
            Ok(())
        })
        .unwrap();
    output
}

#[test]
fn dsp_engine_runs_overlapping_instrument_events_and_repeats_after_reset() {
    let mut plan = instrument_plan(oscillator_program(), 5);
    plan.events = vec![
        note("voice/b", "instrument", 1, 4),
        note("voice/a", "instrument", 0, 3),
    ];
    plan.validate().unwrap();

    let mut engine = DspEngine::new(&plan).unwrap();
    let first = collect(&mut engine);
    let second = collect(&mut engine);
    assert_eq!(first, second);
    let expected = [1.0, 1.0, -1.0, -1.0, 0.0];
    for (frame, expected) in first.iter().zip(expected) {
        assert!((frame[0] - expected).abs() < 1.0e-12);
    }
}

#[test]
fn dsp_automation_is_sampled_at_note_on_and_note_off_for_public_controls() {
    let mut program = oscillator_program();
    program.voice.nodes[1]
        .params
        .insert("frequency".into(), r(-12_000, 1));
    for (name, parameter, default) in [
        ("attack", "attack", r(1, 24_000)),
        ("release", "release", r(1, 24_000)),
    ] {
        program.controls.insert(
            name.into(),
            Control {
                target: ControlTarget {
                    graph: GraphStage::Voice,
                    node: "amp".into(),
                    parameter: parameter.into(),
                },
                default,
            },
        );
    }
    let mut plan = instrument_plan(program, 8);
    plan.events = vec![
        note("voice/a", "instrument", 0, 2),
        note("voice/b", "instrument", 1, 3),
    ];
    plan.automation = vec![
        Automation {
            id: "attack_capture".into(),
            target: PortRef::new("instrument", "attack").unwrap(),
            clock: AutomationClock::Seconds,
            at: r(0, 1),
            points: vec![
                AutomationPoint {
                    position: r(0, 1),
                    value: r(1, 24_000),
                    shape: Interpolation::Step,
                },
                AutomationPoint {
                    position: r(1, 48_000),
                    value: r(0, 1),
                    shape: Interpolation::Step,
                },
            ],
        },
        Automation {
            id: "release_capture".into(),
            target: PortRef::new("instrument", "release").unwrap(),
            clock: AutomationClock::Seconds,
            at: r(0, 1),
            points: vec![
                AutomationPoint {
                    position: r(0, 1),
                    value: r(1, 24_000),
                    shape: Interpolation::Step,
                },
                AutomationPoint {
                    position: r(2, 48_000),
                    value: r(1, 12_000),
                    shape: Interpolation::Step,
                },
                AutomationPoint {
                    position: r(3, 48_000),
                    value: r(0, 1),
                    shape: Interpolation::Step,
                },
            ],
        },
    ];
    plan.validate().unwrap();
    let output = collect(&mut DspEngine::new(&plan).unwrap());
    let expected = [0.0, 1.5, 2.0, 0.75, 0.5, 0.25, 0.0, 0.0];
    for (frame, expected) in output.iter().zip(expected) {
        assert!((frame[0] - expected).abs() < 1.0e-12);
    }
}

#[test]
fn modulation_and_public_controls_reject_out_of_range_results_without_clipping() {
    let mut program = oscillator_program();
    program.voice.nodes.push(GraphNode {
        id: "lfo".into(),
        processor: GraphProcessor::Lfo,
        params: [("frequency".into(), r(0, 1)), ("phase".into(), r(1, 4))]
            .into_iter()
            .collect(),
    });
    program.voice.modulations.push(Modulation {
        id: "too_high".into(),
        from: PortRef::new("lfo", "out").unwrap(),
        to: ParameterTarget {
            node: "osc".into(),
            parameter: "frequency".into(),
        },
        depth: r(24_000, 1),
    });
    let mut runtime = runtime(&program, 1);
    runtime.note_on("note", 480.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap_err().code(), "E_NONFINITE");

    let controlled = controlled_program();
    let compiled = Arc::new(CompiledInstrument::compile(&controlled, &BTreeMap::new()).unwrap());
    let mut controls = BTreeMap::new();
    controls.insert("level".into(), 17.0);
    assert_eq!(
        InstrumentRuntime::new(compiled, 1, RATE, &controls)
            .unwrap_err()
            .code(),
        "E_NONFINITE"
    );
}

#[test]
fn voice_filters_and_modulators_have_independent_per_note_state() {
    let mut program = oscillator_program();
    program.voice.nodes[1]
        .params
        .insert("frequency".into(), r(-480, 1));
    program.voice.nodes.push(GraphNode {
        id: "filter".into(),
        processor: GraphProcessor::OnePole { channels: 1 },
        params: BTreeMap::new(),
    });
    program.voice.nodes.push(GraphNode {
        id: "lfo".into(),
        processor: GraphProcessor::Lfo,
        params: [("frequency".into(), r(0, 1)), ("phase".into(), r(1, 4))]
            .into_iter()
            .collect(),
    });
    program
        .voice
        .connections
        .push(connection("osc_filter", "osc", "filter"));
    program.voice.modulations.push(Modulation {
        id: "lfo_frequency".into(),
        from: PortRef::new("lfo", "out").unwrap(),
        to: ParameterTarget {
            node: "osc".into(),
            parameter: "frequency".into(),
        },
        depth: r(100, 1),
    });
    program.voice.output = PortRef::new("filter", "out").unwrap();

    let compiled = Arc::new(CompiledInstrument::compile(&program, &BTreeMap::new()).unwrap());
    let mut single = InstrumentRuntime::new(compiled.clone(), 1, RATE, &BTreeMap::new()).unwrap();
    single.note_on("only", 480.0, 1.0, 0).unwrap();
    let single_first = single.render(0).unwrap()[0];
    let single_second = single.render(1).unwrap()[0];

    let mut double = InstrumentRuntime::new(compiled, 2, RATE, &BTreeMap::new()).unwrap();
    double.note_on("b", 480.0, 1.0, 0).unwrap();
    double.note_on("a", 480.0, 1.0, 0).unwrap();
    assert!((double.render(0).unwrap()[0] - 2.0 * single_first).abs() < 1.0e-15);
    assert!((double.render(1).unwrap()[0] - 2.0 * single_second).abs() < 1.0e-15);
}

#[test]
fn dsp_reclaims_zero_release_before_same_frame_note_on_and_counts_live_tails() {
    let mut instant = instrument_plan(oscillator_program(), 3);
    if let Processor::Instrument { voices, .. } = &mut instant.nodes[0].processor {
        *voices = 1;
    }
    instant.events = vec![
        note("voice/a", "instrument", 0, 1),
        note("voice/b", "instrument", 1, 2),
    ];
    let output = collect(&mut DspEngine::new(&instant).unwrap());
    assert!((output[0][0] - 1.0).abs() < 1.0e-12);
    assert!((output[1][0] - 1.0).abs() < 1.0e-12);

    let mut tail_program = oscillator_program();
    tail_program.voice.nodes[0]
        .params
        .insert("release".into(), r(1, 24_000));
    let mut tailed = instrument_plan(tail_program, 3);
    if let Processor::Instrument { voices, .. } = &mut tailed.nodes[0].processor {
        *voices = 1;
    }
    tailed.events = instant.events;
    let error = DspEngine::new(&tailed)
        .unwrap()
        .render(|_| Ok(()))
        .unwrap_err();
    assert_eq!(error.code(), "E_VOICE_LIMIT");
}
use maac::bundle::SourceIdentity;
use maac::dsp::DspEngine;
