use maac::{compiler::compile_bundle_artifact, PlanArtifact, SourceBundle};
use serde_json::Value;
fn bundle(body: &str) -> SourceBundle {
    SourceBundle::new(
        "score.maac",
        format!(
            r#"maac 1; project p {{score=[0q,1q];tail=0s;rate=48000Hz;tempo=&clock;meter=&metre;output=&sound:out;}} tempo clock {{points=[(0q,60bpm,step)];}} meter metre {{points=[(0q,4,4)];}} node sound {{type="core.sine/1";}} {body}"#
        ),
    )
}
fn compile(body: &str) -> Value {
    let p = compile_bundle_artifact(&bundle(body)).unwrap();
    let bytes = p.to_json().unwrap();
    assert_eq!(
        PlanArtifact::from_json(&bytes).unwrap().to_json().unwrap(),
        bytes
    );
    serde_json::from_slice(&bytes).unwrap()
}
#[test]
fn native_controls_lower_to_v7_with_exact_defaults() {
    let v = compile(
        r#"node motion {type="core.lfo/1";config={period=2q;phase=-1/4;};} node offset {type="core.constant/1";} modulate a {from=&motion:out;target=&offset.params.value;amount=-2;} modulate b {from=&offset:out;target=&sound.params.level;amount=1/4;}"#,
    );
    assert_eq!(v["version"], 7);
    assert_eq!(v["modulations"].as_array().unwrap().len(), 2);
    let nodes = v["nodes"].as_array().unwrap();
    let m = nodes.iter().find(|n| n["id"] == "motion").unwrap();
    assert_eq!(m["processor"]["config"]["phase"], "3/4");
    assert_eq!(m["processor"]["config"]["wave"], "sine");
}
#[test]
fn units_phase_automation_and_legacy_entrypoint_contracts() {
    let v = compile(
        r#"node motion {type="core.lfo/1";config={period=250ms;wave=square;phase=100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001/4;};} node offset {type="core.constant/1";params={value=-1/2;};} curve line {clock=seconds;points=[(0s,-1,linear),(1s,1,step)];} automation lane {target=&offset.params.value;curve=&line;at=0s;} modulate m {from=&offset:out;target=&sound.params.level;amount=1/4;}"#,
    );
    let m = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "motion")
        .unwrap();
    assert_eq!(m["processor"]["config"]["clock"], "seconds");
    assert_eq!(m["processor"]["config"]["period"], "1/4");
    assert_eq!(m["processor"]["config"]["phase"], "1/4");
    assert_eq!(v["automation"].as_array().unwrap().len(), 1);
    assert!(maac::compiler::compile_bundle_versioned(&bundle(
        r#"node c {type="core.constant/1";}"#
    ))
    .is_err());
    let legacy = bundle("");
    let a = compile_bundle_artifact(&legacy).unwrap();
    let old = maac::compiler::compile_bundle_versioned(&legacy).unwrap();
    assert_eq!(a.to_json().unwrap(), old.to_json().unwrap());
}
#[test]
fn invalid_declarations_and_references_fail_even_unused() {
    for body in [
        r#"node c {type="core.constant/1";config={};}"#,
        r#"node c {type="core.constant/1";params={value=1Hz;};}"#,
        r#"node c {type="core.constant/1";params={unknown=1;};}"#,
        r#"node c {type="core.lfo/1";}"#,
        r#"node c {type="core.lfo/1";config={period=0q;};}"#,
        r#"node c {type="core.lfo/1";config={period=1Hz;};}"#,
        r#"node c {type="core.lfo/1";config={period=1s;wave=noise;};}"#,
        r#"node c {type="core.lfo/1";config={period=1s;phase=1q;};}"#,
        r#"node c {type="core.lfo/1";config={period=1s;};params={};}"#,
        r#"node c {type="core.constant/1";} modulate m {from=&sound:out;target=&c.params.value;amount=0;}"#,
        r#"node c {type="core.constant/1";} modulate m {from=&c:out;target=&sound.config.voices;amount=0;}"#,
        r#"node c {type="core.constant/1";} modulate m {from=&c:out;target=&sound.params.level;amount=0s;}"#,
        r#"node c {type="core.constant/1";} connect e {from=&c:out;to=&sound:events;}"#,
    ] {
        assert!(compile_bundle_artifact(&bundle(body)).is_err(), "{body}");
    }
    let body = r#"node c {type="core.constant/1";} modulate m {from=&c:out;target=&sound.params.attack;amount=0s;}"#;
    assert!(compile_bundle_artifact(&bundle(body))
        .unwrap_err()
        .iter()
        .any(|d| d.code == maac::diagnostic::DiagnosticCode::Capability));
    for body in [
        r#"node c {type="core.constant/1";} modulate m {from=&c:out;target=&c.params.value;amount=0;}"#,
        r#"node c {type="core.constant/1";} node d {type="core.constant/1";} modulate m {from=&c:out;target=&d.params.value;amount=0;} modulate n {from=&d:out;target=&c.params.value;amount=0;}"#,
    ] {
        assert!(compile_bundle_artifact(&bundle(body))
            .unwrap_err()
            .iter()
            .any(|d| d.code == maac::diagnostic::DiagnosticCode::AlgebraicLoop));
    }
}
#[test]
fn shared_connection_and_point_budgets_remain_explicit() {
    use maac::plan::PlanLimits;
    let b = bundle(
        r#"node c {type="core.constant/1";} node sum {type="core.sum/1";config={channels=1;};} connect e {from=&sound:out;to=&sum:in;} modulate m {from=&c:out;target=&sound.params.level;amount=0;}"#,
    );
    let mut limits = PlanLimits {
        max_connections: 2,
        ..Default::default()
    };
    assert!(maac::compiler::compile_bundle_artifact_with_limits(&b, &limits).is_ok());
    limits.max_connections = 1;
    assert!(
        maac::compiler::compile_bundle_artifact_with_limits(&b, &limits)
            .unwrap_err()
            .iter()
            .any(|d| d.code == maac::diagnostic::DiagnosticCode::ResourceLimit)
    );
    let b = bundle(
        r#"curve c {clock=score;points=[(0q,1,linear),(1q,1,step)];} automation a {target=&sound.params.level;curve=&c;at=0q;}"#,
    );
    limits = PlanLimits::default();
    limits.max_automation_points = 1;
    assert!(maac::compiler::compile_bundle_artifact_with_limits(&b, &limits).is_err());
}
fn production_identity(body: &str) -> Value {
    let schema = maac::production_data::SCHEMA_BYTES;
    let extra = format!(
        r#"asset schema {{kind=descriptor;path="production.schema.json";hash="{}";}} extension delivery {{namespace="maac.production/1";schema=&schema;render_affecting=true;data={{deliveries={{release={{rate=48000Hz;resampler="maac.src.kaiser/1";targets={{master={{role=master;output=&sound:out;encoding=wav_f32le;dither={{type=none;}};}};}};}};}};}};}} {body}"#,
        maac::bundle::sha256_digest(schema)
    );
    let mut b = bundle(&extra);
    let text = b.sources.get_mut("score.maac").unwrap();
    *text = text.replace(
        "project p {",
        "project p { requires=[\"maac.production/1\"]; ",
    );
    b.assets
        .insert("production.schema.json".into(), schema.to_vec());
    let p = compile_bundle_artifact(&b).unwrap();
    let v: Value = serde_json::from_slice(&p.to_json().unwrap()).unwrap();
    v["production"]["execution_identity"]["execution_hash"].clone()
}
#[test]
fn production_identity_normalizes_control_defaults_and_tracks_recipe() {
    let a = r#"node c {type="core.constant/1";} node l {type="core.lfo/1";config={period=1s;};} modulate m {from=&l:out;target=&c.params.value;amount=1;}"#;
    let b = r#"node c {type="core.constant/1";params={value=0;};} node l {type="core.lfo/1";config={period=1000ms;wave=sine;phase=0;};} modulate m {from=&l:out;target=&c.params.value;amount=1;}"#;
    let id = production_identity(a);
    assert!(!id.is_null());
    assert_eq!(id, production_identity(b));
    assert_ne!(id, production_identity(&a.replace("amount=1", "amount=2")));
    assert_ne!(
        id,
        production_identity(&a.replace("period=1s", "period=2s"))
    );
}
#[test]
fn mixed_resources_and_native_parameter_units_survive_v7() {
    let bytes: Vec<u8> = [1f32, 2., 3.]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let body = format!(
        r#"asset sample {{kind=audio;path="sample.pcm";hash="{}";format="pcm_f32le_interleaved/1";rate=24000Hz;channels=1;frames=3;}}
node c {{type="core.constant/1";params={{value=-1/4;}};}}
node kit {{type="core.kit/1";config={{channels=1;samples=[{{key="k";asset=&sample;}}];}};}}
audio rateclip {{asset=&sample;at=0q;source=[0frame,3frame];mode=rate;}}
audio warpclip {{asset=&sample;at=0q;source=[0frame,3frame];mode=warp_rate;warp=[(0q,0frame),(1q,3frame)];}}
instrument local {{channels=1;voice v {{channels=1;amplitude=&amp;output=&osc:out;node amp {{type="synth.adsr/1";}} node osc {{type="synth.sine/1";}}}} control ratio {{target=&v.osc.params.ratio;default=1;}} }}
node synth {{instrument=&local;}}
node fx {{type="fx.eq/1";config={{channels=1;mode=peak;}};}}
connect route {{from=&sound:out;to=&fx:in;}}
modulate kitmod {{from=&c:out;target=&kit.params.level;amount=1/4;}}
modulate instmod {{from=&c:out;target=&synth.params.ratio;amount=1/4;}}
modulate fxmod {{from=&c:out;target=&fx.params.gain;amount=2dB;}}
modulate hzmod {{from=&c:out;target=&fx.params.frequency;amount=1kHz;}}
track notes {{target=&sound:events;}} track hits {{target=&kit:events;}}
pattern np {{length=1q;note n {{at=0q;dur=1/2q;pitch=A4;}}}} pattern hp {{length=1q;hit h {{at=0q;key="k";}}}}
place pn {{pattern=&np;track=&notes;at=0q;}} place ph {{pattern=&hp;track=&hits;at=0q;}}"#,
        maac::bundle::sha256_digest(&bytes)
    );
    let mut b = bundle(&body);
    let text = b.sources.get_mut("score.maac").unwrap();
    *text = text
        .replace(
            "project p {",
            "project p { requires=[\"maac.production/1\"]; ",
        )
        .replace("(0q,60bpm,step)", "(0q,60bpm,linear),(1q,120bpm,step)");
    b.assets.insert("sample.pcm".into(), bytes.clone());
    let p = compile_bundle_artifact(&b).unwrap();
    assert_eq!(p.audio_clip_count(), 2);
    assert_eq!(p.event_count(), 2);
    let v: Value = serde_json::from_slice(&p.to_json().unwrap()).unwrap();
    assert_eq!(v["audio_assets"][0]["bytes"], serde_json::json!(bytes));
    assert!(!v["instruments"].is_null());
    let m = v["modulations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "hzmod")
        .unwrap();
    assert_eq!(m["amount"], "1000/1");
}
