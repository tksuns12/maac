use maac::{compiler::compile_bundle_artifact, PlanArtifact, SourceBundle};
use serde_json::{json, Value};

fn fixture() -> Value {
    let bundle = SourceBundle::new(
        "score.maac",
        r#"maac 1;
project p {score=[0q,1q];tail=1s;rate=48000Hz;tempo=&clock;meter=&metre;output=&sound:out;}
tempo clock {points=[(0q,60bpm,step)];} meter metre {points=[(0q,4,4)];}
node sound {type="core.sine/1";} node motion {type="core.lfo/1";config={period=1s;wave=saw;};}
modulate m {from=&motion:out;target=&sound.params.level;amount=0;}
"#,
    );
    serde_json::from_slice(&compile_bundle_artifact(&bundle).unwrap().to_json().unwrap()).unwrap()
}
fn config(v: &mut Value) -> &mut Value {
    &mut v["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["id"] == "motion")
        .unwrap()["processor"]["config"]
}
fn load(v: &Value) -> Result<PlanArtifact, maac::plan::PlanError> {
    PlanArtifact::from_json(&serde_json::to_vec(v).unwrap())
}
#[test]
fn retained_lfo_endpoint_loss_is_rejected_at_public_load() {
    let mut v = fixture();
    let denominator = num_bigint::BigInt::from(1) << 1200;
    let numerator = (&denominator / 4) + 1;
    config(&mut v)["period"] = json!(format!("{numerator}/{denominator}"));
    let e = load(&v).unwrap_err();
    assert_eq!(e.code, "E_TIME_PRECISION");
    assert!(
        e.path.contains("motion") && e.path.contains("config"),
        "{e:?}"
    );
}
#[test]
fn retained_lfo_invalid_fields_have_node_and_field_paths() {
    for (field, value) in [
        ("phase", "1/1"),
        ("phase", "-1/4"),
        ("period", "0/1"),
        ("period", "-1/1"),
    ] {
        let mut v = fixture();
        config(&mut v)[field] = json!(value);
        let e = load(&v).unwrap_err();
        assert_eq!(e.code, "E_RANGE");
        assert_eq!(e.path, format!("nodes.motion.processor.config.{field}"));
    }
}

#[test]
fn retained_native_mode_target_matrix() {
    for (mode, field, accepted) in [
        ("peak", "gain", true),
        ("peak", "q", true),
        ("low_pass", "frequency", true),
        ("low_pass", "gain", false),
        ("low_shelf", "gain", true),
        ("low_shelf", "q", false),
    ] {
        let mut v = fixture();
        v["nodes"].as_array_mut().unwrap().push(json!({"id":"fx","processor":{"kind":"core","processor":{"kind":"fx.eq/1","channels":1,"mode":mode}}}));
        v["connections"] = json!([{"id":"route","from":{"node":"sound","port":"out"},"to":{"node":"fx","port":"in"}}]);
        v["modulations"][0]["target"] = json!({"node":"fx","port":field});
        if accepted {
            let p = load(&v).unwrap();
            load(&serde_json::from_slice(&p.to_json().unwrap()).unwrap()).unwrap();
        } else {
            assert_eq!(load(&v).unwrap_err().code, "E_REFERENCE");
        }
    }
}
#[test]
fn retained_instrument_parameter_rate_matrix() {
    let source = r#"maac 1;
project p {score=[0q,1q];tail=0s;rate=48000Hz;tempo=&clock;meter=&metre;output=&sound:out;}
tempo clock {points=[(0q,60bpm,step)];} meter metre {points=[(0q,4,4)];}
instrument local {channels=1;
voice v {channels=1;amplitude=&amp;output=&osc:out;node amp {type="synth.adsr/1";} node osc {type="synth.sine/1";}}
shared s {channels=1;output=&gain:out;node gain {type="synth.gain/1";config={channels=1;};} connect route {from=&input:out;to=&gain:in;} node lfo {type="synth.lfo/1";}}
control sample {target=&v.osc.params.ratio;default=1;}
control on {target=&v.amp.params.attack;default=0s;}
control off {target=&v.amp.params.release;default=0s;}
control reset {target=&s.lfo.params.phase;default=0;}}
node sound {instrument=&local;} node c {type="core.constant/1";}
modulate m {from=&c:out;target=&sound.params.sample;amount=0;}
"#;
    let p = compile_bundle_artifact(&SourceBundle::new("score.maac", source)).unwrap();
    let original: Value = serde_json::from_slice(&p.to_json().unwrap()).unwrap();
    for (field, amount, accepted) in [
        ("sample", "0/1", true),
        ("on", "1/2", true),
        ("off", "0/1", true),
        ("reset", "0/1", false),
    ] {
        let mut v = original.clone();
        v["modulations"][0]["target"]["port"] = json!(field);
        v["modulations"][0]["amount"] = json!(amount);
        if accepted {
            load(&v).unwrap().to_json().unwrap();
        } else {
            assert_eq!(load(&v).unwrap_err().code, "E_CAPABILITY", "{field}");
        }
    }
}
#[test]
fn retained_core_event_rate_targets_are_admitted() {
    let mut v = fixture();
    v["modulations"][0]["target"] = json!({"node":"sound","port":"attack"});
    v["modulations"][0]["amount"] = json!("0/1");
    load(&v).unwrap().to_json().unwrap();
    v["modulations"][0]["target"] = json!({"node":"sound","port":"release"});
    v["modulations"][0]["amount"] = json!("1/2");
    load(&v).unwrap().to_json().unwrap();
}
#[test]
fn disconnected_controls_and_encoder_enforce_aggregate_work() {
    let mut v = fixture();
    v["modulations"] = json!([]);
    v["source_mappings"] = json!([]);
    let p = load(&v).unwrap();
    let limits = maac::plan::PlanLimits {
        max_work: 800,
        ..Default::default()
    };
    PlanArtifact::from_json_with_limits(&serde_json::to_vec(&v).unwrap(), &limits).unwrap();
    p.to_json_with_limits(&limits).unwrap();
    let mut second = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "motion")
        .unwrap()
        .clone();
    second["id"] = json!("second");
    v["nodes"].as_array_mut().unwrap().push(second);
    assert_eq!(
        PlanArtifact::from_json_with_limits(&serde_json::to_vec(&v).unwrap(), &limits)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    let p = load(&v).unwrap();
    assert_eq!(
        p.to_json_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}
#[test]
fn zero_output_still_validates_disconnected_lfo_metadata() {
    let mut v = fixture();
    v["output"]["score_end_q"] = json!("0/1");
    v["output"]["tail_seconds"] = json!("0/1");
    v["output"]["total_frames"] = json!(0);
    v["modulations"] = json!([]);
    config(&mut v)["period"] = json!("0/1");
    assert_eq!(
        load(&v).unwrap_err().path,
        "nodes.motion.processor.config.period"
    );
}

#[test]
fn retained_kit_level_target_is_admitted() {
    let mut v = fixture();
    let bytes = 1f32.to_le_bytes().to_vec();
    v["audio_assets"] = json!([{"id":"sample","format":"pcm_f32le_interleaved/1","rate_hz":48000,"channels":1,"frames":1,"hash":maac::bundle::sha256_digest(&bytes),"bytes":bytes}]);
    v["nodes"].as_array_mut().unwrap().push(json!({"id":"kit","processor":{"kind":"kit","channels":1,"voices":1,"samples":[{"key":"key","asset":"sample"}]}}));
    v["modulations"][0]["target"] = json!({"node":"kit","port":"level"});
    load(&v).unwrap().to_json().unwrap();
}
#[test]
fn retained_ramp_backward_endpoint_loss_is_rejected() {
    let mut v = fixture();
    let b: num_bigint::BigInt = "9930385589350642984058755095404800790126870706742824492072954375"
        .parse()
        .unwrap();
    let d = "6277101735386680763835789423207666416102355444464034512896";
    v["tempo"]["points"] = json!([
        {"q":"0/1","bpm":format!("{b}/{d}"),"shape":"linear"},
        {"q":"1/1","bpm":format!("{}/{d}", b*3),"shape":"step"}
    ]);
    v["output"]["total_frames"] = json!(2);
    v["output"]["tail_seconds"] = json!("0/1");
    config(&mut v)["clock"] = json!("score");
    config(&mut v)["period"] = json!("2/1");
    let e = load(&v).unwrap_err();
    assert_eq!(e.code, "E_TIME_PRECISION", "{e:?}");
    assert_eq!(e.path, "nodes.motion.processor.config");
}
