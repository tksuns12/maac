//! `synth.bow/1`: source and wire strictness, voice rendering, the work
//! charge and the string storage limit. The string, bow and body kernels'
//! own tests are in src/bow.rs.

use maac::bow::BowBody;
use maac::bundle::SourceBundle;
use maac::graph::{parameter_descriptor, GraphProcessor, GraphUnit, InstrumentProgram};
use maac::library::LibrarySet;
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

const RATE: f64 = 48_000.0;

/// A voice that is one bowed section.
fn library(node: &str) -> Result<LibrarySet, maac::Diagnostics> {
    let source = format!(
        r#"
maac 1;
library orchestra {{ version = "1.0.0"; }}
instrument bowed {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &section:out;
  node amp {{ type = "synth.adsr/1"; params = {{ sustain = 1; release = 300ms; }}; }}
  node section {{ type = "synth.bow/1"; {node} }}
 }}
}}
"#
    );
    LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve()?)
}

#[test]
fn source_configuration_and_parameters_are_strict() {
    for node in [
        "",
        "config = { players = 16; body = bass; seed = 4294967295; };",
        "config = { body = none; };",
        "config = { players = 1; body = cello; channels = 1; seed = 1; }; params = { ratio = 1/8; dynamics = 0; position = 1/10; vibrato = 0; rate = 3Hz; attack = 5ms; timing = 0s; spread = 0; width = 0; pan = -1; level = 0; };",
        "params = { ratio = 8; dynamics = 1; position = 1/7; vibrato = 50; rate = 8Hz; attack = 500ms; timing = 40ms; spread = 1; width = 2; pan = 1; level = 16; };",
    ] {
        library(node).unwrap_or_else(|error| panic!("{node}: {error:?}"));
    }
    for node in [
        "config = { players = 0; };",
        "config = { players = 17; };",
        "config = { body = guitar; };",
        "config = { body = \"violin\"; };",
        "config = { channels = 3; };",
        "config = { seed = 0; };",
        "config = { modes = 8; };",
        "params = { ratio = 0; };",
        "params = { dynamics = 2; };",
        "params = { position = 1/16; };",
        "params = { position = 1/5; };",
        "params = { vibrato = 51; };",
        "params = { vibrato = 12ct; };",
        "params = { rate = 9Hz; };",
        "params = { rate = 5; };",
        "params = { attack = 1ms; };",
        "params = { timing = 41ms; };",
        "params = { spread = 2; };",
        "params = { pan = 2; };",
        "params = { cutoff = 1000Hz; };",
    ] {
        assert!(library(node).is_err(), "{node}");
    }
}

#[test]
fn descriptors_give_every_parameter_its_default_and_range() {
    let processor = GraphProcessor::Bow {
        players: 8,
        body: BowBody::Violin,
        channels: 2,
        seed: 1,
    };
    for (name, unit, default, min, max) in [
        ("ratio", GraphUnit::Dimensionless, "1", "1/8", "8"),
        ("dynamics", GraphUnit::Dimensionless, "1/2", "0", "1"),
        ("position", GraphUnit::Dimensionless, "1/8", "1/10", "1/7"),
        ("vibrato", GraphUnit::Dimensionless, "25/2", "0", "50"),
        ("rate", GraphUnit::Hertz, "28/5", "3", "8"),
        ("attack", GraphUnit::Seconds, "2/25", "1/200", "1/2"),
        ("timing", GraphUnit::Seconds, "9/500", "0", "1/25"),
        ("spread", GraphUnit::Dimensionless, "1", "0", "1"),
        ("width", GraphUnit::Dimensionless, "7/10", "0", "2"),
        ("pan", GraphUnit::Dimensionless, "0", "-1", "1"),
        ("level", GraphUnit::Dimensionless, "1", "0", "16"),
    ] {
        let spec = parameter_descriptor(&processor, name).unwrap();
        assert_eq!(spec.unit, unit, "{name}");
        assert_eq!(spec.default.to_string(), default, "{name}");
        assert_eq!(spec.min.to_string(), min, "{name}");
        assert_eq!(spec.max.to_string(), max, "{name}");
    }
    assert!(parameter_descriptor(&processor, "cutoff").is_none());
}

