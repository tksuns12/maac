use maac::bundle::SourceBundle;
use maac::dsp::DspEngine;
use maac::graph::{GraphProcessor, InstrumentProgram};
use maac::library::LibrarySet;
use maac::plan::Plan;
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use maac::wavetable::{TableBank, Wavetable};
use std::collections::BTreeMap;
use std::sync::Arc;

#[path = "support/frames.rs"]
mod frames;
use frames::FrameProject;

const RATE: f64 = 48_000.0;

/// The source of instrument `plain`: an oscillator `osc` at a quarter turn
/// of phase, so a note that does not move starts at 1, under an envelope
/// `amp`.
struct Instrument {
    channels: u8,
    /// The `params` of `amp`, as fields.
    amp: String,
    /// The type of `osc`, and its `params` after the phase.
    oscillator: String,
    osc: String,
    /// The node the voice renders.
    output: String,
    /// Further children of the voice, and of the instrument.
    voice: String,
    more: String,
}

impl Default for Instrument {
    fn default() -> Self {
        Self {
            channels: 1,
            amp: String::new(),
            oscillator: "synth.sine/1".to_owned(),
            osc: String::new(),
            output: "osc".to_owned(),
            voice: String::new(),
            more: String::new(),
        }
    }
}

impl Instrument {
    fn source(&self) -> String {
        let Self {
            channels,
            amp,
            oscillator,
            osc,
            output,
            voice,
            more,
        } = self;
        format!(
            r#"instrument plain {{
 channels = {channels};
 voice voice {{
  channels = 1; amplitude = &amp; output = &{output}:out;
  node amp {{ type = "synth.adsr/1"; params = {{ {amp} }}; }}
  node osc {{ type = "{oscillator}"; params = {{ phase = 1/4; {osc} }}; }}
  {voice}
 }}
 {more}
}}"#
        )
    }

    /// The program the library resolves the instrument to.
    fn program(&self) -> InstrumentProgram {
        let source = format!(
            "maac 1;\nlibrary studio {{ version = \"1.0.0\"; }}\n{}",
            self.source()
        );
        let bundle = SourceBundle::new("main.maac", source.as_str())
            .resolve()
            .unwrap_or_else(|diagnostics| panic!("{diagnostics:?}\n{source}"));
        LibrarySet::resolve(&bundle)
            .unwrap_or_else(|diagnostics| panic!("{diagnostics:?}\n{source}"))
            .programs
            .remove(0)
    }

    /// A project of `frames` frames whose node `instrument` plays the
    /// instrument with `voices` voices.
    fn project(&self, frames: u64, voices: u32) -> FrameProject {
        FrameProject::instrument(frames, voices, "plain", &self.source())
    }
}

fn oscillator_program() -> InstrumentProgram {
    Instrument::default().program()
}

/// The instrument with a control for each parameter of `amp` and `osc`.
fn controlled_program() -> InstrumentProgram {
    Instrument {
        more: r#"control attack { target = &voice.amp.params.attack; default = 1/24000s; }
 control decay { target = &voice.amp.params.decay; default = 1/24000s; }
 control sustain { target = &voice.amp.params.sustain; default = 1/4; }
 control release { target = &voice.amp.params.release; default = 1/12000s; }
 control phase { target = &voice.osc.params.phase; default = 1/4; }
 control frequency { target = &voice.osc.params.frequency; default = -480Hz; }
 control level { target = &voice.osc.params.level; default = 1; }"#
            .to_owned(),
        ..Instrument::default()
    }
    .program()
}

