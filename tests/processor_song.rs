//! One song that uses every `synth.*/1` processor, some with their config
//! and parameters written out and some with their defaults left implicit.
//! Its plan bytes, its execution hash, its N(A) and the work its plan
//! needs are pinned, so a change to how the compiler lowers, normalizes or
//! charges a processor shows here. The samples are not pinned: they are
//! only claimed equal within one executable and environment.

use std::collections::BTreeMap;
use std::io::Cursor;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::library::LibrarySet;
use maac::normalization::{normalize_document, NormalizationContext};
use maac::plan::PlanLimits;
use maac::production_identity::bundle_execution_identity;

/// A mono float WAV of `samples` at `rate`.
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

/// A triangle of eight steps, each value exact in binary32.
const STEPS: [f32; 8] = [0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5];

fn bundle() -> SourceBundle {
    let tone = float_wav(&STEPS.repeat(60), 24_000);
    // Two cycles of eight: the triangle, then a square.
    let mut cycles = STEPS.to_vec();
    cycles.extend([1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0]);
    let colors = float_wav(&cycles, 48_000);
    let source = SOURCE
        .replace("{tone}", &sha256_digest(&tone))
        .replace("{colors}", &sha256_digest(&colors));
    SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([("main.maac".into(), source)]),
        assets: BTreeMap::from([("tone.wav".into(), tone), ("colors.wav".into(), colors)]),
    }
}

const SOURCE: &str = r#"maac 1;
project p { score = [0q, 2q]; rate = 48000Hz; tail = 1/2s; tempo = &clock; meter = &metre; output = &fx:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
sample tone { path = "tone.wav"; hash = "{tone}"; root = A4; }
wavetable colors { path = "colors.wav"; hash = "{colors}"; cycle_length = 8; }

instrument every {
  channels = 2;
  voice v {
    channels = 1; amplitude = &amp; output = &shape:out;
    node amp { type = "synth.adsr/1"; config = { on_join = restart; }; params = { attack = 5ms; release = 80ms; curve = 2; }; }
    node sine { type = "synth.sine/1"; params = { ratio = 2; level = 1/5; }; }
    node saw { type = "synth.saw/1"; params = { level = 1/10; }; }
    node square { type = "synth.square/1"; params = { phase = 1/4; level = 1/10; }; }
    node tri { type = "synth.triangle/1"; params = { frequency = 3Hz; level = 1/10; }; }
    node hiss { type = "synth.noise/1"; params = { level = 1/20; }; }
    node grain { type = "synth.noise/1"; config = { seed = 7; per_note = true; }; params = { level = 1/20; }; }
    node string { type = "synth.pluck/1"; config = { seed = 5; }; params = { decay = 1s; hardness = 1/2; }; }
    node spot { type = "synth.random/1"; config = { seed = 9; }; }
    node bank { type = "synth.modal/1"; config = { modes = 6; strings = 2; }; params = { decay = 1/2s; position = 1/8; detune = 3; }; }
    node wave { type = "synth.wavetable/1"; config = { table = &colors; }; params = { position = 1/2; level = 1/10; }; }
    node play { type = "synth.sample/1"; config = { zones = [{ sample = &tone; low = key(0); high = key(127); }]; }; params = { level = 1/10; }; }
    node color { type = "synth.timbre/1"; }
    node touch { type = "synth.pressure/1"; }
    node vel { type = "synth.velocity/1"; }
    node pitch { type = "synth.key/1"; }
    node wobble { type = "synth.lfo/1"; params = { frequency = 5Hz; level = 1/2; }; }
    node sum { type = "synth.mix/1"; config = { channels = 1; }; }
    node trim { type = "synth.gain/1"; config = { channels = 1; }; params = { level = 4/5; }; }
    node shape { type = "synth.svf/1"; config = { channels = 1; mode = bandpass; }; params = { cutoff = 2kHz; ratio = 2; q = 2; }; }
    connect sine_sum { from = &sine:out; to = &sum:in; }
    connect saw_sum { from = &saw:out; to = &sum:in; }
    connect square_sum { from = &square:out; to = &sum:in; }
    connect tri_sum { from = &tri:out; to = &sum:in; }
    connect hiss_sum { from = &hiss:out; to = &sum:in; }
    connect string_sum { from = &string:out; to = &sum:in; }
    connect wave_sum { from = &wave:out; to = &sum:in; }
    connect play_sum { from = &play:out; to = &sum:in; }
    connect grain_bank { from = &grain:out; to = &bank:in; }
    connect bank_sum { from = &bank:out; to = &sum:in; }
    connect sum_trim { from = &sum:out; to = &trim:in; }
    connect trim_shape { from = &trim:out; to = &shape:in; }
    modulate wobble_cutoff { from = &wobble:out; to = &shape.params.cutoff; depth = 500Hz; }
    modulate spot_position { from = &spot:out; to = &bank.params.position; depth = 1/40; }
    modulate color_trim { from = &color:out; to = &trim.params.level; depth = 1/10; }
    modulate touch_trim { from = &touch:out; to = &trim.params.level; depth = 1/10; }
    modulate vel_trim { from = &vel:out; to = &trim.params.level; depth = 1/10; }
    modulate pitch_sine { from = &pitch:out; to = &sine.params.level; depth = 1/20; }
  }
  shared room {
    channels = 2; output = &field:out;
    node dark { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 6kHz; }; }
    node thin { type = "synth.highpass/1"; config = { channels = 1; }; params = { cutoff = 80Hz; }; }
    node heat { type = "synth.drive/1"; config = { channels = 1; }; params = { drive = 2; bias = 1/10; }; }
    node place { type = "synth.pan/1"; params = { pan = -1/4; }; }
    node lean { type = "synth.balance/1"; params = { balance = 1/5; }; }
    node field { type = "synth.width/1"; params = { width = 3/2; }; }
    node sway { type = "synth.lfo/1"; params = { frequency = 1Hz; }; }
    connect in_dark { from = &input:out; to = &dark:in; }
    connect dark_thin { from = &dark:out; to = &thin:in; }
    connect thin_heat { from = &thin:out; to = &heat:in; }
    connect heat_place { from = &heat:out; to = &place:in; }
    connect place_lean { from = &place:out; to = &lean:in; }
    connect lean_field { from = &lean:out; to = &field:in; }
    modulate sway_pan { from = &sway:out; to = &place.params.pan; depth = 1/10; }
  }
}

