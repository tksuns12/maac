use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::{
    check_bundle_artifact, compile_bundle_artifact, compile_bundle_artifact_with_limits,
    compile_bundle_versioned,
};
use maac::plan::PlanLimits;
use maac::PlanArtifact;
use serde_json::Value;

fn bundle(body: &str, ramp: bool) -> SourceBundle {
    let bytes: Vec<u8> = [1f32, 0.5].iter().flat_map(|v| v.to_le_bytes()).collect();
    let tempo = if ramp {
        "[(0q, 120bpm, linear), (4q, 240bpm, step)]"
    } else {
        "[(0q, 120bpm, step)]"
    };
    let source = format!(
        r#"maac 1;
        project p {{ score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &kit:out; tail = 1s; }}
        tempo clock {{ points = {tempo}; }} meter metre {{ points = [(0q, 4, 4)]; }}
        asset sample {{ kind = audio; path = "samples/kick.pcm"; hash = "{}"; format = "pcm_f32le_interleaved/1"; rate = 24000Hz; channels = 1; frames = 2; }}
        node kit {{ type = "core.kit/1"; config = {{ channels = 1; samples = [{{ key = ""; asset = &sample; }}]; }}; }}
        track t {{ target = &kit:events; }} {body}"#,
        sha256_digest(&bytes)
    );
    let mut bundle = SourceBundle::new("scores/main.maac", source);
    bundle.assets.insert("samples/kick.pcm".into(), bytes);
    bundle
}
fn json(bundle: &SourceBundle) -> Value {
    let artifact = compile_bundle_artifact(bundle).unwrap();
    assert_eq!(artifact.version(), 4);
    let bytes = artifact.to_json().unwrap();
    assert_eq!(
        PlanArtifact::from_json(&bytes).unwrap().to_json().unwrap(),
        bytes
    );
    serde_json::from_slice(&bytes).unwrap()
}
#[test]
fn nested_hits_edits_and_source_free_assets() {
    let bundle = bundle(
        r#"
        pattern inner { length = 1q; hit h { at = 0q; key = "wrong"; onset_offset = 1ms; } }
        pattern outer { length = 2q; use u { pattern = &inner; at = 0q; count = 2; transpose = 300ct; boundary = cut; } }
        place place { pattern = &outer; track = &t; at = 0q; stretch = 2; boundary = cut;
            override edit { event = "0/u/0/h"; set = { key = ""; at = 1q; }; }
            override gone { event = "0/u/1/h"; delete = true; }
            insert once { hit inserted { at = 2q; key = ""; velocity = 0; } }
        }
    "#,
        false,
    );
    let plan = json(&bundle);
    assert_eq!(plan["events"].as_array().unwrap().len(), 2);
    assert_eq!(plan["events"][0]["address"], "place/0/u/0/h");
    assert_eq!(plan["events"][0]["on_frame"], 24048);
    assert!(plan["events"][0]["off_frame"].is_null());
    assert!(plan["events"][0]["score_off_q"].is_null());
    assert_eq!(plan["events"][0]["release_offset_seconds"], "0/1");
    assert_eq!(plan["events"][0]["release_velocity"], 0.0);
    assert_eq!(
        plan["audio_assets"][0]["bytes"],
        serde_json::json!(bundle.assets["samples/kick.pcm"])
    );
    check_bundle_artifact(&bundle).unwrap();
    assert!(compile_bundle_versioned(&bundle).is_err());
}
#[test]
fn step_and_ramp_timing_reject_outside_onsets_and_final_frame_collision() {
    for ramp in [false, true] {
        let valid = bundle(
            r#"pattern pat { length = 4q; hit h { at = 1q; key = ""; } } place place { pattern = &pat; track = &t; at = 0q; }"#,
            ramp,
        );
        let plan = json(&valid);
        assert!(plan["events"][0]["on_frame"].as_u64().unwrap() > 0);
        for offset in ["-10s", "10s"] {
            let mut invalid = valid.clone();
            let source = invalid.sources.get_mut("scores/main.maac").unwrap();
            *source = source.replace(
                "key = \"\"; } } place",
                &format!("key = \"\"; onset_offset = {offset}; }} }} place"),
            );
            assert!(compile_bundle_artifact(&invalid)
                .unwrap_err()
                .iter()
                .any(|d| d.code == maac::DiagnosticCode::Interval));
        }
    }
    for ramp in [false, true] {
        let collision = bundle(
            r#"pattern pat { length = 4q; hit h { at = 3.999999q; key = ""; } } place place { pattern = &pat; track = &t; at = 0q; }"#,
            ramp,
        );
        assert!(compile_bundle_artifact(&collision)
            .unwrap_err()
            .iter()
            .any(|d| d.code == maac::DiagnosticCode::Interval));
    }
}
#[test]
fn invalid_keys_receivers_assets_and_limits_fail() {
    let valid = bundle(
        r#"pattern pat { length = 4q; hit h { at = 0q; key = ""; } } place place { pattern = &pat; track = &t; at = 0q; }"#,
        false,
    );
    let mut bad = valid.clone();
    bad.assets.get_mut("samples/kick.pcm").unwrap()[0] ^= 1;
    assert!(compile_bundle_artifact(&bad).is_err());
    let mut malformed = valid.clone();
    let old_hash = sha256_digest(&malformed.assets["samples/kick.pcm"]);
    let raw = [f32::NAN.to_le_bytes(), 0f32.to_le_bytes()].concat();
    let source = malformed.sources.get_mut("scores/main.maac").unwrap();
    *source = source.replace(&old_hash, &sha256_digest(&raw));
    malformed.assets.insert("samples/kick.pcm".into(), raw);
    assert!(compile_bundle_artifact(&malformed).is_err());
    assert!(check_bundle_artifact(&malformed).is_err());
    for (from, to) in [
        (
            "hit h { at = 0q; key = \"\"; }",
            "hit h { at = 0q; key = \"missing\"; }",
        ),
        (
            "hit h { at = 0q; key = \"\"; }",
            "note h { at = 0q; dur = 1q; pitch = A4; }",
        ),
    ] {
        let mut bad = valid.clone();
        let source = bad.sources.get_mut("scores/main.maac").unwrap();
        *source = source.replace(from, to);
        assert!(compile_bundle_artifact(&bad).is_err());
    }
    let limits = PlanLimits {
        max_events: 0,
        ..Default::default()
    };
    assert!(compile_bundle_artifact_with_limits(&valid, &limits).is_err());
}
#[test]
fn legacy_bundle_artifact_bytes_are_identical() {
    let bundle = SourceBundle::new("example.maac", include_str!("../example.maac"));
    assert_eq!(
        compile_bundle_artifact(&bundle).unwrap().to_json().unwrap(),
        compile_bundle_versioned(&bundle)
            .unwrap()
            .to_json()
            .unwrap()
    );
}

