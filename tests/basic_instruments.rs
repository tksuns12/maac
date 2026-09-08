//! Behavioral coverage for the complete versioned basic instrument palette.
//! Catalog ranges drive the exhaustive short-gate matrix; each export also crosses
//! source compilation, default release, serialized-plan rendering and PCM16 export.

use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle;
use maac::graph::InstrumentProgram;
use maac::library::LibrarySet;
use maac::stdlib::InstrumentInfo;
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use num_traits::ToPrimitive;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

const RATE: f64 = 48_000.0;

struct Instrument {
    info: InstrumentInfo,
    program: InstrumentProgram,
    compiled: Arc<CompiledInstrument>,
}

fn instruments() -> &'static [Instrument] {
    static INSTRUMENTS: OnceLock<Vec<Instrument>> = OnceLock::new();
    INSTRUMENTS.get_or_init(|| {
        let catalog = maac::stdlib::catalog().expect("complete validated basic catalog");
        assert_eq!(catalog.instruments.len(), 24);
        let bundle = SourceBundle::new("matrix.maac", r#"maac 1; library matrix { version = "1"; } import basic { builtin = "std/basic/1.0.0"; }"#);
        let library = LibrarySet::resolve(&bundle.resolve().unwrap()).unwrap();
        assert_eq!(library.programs.len(), 24);
        catalog.instruments.into_iter().map(|info| {
            let program = library.programs.iter().find(|p| p.source.object == info.name).unwrap().clone();
            let compiled = Arc::new(CompiledInstrument::compile(&program, &BTreeMap::new()).unwrap());
            Instrument { info, program, compiled }
        }).collect()
    })
}

fn runtime(
    instrument: &Instrument,
    controls: &BTreeMap<String, f64>,
    capacity: u32,
) -> InstrumentRuntime {
    InstrumentRuntime::new(instrument.compiled.clone(), capacity, RATE, controls).unwrap()
}

fn hz(midi: u8) -> f64 {
    440.0 * 2.0_f64.powf((f64::from(midi) - 69.0) / 12.0)
}

fn midpoint(info: &InstrumentInfo) -> u8 {
    info.guidance.midi_min + (info.guidance.midi_max - info.guidance.midi_min) / 2
}

fn pitch(midi: u8) -> String {
    format!(
        "{}{}",
        ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"][usize::from(midi % 12)],
        i16::from(midi / 12) - 1
    )
}

fn direct(
    instrument: &Instrument,
    midi: u8,
    velocity: f64,
    controls: &BTreeMap<String, f64>,
    gate: u64,
    frames: u64,
) -> Vec<[f64; 2]> {
    let mut runtime = runtime(instrument, controls, 1);
    runtime.note_on("note", hz(midi), velocity, 0).unwrap();
    (0..frames)
        .map(|frame| {
            if frame == gate {
                runtime.note_off("note", frame).unwrap();
            }
            runtime.prune_finished(frame).unwrap();
            runtime.render(frame).unwrap().try_into().unwrap()
        })
        .collect()
}

fn energy(samples: &[[f64; 2]]) -> f64 {
    samples.iter().flatten().map(|sample| sample * sample).sum()
}

#[test]
fn every_catalog_semitone_and_velocity_is_finite_playable_and_safe() {
    let start = Instant::now();
    let mut pitches = 0;
    let mut frames = 0;
    for instrument in instruments() {
        assert_eq!(instrument.info.channels, 2);
        for midi in instrument.info.guidance.midi_min..=instrument.info.guidance.midi_max {
            pitches += 1;
            for velocity in [0.0, 0.35, 1.0] {
                let samples = direct(instrument, midi, velocity, &BTreeMap::new(), 512, 576);
                frames += samples.len();
                let mut peak = 0.0_f64;
                for &sample in samples.iter().flatten() {
                    assert!(
                        sample.is_finite(),
                        "{} MIDI {midi} velocity {velocity}",
                        instrument.info.name
                    );
                    peak = peak.max(sample.abs());
                }
                assert!(
                    peak < 1.0,
                    "{} MIDI {midi} velocity {velocity}: peak {peak}",
                    instrument.info.name
                );
                if velocity == 0.0 {
                    assert_eq!(peak, 0.0);
                } else {
                    assert!(
                        peak > 1e-12,
                        "{} MIDI {midi} velocity {velocity} is silent",
                        instrument.info.name
                    );
                }
            }
        }
    }
    eprintln!("basic range matrix: 24 exports, {pitches} MIDI pitches × 3 velocities, {frames} stereo frames, {:?}", start.elapsed());
}

