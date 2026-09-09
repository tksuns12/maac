use std::collections::BTreeMap;
use std::io::Cursor;

use hound::{SampleFormat, WavReader};
use maac::export::{render_wav_to_path, write_wav, ExportError, WavFormat};
use maac::plan::{
    EventKind, EventTarget, Interpolation, Node, OutputSettings, Plan, PortRef, Processor,
    Rational, ResolvedEvent, SourceMapping, TempoMap, TempoPoint,
};
use tempfile::tempdir;

fn rational(numerator: i64, denominator: i64) -> Rational {
    maac::parse_rational(&format!("{numerator}/{denominator}")).unwrap()
}

fn one_note_plan(level: Rational) -> Plan {
    one_note_plan_with_pitch(level, 440.0)
}

fn one_note_plan_with_pitch(level: Rational, pitch_hz: f64) -> Plan {
    let mut params = BTreeMap::new();
    params.insert("attack".into(), rational(0, 1));
    params.insert("release".into(), rational(0, 1));
    params.insert("level".into(), level);
    Plan {
        version: 1,
        output: OutputSettings {
            score_start_q: rational(0, 1),
            score_end_q: rational(1, 1),
            tail_seconds: rational(0, 1),
            sample_rate_hz: 48_000,
            channels: 1,
            total_frames: 24_000,
            output: PortRef::new("sine", "out").unwrap(),
        },
        tempo: TempoMap {
            points: vec![TempoPoint {
                q: rational(0, 1),
                bpm: rational(120, 1),
                shape: Interpolation::Step,
            }],
        },
        events: vec![ResolvedEvent {
            address: "main/0/n".into(),
            source: SourceMapping {
                object: "n".into(),
                path: vec!["main".into(), "n".into()],
                span: None,
            },
            target: EventTarget::new("sine", "events").unwrap(),
            kind: EventKind::Note {
                pitch_expression: None,
                gain_expression: None,
                pitch_hz,
                velocity: rational(1, 1),
            },
            score_on_q: rational(0, 1),
            score_off_q: Some(rational(1, 2)),
            onset_offset_seconds: rational(0, 1),
            release_offset_seconds: rational(0, 1),
            on_seconds: rational(0, 1),
            off_seconds: Some(rational(1, 4)),
            release_velocity: 0.0,
            on_frame: 0,
            off_frame: Some(12_000),
            order: 0,
        }],
        nodes: vec![Node {
            id: "sine".into(),
            processor: Processor::sine(4),
            params,
        }],
        connections: Vec::new(),
        automation: Vec::new(),
        regions: Vec::new(),
        source_mappings: Vec::new(),
        instruments: None,
        production: None,
    }
}

#[test]
fn float32_export_has_expected_header_frames_and_audio() {
    let plan = one_note_plan(rational(1, 1));
    let mut sink = Cursor::new(Vec::<u8>::new());
    let stats = write_wav(&mut sink, &plan, WavFormat::Float32).unwrap();
    assert_eq!(stats.frames, 24_000);
    assert_eq!(stats.channels, 1);

    let bytes = sink.into_inner();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut reader = WavReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.spec().sample_format, SampleFormat::Float);
    assert_eq!(reader.spec().bits_per_sample, 32);
    assert_eq!(reader.duration(), 24_000);
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples.len(), 24_000);
    assert!(samples.iter().any(|sample| sample.abs() > 0.0));
    assert!(samples.iter().all(|sample| sample.is_finite()));
}

#[test]
fn pcm16_export_uses_integer_header_and_rejects_overload() {
    let plan = one_note_plan(rational(2, 1));
    let mut sink = Cursor::new(Vec::<u8>::new());
    let error = write_wav(&mut sink, &plan, WavFormat::Pcm16).unwrap_err();
    assert!(matches!(error, ExportError::Pcm16Overload { .. }));

    let plan = one_note_plan(rational(1, 1));
    let mut sink = Cursor::new(Vec::<u8>::new());
    let stats = write_wav(&mut sink, &plan, WavFormat::Pcm16).unwrap();
    assert_eq!(stats.frames, 24_000);
    let bytes = sink.into_inner();
    let mut reader = WavReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(reader.spec().sample_format, SampleFormat::Int);
    assert_eq!(reader.spec().bits_per_sample, 16);
    assert_eq!(reader.duration(), 24_000);
    let samples: Vec<i16> = reader.samples::<i16>().map(Result::unwrap).collect();
    assert!(samples.iter().any(|sample| *sample != 0));
}

#[test]
fn pcm16_quantization_has_symmetric_full_scale_endpoints() {
    let plan = one_note_plan_with_pitch(rational(1, 1), 12_000.0);
    let mut sink = Cursor::new(Vec::<u8>::new());
    write_wav(&mut sink, &plan, WavFormat::Pcm16).unwrap();
    let mut reader = WavReader::new(Cursor::new(sink.into_inner())).unwrap();
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .map(Result::unwrap)
        .take(4)
        .collect();
    assert_eq!(samples, vec![0, 32_767, 0, -32_767]);
}

#[test]
fn failed_render_preserves_existing_file_and_leaves_new_destination_absent() {
    let plan = one_note_plan(rational(2, 1));
    let directory = tempdir().unwrap();
    let existing = directory.path().join("existing.wav");
    std::fs::write(&existing, b"old destination").unwrap();
    let error = render_wav_to_path(&plan, &existing, WavFormat::Pcm16, false).unwrap_err();
    assert!(matches!(error, ExportError::OutputExists { .. }));
    assert_eq!(std::fs::read(&existing).unwrap(), b"old destination");

    let error = render_wav_to_path(&plan, &existing, WavFormat::Pcm16, true).unwrap_err();
    assert!(matches!(error, ExportError::Pcm16Overload { .. }));
    assert_eq!(std::fs::read(&existing).unwrap(), b"old destination");

    let missing = directory.path().join("failed.wav");
    let error = render_wav_to_path(&plan, &missing, WavFormat::Pcm16, true).unwrap_err();
    assert!(matches!(error, ExportError::Pcm16Overload { .. }));
    assert!(!missing.exists());
}

#[test]
fn successful_render_requires_force_to_replace_destination() {
    let plan = one_note_plan(rational(1, 1));
    let directory = tempdir().unwrap();
    let destination = directory.path().join("output.wav");
    render_wav_to_path(&plan, &destination, WavFormat::Float32, false).unwrap();
    let original = std::fs::read(&destination).unwrap();
    let error = render_wav_to_path(&plan, &destination, WavFormat::Pcm16, false).unwrap_err();
    assert!(matches!(error, ExportError::OutputExists { .. }));
    assert_eq!(std::fs::read(&destination).unwrap(), original);

    render_wav_to_path(&plan, &destination, WavFormat::Pcm16, true).unwrap();
    let replaced = std::fs::read(&destination).unwrap();
    assert_ne!(replaced, original);
}

#[test]
fn invalid_plan_is_rejected_before_wav_header_is_written() {
    let mut plan = one_note_plan(rational(1, 1));
    plan.output.channels = 0;
    let mut sink = Cursor::new(Vec::<u8>::new());
    let error = write_wav(&mut sink, &plan, WavFormat::Float32).unwrap_err();
    assert_eq!(error.code(), "E_RANGE");
    assert!(sink.into_inner().is_empty());
}
