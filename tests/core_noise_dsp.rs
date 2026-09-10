use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact;
use maac::dsp::DspEngine;
use maac::PlanArtifact;

const DEFAULT_SEED0_MONO_BITS: [u64; 6] = [
    0x3fe42215519abf64,
    0xbfeceb57250a99aa,
    0x3fe07884ab23ed32,
    0xbfe6286297abdc30,
    0x3fe67f064b2c92f8,
    0x3fd0a6d833fb18b8,
];

const DEFAULT_SEED0_STEREO_BITS: [[u64; 2]; 6] = [
    [0x3fe42215519abf64, 0x3fd889d544b2fdd0],
    [0xbfeceb57250a99aa, 0xbfe5a0f9fe515c66],
    [0x3fe07884ab23ed32, 0x3fe16dc03608c29c],
    [0xbfe6286297abdc30, 0xbfedeca7d8f16880],
    [0x3fe67f064b2c92f8, 0xbfc31505799b3ba0],
    [0x3fd0a6d833fb18b8, 0xbfe8aca7dd7fbb22],
];

const MAX_SEED_STEREO_BITS: [[u64; 2]; 4] = [
    [0xbfb18fc1f3955810, 0x3feb8bbaa9ec6eee],
    [0x3fb3ec62a2e9a0b0, 0x3fe88ae8025268bc],
    [0xbfe7a1e60c3126b6, 0x3fd2df9972aab7fc],
    [0xbfc5903bcf728ff8, 0x3fec74b642004658],
];

const RENAMED_MAX_SEED_STEREO_BITS: [[u64; 2]; 4] = [
    [0x3fe12db92686a9f2, 0xbfe968b05c4aa5d6],
    [0xbfd4271b3c272198, 0xbfd87ebc45caded0],
    [0xbf8f9de2a306ab00, 0xbfe028b00e606f50],
    [0xbfe866546f5bf410, 0xbfee28c15f2c77b4],
];

const LONG_ID_SEED0_STEREO_BITS: [[u64; 2]; 4] = [
    [0xbfe1fa0cc32c8fbe, 0x3fd3b9a1afb118e4],
    [0xbfee92d9baf12958, 0x3fedf827829960ee],
    [0x3fe32d6595d74e74, 0xbfee7673dbad9184],
    [0x3fbc71ecaedca0e0, 0x3fe932b67da71ddc],
];

// Independently generated with Python 3 hashlib.sha256: the payload prefix
// through the node-ID terminator is 108 bytes for this legal 86-byte ID, so
// frame 0 crosses the SHA-256 three-block path before the frame/channel bytes.
const LONGER_ID_SEED0_STEREO_BITS: [[u64; 2]; 4] = [
    [0x3fd602ab46c57698, 0xbfe5cf9045dfd6b8],
    [0xbfec6f33db9e12fa, 0x3fe8507ec623e7e6],
    [0xbfe21c33348655c6, 0xbfc0e7cd512ef188],
    [0xbfac0e283c8cb8c0, 0x3fe57e1196fe7cc4],
];

