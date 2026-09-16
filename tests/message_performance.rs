use maac::bundle::SourceBundle;
use maac::compiler::{check_bundle_artifact, compile_bundle_artifact};
use maac::{render_artifact, PlanArtifact};
use serde_json::Value;

fn bundle(body: &str) -> SourceBundle {
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,4q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
node s {{ type="core.sine/1"; config={{ voices=8; }}; }}
track events {{ target=&s:events; }}
{body}
"#
    );
    SourceBundle::new("main.maac", source)
}

fn artifact_json(bundle: &SourceBundle) -> (PlanArtifact, Value) {
    let artifact = compile_bundle_artifact(bundle).unwrap();
    let bytes = artifact.to_json().unwrap();
    let loaded = PlanArtifact::from_json(&bytes).unwrap();
    assert_eq!(loaded.to_json().unwrap(), bytes);
    let json = serde_json::from_slice(&bytes).unwrap();
    (loaded, json)
}

#[test]
fn message_protocol_bytes_timing_and_roundtrip_are_preserved() {
    let bundle = bundle(
        r#"
pattern ptn { length=2q; message m { at=1q; protocol="midi1"; bytes=[144,60,127]; onset_offset=1ms; order=-2; } }
place play { pattern=&ptn; track=&events; at=0q; }
"#,
    );
    let (_artifact, json) = artifact_json(&bundle);
    let event = &json["events"][0];
    assert_eq!(event["kind"]["kind"], "message");
    assert_eq!(event["kind"]["protocol"], "midi1");
    assert_eq!(event["kind"]["bytes"], serde_json::json!([144, 60, 127]));
    assert_eq!(event["score_on_q"], "1/1");
    assert_eq!(event["onset_offset_seconds"], "1/1000");
    assert_eq!(event["on_frame"], 24048);
    assert_eq!(event["order"], -2);
    assert!(event["score_off_q"].is_null());
    assert!(event["off_frame"].is_null());
    check_bundle_artifact(&bundle).unwrap();
}

#[test]
fn nested_messages_inserts_and_overrides_expand_by_address() {
    let bundle = bundle(
        r#"
pattern inner { length=1q; message m { at=0q; protocol="raw/a"; bytes=[1]; } }
pattern outer { length=2q; use u { pattern=&inner; at=0q; count=2; } }
place play { pattern=&outer; track=&events; at=0q;
  override edit { event="0/u/0/m"; set={ protocol="raw/b"; bytes=[2,3]; order=4; }; }
  override gone { event="0/u/1/m"; delete=true; }
  insert once { message extra { at=3/2q; protocol="raw/c"; bytes=[255]; } }
}
"#,
    );
    let (_artifact, json) = artifact_json(&bundle);
    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["address"], "play/0/u/0/m");
    assert_eq!(events[0]["kind"]["protocol"], "raw/b");
    assert_eq!(events[0]["kind"]["bytes"], serde_json::json!([2, 3]));
    assert_eq!(events[0]["order"], 4);
    assert_eq!(events[1]["address"], "play/once/extra");
    assert_eq!(events[1]["kind"]["protocol"], "raw/c");
}

#[test]
fn message_bytes_are_strict_and_builtin_renderer_rejects_missing_adapter() {
    for value in ["-1", "256", "1/2"] {
        let invalid = bundle(&format!(
            r#"pattern ptn {{ length=1q; message m {{ at=0q; protocol="x"; bytes=[{value}]; }} }} place play {{ pattern=&ptn; track=&events; at=0q; }}"#
        ));
        assert!(compile_bundle_artifact(&invalid).is_err());
    }
    let valid = bundle(
        r#"pattern ptn { length=1q; message m { at=0q; protocol="midi1"; bytes=[0]; } } place play { pattern=&ptn; track=&events; at=0q; }"#,
    );
    let (artifact, _) = artifact_json(&valid);
    let error = render_artifact(&artifact, |_| Ok(())).unwrap_err();
    assert!(error.to_string().contains("E_CAPABILITY"), "{error}");
}

#[test]
fn adapter_protocol_is_exact_and_same_frame_class_order_is_normative() {
    use maac::plan::PortRef;
    use maac::{MessageAdapterCapability, PerformanceDispatchKind};

    let bundle = bundle(
        r#"
pattern ptn { length=2q;
  note old { at=0q; dur=1q; pitch=C4; order=-100; }
  message msg { at=1q; protocol="midi1"; bytes=[176,1,64]; order=100; }
  note new { at=1q; dur=1/2q; pitch=E4; order=-200; }
}
place play { pattern=&ptn; track=&events; at=0q; }
"#,
    );
    let (artifact, _) = artifact_json(&bundle);
    let target = PortRef::new("s", "events").unwrap();

    let missing = artifact.performance_dispatches(&[]).unwrap_err();
    assert_eq!(missing.code, "E_CAPABILITY");
    let wrong = artifact
        .performance_dispatches(&[MessageAdapterCapability {
            target: target.clone(),
            protocols: vec!["MIDI1".into()],
        }])
        .unwrap_err();
    assert_eq!(wrong.code, "E_CAPABILITY");

    let dispatches = artifact
        .performance_dispatches(&[MessageAdapterCapability {
            target,
            protocols: vec!["midi1".into()],
        }])
        .unwrap();
    let at_frame: Vec<_> = dispatches
        .iter()
        .filter(|dispatch| dispatch.frame == 24_000)
        .collect();
    assert_eq!(at_frame.len(), 3);
    assert!(matches!(
        at_frame[0].dispatch,
        PerformanceDispatchKind::Message { .. }
    ));
    assert!(matches!(
        at_frame[1].dispatch,
        PerformanceDispatchKind::NoteOff
    ));
    assert!(matches!(
        at_frame[2].dispatch,
        PerformanceDispatchKind::NoteOn { .. }
    ));
    assert_eq!(at_frame[0].address, "play/0/msg");
    assert_eq!(at_frame[1].address, "play/0/old");
    assert_eq!(at_frame[2].address, "play/0/new");
}
