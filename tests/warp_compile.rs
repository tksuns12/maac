use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::{
    check_bundle_artifact, compile_bundle_artifact, compile_bundle_artifact_with_limits,
};
use maac::{plan::PlanLimits, PlanArtifact};
use serde_json::Value;

fn bundle(body: &str, ramp: bool) -> SourceBundle {
    let bytes: Vec<u8> = [1f32, 2., 3.]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let tempo = if ramp {
        "[(0q, 60bpm, linear), (4q, 120bpm, step)]"
    } else {
        "[(0q, 60bpm, step)]"
    };
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,4q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&clip:out; tail=1s; }}
tempo clock {{ points={tempo}; }} meter metre {{ points=[(0q,4,4)]; }}
asset take {{ kind=audio; path="take.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=24000Hz; channels=1; frames=3; }}
{body}"#,
        sha256_digest(&bytes)
    );
    let mut b = SourceBundle::new("score.maac", source);
    b.assets.insert("take.pcm".into(), bytes);
    b
}
fn json(b: &SourceBundle) -> Value {
    let artifact = compile_bundle_artifact(b).unwrap();
    assert_eq!(artifact.version(), 6);
    assert_eq!(artifact.audio_clip_count(), 1);
    let bytes = artifact.to_json().unwrap();
    assert_eq!(
        PlanArtifact::from_json(&bytes).unwrap().to_json().unwrap(),
        bytes
    );
    serde_json::from_slice(&bytes).unwrap()
}
fn clip<'a>(v: &'a Value, id: &str) -> &'a Value {
    &v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .unwrap()["processor"]["clip"]
}
#[test]
fn native_warp_defaults_and_exact_recipe() {
    let v=json(&bundle("audio clip { asset=&take;at=0q;source=[0frame,3frame];mode=warp_rate;warp=[(0q,0frame),(1q,3frame)]; }",false));
    let c = clip(&v, "clip");
    assert_eq!(c["at_q"], "0/1");
    assert_eq!(c["gain"], "1/1");
    assert_eq!(c["fade_in_seconds"], "0/1");
    assert_eq!(c["fade_shape"], "linear");
    assert!(c.get("speed").is_none());
    assert!(c.get("reverse").is_none());
    assert_eq!(c["start_frame"], 0);
    assert_eq!(c["end_frame"], 48000);
    assert_eq!(v["nodes"][0]["processor"]["kind"], "warp_rate");
    assert!(v["events"].as_array().unwrap().is_empty());
}

