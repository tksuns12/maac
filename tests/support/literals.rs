//! Hand-built plans and programs, for the tests whose subject is the plan or
//! the program itself: what a malformed one, or one of an older version,
//! does. A test of a sound writes MaaC source instead (`frames.rs`,
//! `programs.rs`).

#![allow(dead_code)]

use std::collections::BTreeMap;

use maac::bundle::SourceIdentity;
use maac::graph::{GraphNode, GraphProcessor, GraphProgram, InstrumentProgram, ProgramSource};
use maac::instrument_plan::InstrumentResources;
use maac::plan::{
    Connection, EventKind, EventTarget, Interpolation, Node, OutputSettings, Plan, PortRef,
    Processor, Rational, Region, ResolvedEvent, SourceMapping, TempoMap, TempoPoint,
};

pub fn rat(n: i64, d: i64) -> Rational {
    maac::parse_rational(&format!("{n}/{d}")).expect("test rational")
}

/// A version 1 plan of a 4q score at 120 bpm: one note of 1q into the node
/// `sine`, which the node `sum` renders, and one region over the score.
pub fn base_plan() -> Plan {
    Plan {
        version: 1,
        output: OutputSettings {
            score_start_q: rat(0, 1),
            score_end_q: rat(4, 1),
            tail_seconds: rat(0, 1),
            sample_rate_hz: 48_000,
            channels: 1,
            total_frames: 96_000,
            output: PortRef::new("sum", "out").unwrap(),
        },
        tempo: TempoMap {
            points: vec![TempoPoint {
                q: rat(0, 1),
                bpm: rat(120, 1),
                shape: Interpolation::Step,
            }],
        },
        events: vec![ResolvedEvent {
            address: "main/0/n1".into(),
            source: SourceMapping {
                object: "n1".into(),
                path: vec!["main".into(), "n1".into()],
                span: None,
            },
            target: EventTarget::new("sine", "events").unwrap(),
            kind: EventKind::Note {
                timbre_expression: None,
                pressure_expression: None,
                pitch_expression: None,
                gain_expression: None,
                pitch_hz: 440.0,
                velocity: rat(1, 1),
            },
            score_on_q: rat(0, 1),
            score_off_q: Some(rat(1, 1)),
            onset_offset_seconds: rat(0, 1),
            release_offset_seconds: rat(0, 1),
            on_seconds: rat(0, 1),
            off_seconds: Some(rat(1, 2)),
            release_velocity: 0.0,
            on_frame: 0,
            off_frame: Some(24_000),
            order: 0,
            join: None,
        }],
        nodes: vec![
            Node::new("sine", Processor::sine(16)).unwrap(),
            Node::new("sum", Processor::sum(1)).unwrap(),
        ],
        connections: vec![Connection::new(
            "sine_to_sum",
            PortRef::new("sine", "out").unwrap(),
            PortRef::new("sum", "in").unwrap(),
        )
        .unwrap()],
        automation: Vec::new(),
        regions: vec![Region {
            id: "whole".into(),
            start_q: rat(0, 1),
            end_q: rat(4, 1),
            label: None,
        }],
        source_mappings: Vec::new(),
        instruments: None,
        production: None,
    }
}

/// The voice of a plain instrument: an oscillator `osc` under the envelope
/// `amp`, each with no parameter set.
pub fn sine_voice() -> GraphProgram {
    let node = |id: &str, processor| GraphNode {
        id: id.into(),
        processor,
        params: BTreeMap::new(),
    };
    GraphProgram {
        channels: 1,
        nodes: vec![
            node("amp", GraphProcessor::ADSR),
            node("osc", GraphProcessor::Sine),
        ],
        connections: Vec::new(),
        modulations: Vec::new(),
        output: PortRef::new("osc", "out").unwrap(),
        amplitude: Some("amp".into()),
        connected: false,
    }
}

/// The program `id` of a plain instrument, declared as `object` in `file`:
/// `sine_voice`, with no shared graph and no control.
pub fn sine_program(id: &str, file: &str, object: &str) -> InstrumentProgram {
    InstrumentProgram {
        id: id.into(),
        voice: sine_voice(),
        shared: None,
        controls: BTreeMap::new(),
        source: ProgramSource {
            file: file.into(),
            object: object.into(),
            span: None,
        },
    }
}

/// The resources of a plan whose one source file, `score.maac`, declares
/// `programs` and nothing else.
pub fn resources(programs: Vec<InstrumentProgram>) -> InstrumentResources {
    InstrumentResources {
        entry_source: "score.maac".into(),
        kits: Vec::new(),
        effects: Vec::new(),
        programs,
        wavetables: Vec::new(),
        wavetable_sources: Vec::new(),
        samples: Vec::new(),
        sample_sources: Vec::new(),
        source_files: vec![SourceIdentity {
            path: "score.maac".into(),
            hash: format!("sha256:{}", "1".repeat(64)),
        }],
        dependencies: Vec::new(),
        libraries: Vec::new(),
    }
}