instrument section {
  channels = 2;
  voice v {
    channels = 2; amplitude = &amp; output = &bows:out;
    node amp { type = "synth.adsr/1"; params = { sustain = 1; release = 200ms; }; }
    node bows { type = "synth.bow/1"; config = { players = 2; body = cello; channels = 2; seed = 3; }; params = { vibrato = 1/2; level = 3/10; }; }
  }
}

effect echo {
  channels = 2;
  shared main {
    channels = 2; output = &out_mix:out;
    node feed { type = "synth.mix/1"; config = { channels = 2; }; }
    node line { type = "synth.delay/1"; config = { channels = 2; clock = score; max = 2s; }; params = { time = 1/4q; feedback = 1/5; }; }
    node cross { type = "synth.width/1"; params = { width = -1/2; }; }
    node regen { type = "synth.gain/1"; config = { channels = 2; }; params = { level = 2/5; }; }
    node plain { type = "synth.delay/1"; config = { channels = 2; max = 1/2s; }; params = { time = 30ms; }; }
    node out_mix { type = "synth.mix/1"; config = { channels = 2; }; }
    connect in_feed { from = &input:out; to = &feed:in; }
    connect feed_line { from = &feed:out; to = &line:in; }
    connect line_cross { from = &line:out; to = &cross:in; }
    connect cross_regen { from = &cross:out; to = &regen:in; }
    connect regen_feed { from = &regen:out; to = &feed:in; }
    connect in_plain { from = &input:out; to = &plain:in; }
    connect in_out { from = &input:out; to = &out_mix:in; }
    connect line_out { from = &line:out; to = &out_mix:in; }
    connect plain_out { from = &plain:out; to = &out_mix:in; }
  }
}