/// An LFO held at 1, and a modulation `id` that moves the frequency of `osc`
/// by `depth` of it.
fn held_lfo(id: &str, depth: &str) -> String {
    format!(
        "node lfo {{ type = \"synth.lfo/1\"; params = {{ frequency = 0Hz; phase = 1/4; }}; }}\n\
         modulate {id} {{ from = &lfo:out; to = &osc.params.frequency; depth = {depth}; }}"
    )
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
    let tail_program = Instrument {
        amp: "release = 1/24000s;".to_owned(),
        ..Instrument::default()
    }
    .program();
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

    let silent_program = Instrument {
        amp: "sustain = 0; release = 1/24000s;".to_owned(),
        ..Instrument::default()
    }
    .program();
    let mut silent = runtime(&silent_program, 1);
    silent.note_on("silent", 480.0, 1.0, 0).unwrap();
    assert_eq!(silent.render(0).unwrap(), &[0.0]);
    silent.prune_finished(100).unwrap();
    assert_eq!(silent.active_voice_count(), 1);
    silent.note_off("silent", 100).unwrap();
    silent.prune_finished(100).unwrap();
    assert_eq!(silent.active_voice_count(), 0);
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

#[test]
fn shared_palette_processes_filter_tails_without_active_voices_and_resets_exactly() {
    let program = Instrument {
        channels: 2,
        more: r#"shared palette {
  channels = 2; output = &pan:out;
  node gain { type = "synth.gain/1"; config = { channels = 1; }; params = { level = 1/2; }; }
  node lfo { type = "synth.lfo/1"; params = { phase = 1/4; }; }
  node filter { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 1000Hz; }; }
  node mix { type = "synth.mix/1"; config = { channels = 1; }; }
  node pan { type = "synth.pan/1"; }
  connect a_input { from = &input:out; to = &gain:in; }
  connect b_gain { from = &gain:out; to = &filter:in; }
  connect c_filter { from = &filter:out; to = &mix:in; }
  connect d_mix { from = &mix:out; to = &pan:in; }
  modulate cutoff_lfo { from = &lfo:out; to = &filter.params.cutoff; depth = 100Hz; }
 }"#
        .to_owned(),
        ..Instrument::default()
    }
    .program();
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
        "synth.sine/1",
        "synth.saw/1",
        "synth.square/1",
        "synth.triangle/1",
    ] {
        let program = Instrument {
            oscillator: waveform.to_owned(),
            ..Instrument::default()
        }
        .program();
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
    // The bank is built here, not read from an asset, so the oscillator of a
    // resolved program is pointed at it.
    let mut program = Instrument {
        osc: "level = 2;".to_owned(),
        ..Instrument::default()
    }
    .program();
    let osc = program
        .voice
        .nodes
        .iter_mut()
        .find(|node| node.id == "osc")
        .unwrap();
    osc.processor = GraphProcessor::Wavetable {
        table: "colors".into(),
    };
    osc.params
        .insert("position".into(), maac::parse_rational("1/4").unwrap());
    let compiled = Arc::new(CompiledInstrument::compile(&program, &tables).unwrap());
    assert_eq!(Arc::strong_count(&bank), 3);
    let mut runtime = InstrumentRuntime::new(compiled, 1, RATE, &BTreeMap::new()).unwrap();
    runtime.note_on("wavetable", 480.0, 1.0, 0).unwrap();
    assert!((runtime.render(0).unwrap()[0] - 1.0).abs() < 1.0e-12);
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
    let plan = Instrument::default()
        .project(5, 2)
        .note("b", 1, 4, "12000Hz", "")
        .note("a", 0, 3, "12000Hz", "")
        .plan();
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
    let plan = Instrument {
        osc: "frequency = -12000Hz;".to_owned(),
        more: r#"control attack { target = &voice.amp.params.attack; default = 1/24000s; }
 control release { target = &voice.amp.params.release; default = 1/24000s; }"#
            .to_owned(),
        ..Instrument::default()
    }
    .project(8, 2)
    .note("a", 0, 2, "12000Hz", "")
    .note("b", 1, 3, "12000Hz", "")
    .with(
        r#"curve attack_curve { clock = seconds; points = [(0s, 1/24000s, step), (1/48000s, 0s, step)]; }
automation attack_capture { target = &instrument.params.attack; curve = &attack_curve; at = 0s; }
curve release_curve { clock = seconds; points = [(0s, 1/24000s, step), (2/48000s, 1/12000s, step), (3/48000s, 0s, step)]; }
automation release_capture { target = &instrument.params.release; curve = &release_curve; at = 0s; }"#,
    )
    .plan();
    plan.validate().unwrap();
    let output = collect(&mut DspEngine::new(&plan).unwrap());
    let expected = [0.0, 1.5, 2.0, 0.75, 0.5, 0.25, 0.0, 0.0];
    for (frame, expected) in output.iter().zip(expected) {
        assert!((frame[0] - expected).abs() < 1.0e-12);
    }
}

#[test]
fn modulation_and_public_controls_reject_out_of_range_results_without_clipping() {
    let program = Instrument {
        voice: held_lfo("too_high", "24000Hz"),
        ..Instrument::default()
    }
    .program();
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
    let program = Instrument {
        osc: "frequency = -480Hz;".to_owned(),
        output: "filter".to_owned(),
        voice: format!(
            "node filter {{ type = \"synth.onepole/1\"; config = {{ channels = 1; }}; }}\n\
             connect osc_filter {{ from = &osc:out; to = &filter:in; }}\n{}",
            held_lfo("lfo_frequency", "100Hz")
        ),
        ..Instrument::default()
    }
    .program();

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

/// Two notes, the second starting on the frame the first ends.
fn back_to_back(instrument: &Instrument) -> Plan {
    instrument
        .project(3, 1)
        .note("a", 0, 1, "12000Hz", "")
        .note("b", 1, 2, "12000Hz", "")
        .plan()
}

#[test]
fn dsp_reclaims_zero_release_before_same_frame_note_on_and_counts_live_tails() {
    let instant = back_to_back(&Instrument::default());
    let output = collect(&mut DspEngine::new(&instant).unwrap());
    assert!((output[0][0] - 1.0).abs() < 1.0e-12);
    assert!((output[1][0] - 1.0).abs() < 1.0e-12);

    let tailed = back_to_back(&Instrument {
        amp: "release = 1/24000s;".to_owned(),
        ..Instrument::default()
    });
    let error = DspEngine::new(&tailed)
        .unwrap()
        .render(|_| Ok(()))
        .unwrap_err();
    assert_eq!(error.code(), "E_VOICE_LIMIT");
}