fn composition(info: &InstrumentInfo) -> String {
    let low = pitch(info.guidance.midi_min);
    let mid = pitch(midpoint(info));
    let high = pitch(info.guidance.midi_max);
    let release = &info.controls["release"].default;
    format!(
        r#"maac 1;
import basic {{ builtin = "std/basic/1.0.0"; }}
project matrix {{ score = [0q, 1/2q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; tail = {release}s; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node sound {{ instrument = &basic.{name}; config = {{ voices = 4; }}; }}
pattern phrase {{ length = 1/2q;
note low {{ at = 0q; dur = 1/4q; pitch = {low}; velocity = 1; }}
note mid {{ at = 0q; dur = 1/4q; pitch = {mid}; velocity = 1; }}
note high {{ at = 0q; dur = 1/4q; pitch = {high}; velocity = 1; }}
note silent {{ at = 0q; dur = 1/4q; pitch = {mid}; velocity = 0; }}
}}
track melody {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &melody; at = 0q; }}
"#,
        name = info.name
    )
}

fn collect(engine: &mut maac::dsp::DspEngine<'_>) -> Vec<[u64; 2]> {
    let mut samples = Vec::new();
    engine
        .render(|frame| {
            assert_eq!(frame.len(), 2);
            assert!(
                frame
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() < 1.0),
                "unsafe frame {frame:?}"
            );
            samples.push([frame[0].to_bits(), frame[1].to_bits()]);
            Ok(())
        })
        .unwrap();
    samples
}

#[test]
fn every_export_defaults_overlap_release_plan_roundtrip_and_pcm16_are_deterministic() {
    let start = Instant::now();
    let mut frames = 0;
    for instrument in instruments() {
        let name = &instrument.info.name;
        let plan = compile_bundle(&SourceBundle::new(
            "matrix.maac",
            composition(&instrument.info),
        ))
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(
            plan.nodes
                .iter()
                .find(|node| node.id == "sound")
                .unwrap()
                .params
                .iter()
                .all(|(control, value)| value == &instrument.info.controls[control].default),
            "{name}: default instance controls changed"
        );
        let loaded = maac::load_plan(&plan.to_json().unwrap()).unwrap();
        let mut engine = maac::dsp::DspEngine::new(&plan).unwrap();
        let expected = collect(&mut engine);
        frames += expected.len();
        assert!(
            expected
                .iter()
                .flatten()
                .any(|bits| f64::from_bits(*bits).abs() > 1e-12),
            "{name} silent"
        );
        assert!(
            expected
                .last()
                .unwrap()
                .iter()
                .all(|bits| f64::from_bits(*bits).abs() < 1e-12),
            "{name} release tail did not finish"
        );
        assert_eq!(expected, collect(&mut engine), "{name} repeat differs");
        assert_eq!(
            expected,
            collect(&mut maac::dsp::DspEngine::new(&loaded).unwrap()),
            "{name} JSON plan differs"
        );
        let mut pcm = Cursor::new(Vec::new());
        let stats = maac::export::write_wav(&mut pcm, &loaded, maac::export::WavFormat::Pcm16)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(stats.frames, expected.len() as u64);
        assert_eq!(stats.channels, 2);
        let wav = hound::WavReader::new(Cursor::new(pcm.into_inner())).unwrap();
        assert_eq!(wav.duration(), stats.frames as u32);
        eprintln!(
            "basic boundary {name}: {} frames × 4 renders",
            expected.len()
        );
    }
    eprintln!("basic source/JSON/PCM16 matrix: 24 exports, {frames} unique stereo frames × 4 renders, {:?}", start.elapsed());
}

#[test]
fn every_export_common_controls_change_the_authored_sound() {
    for instrument in instruments() {
        let name = &instrument.info.name;
        let midi = midpoint(&instrument.info);
        let baseline = direct(instrument, midi, 1.0, &BTreeMap::new(), 2048, 2048);
        let default_level = instrument.info.controls["level"].default.to_f64().unwrap();
        let half = direct(
            instrument,
            midi,
            1.0,
            &[("level".into(), default_level * 0.5)]
                .into_iter()
                .collect(),
            2048,
            2048,
        );
        for (&base, &scaled) in baseline.iter().flatten().zip(half.iter().flatten()) {
            assert_eq!(base * 0.5, scaled, "{name} level scaling");
        }
        let zero = direct(
            instrument,
            midi,
            1.0,
            &[("level".into(), 0.0)].into_iter().collect(),
            2048,
            2048,
        );
        assert_eq!(energy(&zero), 0.0, "{name} zero level");
        let left = direct(
            instrument,
            midi,
            1.0,
            &[("pan".into(), -1.0)].into_iter().collect(),
            2048,
            2048,
        );
        assert!(
            left.iter().all(|sample| sample[1].abs() < 1e-15),
            "{name} left pan leaks right"
        );
        assert!(
            left.iter().any(|sample| sample[0].abs() > 1e-12),
            "{name} left pan silent"
        );
        let brightness = instrument.info.controls["brightness"]
            .default
            .to_f64()
            .unwrap();
        let dark = direct(
            instrument,
            midi,
            1.0,
            &[("brightness".into(), brightness * 0.5)]
                .into_iter()
                .collect(),
            2048,
            2048,
        );
        assert!(
            baseline
                .iter()
                .flatten()
                .zip(dark.iter().flatten())
                .any(|(a, b)| (a - b).abs() > 1e-12),
            "{name} brightness has no effect"
        );
        let short = direct(
            instrument,
            midi,
            1.0,
            &[("release".into(), 0.001)].into_iter().collect(),
            480,
            2880,
        );
        let long = direct(
            instrument,
            midi,
            1.0,
            &[("release".into(), 0.05)].into_iter().collect(),
            480,
            2880,
        );
        assert!(
            energy(&long[600..]) > energy(&short[600..]) + 1e-16,
            "{name} release has no tail effect"
        );
    }
}

#[test]
fn every_drum_has_fixed_tuning_across_extreme_note_pitches() {
    let drums: Vec<_> = instruments()
        .iter()
        .filter(|instrument| instrument.info.family == "drums")
        .collect();
    assert_eq!(drums.len(), 8);
    for instrument in drums {
        let baseline = direct(instrument, 0, 1.0, &BTreeMap::new(), 1024, 2048);
        for midi in [36, 60, 127] {
            assert_eq!(
                baseline,
                direct(instrument, midi, 1.0, &BTreeMap::new(), 1024, 2048),
                "{} pitch {midi} changed fixed tuning",
                instrument.info.name
            );
        }
    }
}

#[test]
fn basic_release_controls_hold_capacity_and_reset_restarts_voices() {
    let instrument = instruments()
        .iter()
        .find(|instrument| instrument.info.name == "organ")
        .unwrap();
    let mut runtime = runtime(instrument, &BTreeMap::new(), 1);
    runtime.note_on("first", hz(60), 1.0, 0).unwrap();
    let first: Vec<_> = (0..480)
        .map(|frame| runtime.render(frame).unwrap().to_vec())
        .collect();
    runtime.note_off("first", 480).unwrap();
    runtime.prune_finished(480).unwrap();
    assert_eq!(runtime.active_voice_count(), 1);
    assert_eq!(
        runtime
            .note_on("overflow", hz(60), 1.0, 480)
            .unwrap_err()
            .code(),
        "E_VOICE_LIMIT"
    );
    let release = instrument.program.controls["release"]
        .default
        .to_f64()
        .unwrap();
    runtime
        .prune_finished(480 + (release * RATE).ceil() as u64 + 1)
        .unwrap();
    assert_eq!(runtime.active_voice_count(), 0);
    runtime.reset(&BTreeMap::new()).unwrap();
    runtime.note_on("first", hz(60), 1.0, 0).unwrap();
    for (frame, expected) in first.into_iter().enumerate() {
        assert_eq!(runtime.render(frame as u64).unwrap(), expected);
    }
}

#[test]
fn every_export_keeps_instance_controls_and_history_independent_and_retires_voices() {
    for instrument in instruments() {
        let name = &instrument.info.name;
        let midi = midpoint(&instrument.info);
        let expected = direct(instrument, midi, 1.0, &BTreeMap::new(), 256, 256);
        let mut first = runtime(
            instrument,
            &[("pan".into(), -1.0), ("brightness".into(), 100.0)]
                .into_iter()
                .collect(),
            1,
        );
        let mut second = runtime(instrument, &BTreeMap::new(), 1);
        first
            .note_on("other", hz(instrument.info.guidance.midi_min), 0.35, 0)
            .unwrap();
        second.note_on("note", hz(midi), 1.0, 0).unwrap();
        for (frame, expected) in expected.iter().enumerate() {
            first.render(frame as u64).unwrap();
            assert_eq!(
                second.render(frame as u64).unwrap(),
                expected,
                "{name} instance history leaked"
            );
        }
        second.note_off("note", 256).unwrap();
        let release = instrument.info.controls["release"]
            .default
            .to_f64()
            .unwrap();
        second
            .prune_finished(256 + (release * RATE).ceil() as u64 + 1)
            .unwrap();
        assert_eq!(
            second.active_voice_count(),
            0,
            "{name} did not retire its voice"
        );
        assert_eq!(
            first.active_voice_count(),
            1,
            "{name} instances shared voice lifecycle"
        );
    }
}