#[test]
fn the_wire_form_is_strict_and_round_trips() {
    let processor = GraphProcessor::Bow {
        players: 8,
        body: BowBody::Viola,
        channels: 2,
        seed: 3,
    };
    assert_eq!(processor.identity(), "synth.bow/1");
    let wire = serde_json::json!({
        "kind": "synth.bow/1", "players": 8, "body": "viola", "channels": 2, "seed": 3
    });
    assert_eq!(serde_json::to_value(&processor).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<GraphProcessor>(wire).unwrap(),
        processor
    );
    for wire in [
        serde_json::json!({"kind": "synth.bow/1", "players": 8, "body": "viola", "channels": 2}),
        serde_json::json!({"kind": "synth.bow/1", "players": 8, "body": "harp", "channels": 2, "seed": 3}),
        serde_json::json!({"kind": "synth.bow/1", "players": 8, "body": "viola", "channels": 2, "seed": 0}),
        serde_json::json!({"kind": "synth.bow/1", "players": 8, "body": "viola", "channels": 2, "seed": 3, "modes": 1}),
        serde_json::json!({"kind": "synth.bow/2", "players": 8, "body": "viola", "channels": 2, "seed": 3}),
    ] {
        assert!(serde_json::from_value::<GraphProcessor>(wire).is_err());
    }
    // The plan validator applies the same bounds as the source.
    for (players, channels) in [(0, 1), (17, 1), (1, 0), (1, 3)] {
        let mut program = program("");
        let node = program
            .voice
            .nodes
            .iter_mut()
            .find(|node| node.id == "section")
            .unwrap();
        node.processor = GraphProcessor::Bow {
            players,
            body: BowBody::Violin,
            channels,
            seed: 1,
        };
        assert!(program.validate().is_err(), "{players} {channels}");
    }
}

#[test]
fn a_bowed_section_is_voice_only() {
    let source = r#"
maac 1;
library orchestra { version = "1.0.0"; }
instrument bowed {
 channels = 1;
 voice v { channels = 1; amplitude = &amp; output = &tone:out;
  node amp { type = "synth.adsr/1"; }
  node tone { type = "synth.sine/1"; }
 }
 shared s { channels = 1; output = &section:out;
  node section { type = "synth.bow/1"; }
 }
}
"#;
    assert!(
        LibrarySet::resolve(&SourceBundle::new("main.maac", source).resolve().unwrap()).is_err()
    );
}

fn program(node: &str) -> InstrumentProgram {
    library(node)
        .unwrap_or_else(|error| panic!("{node}: {error:?}"))
        .programs
        .remove(0)
}

/// The first `frames` frames of one note at `pitch_hz` and `velocity`.
fn render(program: &InstrumentProgram, pitch_hz: f64, frames: u64) -> maac::dsp::Result<Vec<f64>> {
    let compiled = CompiledInstrument::compile(program, &BTreeMap::new())?;
    let mut runtime = InstrumentRuntime::new(Arc::new(compiled), 1, RATE, &BTreeMap::new())?;
    runtime.note_on("note", pitch_hz, 1.0, 0)?;
    (0..frames)
        .map(|frame| runtime.render(frame).map(|output| output[0]))
        .collect()
}

/// The frequency of the strongest component near `around`, by correlation
/// over `samples`.
fn peak_near(samples: &[f64], around: f64) -> f64 {
    let power = |frequency: f64| {
        let (mut re, mut im) = (0.0, 0.0);
        for (index, sample) in samples.iter().enumerate() {
            let phase = std::f64::consts::TAU * frequency * index as f64 / RATE;
            re += sample * phase.cos();
            im += sample * phase.sin();
        }
        re * re + im * im
    };
    let mut best = (around, 0.0);
    let mut step = around * 0.02;
    let mut centre = around;
    for _ in 0..5 {
        for k in -10..=10 {
            let frequency = centre + step * k as f64 / 10.0;
            let value = power(frequency);
            if value > best.1 {
                best = (frequency, value);
            }
        }
        centre = best.0;
        step /= 10.0;
    }
    best.0
}

#[test]
fn a_held_note_speaks_at_its_pitch_after_the_attack() {
    let program = program(
        "config = { body = none; }; params = { vibrato = 0; spread = 0; timing = 0s; level = 1/10; };",
    );
    let samples = render(&program, 523.25, 36_000).unwrap();
    assert!(samples.iter().all(|sample| sample.is_finite()));
    // The bow starts from rest: the first frame is silent.
    assert_eq!(samples[0], 0.0);
    let measured = peak_near(&samples[24_000..], 523.25);
    let cents = 1200.0 * (measured / 523.25).log2();
    assert!(cents.abs() < 3.0, "{cents} cents");
}

#[test]
fn identical_sections_render_identically_and_a_seed_changes_them() {
    let program_a = program("config = { players = 4; seed = 7; };");
    let program_b = program("config = { players = 4; seed = 8; };");
    let a = render(&program_a, 392.0, 12_000).unwrap();
    let again = render(&program_a, 392.0, 12_000).unwrap();
    let b = render(&program_b, 392.0, 12_000).unwrap();
    assert!(a
        .iter()
        .zip(&again)
        .all(|(x, y)| x.to_bits() == y.to_bits()));
    assert!(a.iter().zip(&b).any(|(x, y)| x != y));
}

