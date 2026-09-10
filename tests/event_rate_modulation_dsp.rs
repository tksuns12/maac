use maac::dsp::{DspEngine, RenderError};
use maac::graph::{
    Control, ControlTarget, GraphNode, GraphProcessor, GraphProgram, GraphStage, InstrumentProgram,
    ProgramSource,
};
use maac::instrument_plan::InstrumentResources;
use maac::{bundle::sha256_digest, PlanArtifact, Rational};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const RATE: u64 = 48_000;

fn rational_frame(frame: u64) -> String {
    let value = Rational::new(frame.into(), RATE.into());
    format!("{}/{}", value.numer(), value.denom())
}

fn note(address: &str, on: u64, off: u64, target: &str, pitch_hz: f64) -> Value {
    json!({
        "address": address,
        "source": {"object": address, "path": [address]},
        "target": {"node": target, "port": "events"},
        "kind": {"kind": "note", "pitch_hz": pitch_hz, "velocity": "1/1"},
        "score_on_q": rational_frame(on),
        "score_off_q": rational_frame(off),
        "onset_offset_seconds": "0/1",
        "release_offset_seconds": "0/1",
        "release_velocity": 0.5,
        "on_frame": on,
        "off_frame": off
    })
}

fn constant(id: &str, value: &str) -> Value {
    json!({
        "id": id,
        "processor": {"kind": "constant"},
        "params": {"value": value}
    })
}

fn modulation(id: &str, from: &str, target: &str, parameter: &str, amount: &str) -> Value {
    json!({
        "id": id,
        "from": {"node": from, "port": "out"},
        "target": {"node": target, "port": parameter},
        "amount": amount
    })
}

fn score_automation(id: &str, node: &str, points: &[(u64, &str)]) -> Value {
    json!({
        "id": id,
        "target": {"node": node, "port": "value"},
        "clock": "score",
        "at": {"kind": "score", "q": "0/1"},
        "points": points
            .iter()
            .map(|(frame, value)| {
                json!({"position": rational_frame(*frame), "value": *value, "shape": "step"})
            })
            .collect::<Vec<_>>()
    })
}

fn core_fixture(total_frames: u64, voices: u32) -> Value {
    json!({
        "version": 7,
        "output": {
            "score_start_q": "0/1",
            "score_end_q": rational_frame(total_frames),
            "tail_seconds": "0/1",
            "sample_rate_hz": RATE,
            "channels": 1,
            "total_frames": total_frames,
            "output": {"node": "sine", "port": "out"}
        },
        "tempo": {"points": [{"q": "0/1", "bpm": "60/1", "shape": "step"}]},
        "events": [],
        "nodes": [{
            "id": "sine",
            "processor": {"kind": "core", "processor": {"kind": "sine", "voices": voices}},
            "params": {"attack": "0/1", "release": "0/1", "level": "1/1"}
        }],
        "audio_assets": [],
        "connections": [],
        "modulations": [],
        "automation": []
    })
}

fn artifact(value: &Value) -> PlanArtifact {
    PlanArtifact::from_json(&serde_json::to_vec(value).unwrap()).unwrap()
}

fn render(value: &Value) -> Result<Vec<f64>, RenderError> {
    let plan = artifact(value);
    let mut engine = DspEngine::new_artifact(&plan)?;
    let mut samples = Vec::new();
    engine.render(|frame| {
        samples.push(frame[0]);
        Ok(())
    })?;
    Ok(samples)
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1.0e-12,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn core_sine_attack_modulation_is_captured_at_note_on() {
    let mut value = core_fixture(5, 2);
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("attack_source", "1/1"));
    value["automation"] = json!([score_automation(
        "attack_lane",
        "attack_source",
        &[(0, "1/1"), (2, "0/1")],
    )]);
    value["modulations"] = json!([modulation(
        "attack_mod",
        "attack_source",
        "sine",
        "attack",
        "1/12000",
    )]);
    value["events"] = json!([
        note("first", 0, 5, "sine", 12_000.0),
        note("second", 2, 5, "sine", 12_000.0)
    ]);

    let samples = render(&value).unwrap();
    assert_eq!(samples.len(), 5);
    // The first note captures a four-frame attack. The source changes to zero
    // at frame two, so the second note starts with an immediate attack while
    // the first note keeps its original slope.
    assert_close(samples[0], 0.0);
    assert_close(samples[1], 0.25);
    assert_close(samples[2], 0.0);
    // Each core voice has an independent oscillator phase, so the second
    // voice contributes +1 at frame three while the first contributes -0.75.
    assert_close(samples[3], 0.25);
    assert_close(samples[4], 0.0);

    let plan = artifact(&value);
    let mut engine = DspEngine::new_artifact(&plan).unwrap();
    let mut first = Vec::new();
    engine
        .render(|frame| {
            first.push(frame[0]);
            Ok(())
        })
        .unwrap();
    let mut second = Vec::new();
    engine
        .render(|frame| {
            second.push(frame[0]);
            Ok(())
        })
        .unwrap();
    assert_eq!(first, second);
}