const WARP: &str = "audio clip {asset=&take;at=0q;source=[0frame,3frame];mode=warp_rate;warp=[(0q,0frame),(1q,3frame)];}";
#[test]
fn musical_anchors_fractional_starts_and_tail_tempo() {
    for (at, expected) in [("1q", 48000), ("bar(1,2)", 48000), ("1/192000q", 1)] {
        let b = bundle(&WARP.replace("at=0q", &format!("at={at}")), false);
        let v = json(&b);
        assert_eq!(clip(&v, "clip")["start_frame"], expected);
        check_bundle_artifact(&b).unwrap();
    }
    let mut b = bundle(
        &WARP
            .replace("at=0q", "at=3q")
            .replace("(1q,3frame)", "(2q,3frame)"),
        false,
    );
    let text = b.sources.get_mut("score.maac").unwrap();
    *text = text.replace("[(0q, 60bpm, step)]", "[(0q,60bpm,step),(4q,120bpm,step)]");
    assert_eq!(clip(&json(&b), "clip")["end_frame"], 216000);
    assert!(compile_bundle_artifact(&bundle(WARP, true)).is_ok());
}
#[test]
fn mixed_rate_warp_kit_and_note_records_remain_native() {
    let rate = "audio rateclip {asset=&take;at=0q;source=[0frame,3frame];mode=rate;}";
    let extra = r#"node kit {type="core.kit/1";config={channels=1;samples=[{key="k";asset=&take;}];};}
node sine {type="core.sine/1";}
track notes {target=&sine:events;} track hits {target=&kit:events;}
pattern np {length=1q;note n {at=0q;dur=1q;pitch=440Hz;}}
pattern hp {length=1q;hit h {at=0q;key="k";}}
place n {pattern=&np;track=&notes;at=0q;} place h {pattern=&hp;track=&hits;at=0q;}
"#;
    let b = bundle(&format!("{WARP}{rate}{extra}"), false);
    let artifact = compile_bundle_artifact(&b).unwrap();
    assert_eq!(artifact.audio_clip_count(), 2);
    assert_eq!(artifact.event_count(), 2);
    let v: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    assert_eq!(clip(&v, "rateclip")["speed"], "1/1");
    assert_eq!(clip(&v, "rateclip")["reverse"], false);
    assert!(clip(&v, "clip").get("speed").is_none());
    let old = compile_bundle_artifact(&bundle(
        &WARP.replace(
            "mode=warp_rate;warp=[(0q,0frame),(1q,3frame)];",
            "mode=rate;",
        ),
        false,
    ))
    .unwrap();
    assert_eq!(old.version(), 5);
}
#[test]
fn warp_rejections_and_shared_expanded_point_budget() {
    for change in [
        "mode=warp_preserve;",
        "mode=warp_rate;speed=1;",
        "mode=warp_rate;reverse=false;",
        "mode=warp_rate;processor=&take;",
    ] {
        assert!(
            compile_bundle_artifact(&bundle(&WARP.replace("mode=warp_rate;", change), false))
                .is_err()
        );
    }
    for at in ["0s", "bar(1/2,1)", "bar(1,0)", "bar(2,1)"] {
        assert!(compile_bundle_artifact(&bundle(
            &WARP.replace("at=0q", &format!("at={at}")),
            false
        ))
        .is_err());
    }
    let extra = r#"node sine {type="core.sine/1";config={voices=4;};} track notes {target=&sine:events;}
curve bend {clock=normalized;points=[(0,1,linear),(1,1,step)];}
curve level {clock=score;points=[(0q,1,step)];}
automation a {target=&sine.params.level;curve=&level;at=0q;}
pattern np {length=1q;note n {at=0q;dur=1q;pitch=440Hz;expression e {kind=gain;curve=&bend;}}}
place n {pattern=&np;track=&notes;at=0q;count=2;}
"#;
    let b = bundle(&format!("{WARP}{extra}"), false);
    for (max, pass) in [(6, false), (7, true)] {
        let result = compile_bundle_artifact_with_limits(
            &b,
            &PlanLimits {
                max_automation_points: max,
                ..Default::default()
            },
        );
        assert_eq!(result.is_ok(), pass);
        if !pass {
            assert!(result.unwrap_err().iter().any(|error| {
                error.code == maac::DiagnosticCode::ResourceLimit
                    && error.message
                        == "aggregate automation, expression and warp point budget exceeded"
            }));
        }
    }

    assert!(compile_bundle_artifact_with_limits(
        &b,
        &PlanLimits {
            max_work: 0,
            ..Default::default()
        }
    )
    .is_err());
}

