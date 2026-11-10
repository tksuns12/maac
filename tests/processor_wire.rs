//! The plan bytes of every `synth.*/1` processor. A plan records each graph
//! node's processor as JSON, and an earlier plan must replay byte for byte,
//! so these strings are pinned exactly: field names, their order, and which
//! fields are left out when they hold their defaults.

use maac::graph::GraphProcessor;

/// Each pair is a processor as a plan may hold it, and the bytes it is
/// written back as. Most are the same; the rest show a default the writer
/// leaves out, or one the reader supplies.
const WIRE: &[(&str, &str)] = &[
    same(r#"{"kind":"synth.sine/1"}"#),
    same(r#"{"kind":"synth.saw/1"}"#),
    same(r#"{"kind":"synth.square/1"}"#),
    same(r#"{"kind":"synth.triangle/1"}"#),
    same(r#"{"kind":"synth.noise/1","seed":1}"#),
    same(r#"{"kind":"synth.noise/1","seed":7,"per_note":true}"#),
    (
        r#"{"kind":"synth.noise/1","seed":7,"per_note":false}"#,
        r#"{"kind":"synth.noise/1","seed":7}"#,
    ),
    same(r#"{"kind":"synth.pluck/1","seed":5}"#),
    same(r#"{"kind":"synth.random/1","seed":9}"#),
    same(r#"{"kind":"synth.modal/1","modes":16,"strings":3}"#),
    same(r#"{"kind":"synth.bow/1","players":1,"body":"violin","channels":1,"seed":1}"#),
    same(r#"{"kind":"synth.bow/1","players":8,"body":"none","channels":2,"seed":3}"#),
    same(r#"{"kind":"synth.wavetable/1","table":"colors"}"#),
    same(r#"{"kind":"synth.sample/1","zones":[{"sample":"tone","low_key":0,"high_key":127}]}"#),
    same(
        r#"{"kind":"synth.sample/1","channels":2,"zones":[{"sample":"tone","low_key":0,"high_key":60,"velocity":{"low":"0/1","high":"1/2"},"key_fade":{"low":"1/1","high":"2/1"},"velocity_fade":{"low":"0/1","high":"1/10"}}],"fade_shape":"equal_power"}"#,
    ),
    (
        r#"{"kind":"synth.sample/1","channels":1,"zones":[{"sample":"tone","low_key":0,"high_key":127}],"fade_shape":"linear"}"#,
        r#"{"kind":"synth.sample/1","zones":[{"sample":"tone","low_key":0,"high_key":127}]}"#,
    ),
    same(r#"{"kind":"synth.adsr/1"}"#),
    same(r#"{"kind":"synth.adsr/1","on_join":"restart"}"#),
    (
        r#"{"kind":"synth.adsr/1","on_join":"hold"}"#,
        r#"{"kind":"synth.adsr/1"}"#,
    ),
    same(r#"{"kind":"synth.timbre/1"}"#),
    same(r#"{"kind":"synth.pressure/1"}"#),
    same(r#"{"kind":"synth.velocity/1"}"#),
    same(r#"{"kind":"synth.key/1"}"#),
    same(r#"{"kind":"synth.lfo/1"}"#),
    same(r#"{"kind":"synth.gain/1","channels":2}"#),
    same(r#"{"kind":"synth.gain/1","channels":1}"#),
    same(r#"{"kind":"synth.onepole/1","channels":2}"#),
    same(r#"{"kind":"synth.onepole/1","channels":1}"#),
    same(r#"{"kind":"synth.highpass/1","channels":2}"#),
    same(r#"{"kind":"synth.highpass/1","channels":1}"#),
    same(r#"{"kind":"synth.svf/1","channels":1,"mode":"lowpass"}"#),
    same(r#"{"kind":"synth.svf/1","channels":2,"mode":"bandpass"}"#),
    same(r#"{"kind":"synth.drive/1","channels":2}"#),
    same(r#"{"kind":"synth.mix/1","channels":2}"#),
    same(r#"{"kind":"synth.mix/1","channels":1}"#),
    same(r#"{"kind":"synth.pan/1"}"#),
    same(r#"{"kind":"synth.balance/1"}"#),
    same(r#"{"kind":"synth.width/1"}"#),
    same(r#"{"kind":"synth.delay/1","channels":1,"clock":"seconds","max":"1/2"}"#),
    same(r#"{"kind":"synth.delay/1","channels":2,"clock":"score","max":"4/1"}"#),
];

/// Processors a plan may not hold: a required field missing, a field no
/// processor has, or a seed of 0.
const REFUSED: &[&str] = &[
    r#"{"kind":"synth.gain/1"}"#,
    r#"{"kind":"synth.svf/1","channels":1}"#,
    r#"{"kind":"synth.delay/1","channels":1,"max":"1/2"}"#,
    r#"{"kind":"synth.bow/1","players":1,"body":"violin","channels":1}"#,
    r#"{"kind":"synth.sine/1","channels":1}"#,
    r#"{"kind":"synth.timbre/1","seed":1}"#,
    r#"{"kind":"synth.noise/1","seed":0}"#,
    r#"{"kind":"synth.random/1","seed":0}"#,
];

const fn same(wire: &str) -> (&str, &str) {
    (wire, wire)
}

#[test]
fn every_processor_is_written_back_as_pinned() {
    for (read, written) in WIRE {
        let processor: GraphProcessor =
            serde_json::from_str(read).unwrap_or_else(|error| panic!("{read}: {error}"));
        assert_eq!(
            &serde_json::to_string(&processor).unwrap(),
            written,
            "{read}"
        );
        // The written form reads back as the same processor.
        let again: GraphProcessor = serde_json::from_str(written).unwrap();
        assert_eq!(again, processor, "{written}");
    }
}

#[test]
fn a_plan_may_not_hold_an_incomplete_or_unknown_processor() {
    for wire in REFUSED {
        assert!(
            serde_json::from_str::<GraphProcessor>(wire).is_err(),
            "{wire} was accepted"
        );
    }
}

#[test]
fn every_processor_is_pinned() {
    let mut kinds: Vec<&str> = WIRE
        .iter()
        .map(|(read, _)| {
            let processor: GraphProcessor = serde_json::from_str(read).unwrap();
            processor.identity()
        })
        .collect();
    kinds.dedup();
    assert_eq!(kinds.len(), 27, "{kinds:?}");
}