#[test]
fn core_sine_release_modulation_is_captured_at_note_off() {
    let mut value = core_fixture(6, 1);
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("release_source", "1/1"));
    value["automation"] = json!([score_automation(
        "release_lane",
        "release_source",
        &[(0, "1/1"), (2, "2/1"), (3, "0/1")],
    )]);
    value["modulations"] = json!([modulation(
        "release_mod",
        "release_source",
        "sine",
        "release",
        "1/48000",
    )]);
    value["events"] = json!([note("released", 0, 2, "sine", 12_000.0)]);

    let samples = render(&value).unwrap();
    assert_eq!(samples.len(), 6);
    // The source is one frame at onset, two frames at note-off, and zero
    // afterward. Release must retain the two-frame value captured at off.
    assert_close(samples[0], 0.0);
    assert_close(samples[1], 1.0);
    assert_close(samples[2], 0.0);
    assert_close(samples[3], -0.5);
    assert_close(samples[4], 0.0);
    assert_close(samples[5], 0.0);
}

#[test]
fn event_rate_baseline_and_signed_modulations_are_id_ordered() {
    let mut value = core_fixture(4, 1);
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("source", "1/1"));
    value["automation"] = json!([{
        "id": "attack_baseline",
        "target": {"node": "sine", "port": "attack"},
        "clock": "score",
        "at": {"kind": "score", "q": "0/1"},
        "points": [{"position": "0/1", "value": "1/48000", "shape": "step"}]
    }]);
    // The sorted reduction is baseline + huge + (-huge) + two frames. If the
    // declaration order were used, the final small contribution would be
    // swallowed by the large intermediate value and the attack would be zero.
    value["modulations"] = json!([
        modulation("c_small", "source", "sine", "attack", "1/24000"),
        modulation("b_negative", "source", "sine", "attack", "-1099511627776/1"),
        modulation("a_positive", "source", "sine", "attack", "1099511627776/1")
    ]);
    value["events"] = json!([note("n", 0, 4, "sine", 12_000.0)]);

    let expected = render(&value).unwrap();
    assert_close(expected[1], 0.5);

    value["modulations"].as_array_mut().unwrap().reverse();
    assert_eq!(render(&value).unwrap(), expected);
}

