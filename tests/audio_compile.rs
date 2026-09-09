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
    assert_eq!(artifact.version(), 5);
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
fn defaults_are_native_sources_with_exact_assets_and_source_mapping() {
    let b=bundle("track group {} audio clip { asset=&take; at=0q; source=[0frame,3frame]; mode=rate; track=&group; }",false);
    let v = json(&b);
    assert!(v["events"].as_array().unwrap().is_empty());
    let c = clip(&v, "clip");
    assert_eq!(c["speed"], "1/1");
    assert_eq!(c["gain"], "1/1");
    assert_eq!(c["reverse"], false);
    assert_eq!(c["fade_in_seconds"], "0/1");
    assert_eq!(c["fade_out_seconds"], "0/1");
    assert_eq!(c["fade_shape"], "linear");
    assert_eq!(c["track"], "group");
    assert_eq!(c["source"]["object"], "clip");
    assert_eq!(c["source"]["path"], serde_json::json!(["clip"]));
    assert!(
        c["source"]["span"]["end"].as_u64().unwrap()
            > c["source"]["span"]["start"].as_u64().unwrap()
    );
    assert_eq!(v["nodes"][0]["params"], serde_json::json!({}));
    assert_eq!(
        (c["start_frame"].as_u64(), c["end_frame"].as_u64()),
        (Some(0), Some(6))
    );
    assert_eq!(
        v["audio_assets"][0]["bytes"],
        serde_json::json!(b.assets["take.pcm"])
    );
    check_bundle_artifact(&b).unwrap();
}
#[test]
fn all_global_anchors_reverse_fades_and_fractional_bounds() {
    for (at, kind, value) in [
        ("1q", "score", "1/1"),
        ("bar(1,1)", "score", "0/1"),
        ("1/192000s", "seconds", "1/192000"),
        ("1/192ms", "seconds", "1/192000"),
    ] {
        let b=bundle(&format!("audio clip {{ asset=&take; at={at}; source=[0frame,3frame]; mode=rate; speed=2; reverse=true; gain=1/2; fade_in=1/48ms; fade_out=1/24ms; fade_shape=equal_power; }}"),false);
        let v = json(&b);
        let c = clip(&v, "clip");
        assert_eq!(c["at"]["kind"], kind);
        assert_eq!(
            c["at"][if kind == "score" { "q" } else { "seconds" }],
            value
        );
        assert_eq!(c["reverse"], true);
        assert_eq!(c["fade_shape"], "equal_power");
        assert!(c.get("track").is_none());
        if kind == "seconds" {
            assert_eq!(c["start_frame"], 1);
            assert_eq!(c["end_frame"], 4);
        }
    }
}
#[test]
fn zero_sample_tail_and_ramp_sources_compile_without_synthetic_events() {
    for (at, speed, start, end) in [
        ("1/192000s", 100, 1, 1),
        ("767999/192000s", 1, 192000, 192006),
    ] {
        let v=json(&bundle(&format!("audio clip {{ asset=&take; at={at}; source=[0frame,3frame]; mode=rate; speed={speed}; }}"),false));
        let c = clip(&v, "clip");
        assert_eq!(c["start_frame"], start);
        assert_eq!(c["end_frame"], end);
    }
    let v = json(&bundle(
        "audio clip { asset=&take; at=1q; source=[0frame,3frame]; mode=rate; }",
        true,
    ));
    assert!(clip(&v, "clip")["start_frame"].as_u64().unwrap() > 0);
}
#[test]
fn mixed_native_nodes_keep_note_hit_counts_and_sort_graph_ids() {
    let b = bundle(
        r#"audio clip { asset=&take; at=0q; source=[0frame,3frame]; mode=rate; }
node kit { type="core.kit/1"; config={channels=1; samples=[{key="k";asset=&take;}];}; }
node sine { type="core.sine/1"; }
track notes { target=&sine:events; } track hits { target=&kit:events; }
pattern np { length=1q; note n { at=0q; dur=1q; pitch=440Hz; } }
pattern hp { length=1q; hit h { at=0q; key="k"; } }
place n { pattern=&np;track=&notes;at=0q; } place h {pattern=&hp;track=&hits;at=0q;}
"#,
        false,
    );
    let v = json(&b);
    assert_eq!(v["events"].as_array().unwrap().len(), 2);
    let ids: Vec<_> = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["clip", "kit", "sine"]);
}
#[test]
fn invalid_modes_intervals_references_and_budgets_fail() {
    for fields in [
        "at=0q;source=[0frame,3frame];mode=warp_rate;",
        "at=-1s;source=[0frame,3frame];mode=rate;",
        "at=4q;source=[0frame,3frame];mode=rate;",
        "at=0q;source=[0frame,4frame];mode=rate;",
        "at=0q;source=[0frame,3frame];mode=rate;track=&missing;",
    ] {
        assert!(
            compile_bundle_artifact(&bundle(
                &format!("audio clip {{ asset=&take;{fields} }}"),
                false
            ))
            .is_err(),
            "{fields}"
        );
    }
    let b = bundle(
        "audio clip {asset=&take;at=0q;source=[0frame,3frame];mode=rate;}",
        false,
    );
    let limits = PlanLimits {
        max_nodes: 0,
        ..PlanLimits::default()
    };
    assert!(compile_bundle_artifact_with_limits(&b, &limits).is_err());
}

#[test]
fn bar_coordinates_match_global_q_and_reject_invalid_arguments() {
    let source = |at: &str| {
        bundle(
            &format!("audio clip {{ asset=&take; at={at}; source=[0frame,3frame]; mode=rate; }}"),
            false,
        )
    };
    let q = json(&source("1q"));
    let bar = json(&source("bar(1,2)"));
    let mut q_clip = clip(&q, "clip").clone();
    let mut bar_clip = clip(&bar, "clip").clone();
    q_clip.as_object_mut().unwrap().remove("source");
    bar_clip.as_object_mut().unwrap().remove("source");
    assert_eq!(q_clip, bar_clip);
    for at in [
        "bar(1)",
        "bar(1/2,1)",
        "bar(1,0)",
        "bar(1,5)",
        "bar(2,1)",
        "bar(99999999999999999999999,1)",
    ] {
        assert!(compile_bundle_artifact(&source(at)).is_err(), "{at}");
    }
}