#[test]
fn mixed_note_hit_receivers_and_kit_level_automation() {
    let body = r#"
        node sine { type = "core.sine/1"; }
        node mix { type = "core.sum/1"; config = { channels = 1; }; }
        connect drums { from = &kit:out; to = &mix:in; }
        connect tone { from = &sine:out; to = &mix:in; }
        track notes { target = &sine:events; }
        pattern beats { length = 4q; hit h { at = 0q; key = ""; } }
        pattern melody { length = 4q; note n { at = 0q; dur = 1q; pitch = A4; } }
        place drums_place { pattern = &beats; track = &t; at = 0q; }
        place notes_place { pattern = &melody; track = &notes; at = 0q; }
        curve level { clock = seconds; points = [(0s, 0, linear), (1s, 1, step)]; }
        automation lane { target = &kit.params.level; curve = &level; at = 0s; }
    "#;
    let mut good = bundle(body, true);
    let source = good.sources.get_mut("scores/main.maac").unwrap();
    *source = source.replace("output = &kit:out", "output = &mix:out");
    let plan = json(&good);
    assert_eq!(plan["events"].as_array().unwrap().len(), 2);
    assert!(plan["events"][0]["off_frame"].is_null());
    assert!(plan["events"][1]["off_frame"].as_u64().unwrap() > 0);
    assert_eq!(plan["automation"][0]["target"]["node"], "kit");
    let mut instrument = good.clone();
    let source = instrument.sources.get_mut("scores/main.maac").unwrap();
    *source = source.replace(
        r#"node sine { type = "core.sine/1"; }"#,
        r#"
        instrument lead { channels = 1;
            voice v { channels = 1; amplitude = &amp; output = &osc:out;
                node amp { type = "synth.adsr/1"; }
                node osc { type = "synth.sine/1"; }
            }
        }
        node sine { instrument = &lead; }
    "#,
    );
    assert_eq!(
        json(&instrument)["instruments"]["programs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut bad = good.clone();
    let source = bad.sources.get_mut("scores/main.maac").unwrap();
    *source = source.replace(
        "track t { target = &kit:events; }",
        "track t { target = &sine:events; }",
    );
    assert!(compile_bundle_artifact(&bad).is_err());
}

#[test]
fn empty_and_unused_assets_are_preserved_and_verified() {
    let mut source = bundle(
        r#"pattern pat { length = 4q; hit h { at = 0q; key = ""; } } place place { pattern = &pat; track = &t; at = 0q; }"#,
        false,
    );
    let old_hash = sha256_digest(&source.assets["samples/kick.pcm"]);
    source.assets.insert("samples/kick.pcm".into(), Vec::new());
    source
        .assets
        .insert("unused.pcm".into(), 2f32.to_le_bytes().to_vec());
    let text = source.sources.get_mut("scores/main.maac").unwrap();
    *text = text
        .replace(&old_hash, &sha256_digest(&[]))
        .replace("frames = 2", "frames = 0");
    text.push_str(&format!(r#"asset unused {{ kind = audio; path = "unused.pcm"; hash = "{}"; format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 1; frames = 1; }}"#, sha256_digest(&2f32.to_le_bytes())));
    let artifact = compile_bundle_artifact(&source).unwrap();
    let bytes = artifact.to_json().unwrap();
    let plan: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(plan["audio_assets"].as_array().unwrap().len(), 2);
    assert_eq!(plan["audio_assets"][0]["frames"], 0);
    drop(source);
    assert_eq!(
        PlanArtifact::from_json(&bytes).unwrap().to_json().unwrap(),
        bytes
    );
    let source = bundle(
        r#"pattern pat { length = 4q; hit h { at = 0q; key = ""; } } place place { pattern = &pat; track = &t; at = 0q; }"#,
        false,
    );
    let exact_size = compile_bundle_artifact(&source)
        .unwrap()
        .to_json()
        .unwrap()
        .len();
    let limits = PlanLimits {
        max_json_bytes: exact_size - 1,
        ..Default::default()
    };
    assert!(compile_bundle_artifact_with_limits(&source, &limits).is_err());
}

#[test]
fn legacy_step_and_ramp_dispatch_use_ast_not_labels() {
    for tempo in [
        "[(0q, 120bpm, step)]",
        "[(0q, 120bpm, linear), (4q, 240bpm, step)]",
    ] {
        let source = SourceBundle::new(
            "main.maac",
            format!(
                r#"maac 1;
            project p {{ score=[0q,4q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; label="hit core.kit/1"; }}
            tempo t {{ points={tempo}; }} meter m {{ points=[(0q,4,4)]; }}
            node s {{ type="core.sine/1"; }} track tr {{ target=&s:events; }}
            pattern pat {{ length=4q; note n {{ at=0q; dur=1q; pitch=A4; }} }}
            place pl {{ pattern=&pat; track=&tr; at=0q; }}"#
            ),
        );
        assert_eq!(
            compile_bundle_artifact(&source).unwrap().to_json().unwrap(),
            compile_bundle_versioned(&source)
                .unwrap()
                .to_json()
                .unwrap()
        );
    }
}