#[test]
fn invalid_event_rate_values_fail_during_render_and_before_callbacks() {
    let mut range = core_fixture(4, 1);
    range["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("source", "1/1"));
    range["automation"] = json!([score_automation(
        "bad_lane",
        "source",
        &[(0, "0/1"), (2, "-1/1")],
    )]);
    range["modulations"] = json!([modulation(
        "bad_attack",
        "source",
        "sine",
        "attack",
        "1/48000",
    )]);
    range["events"] = json!([note("n", 0, 4, "sine", 12_000.0)]);
    let plan = artifact(&range);
    let mut engine = DspEngine::new_artifact(&plan).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_RANGE");
    assert_eq!(callbacks, 2);

    let huge = format!("1{}/1", "0".repeat(308));
    let mut nonfinite = core_fixture(1, 1);
    nonfinite["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("source", &huge));
    nonfinite["modulations"] = json!([modulation("overflow", "source", "sine", "attack", "2/1",)]);
    let plan = artifact(&nonfinite);
    let mut engine = DspEngine::new_artifact(&plan).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_NONFINITE");
    assert_eq!(callbacks, 0);
}

fn instrument_resources() -> InstrumentResources {
    let rational = |value: &str| maac::parse_rational(value).unwrap();
    let voice = GraphProgram {
        channels: 1,
        nodes: vec![
            GraphNode {
                id: "amp".into(),
                processor: GraphProcessor::Adsr,
                params: BTreeMap::from([
                    ("attack".into(), rational("0/1")),
                    ("decay".into(), rational("0/1")),
                    ("sustain".into(), rational("1/1")),
                    ("release".into(), rational("0/1")),
                ]),
            },
            GraphNode {
                id: "osc".into(),
                processor: GraphProcessor::Sine,
                params: BTreeMap::from([("phase".into(), rational("0/1"))]),
            },
        ],
        connections: Vec::new(),
        modulations: Vec::new(),
        output: maac::plan::PortRef::new("osc", "out").unwrap(),
        amplitude: Some("amp".into()),
    };
    let controls = BTreeMap::from([
        (
            "attack".into(),
            Control {
                target: ControlTarget {
                    graph: GraphStage::Voice,
                    node: "amp".into(),
                    parameter: "attack".into(),
                },
                default: rational("0/1"),
            },
        ),
        (
            "release".into(),
            Control {
                target: ControlTarget {
                    graph: GraphStage::Voice,
                    node: "amp".into(),
                    parameter: "release".into(),
                },
                default: rational("0/1"),
            },
        ),
        (
            "phase".into(),
            Control {
                target: ControlTarget {
                    graph: GraphStage::Voice,
                    node: "osc".into(),
                    parameter: "phase".into(),
                },
                default: rational("0/1"),
            },
        ),
    ]);
    let source = b"event-rate modulation instrument fixture";
    InstrumentResources {
        entry_source: "fixture.maac".into(),
        programs: vec![InstrumentProgram {
            id: "program".into(),
            voice,
            shared: None,
            controls,
            source: ProgramSource {
                file: "fixture.maac".into(),
                object: "program".into(),
                span: None,
            },
        }],
        wavetables: Vec::new(),
        wavetable_sources: Vec::new(),
        source_files: vec![maac::bundle::SourceIdentity {
            path: "fixture.maac".into(),
            hash: sha256_digest(source),
        }],
        dependencies: Vec::new(),
        libraries: Vec::new(),
    }
}

fn instrument_fixture(total_frames: u64, voices: u32) -> Value {
    let mut value = core_fixture(total_frames, voices);
    value["output"]["output"] = json!({"node": "instrument", "port": "out"});
    value["nodes"] = json!([{
        "id": "instrument",
        "processor": {"kind": "core", "processor": {
            "kind": "instrument", "program": "program", "voices": voices, "channels": 1
        }},
        "params": {"attack": "0/1", "release": "0/1", "phase": "0/1"}
    }]);
    value["instruments"] = serde_json::to_value(instrument_resources()).unwrap();
    value
}

#[test]
fn instrument_public_release_modulation_is_captured_at_note_off() {
    let mut value = instrument_fixture(6, 1);
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("release_source", "0/1"));
    value["automation"] = json!([score_automation(
        "release_lane",
        "release_source",
        &[(0, "0/1"), (2, "1/1"), (3, "0/1")],
    )]);
    value["modulations"] = json!([modulation(
        "release",
        "release_source",
        "instrument",
        "release",
        "1/24000",
    )]);
    value["events"] = json!([note("released", 0, 2, "instrument", 12_000.0)]);

    let samples = render(&value).unwrap();
    assert_eq!(samples.len(), 6);
    assert_close(samples[0], 0.0);
    assert_close(samples[1], 1.0);
    assert_close(samples[2], 0.0);
    assert_close(samples[3], -0.5);
    assert_close(samples[4], 0.0);
    assert_close(samples[5], 0.0);
}

#[test]
fn instrument_public_event_rates_capture_per_voice_and_release_controls_capacity() {
    let mut value = instrument_fixture(5, 2);
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("attack_source", "1/1"));
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("phase_source", "0/1"));
    value["automation"] = json!([
        score_automation("attack_lane", "attack_source", &[(0, "1/1"), (2, "0/1")]),
        score_automation("phase_lane", "phase_source", &[(0, "0/1"), (2, "1/4")])
    ]);
    value["modulations"] = json!([
        modulation("attack", "attack_source", "instrument", "attack", "1/12000"),
        modulation("phase", "phase_source", "instrument", "phase", "1/1")
    ]);
    value["events"] = json!([
        note("first", 0, 5, "instrument", 12_000.0),
        note("second", 2, 5, "instrument", 12_000.0)
    ]);
    let samples = render(&value).unwrap();
    assert_close(samples[0], 0.0);
    assert_close(samples[1], 0.25);
    assert_close(samples[2], 1.0);
    assert_close(samples[3], -0.75);
    assert_close(samples[4], -1.0);

    let mut capacity = instrument_fixture(4, 1);
    capacity["nodes"]
        .as_array_mut()
        .unwrap()
        .push(constant("release_source", "0/1"));
    capacity["automation"] = json!([score_automation(
        "release_lane",
        "release_source",
        &[(0, "0/1"), (2, "1/48000")],
    )]);
    capacity["modulations"] = json!([modulation(
        "release",
        "release_source",
        "instrument",
        "release",
        "1/1",
    )]);
    capacity["events"] = json!([
        note("old", 0, 2, "instrument", 12_000.0),
        note("new", 2, 4, "instrument", 12_000.0)
    ]);
    let plan = artifact(&capacity);
    let mut engine = DspEngine::new_artifact(&plan).unwrap();
    let mut callbacks = 0;
    let error = engine
        .render(|_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_VOICE_LIMIT");
    assert_eq!(callbacks, 2);

    capacity["automation"] = json!([score_automation(
        "release_lane",
        "release_source",
        &[(0, "0/1"), (2, "0/1")],
    )]);
    assert_eq!(render(&capacity).unwrap().len(), 4);
}