fn work_source(nodes: &str, output: &str, voices: u32) -> SourceBundle {
    SourceBundle::new(
        "score.maac",
        format!(
            r#"maac 1;
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument tone {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &{output}:out;
  node amp {{ type = "synth.adsr/1"; }}
  {nodes}
 }}
}}
node sound {{ instrument = &tone; config = {{ voices = {voices}; }}; }}
track t {{ target = &sound:events; }}
pattern one {{ length = 1q; note a {{ at = 0q; dur = 1q; pitch = A4; }} }}
place p1 {{ pattern = &one; track = &t; at = 0q; }}
"#
        ),
    )
}

fn minimum_budget(nodes: &str, output: &str) -> u64 {
    use maac::plan::PlanLimits;
    let limits = |max_execution_work| PlanLimits {
        max_execution_work,
        ..PlanLimits::song()
    };
    let artifact = maac::compiler::compile_bundle_artifact_with_limits(
        &work_source(nodes, output, 1),
        &PlanLimits::song(),
    )
    .unwrap();
    let (mut low, mut high) = (0, PlanLimits::song().max_execution_work);
    while low < high {
        let middle = low + (high - low) / 2;
        if artifact.validate_with_limits(&limits(middle)).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}

#[test]
fn each_player_is_charged_for_its_string_and_body() {
    let sine = r#"node osc { type = "synth.sine/1"; }"#;
    let base = minimum_budget(sine, "osc");
    // A gain and its connection add two units.
    let gain = minimum_budget(
        &format!(
            r#"{sine} node extra {{ type = "synth.gain/1"; config = {{ channels = 1; }}; }}
  connect into {{ from = &osc:out; to = &extra:in; }}"#
        ),
        "extra",
    ) - base;
    assert!(gain > 0 && gain.is_multiple_of(2));
    let unit = gain / 2;
    for (players, body, charge) in [
        (1, "none", 23),
        (1, "violin", 33),
        (8, "violin", 264),
        (3, "bass", 99),
    ] {
        assert_eq!(
            maac::graph::bow_sample_work(players, BowBody::parse(body).unwrap()),
            charge
        );
        // The section replaces the one-unit sine.
        let measured = minimum_budget(
            &format!(
                r#"node section {{ type = "synth.bow/1"; config = {{ players = {players}; body = {body}; }}; }}"#
            ),
            "section",
        );
        assert_eq!(measured - base, unit * (charge - 1), "{players} {body}");
    }
}

#[test]
fn string_storage_is_bounded_per_instance() {
    let source = |voices| {
        work_source(
            r#"node section { type = "synth.bow/1"; config = { players = 16; }; }"#,
            "section",
            voices,
        )
    };
    // 128 voices of 16 players need 16,777,216 cells: at the limit.
    let limits = maac::plan::PlanLimits::song();
    maac::compiler::compile_bundle_artifact_with_limits(&source(128), &limits).unwrap();
    let error =
        maac::compiler::compile_bundle_artifact_with_limits(&source(129), &limits).unwrap_err();
    assert!(
        format!("{error:?}").contains("bowed string storage exceeds"),
        "{error:?}"
    );
}

/// A song whose envelope's release may be moved by a bowed section's output
/// at note-off: `{modulate}` is written inside the voice.
fn release_source(modulate: &str) -> SourceBundle {
    SourceBundle::new(
        "score.maac",
        format!(
            r#"maac 1;
project p {{ score = [0q, 2q]; rate = 48000Hz; tail = 1s; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument tone {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; params = {{ sustain = 1; release = 300ms; }}; }}
  node osc {{ type = "synth.sine/1"; }}
  node section {{ type = "synth.bow/1"; }}
  {modulate}
 }}
}}
node sound {{ instrument = &tone; config = {{ voices = 1; }}; }}
track t {{ target = &sound:events; }}
pattern one {{ length = 1q; note a {{ at = 0q; dur = 1q; pitch = A4; }} }}
place p1 {{ pattern = &one; track = &t; at = 0q; }}
"#
        ),
    )
}

#[test]
fn a_bowed_section_may_move_a_release_at_note_off() {
    // The note-off capture evaluates the bow with all eleven of its
    // parameters; it once had room for seven, and panicked.
    use maac::plan::PlanLimits;
    let render = |modulate: &str| {
        let artifact = maac::compiler::compile_bundle_artifact_with_limits(
            &release_source(modulate),
            &PlanLimits::song(),
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        let mut out = Vec::new();
        maac::render_artifact_with_limits(&artifact, &PlanLimits::song(), |frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
        out
    };
    let fixed = render("");
    let moved =
        render("modulate rel { from = &section:out; to = &amp.params.release; depth = 100ms; }");
    assert_eq!(fixed.len(), moved.len());
    assert!(moved.iter().all(|sample| sample.is_finite()));
    // Up to the note-off the two are the same; the release then differs.
    let note_off = 48_000;
    assert_eq!(fixed[..note_off], moved[..note_off]);
    assert_ne!(fixed[note_off..], moved[note_off..]);
}
