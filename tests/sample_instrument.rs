//! `synth.sample/1`: pitched, zoned, optionally looped sample playback inside
//! instrument voice graphs.

use std::collections::BTreeMap;
use std::io::Cursor;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle;
use maac::sample_instrument::InstrumentSample;
use maac::{load_plan, render, DiagnosticCode, Plan};

fn float_wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for sample in samples {
            writer.write_sample(*sample).unwrap();
        }
        writer.finalize().unwrap();
    }
    bytes.into_inner()
}

/// 500 Hz sine at 24 kHz: 48 samples per period, ten periods.
fn tone() -> Vec<f32> {
    (0..480)
        .map(|j| (std::f64::consts::TAU * j as f64 / 48.0).sin() as f32)
        .collect()
}

struct Sample<'a> {
    name: &'a str,
    values: Vec<f32>,
    rate: u32,
    extra: &'a str,
}

fn bundle(samples: &[Sample<'_>], zones: &str, notes: &str) -> SourceBundle {
    let mut assets = BTreeMap::new();
    let mut declarations = String::new();
    for sample in samples {
        let wav = float_wav(&sample.values, sample.rate);
        declarations.push_str(&format!(
            "sample {} {{ path = \"{}.wav\"; hash = \"{}\"; {} }}\n",
            sample.name,
            sample.name,
            sha256_digest(&wav),
            sample.extra
        ));
        assets.insert(format!("{}.wav", sample.name), wav);
    }
    let source = format!(
        r#"maac 1;
project song {{ score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &keys:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
{declarations}
instrument sampler {{
  channels = 1;
  voice v {{
    channels = 1; amplitude = &amp; output = &play:out;
    node amp {{ type = "synth.adsr/1"; }}
    node play {{ type = "synth.sample/1"; config = {{ zones = [{zones}]; }}; }}
  }}
}}
node keys {{ instrument = &sampler; }}
pattern phrase {{ length = 4q; {notes} }}
track melody {{ target = &keys:events; }}
place play {{ pattern = &phrase; track = &melody; at = 0q; }}
"#
    );
    SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([("main.maac".into(), source)]),
        assets,
    }
}

fn tone_bundle(notes: &str) -> SourceBundle {
    bundle(
        &[Sample {
            name: "tone",
            values: tone(),
            rate: 24_000,
            extra: "root = A4;",
        }],
        "{ sample = &tone; low = key(0); high = key(127); }",
        notes,
    )
}

fn audio(plan: &Plan) -> Vec<f64> {
    let mut output = Vec::new();
    render(plan, |frame| {
        output.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    output
}

fn codes(bundle: &SourceBundle) -> Vec<DiagnosticCode> {
    compile_bundle(bundle)
        .expect_err("bundle must be refused")
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn root_note_plays_the_recording_at_its_own_rate() {
    let plan = compile_bundle(&tone_bundle("note n { at = 0q; dur = 1q; pitch = A4; }")).unwrap();
    let out = audio(&plan);
    let tone = tone();
    // A 24 kHz recording advances half a source frame per 48 kHz output
    // frame, so every even output frame lands exactly on a recorded sample.
    for k in 0..400 {
        assert!(
            (out[2 * k] - f64::from(tone[k])).abs() < 1e-12,
            "frame {}",
            2 * k
        );
        let midpoint = 0.5 * (f64::from(tone[k]) + f64::from(tone[k + 1]));
        assert!((out[2 * k + 1] - midpoint).abs() < 1e-12);
    }
    // The ten recorded periods (960 output frames) are followed by silence.
    assert!(out[962..48_000].iter().all(|sample| *sample == 0.0));
}

#[test]
fn an_octave_up_doubles_the_playback_rate() {
    let plan = compile_bundle(&tone_bundle("note n { at = 0q; dur = 1q; pitch = A5; }")).unwrap();
    let out = audio(&plan);
    let tone = tone();
    for (k, value) in tone.iter().enumerate().take(470) {
        assert!((out[k] - f64::from(*value)).abs() < 1e-12, "frame {k}");
    }
    assert!(out[481..48_000].iter().all(|sample| *sample == 0.0));
}

#[test]
fn zones_choose_samples_by_nearest_key_and_uncovered_notes_fail() {
    let constant = |name, value, low: &str| Sample {
        name,
        values: vec![value; 4_800],
        rate: 48_000,
        extra: if low == "low" {
            "root = C3;"
        } else {
            "root = C5;"
        },
    };
    let samples = [
        constant("low", 0.25_f32, "low"),
        constant("high", 0.75, "high"),
    ];
    let zones = "{ sample = &low; low = key(48); high = B3; }, { sample = &high; low = C4; high = key(72); }";
    let plan = compile_bundle(&bundle(
        &samples,
        zones,
        "note a { at = 0q; dur = 1/2q; pitch = B3; } note b { at = 1q; dur = 1/2q; pitch = C4; }",
    ))
    .unwrap();
    let out = audio(&plan);
    assert!((out[100] - 0.25).abs() < 1e-12);
    assert!((out[48_000 + 100] - 0.75).abs() < 1e-12);

    let uncovered = compile_bundle(&bundle(
        &samples,
        zones,
        "note a { at = 0q; dur = 1/2q; pitch = C2; }",
    ))
    .unwrap();
    let mut rendered = Vec::new();
    let error = render(&uncovered, |frame| {
        rendered.extend_from_slice(frame);
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code(), "E_RANGE", "{error}");
}

#[test]
fn sustain_loops_wrap_while_unlooped_samples_end() {
    let ramp: Vec<f32> = (0..100).map(|j| j as f32).collect();
    let looped = |extra| {
        bundle(
            &[Sample {
                name: "ramp",
                values: ramp.clone(),
                rate: 48_000,
                extra,
            }],
            "{ sample = &ramp; low = key(0); high = key(127); }",
            "note n { at = 0q; dur = 1q; pitch = A4; }",
        )
    };
    let out = audio(&compile_bundle(&looped("root = A4; loop = [50frame, 100frame];")).unwrap());
    assert_eq!(out[99], 99.0);
    // Frame 100 wraps to the loop start; the last loop frame's right
    // neighbour is the loop start, so frame 149 interpolates nothing new.
    assert_eq!(out[100], 50.0);
    assert_eq!(out[120], 70.0);
    assert_eq!(out[1_000], 50.0 + ((1_000 - 50) % 50) as f64);

    let out = audio(&compile_bundle(&looped("root = A4;")).unwrap());
    assert_eq!(out[99], 99.0);
    assert!(out[100..48_000].iter().all(|sample| *sample == 0.0));
}

#[test]
fn plans_embed_samples_and_round_trip() {
    let plan = compile_bundle(&tone_bundle("note n { at = 0q; dur = 1q; pitch = A4; }")).unwrap();
    let resources = plan.instruments.as_ref().unwrap();
    assert_eq!(resources.samples.len(), 1);
    assert_eq!(resources.samples[0].rate_hz, 24_000);
    assert_eq!(resources.samples[0].root_key, 69);
    assert_eq!(resources.sample_sources[0].path, "tone.wav");
    let bytes = serde_json::to_vec(&plan).unwrap();
    let loaded = load_plan(&bytes).unwrap();
    assert_eq!(audio(&loaded), audio(&plan));
}

#[test]
fn invalid_declarations_and_zones_are_refused() {
    let with = |extra: &str, zones: &str| {
        bundle(
            &[Sample {
                name: "tone",
                values: tone(),
                rate: 24_000,
                extra,
            }],
            zones,
            "note n { at = 0q; dur = 1q; pitch = A4; }",
        )
    };
    let all = "{ sample = &tone; low = key(0); high = key(127); }";
    for (extra, zones, code) in [
        ("", all, DiagnosticCode::UnknownField),
        ("root = key(128);", all, DiagnosticCode::Range),
        ("root = 440Hz;", all, DiagnosticCode::Unit),
        ("root = A4; loop = [10frame, 10frame];", all, DiagnosticCode::Range),
        ("root = A4; loop = [0frame, 481frame];", all, DiagnosticCode::Range),
        ("root = A4; loop = [0q, 1q];", all, DiagnosticCode::Unit),
        (
            "root = A4;",
            "{ sample = &tone; low = key(60); high = key(59); }",
            DiagnosticCode::Range,
        ),
        (
            "root = A4;",
            "{ sample = &tone; low = key(0); high = key(64); }, { sample = &tone; low = key(64); high = key(127); }",
            DiagnosticCode::Range,
        ),
        (
            "root = A4;",
            "{ sample = &clock; low = key(0); high = key(127); }",
            DiagnosticCode::Reference,
        ),
        (
            "root = A4;",
            "{ sample = &tone; low = key(0); }",
            DiagnosticCode::UnknownField,
        ),
    ] {
        let found = codes(&with(extra, zones));
        assert!(found.contains(&code), "{extra} {zones}: {found:?}");
    }

    let stereo = {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut bytes,
                hound::WavSpec {
                    channels: 2,
                    sample_rate: 48_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for _ in 0..8 {
                writer.write_sample(0_i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        bytes.into_inner()
    };
    assert_eq!(
        InstrumentSample::from_wav("stereo", &stereo, 60, None)
            .unwrap_err()
            .code,
        "E_RANGE"
    );
}

#[test]
fn sample_nodes_are_voice_only_and_have_no_phase() {
    let base = tone_bundle("note n { at = 0q; dur = 1q; pitch = A4; }");
    let source = &base.sources["main.maac"];
    let with_phase = source.replace(
        "config = { zones = [",
        "params = { phase = 0; }; config = { zones = [",
    );
    let mut bundle = base.clone();
    bundle.sources.insert("main.maac".into(), with_phase);
    let found = codes(&bundle);
    assert!(found.contains(&DiagnosticCode::UnknownField), "{found:?}");

    let shared = source.replace(
        "  }\n}\nnode keys",
        "  }\n  shared s { channels = 1; output = &extra:out; node extra { type = \"synth.sample/1\"; config = { zones = [{ sample = &tone; low = key(0); high = key(127); }]; }; } }\n}\nnode keys",
    );
    assert_ne!(&shared, source);
    let mut bundle = base.clone();
    bundle.sources.insert("main.maac".into(), shared);
    let found = codes(&bundle);
    assert!(found.contains(&DiagnosticCode::Capability), "{found:?}");
}