fn noise_source(
    project_seed: Option<&str>,
    node_id: &str,
    channels: u8,
    node_seed: Option<&str>,
    tail_frames: u8,
    extras: &str,
) -> String {
    let project_seed = project_seed
        .map(|seed| format!("seed = {seed};"))
        .unwrap_or_default();
    let node_seed = node_seed
        .map(|seed| format!(" seed = {seed};"))
        .unwrap_or_default();
    format!(
        r#"maac 1;
project noise_project {{ score = [0q, 1/12000q]; tail = {tail_frames}/48000s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &{node_id}:out; {project_seed} }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node {node_id} {{ type = "core.noise/1"; config = {{ channels = {channels};{node_seed} }}; }}
{extras}
"#,
        project_seed = project_seed,
        node_id = node_id,
        channels = channels,
        node_seed = node_seed,
        tail_frames = tail_frames,
        extras = extras,
    )
}

fn artifact(source: String) -> PlanArtifact {
    let compiled = compile_bundle_artifact(&SourceBundle::new("core-noise.maac", source))
        .expect("core noise source should compile");
    PlanArtifact::from_json(&compiled.to_json().unwrap()).unwrap()
}

fn render(artifact: &PlanArtifact) -> Vec<Vec<f64>> {
    let mut engine = DspEngine::new_artifact(artifact).unwrap();
    let mut samples = Vec::new();
    engine
        .render(|frame| {
            samples.push(frame.to_vec());
            Ok(())
        })
        .unwrap();
    samples
}

fn assert_bits(actual: f64, expected: u64) {
    assert_eq!(actual.to_bits(), expected);
}

fn assert_mono_bits(samples: &[Vec<f64>], expected: &[u64]) {
    assert_eq!(samples.len(), expected.len());
    for (frame, bits) in samples.iter().zip(expected) {
        assert_eq!(frame.len(), 1);
        assert_bits(frame[0], *bits);
    }
}

fn assert_stereo_bits(samples: &[Vec<f64>], expected: &[[u64; 2]]) {
    assert_eq!(samples.len(), expected.len());
    for (frame, bits) in samples.iter().zip(expected) {
        assert_eq!(frame.len(), 2);
        assert_bits(frame[0], bits[0]);
        assert_bits(frame[1], bits[1]);
    }
}

#[test]
fn noise_default_project_seed_matches_independent_mono_stereo_and_tail_vectors() {
    let mono = render(&artifact(noise_source(None, "noise", 1, None, 2, "")));
    assert_mono_bits(&mono, &DEFAULT_SEED0_MONO_BITS);

    let stereo = render(&artifact(noise_source(None, "noise", 2, None, 2, "")));
    assert_stereo_bits(&stereo, &DEFAULT_SEED0_STEREO_BITS);
    for (mono_frame, stereo_frame) in mono.iter().zip(&stereo) {
        assert_eq!(mono_frame[0].to_bits(), stereo_frame[0].to_bits());
    }
}

#[test]
fn noise_explicit_u64_seed_matches_vectors_and_identity_changes_are_intended() {
    let max_seed = render(&artifact(noise_source(
        Some("0"),
        "noise",
        2,
        Some("18446744073709551615"),
        0,
        "",
    )));
    assert_stereo_bits(&max_seed, &MAX_SEED_STEREO_BITS);

    let renamed = render(&artifact(noise_source(
        Some("0"),
        "renamed",
        2,
        Some("18446744073709551615"),
        0,
        "",
    )));
    assert_stereo_bits(&renamed, &RENAMED_MAX_SEED_STEREO_BITS);
    assert_ne!(max_seed, renamed);

    let seed_one = render(&artifact(noise_source(
        Some("0"),
        "noise",
        2,
        Some("1"),
        0,
        "",
    )));
    assert_ne!(max_seed, seed_one);

    let long_id = render(&artifact(noise_source(
        Some("0"),
        "noise_abcdefghijklmnop",
        2,
        None,
        0,
        "",
    )));
    assert_stereo_bits(&long_id, &LONG_ID_SEED0_STEREO_BITS);

    let longer_id = format!("noise_{}", "a".repeat(80));
    assert_eq!(longer_id.len(), 86);
    let longer_id_samples = render(&artifact(noise_source(
        Some("0"),
        &longer_id,
        2,
        None,
        0,
        "",
    )));
    assert_stereo_bits(&longer_id_samples, &LONGER_ID_SEED0_STEREO_BITS);
}

#[test]
fn noise_reset_and_retained_artifact_replay_are_bitwise_and_unrelated_track_independent() {
    let plain = artifact(noise_source(None, "noise", 2, None, 1, ""));
    let extra = artifact(noise_source(
        None,
        "noise",
        2,
        None,
        1,
        r#"node spare { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 1; }; }
pattern spare_phrase { length = 1/12000q; note n { at = 0q; dur = 1/12000q; pitch = 12000Hz; velocity = 1; } }
track spare_track { target = &spare:events; }
place spare_play { pattern = &spare_phrase; track = &spare_track; at = 0q; }"#,
    ));
    let expected = render(&plain);
    assert_eq!(render(&extra), expected);

    let retained = PlanArtifact::from_json(&plain.to_json().unwrap()).unwrap();
    assert_eq!(render(&retained), expected);

    let mut engine = DspEngine::new_artifact(&retained).unwrap();
    for _ in 0..2 {
        engine.reset();
        let mut replay = Vec::new();
        engine
            .render(|frame| {
                replay.push(frame.to_vec());
                Ok(())
            })
            .unwrap();
        assert_eq!(replay, expected);
    }
}

#[test]
fn noise_channels_and_node_ids_are_stream_local_and_tail_uses_absolute_frames() {
    let stereo = render(&artifact(noise_source(Some("0"), "noise", 2, None, 2, "")));
    assert_eq!(stereo.len(), 6);
    assert_ne!(stereo[0][0].to_bits(), stereo[0][1].to_bits());
    assert_ne!(stereo[0][0].to_bits(), stereo[1][0].to_bits());
    assert_ne!(stereo[4][0].to_bits(), stereo[0][0].to_bits());
    assert_ne!(stereo[5][1].to_bits(), stereo[1][1].to_bits());

    let mono = render(&artifact(noise_source(Some("0"), "noise", 1, None, 2, "")));
    assert_eq!(mono.len(), 6);
    for (mono_frame, stereo_frame) in mono.iter().zip(&stereo) {
        assert_eq!(mono_frame[0].to_bits(), stereo_frame[0].to_bits());
    }
}