node keys { instrument = &every; config = { voices = 2; }; }
node strings { instrument = &section; }
node fx { effect = &echo; }
connect keys_fx { from = &keys:out; to = &fx:in; }
connect strings_fx { from = &strings:out; to = &fx:in; }
track lead { target = &keys:events; }
track bowed { target = &strings:events; }
pattern tune { length = 2q; note a { at = 0q; dur = 1/2q; pitch = A4; velocity = 0.7; } note b { at = 1q; dur = 1/2q; pitch = E5; } }
pattern hold { length = 2q; note c { at = 0q; dur = 3/2q; pitch = D3; } }
place lead_tune { pattern = &tune; track = &lead; at = 0q; }
place bowed_hold { pattern = &hold; track = &bowed; at = 0q; }
"#;

/// The least `max_execution_work` the plan validates under.
fn minimum_work(artifact: &maac::PlanArtifact) -> u64 {
    let limits = |max_execution_work| PlanLimits {
        max_execution_work,
        ..PlanLimits::song()
    };
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
fn the_song_of_every_processor_compiles_normalizes_and_is_charged_as_pinned() {
    let bundle = bundle();
    let artifact = compile_bundle_artifact_with_limits(&bundle, &PlanLimits::song())
        .unwrap_or_else(|error| panic!("{error:?}"));
    let plan = artifact.to_json_with_limits(&PlanLimits::song()).unwrap();

    let identity = bundle_execution_identity(&bundle).unwrap_or_else(|e| panic!("{e:?}"));
    let document = maac::parse(&bundle.sources["main.maac"]).unwrap();

    let resolved = bundle.resolve().unwrap();
    let libraries = LibrarySet::resolve(&resolved).unwrap();
    let authored = document.to_syntax_json_value();
    let context = NormalizationContext::new(&authored, "main.maac", Some(&libraries)).unwrap();
    let normalized = normalize_document(&context).unwrap();
    let normalized = serde_json::to_vec(&normalized).unwrap();

    let actual = format!(
        "plan {}\nexecution {}\nN(A) {}\nwork {}\n",
        sha256_digest(&plan),
        identity.execution_hash,
        sha256_digest(&normalized),
        minimum_work(&artifact),
    );
    assert_eq!(actual, PINNED, "{actual}");
}

/// Measured on the code before any processor fact moved (5a85b4e).
const PINNED: &str = "\
plan sha256:ad8279137b72e555f4a3641ddfc841d829e4dc233debb608d82237e69dec28e2
execution sha256:d70e2a3f3921bc07037e8036fb36c19feef59edbdba49cb0ccbc3006bdbe627c
N(A) sha256:bb3692fc6e535cf416c78f5a6c628f7b7b5f2638fb42843fa0517221bc759969
work 7354084
";

const IDENTITIES: [&str; 27] = [
    "synth.sine/1",
    "synth.saw/1",
    "synth.square/1",
    "synth.triangle/1",
    "synth.noise/1",
    "synth.pluck/1",
    "synth.random/1",
    "synth.modal/1",
    "synth.bow/1",
    "synth.wavetable/1",
    "synth.sample/1",
    "synth.adsr/1",
    "synth.timbre/1",
    "synth.pressure/1",
    "synth.velocity/1",
    "synth.key/1",
    "synth.lfo/1",
    "synth.gain/1",
    "synth.onepole/1",
    "synth.highpass/1",
    "synth.svf/1",
    "synth.drive/1",
    "synth.mix/1",
    "synth.pan/1",
    "synth.balance/1",
    "synth.width/1",
    "synth.delay/1",
];

#[test]
fn the_song_of_every_processor_uses_every_processor_and_renders() {
    let source = SOURCE;
    let mut missing: Vec<&str> = IDENTITIES
        .iter()
        .copied()
        .filter(|identity| !source.contains(&format!("\"{identity}\"")))
        .collect();
    missing.sort();
    assert!(missing.is_empty(), "{missing:?}");

    let artifact = compile_bundle_artifact_with_limits(&bundle(), &PlanLimits::song()).unwrap();
    let mut peak = 0.0f64;
    maac::render_artifact_with_limits(&artifact, &PlanLimits::song(), |frame| {
        for sample in frame {
            assert!(sample.is_finite());
            peak = peak.max(sample.abs());
        }
        Ok(())
    })
    .unwrap();
    assert!(peak > 1e-3, "{peak}");
}