use maac::production_data::SCHEMA_BYTES;
fn production_bundle(rate: u32, sample: f32, checks: &str) -> SourceBundle {
    let bytes: Vec<u8> = [sample, sample / 2., 0.]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let source = format!(
        r#"maac 1;
        project p {{ score=[0q,1/25q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&master:out; tail=1/100s; requires=["maac.production/1"]; }}
        tempo clock {{ points=[(0q,120bpm,linear),(1/25q,240bpm,step)]; }}
        meter metre {{ points=[(0q,4,4)]; }}
        asset schema {{ kind=descriptor; path="production.schema.json"; hash="{}"; }}
        asset sample {{ kind=audio; path="sample.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=24000Hz; channels=1; frames=3; }}
        audio clip {{asset=&sample;at=0q;source=[0frame,3frame];mode=rate;}}
        node room {{ type="fx.reverb/1"; config={{channels=1;}}; params={{mix=0.5;}}; }}
        node compressor {{ type="fx.compressor/1"; config={{channels=1;detector=external;sidechain_channels=1;}}; params={{ratio=2;attack=0s;release=0s;}}; }}
        node master {{ type="core.pan/1"; }}
        connect room_in {{from=&clip:out;to=&room:in;}}
        connect compressor_in {{from=&room:out;to=&compressor:in;}}
        connect detector {{from=&clip:out;to=&compressor:sidechain;}}
        connect master_in {{from=&compressor:out;to=&master:in;}}
        extension deliveries {{namespace="maac.production/1";schema=&schema;render_affecting=true;data={{deliveries={{release={{rate={rate}Hz;resampler="maac.src.kaiser/1";targets={{
            master={{role=master;output=&master:out;encoding=wav_f32le;dither={{type=none;}};{checks}}};
            stem={{role=stem;output=&clip:out;encoding=wav_f32le;dither={{type=none;}};}};
            shared={{role=stem;output=&room:out;encoding=wav_f32le;dither={{type=none;}};}};
        }};}};}};}};}}
    "#,
        sha256_digest(SCHEMA_BYTES),
        sha256_digest(&bytes)
    );
    let mut bundle = SourceBundle::new("scores/main.maac", source);
    bundle
        .assets
        .insert("production.schema.json".into(), SCHEMA_BYTES.to_vec());
    bundle.assets.insert("sample.pcm".into(), bytes);
    bundle
}
fn identity(plan: &PlanArtifact) -> Value {
    let value: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    value["production"]["execution_identity"].clone()
}

#[test]
fn production_warp_identity_normalizes_only_mode_valid_defaults() {
    let mut b = production_bundle(48000, 0.5, "");
    let text = b.sources.get_mut("scores/main.maac").unwrap();
    *text = text.replace(
        "mode=rate;",
        "mode=warp_rate;warp=[(0q,0frame),(1/50q,3frame)];",
    );
    let base = identity(&compile_bundle_artifact(&b).unwrap());
    let normalized = base["normalized_source_json"].as_str().unwrap();
    assert!(!normalized.contains("\"speed\""));
    assert!(!normalized.contains("\"reverse\""));
    let mut explicit = b.clone();
    let text = explicit.sources.get_mut("scores/main.maac").unwrap();
    *text = text
        .replace(
            "mode=warp_rate;",
            "mode=warp_rate;gain=1;fade_in=0ms;fade_out=0s;fade_shape=linear;",
        )
        .replace("at=0q;source=", "at=bar(1,1);source=");
    assert_eq!(
        base["execution_hash"],
        identity(&compile_bundle_artifact(&explicit).unwrap())["execution_hash"]
    );
    for (old, new) in [
        ("1/50q,3frame", "1/100q,3frame"),
        ("120bpm", "121bpm"),
        (
            "mode=warp_rate;warp=[(0q,0frame),(1/50q,3frame)];",
            "mode=rate;",
        ),
    ] {
        let mut changed = b.clone();
        let text = changed.sources.get_mut("scores/main.maac").unwrap();
        *text = text.replace(old, new);
        assert_ne!(
            base["execution_hash"],
            identity(&compile_bundle_artifact(&changed).unwrap())["execution_hash"]
        );
    }
    let mut changed = b.clone();
    let bytes: Vec<u8> = [0.25f32, 0.125, 0.]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let old_hash = sha256_digest(&changed.assets["sample.pcm"]);
    changed.assets.insert("sample.pcm".into(), bytes.clone());
    let text = changed.sources.get_mut("scores/main.maac").unwrap();
    *text = text.replace(&old_hash, &sha256_digest(&bytes));
    assert_ne!(
        base["execution_hash"],
        identity(&compile_bundle_artifact(&changed).unwrap())["execution_hash"]
    );
}
