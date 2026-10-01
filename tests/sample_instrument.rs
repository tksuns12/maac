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
            "{ sample = &tone; low = key(0); high = key(127); velocity = [1/2, 1/2]; }",
            DiagnosticCode::Range,
        ),
        (
            "root = A4;",
            "{ sample = &tone; low = key(0); high = key(127); velocity = [0, 2]; }",
            DiagnosticCode::Range,
        ),
        (
            "root = A4;",
            "{ sample = &tone; low = key(60); high = key(61); key_fade = [2, 1]; }",
            DiagnosticCode::Range,
        ),
        (
            "root = A4;",
            "{ sample = &tone; low = key(0); high = key(127); velocity = [0, 1/2]; velocity_fade = [1/4, 1/2]; }",
            DiagnosticCode::Range,
        ),
        (
            "root = A4;",
            "{ sample = &tone; low = key(0); high = key(127); velocity_fade = [-1/4, 0]; }",
            DiagnosticCode::Range,
        ),
        (
            "root = A4;",
            "{ sample = &tone; low = key(0); high = key(127); velocity = 1; }",
            DiagnosticCode::Unit,
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

    // Mono and stereo WAVs decode; three or more channels are refused.
    let wav = |channels: u16| {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut bytes,
                hound::WavSpec {
                    channels,
                    sample_rate: 48_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for _ in 0..12 {
                writer.write_sample(0_i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        bytes.into_inner()
    };
    let stereo = InstrumentSample::from_wav("stereo", &wav(2), 60, None).unwrap();
    assert_eq!((stereo.channels, stereo.frames()), (2, 6));
    assert_eq!(
        InstrumentSample::from_wav("surround", &wav(3), 60, None)
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

fn pcm(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// The tone bundle with its sample declared as a mono core PCM asset.
fn native_bundle(values: &[f32], declared_frames: usize, notes: &str) -> SourceBundle {
    let mut bundle = tone_bundle(notes);
    let bytes = pcm(values);
    let wav_decl = bundle.sources["main.maac"]
        .lines()
        .find(|line| line.starts_with("sample tone"))
        .unwrap()
        .to_owned();
    let native_decl = format!(
        "sample tone {{ path = \"tone.pcm\"; hash = \"{}\"; format = \"pcm_f32le_interleaved/1\"; rate = 24000Hz; frames = {declared_frames}; root = A4; }}",
        sha256_digest(&bytes)
    );
    let source = bundle.sources["main.maac"].replace(&wav_decl, &native_decl);
    bundle.sources.insert("main.maac".into(), source);
    bundle.assets = BTreeMap::from([("tone.pcm".into(), bytes)]);
    bundle
}

fn artifact_audio(artifact: &maac::PlanArtifact) -> Vec<f64> {
    let mut output = Vec::new();
    maac::render_artifact(artifact, |frame| {
        output.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    output
}

#[test]
fn asset_backed_samples_render_like_embedded_ones() {
    use maac::compiler::compile_bundle_artifact;
    let notes = "note n { at = 0q; dur = 1q; pitch = A4; }";
    let embedded = compile_bundle_artifact(&tone_bundle(notes)).unwrap();
    let native = compile_bundle_artifact(&native_bundle(&tone(), 480, notes)).unwrap();

    let wire: serde_json::Value = serde_json::from_slice(&native.to_json().unwrap()).unwrap();
    let sample = &wire["instruments"]["samples"][0];
    assert_eq!(sample["asset"]["id"], "__sample_0");
    assert_eq!(sample["asset"]["frames"], 480);
    assert_eq!(sample["samples"], serde_json::json!([]));
    assert!(wire["audio_assets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|asset| asset["id"] == "__sample_0" && asset["channels"] == 1));

    assert_eq!(artifact_audio(&native), artifact_audio(&embedded));
    let reloaded = maac::PlanArtifact::from_json(&native.to_json().unwrap()).unwrap();
    assert_eq!(artifact_audio(&reloaded), artifact_audio(&embedded));
}

#[test]
fn asset_backed_samples_are_not_limited_by_the_embedded_budget() {
    use maac::compiler::compile_bundle_artifact;
    // 300,000 frames exceed the 262,144-frame embedded budget.
    let long: Vec<f32> = (0..300_000).map(|j| ((j % 100) as f32) / 100.0).collect();
    let native = compile_bundle_artifact(&native_bundle(
        &long,
        long.len(),
        "note n { at = 0q; dur = 4q; pitch = A5; }",
    ))
    .unwrap();
    let out = artifact_audio(&native);
    // A5 plays the 24 kHz recording one source frame per output frame.
    for frame in [0usize, 99, 150_000, 191_999] {
        assert!((out[frame] - f64::from(long[frame])).abs() < 1e-12);
    }
}

#[test]
fn asset_backed_sample_refusals() {
    use maac::compiler::compile_bundle_artifact;
    let notes = "note n { at = 0q; dur = 1q; pitch = A4; }";
    // Version 2 plans cannot carry audio assets.
    let bundle = native_bundle(&tone(), 480, notes);
    assert!(codes(&bundle).contains(&DiagnosticCode::Capability));

    // Declared frames must match the exact bytes.
    let wrong = native_bundle(&tone(), 479, notes);
    let error = compile_bundle_artifact(&wrong).unwrap_err();
    assert!(
        error.iter().any(|d| d.code == DiagnosticCode::Asset),
        "{error:?}"
    );

    // A composition asset may not take a reserved sample asset ID.
    let mut clash = native_bundle(&tone(), 480, notes);
    let bytes = pcm(&[0.0; 4]);
    let source = clash.sources["main.maac"].replace(
        "node keys",
        &format!(
            "asset __sample_0 {{ kind = audio; path = \"other.pcm\"; hash = \"{}\"; format = \"pcm_f32le_interleaved/1\"; rate = 48000Hz; channels = 1; frames = 4; }}\nnode keys",
            sha256_digest(&bytes)
        ),
    );
    clash.sources.insert("main.maac".into(), source);
    clash.assets.insert("other.pcm".into(), bytes);
    let error = compile_bundle_artifact(&clash).unwrap_err();
    assert!(
        error.iter().any(|d| d.code == DiagnosticCode::DuplicateId),
        "{error:?}"
    );

    // A plan whose sample points at a mismatched asset does not load.
    let artifact = compile_bundle_artifact(&native_bundle(&tone(), 480, notes)).unwrap();
    let mut wire: serde_json::Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    wire["instruments"]["samples"][0]["asset"]["frames"] = 479.into();
    let error = maac::PlanArtifact::from_json(&serde_json::to_vec(&wire).unwrap()).unwrap_err();
    assert_eq!(error.code, "E_ASSET", "{error:?}");
}

/// Constant-valued samples make zone gains directly observable.
fn constant(name: &'static str, value: f32) -> Sample<'static> {
    Sample {
        name,
        values: vec![value; 48_000],
        rate: 48_000,
        extra: "root = A4;",
    }
}

fn first_frame(samples: &[Sample<'_>], zones: &str, note: &str, fade: &str) -> f64 {
    let mut bundle = bundle(
        samples,
        zones,
        &format!("note n {{ at = 0q; dur = 1q; {note} }}"),
    );
    if !fade.is_empty() {
        let source = bundle.sources["main.maac"].replace(
            "config = { zones = [",
            &format!("config = {{ fade_shape = {fade}; zones = ["),
        );
        bundle.sources.insert("main.maac".into(), source);
    }
    audio(&compile_bundle(&bundle).unwrap())[10]
}

#[test]
fn velocity_layers_choose_samples_by_note_velocity() {
    let samples = [constant("soft", 0.25), constant("loud", 0.75)];
    let zones = "{ sample = &soft; low = key(0); high = key(127); velocity = [0, 1/2]; }, \
                 { sample = &loud; low = key(0); high = key(127); velocity = [1/2, 1]; }";
    // Voice amplitude is the envelope times velocity, so divide it out.
    let soft = first_frame(&samples, zones, "pitch = A4; velocity = 3/10;", "");
    assert!((soft / 0.3 - 0.25).abs() < 1e-12, "{soft}");
    let loud = first_frame(&samples, zones, "pitch = A4; velocity = 1/2;", "");
    assert!((loud / 0.5 - 0.75).abs() < 1e-12, "{loud}");
    let top = first_frame(&samples, zones, "pitch = A4; velocity = 1;", "");
    assert!((top - 0.75).abs() < 1e-12, "{top}");
}

#[test]
fn key_crossfades_sum_complementary_gains() {
    let samples = [constant("low", 1.0), constant("high", 0.5)];
    // Keys 60..64 overlap; each zone fades across the 5-semitone overlap.
    let zones = "{ sample = &low; low = key(0); high = key(64); key_fade = [0, 5]; }, \
                 { sample = &high; low = key(60); high = key(127); key_fade = [5, 0]; }";
    for (key, low_gain) in [(59, 1.0), (60, 0.9), (62, 0.5), (64, 0.1), (65, 0.0)] {
        let note = format!("pitch = key({key});");
        let value = first_frame(&samples, zones, &note, "");
        let expected = low_gain * 1.0 + (1.0 - low_gain) * 0.5;
        assert!((value - expected).abs() < 1e-12, "key {key}: {value}");
        let equal = first_frame(&samples, zones, &note, "equal_power");
        let shape = |x: f64| (std::f64::consts::FRAC_PI_2 * x).sin();
        let expected = shape(low_gain) * 1.0 + shape(1.0 - low_gain) * 0.5;
        assert!((equal - expected).abs() < 1e-12, "key {key}: {equal}");
    }
}

#[test]
fn velocity_crossfades_and_unfaded_overlaps_stack() {
    let samples = [constant("soft", 1.0), constant("loud", 0.5)];
    let zones = "{ sample = &soft; low = key(0); high = key(127); velocity = [0, 3/5]; velocity_fade = [0, 1/5]; }, \
                 { sample = &loud; low = key(0); high = key(127); velocity = [2/5, 1]; velocity_fade = [1/5, 0]; }";
    let value = first_frame(&samples, zones, "pitch = A4; velocity = 1/2;", "");
    // Velocity 1/2 is halfway through the [2/5, 3/5) crossfade.
    assert!((value / 0.5 - 0.75).abs() < 1e-12, "{value}");

    let stacked = "{ sample = &soft; low = key(0); high = key(127); }, \
                   { sample = &loud; low = key(60); high = key(80); }";
    let value = first_frame(&samples, stacked, "pitch = A4;", "");
    assert!((value - 1.5).abs() < 1e-12, "{value}");
}

#[test]
fn fade_shape_must_be_known() {
    let samples = [constant("low", 1.0)];
    let mut bundle = bundle(
        &samples,
        "{ sample = &low; low = key(0); high = key(127); }",
        "note n { at = 0q; dur = 1q; pitch = A4; }",
    );
    let source = bundle.sources["main.maac"].replace(
        "config = { zones = [",
        "config = { fade_shape = cubic; zones = [",
    );
    bundle.sources.insert("main.maac".into(), source);
    assert!(codes(&bundle).contains(&DiagnosticCode::Range));
}

#[test]
fn zone_defaults_do_not_change_execution_identity() {
    let identity = |zones: &str, fade: &str| {
        let bundle = bundle(
            &[constant("low", 1.0)],
            zones,
            "note n { at = 0q; dur = 1q; pitch = A4; }",
        );
        let mut source = bundle.sources["main.maac"].clone();
        if !fade.is_empty() {
            source = source.replace(
                "config = { zones = [",
                &format!("config = {{ fade_shape = {fade}; zones = ["),
            );
        }
        let mut bundle = bundle;
        bundle.sources.insert("main.maac".into(), source.clone());
        let plan = compile_bundle(&bundle).unwrap();
        maac::production_identity::execution_identity(&maac::parse(&source).unwrap(), &plan)
            .unwrap()
            .execution_hash
    };
    let omitted = identity("{ sample = &low; low = key(0); high = key(127); }", "");
    let explicit = identity(
        "{ sample = &low; low = key(0); high = key(127); velocity = [0, 1]; key_fade = [0, 0]; velocity_fade = [0, 0]; }",
        "linear",
    );
    assert_eq!(omitted, explicit);
    let layered = identity(
        "{ sample = &low; low = key(0); high = key(127); velocity = [0, 1/2]; }",
        "",
    );
    assert_ne!(omitted, layered);
}

/// A two-channel binary32 WAV interleaving `left` and `right`.
fn stereo_wav(left: &[f32], right: &[f32], rate: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 2,
                sample_rate: rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for (l, r) in left.iter().zip(right) {
            writer.write_sample(*l).unwrap();
            writer.write_sample(*r).unwrap();
        }
        writer.finalize().unwrap();
    }
    bytes.into_inner()
}

/// One stereo sample: per-channel values, rate, and declaration extras.
struct StereoSample<'a> {
    name: &'a str,
    left: Vec<f32>,
    right: Vec<f32>,
    rate: u32,
    extra: &'a str,
}

impl StereoSample<'_> {
    fn channel(&self, right: bool) -> Sample<'_> {
        Sample {
            name: self.name,
            values: if right { &self.right } else { &self.left }.clone(),
            rate: self.rate,
            extra: self.extra,
        }
    }
}

/// The mono test bundle widened to a stereo instrument, voice and sample node.
fn stereo_bundle(samples: &[StereoSample<'_>], zones: &str, notes: &str) -> SourceBundle {
    let mut bundle = bundle(
        &samples
            .iter()
            .map(|sample| sample.channel(false))
            .collect::<Vec<_>>(),
        zones,
        notes,
    );
    let mut source = bundle.sources["main.maac"]
        .replace(
            "  channels = 1;\n  voice v {\n    channels = 1;",
            "  channels = 2;\n  voice v {\n    channels = 2;",
        )
        .replace("config = { zones = [", "config = { channels = 2; zones = [");
    for sample in samples {
        let mono = float_wav(&sample.left, sample.rate);
        let wav = stereo_wav(&sample.left, &sample.right, sample.rate);
        source = source.replace(&sha256_digest(&mono), &sha256_digest(&wav));
        bundle.assets.insert(format!("{}.wav", sample.name), wav);
    }
    assert!(source.contains("channels = 2; zones"));
    bundle.sources.insert("main.maac".into(), source);
    bundle
}

/// A looped 24 kHz tone and a 48 kHz ramp pair whose channels all differ.
fn stereo_pair() -> [StereoSample<'static>; 2] {
    let tone = tone();
    [
        StereoSample {
            name: "low",
            right: tone.iter().map(|value| -0.5 * value).collect(),
            left: tone,
            rate: 24_000,
            extra: "root = A4; loop = [48frame, 432frame];",
        },
        StereoSample {
            name: "high",
            left: (0..4_800).map(|j| j as f32 / 4_800.0).collect(),
            right: (0..4_800).map(|j| 0.25 - j as f32 / 9_600.0).collect(),
            rate: 48_000,
            extra: "root = E5;",
        },
    ]
}

/// The two stereo zones crossfade over A4..B4.
const PAIR_ZONES: &str = "{ sample = &low; low = key(0); high = key(71); key_fade = [0, 3]; }, \
                          { sample = &high; low = key(69); high = key(127); key_fade = [3, 0]; }";
const PAIR_NOTES: &str = "note a { at = 0q; dur = 1q; pitch = A4; } \
                          note b { at = 1q; dur = 1q; pitch = A#4; } \
                          note c { at = 2q; dur = 2q; pitch = E4; }";

#[test]
fn stereo_samples_render_each_channel_like_a_mono_sample() {
    let pair = stereo_pair();
    let plan = compile_bundle(&stereo_bundle(&pair, PAIR_ZONES, PAIR_NOTES)).unwrap();
    assert_eq!(plan.output.channels, 2);
    let resources = plan.instruments.as_ref().unwrap();
    assert!(resources.samples.iter().all(|sample| sample.channels == 2));
    let stereo = audio(&plan);
    for (offset, right) in [(0, false), (1, true)] {
        let mono = audio(
            &compile_bundle(&bundle(
                &pair
                    .iter()
                    .map(|sample| sample.channel(right))
                    .collect::<Vec<_>>(),
                PAIR_ZONES,
                PAIR_NOTES,
            ))
            .unwrap(),
        );
        assert_eq!(stereo.len(), 2 * mono.len());
        for (frame, value) in mono.iter().enumerate() {
            // Rate conversion, crossfade gains and the sustain loop are shared
            // by both channels, so each matches its mono render exactly.
            assert_eq!(
                stereo[2 * frame + offset].to_bits(),
                value.to_bits(),
                "channel {offset}, frame {frame}"
            );
        }
        assert!(mono.iter().any(|value| *value != 0.0));
    }
    // The retained plan reloads and renders the same stereo audio.
    let loaded = load_plan(&serde_json::to_vec(&plan).unwrap()).unwrap();
    assert_eq!(audio(&loaded), stereo);
}

#[test]
fn stereo_core_pcm_samples_render_like_stereo_wavs() {
    use maac::compiler::compile_bundle_artifact;
    let pair = stereo_pair();
    let wav = stereo_bundle(&pair, PAIR_ZONES, PAIR_NOTES);
    let mut native = wav.clone();
    let mut source = native.sources["main.maac"].clone();
    for sample in &pair {
        let interleaved: Vec<f32> = sample
            .left
            .iter()
            .zip(&sample.right)
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        let bytes = pcm(&interleaved);
        let declaration = source
            .lines()
            .find(|line| line.starts_with(&format!("sample {} ", sample.name)))
            .unwrap()
            .to_owned();
        source = source.replace(
            &declaration,
            &format!(
                "sample {name} {{ path = \"{name}.pcm\"; hash = \"{hash}\"; format = \"pcm_f32le_interleaved/1\"; rate = {rate}Hz; channels = 2; frames = {frames}; {extra} }}",
                name = sample.name,
                hash = sha256_digest(&bytes),
                rate = sample.rate,
                frames = sample.left.len(),
                extra = sample.extra,
            ),
        );
        native.assets.remove(&format!("{}.wav", sample.name));
        native.assets.insert(format!("{}.pcm", sample.name), bytes);
    }
    native.sources.insert("main.maac".into(), source);
    let native = compile_bundle_artifact(&native).unwrap_or_else(|e| panic!("{e:?}"));
    let wire: serde_json::Value = serde_json::from_slice(&native.to_json().unwrap()).unwrap();
    assert!(wire["audio_assets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|asset| asset["id"] == "__sample_0" && asset["channels"] == 2));
    assert_eq!(wire["instruments"]["samples"][0]["channels"], 2);
    let embedded = compile_bundle_artifact(&wav).unwrap();
    assert_eq!(artifact_audio(&native), artifact_audio(&embedded));
    let reloaded = maac::PlanArtifact::from_json(&native.to_json().unwrap()).unwrap();
    assert_eq!(artifact_audio(&reloaded), artifact_audio(&embedded));
}

#[test]
fn mono_plans_keep_their_bytes() {
    // Mono samples and nodes carry no `channels` field on the wire.
    let plan = compile_bundle(&tone_bundle("note n { at = 0q; dur = 1q; pitch = A4; }")).unwrap();
    let wire = serde_json::to_value(&plan).unwrap();
    assert!(wire["instruments"]["samples"][0].get("channels").is_none());
    let nodes = &wire["instruments"]["programs"][0]["voice"]["nodes"];
    let play = nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "play")
        .unwrap();
    assert!(play["processor"].get("channels").is_none());
}

#[test]
fn stereo_channel_mismatches_are_refused() {
    let pair = stereo_pair();
    let stereo = stereo_bundle(&pair, PAIR_ZONES, PAIR_NOTES);
    let edit = |from: &str, to: &str| {
        let mut bundle = stereo.clone();
        let source = bundle.sources["main.maac"].replace(from, to);
        assert_ne!(source, bundle.sources["main.maac"], "{from}");
        bundle.sources.insert("main.maac".into(), source);
        codes(&bundle)
    };
    // A mono node cannot play stereo samples.
    let found = edit("config = { channels = 2; zones", "config = { zones");
    assert!(found.contains(&DiagnosticCode::PortType), "{found:?}");
    // A stereo node cannot feed a mono voice output.
    let found = edit(
        "  channels = 2;\n  voice v {\n    channels = 2;",
        "  channels = 1;\n  voice v {\n    channels = 1;",
    );
    assert!(found.contains(&DiagnosticCode::PortType), "{found:?}");
    let found = edit(
        "config = { channels = 2; zones",
        "config = { channels = 3; zones",
    );
    assert!(found.contains(&DiagnosticCode::Range), "{found:?}");

    // A stereo node cannot play a mono sample.
    let mut mixed = stereo_bundle(
        &pair[..1],
        "{ sample = &low; low = key(0); high = key(127); }",
        PAIR_NOTES,
    );
    let mono = float_wav(&pair[0].left, pair[0].rate);
    let wav = mixed.assets["low.wav"].clone();
    let source = mixed.sources["main.maac"].replace(&sha256_digest(&wav), &sha256_digest(&mono));
    mixed.sources.insert("main.maac".into(), source);
    mixed.assets.insert("low.wav".into(), mono);
    let found = codes(&mixed);
    assert!(found.contains(&DiagnosticCode::PortType), "{found:?}");

    // A retained plan whose node and sample channels disagree does not load.
    let plan = compile_bundle(&stereo).unwrap();
    let mut wire = serde_json::to_value(&plan).unwrap();
    for sample in wire["instruments"]["samples"].as_array_mut().unwrap() {
        sample["channels"] = 1.into();
    }
    let error = load_plan(&serde_json::to_vec(&wire).unwrap()).unwrap_err();
    assert_eq!(error.code, "E_PORT_TYPE", "{error:?}");
}

#[test]
fn stereo_core_pcm_declarations_are_checked() {
    use maac::compiler::compile_bundle_artifact;
    let notes = "note n { at = 0q; dur = 1q; pitch = A4; }";
    let with = |channels: &str| {
        let mut bundle = native_bundle(&tone(), 480, notes);
        let source = bundle.sources["main.maac"]
            .replace("frames = 480;", &format!("{channels} frames = 480;"));
        bundle.sources.insert("main.maac".into(), source);
        compile_bundle_artifact(&bundle)
    };
    // 480 mono frames of bytes cannot be 480 stereo frames.
    let error = with("channels = 2;").unwrap_err();
    assert!(
        error.iter().any(|d| d.code == DiagnosticCode::Asset),
        "{error:?}"
    );
    let error = with("channels = 3;").unwrap_err();
    assert!(
        error.iter().any(|d| d.code == DiagnosticCode::Range),
        "{error:?}"
    );
    with("channels = 1;").unwrap();
}

#[test]
fn stereo_values_share_the_embedded_budget() {
    // The budget counts values, so a stereo frame counts twice.
    let half = maac::graph::MAX_EMBEDDED_SAMPLES / 2;
    let fits = stereo_wav(&vec![0.0; half], &vec![0.0; half], 48_000);
    let sample = InstrumentSample::from_wav("fits", &fits, 69, None).unwrap();
    assert_eq!(sample.frames(), half as u64);
    let over = stereo_wav(&vec![0.0; half + 1], &vec![0.0; half + 1], 48_000);
    assert_eq!(
        InstrumentSample::from_wav("over", &over, 69, None)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn channel_defaults_do_not_change_execution_identity() {
    use maac::compiler::compile_bundle_artifact;
    // Execution identity is recorded by a production delivery extension.
    let identity = |declaration_channels: &str, node_channels: &str| {
        let mut bundle = native_bundle(&tone(), 480, "note n { at = 0q; dur = 1q; pitch = A4; }");
        let schema = maac::production_data::SCHEMA_BYTES;
        let mut source = bundle.sources["main.maac"]
            .replace(
                "frames = 480;",
                &format!("{declaration_channels} frames = 480;"),
            )
            .replace(
                "config = { zones",
                &format!("config = {{ {node_channels} zones"),
            )
            .replace(
                "output = &keys:out; }",
                "output = &keys:out; requires = [\"maac.production/1\"]; }",
            );
        source.push_str(&format!(
            "asset schema {{ kind = descriptor; path = \"production.schema.json\"; hash = \"{}\"; }}\n\
             extension deliveries {{ namespace = \"maac.production/1\"; schema = &schema; render_affecting = true; data = {{ deliveries = {{ release = {{ rate = 48000Hz; resampler = \"maac.src.kaiser/1\"; targets = {{ master = {{ role = master; output = &keys:out; encoding = wav_f32le; dither = {{ type = none; }}; }}; }}; }}; }}; }}; }}\n",
            sha256_digest(schema)
        ));
        bundle.sources.insert("main.maac".into(), source);
        bundle
            .assets
            .insert("production.schema.json".into(), schema.to_vec());
        let artifact = compile_bundle_artifact(&bundle).unwrap_or_else(|e| panic!("{e:?}"));
        let wire: serde_json::Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
        wire["production"]["execution_identity"]["execution_hash"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let omitted = identity("", "");
    assert_eq!(omitted, identity("channels = 1;", "channels = 1;"));
    assert_eq!(omitted, identity("channels = 1;", ""));
    assert_eq!(omitted, identity("", "channels = 1;"));
}
