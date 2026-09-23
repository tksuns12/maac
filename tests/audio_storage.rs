use maac::{
    dsp::{AudioStorageMode, DspEngine},
    PlanArtifact,
};
use serde_json::json;

fn artifact_with_shared_audio() -> PlanArtifact {
    let samples = [1f32, 2., 4.];
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let mut value = json!({
        "version": 6,
        "output": {
            "score_start_q": "0/1",
            "score_end_q": "1/6000",
            "tail_seconds": "1/12000",
            "sample_rate_hz": 48000,
            "channels": 1,
            "total_frames": 12,
            "output": {"node": "mix", "port": "out"}
        },
        "tempo": {"points": [{"q": "0/1", "bpm": "60/1", "shape": "step"}]},
        "nodes": [{
            "id": "warp",
            "processor": {
                "kind": "warp_rate",
                "clip": {
                    "asset": "sample",
                    "channels": 1,
                    "at_q": "1/192000",
                    "source_start_frame": 0,
                    "source_end_frame": 3,
                    "warp": [
                        {"q": "0/1", "source_frame": 0},
                        {"q": "1/16000", "source_frame": 3}
                    ],
                    "gain": "1/1",
                    "fade_in_seconds": "0/1",
                    "fade_out_seconds": "0/1",
                    "fade_shape": "linear",
                    "source": {"object": "warp", "path": ["warp"]},
                    "start_frame": 1,
                    "end_frame": 4
                }
            }
        }],
        "audio_assets": [{
            "id": "sample",
            "format": "pcm_f32le_interleaved/1",
            "rate_hz": 48000,
            "channels": 1,
            "frames": samples.len(),
            "hash": maac::bundle::sha256_digest(&bytes),
            "bytes": bytes
        }]
    });

    let rate_clip = json!({
        "id": "rate",
        "processor": {
            "kind": "audio",
            "clip": {
                "asset": "sample",
                "channels": 1,
                "at": {"kind": "seconds", "seconds": "1/192000"},
                "source_start_frame": 0,
                "source_end_frame": 3,
                "speed": "1/1",
                "reverse": false,
                "gain": "1/1",
                "fade_in_seconds": "0/1",
                "fade_out_seconds": "0/1",
                "fade_shape": "linear",
                "source": {"object": "rate", "path": ["rate"]},
                "start_frame": 1,
                "end_frame": 4
            }
        }
    });
    value["nodes"].as_array_mut().unwrap().extend([
        rate_clip,
        json!({
            "id": "kit",
            "processor": {
                "kind": "kit",
                "channels": 1,
                "voices": 1,
                "samples": [{"key": "hit", "asset": "sample"}]
            }
        }),
        json!({
            "id": "mix",
            "processor": {
                "kind": "core",
                "processor": {"kind": "sum", "channels": 1}
            }
        }),
    ]);
    value["connections"] = json!([
        {"id": "warp_mix", "from": {"node": "warp", "port": "out"}, "to": {"node": "mix", "port": "in"}},
        {"id": "rate_mix", "from": {"node": "rate", "port": "out"}, "to": {"node": "mix", "port": "in"}},
        {"id": "kit_mix", "from": {"node": "kit", "port": "out"}, "to": {"node": "mix", "port": "in"}}
    ]);
    value["events"] = json!([{
        "address": "hit",
        "source": {"object": "hit", "path": ["hit"]},
        "target": {"node": "kit", "port": "events"},
        "kind": {"kind": "hit", "key": "hit", "velocity": "1/1"},
        "score_on_q": "0/1",
        "onset_offset_seconds": "0/1",
        "release_offset_seconds": "0/1",
        "release_velocity": 0.0,
        "on_frame": 0
    }]);
    PlanArtifact::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn bits(samples: &[f64]) -> Vec<u64> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}

fn render_with_mode(
    artifact: &PlanArtifact,
    storage_mode: AudioStorageMode,
    with_limits: bool,
) -> Vec<f64> {
    let mut engine = if with_limits {
        DspEngine::new_artifact_with_limits_and_storage_mode(
            artifact,
            &Default::default(),
            storage_mode,
        )
    } else {
        DspEngine::new_artifact_with_storage_mode(artifact, storage_mode)
    }
    .unwrap();
    let mut output = Vec::new();
    engine
        .render(|frame| {
            output.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    output
}

fn render_default_memory(artifact: &PlanArtifact) -> Vec<f64> {
    let mut engine = DspEngine::new_artifact(artifact).unwrap();
    let mut output = Vec::new();
    engine
        .render(|frame| {
            output.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    output
}

#[test]
fn memory_and_disk_storage_match_for_clips_warp_and_kit_and_replay() {
    let artifact = artifact_with_shared_audio();
    let serialized_before = artifact.to_json().unwrap();
    let memory = render_default_memory(&artifact);

    assert_eq!(
        bits(&render_with_mode(
            &artifact,
            AudioStorageMode::Memory,
            false
        )),
        bits(&memory)
    );

    let mut disk_engine =
        DspEngine::new_artifact_with_storage_mode(&artifact, AudioStorageMode::Disk).unwrap();
    for _ in 0..2 {
        disk_engine.reset();
        let mut disk_output = Vec::new();
        disk_engine
            .render(|frame| {
                disk_output.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        assert_eq!(bits(&disk_output), bits(&memory));
    }

    assert_eq!(
        bits(&render_with_mode(&artifact, AudioStorageMode::Disk, true)),
        bits(&memory)
    );
    assert_eq!(artifact.to_json().unwrap(), serialized_before);
}
