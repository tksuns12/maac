use std::collections::BTreeMap;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::editing::{
    BundleEditContext, FoundationEditContext, MaterializeInstancePlan, Operation, SourceDocument,
    Transaction,
};
use maac::plan::{EventKind, ResolvedEvent};
use serde_json::{json, Value};

fn source(body: &str) -> String {
    format!(
        r#"maac 1;
// This header and the original objects are caller-owned source text.
project song {{ score=[0q,32q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&synth:out; }}
tempo clock {{ points=[(0q,120bpm,step)]; }}
meter metre {{ points=[(0q,4,4)]; }}
node synth {{ type="core.sine/1"; }}
track notes {{ target=&synth:events; }}
{body}
"#
    )
}

fn events(source: &str) -> Vec<ResolvedEvent> {
    maac::compile(&maac::parse(source).unwrap()).unwrap().events
}

fn artifact_events(bundle: &SourceBundle) -> Vec<Value> {
    let artifact = maac::compiler::compile_bundle_artifact(bundle).unwrap();
    let json: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    json["events"].as_array().unwrap().clone()
}

fn mapped_artifact_event_values(before: &[Value], after: &[Value], plan: &MaterializeInstancePlan) {
    assert_eq!(before.len(), after.len());
    for event in before {
        let old_address = event["address"].as_str().unwrap();
        let address = plan
            .mappings
            .iter()
            .find(|mapping| mapping.old_event_address == old_address)
            .map_or(old_address, |mapping| mapping.new_event_address.as_str());
        let mut actual = after
            .iter()
            .find(|candidate| candidate["address"] == address)
            .unwrap()
            .clone();
        let mut expected = event.clone();
        for value in [&mut actual, &mut expected] {
            value.as_object_mut().unwrap().remove("address");
            value.as_object_mut().unwrap().remove("source");
        }
        assert_eq!(actual, expected, "event {old_address}");
    }
}

fn mapped_event_values(
    before: &[ResolvedEvent],
    after: &[ResolvedEvent],
    plan: &MaterializeInstancePlan,
) {
    assert_eq!(before.len(), after.len());
    for event in before {
        let address = plan
            .mappings
            .iter()
            .find(|mapping| mapping.old_event_address == event.address)
            .map_or(event.address.as_str(), |mapping| {
                mapping.new_event_address.as_str()
            });
        let actual = after
            .iter()
            .find(|candidate| candidate.address == address)
            .unwrap();
        let mut expected = serde_json::to_value(event).unwrap();
        let mut actual = serde_json::to_value(actual).unwrap();
        for object in [&mut expected, &mut actual] {
            object.as_object_mut().unwrap().remove("address");
            object.as_object_mut().unwrap().remove("source");
        }
        assert_eq!(actual, expected, "event {}", event.address);
    }
}

fn edit(document: &mut SourceDocument, path: Vec<String>, field: Vec<String>, value: Value) {
    let transaction = Transaction::new(
        document.revision().into(),
        vec![Operation::Set {
            object: path,
            field,
            value,
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    document
        .apply(&transaction, &FoundationEditContext)
        .unwrap();
}

#[test]
fn repeated_nested_cut_preserves_final_overrides_offsets_and_deleted_provenance() {
    let text = source(
        r#"
pattern leaf { length=1q; note n { at=3/4q; dur=1/2q; pitch=C4; onset_offset=10ms; release_offset=20ms; order=-3; } }
pattern phrase { length=2q; use u { pattern=&leaf; at=0q; count=2; stretch=2; transpose=700ct; boundary=cut; } }
place play { pattern=&phrase; track=&notes; at=1q; count=2; stretch=3/2; transpose=500ct; boundary=cut;
  override final { event="0/u/0/n"; set={at=1/4q;dur=1/3q;pitch=220Hz;}; }
  override gone { event="1/u/0/n"; delete=true; }
  insert once { note extra { at=1/2q; dur=1/4q; pitch=A4; } }
}
place other { pattern=&leaf; track=&notes; at=8q; }
"#,
    );
    // The source itself must respect use starts inside the containing pattern.
    let text = text.replace("pattern phrase { length=2q", "pattern phrase { length=4q");
    let before = events(&text);
    let mut document = SourceDocument::parse(&text).unwrap();
    let original = document.authored().clone();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "play", "independent")
        .unwrap();
    assert_eq!(document.authored(), &original);
    assert_eq!(plan.mappings.len(), 5); // Four underlying leaves plus one insert.
    let deleted = plan
        .mappings
        .iter()
        .find(|mapping| mapping.old_event_address == "play/1/u/0/n")
        .unwrap();
    assert_eq!(deleted.old_source_object_path, vec!["leaf", "n"]);
    assert!(deleted.new_event_address.starts_with("play/0/_repeat1/0/"));
    assert_eq!(plan.diagnostics[0].code, "W_IDENTITY_CHANGE");
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    let after = events(document.source());
    mapped_event_values(&before, &after, &plan);
    let final_note = after
        .iter()
        .find(|event| event.address == plan.mappings[0].new_event_address)
        .unwrap();
    assert_eq!(final_note.score_on_q.to_string(), "5/4");
    assert_eq!(
        final_note.score_off_q.as_ref().unwrap().to_string(),
        "19/12"
    );
    assert_eq!(final_note.on_frame, 30480);
    assert_eq!(final_note.off_frame, Some(38960));
    assert!(matches!(
        final_note.kind,
        EventKind::Note {
            pitch_hz: 220.0,
            ..
        }
    ));
    assert_eq!(
        after
            .iter()
            .filter(|event| event.address == "play/once/extra")
            .count(),
        1
    );
    assert_eq!(
        document.authored().tree()["objects"]["independent"]["fields"]["length"],
        json!({"t":"quantity","n":"12","d":"1","u":"q"})
    );
}

#[test]
fn every_repeated_leaf_and_expression_dependency_can_be_edited_independently() {
    let text = source(
        r#"
curve gain { clock=score; points=[(0q,1,linear),(1q,2,step)]; }
tuning tune { period=1200ct; steps=[0ct,700ct]; reference_index=0; reference_frequency=220Hz; }
pattern riff { length=1q; note n { at=0q;dur=1/2q;pitch=degree(0,&tune); expression e {kind=gain;curve=&gain;} } }
place play {pattern=&riff;track=&notes;at=0q;count=2;}
place other {pattern=&riff;track=&notes;at=4q;}
"#,
    );
    let mut document = SourceDocument::parse(&text).unwrap();
    let before = events(&text);
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    mapped_event_values(&before, &events(document.source()), &plan);
    let leaf_paths: Vec<_> = plan
        .mappings
        .iter()
        .map(|mapping| mapping.new_source_object_path.clone())
        .collect();
    assert_ne!(leaf_paths[0], leaf_paths[1]);
    let tree = document.authored().tree();
    let curve_ref = |path: &[String]| {
        tree["objects"][&path[0]]["children"][&path[1]]["children"]["e"]["fields"]["curve"]["path"]
            [0]
        .as_str()
        .unwrap()
        .to_owned()
    };
    let first_curve = curve_ref(&leaf_paths[0]);
    let second_curve = curve_ref(&leaf_paths[1]);
    assert_ne!(first_curve, second_curve);
    edit(
        &mut document,
        vec![first_curve],
        vec!["points".into()],
        json!({"t":"list","items":[
            {"t":"tuple","items":[{"t":"quantity","n":"0","d":"1","u":"q"},{"t":"number","n":"3","d":"1"},{"t":"symbol","v":"linear"}]},
            {"t":"tuple","items":[{"t":"quantity","n":"1","d":"1","u":"q"},{"t":"number","n":"4","d":"1"},{"t":"symbol","v":"step"}]}
        ]}),
    );
    edit(
        &mut document,
        leaf_paths[0].clone(),
        vec!["pitch".into()],
        json!({"t":"quantity","n":"330","d":"1","u":"Hz"}),
    );
    let after = events(document.source());
    let pitches: Vec<_> = after
        .iter()
        .map(|event| match event.kind {
            EventKind::Note { pitch_hz, .. } => pitch_hz,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(pitches, vec![330.0, 220.0, 220.0]);
    let gains: Vec<_> = after
        .iter()
        .map(|event| match &event.kind {
            EventKind::Note {
                gain_expression: Some(expression),
                ..
            } => expression.points[0].gain.to_string(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(gains, vec!["3", "1", "1"]);
    assert_eq!(
        document.authored().tree()["objects"]["other"]["fields"]["pattern"]["path"],
        json!(["riff"])
    );
}

#[test]
fn expression_clocks_keep_their_exact_scale_through_materialization() {
    for (clock, points, expected) in [
        ("score", "[(0q,1,linear),(1q,2,step)]", "6"),
        ("seconds", "[(0s,1,linear),(1s,2,step)]", "1"),
        ("normalized", "[(0,1,linear),(1,2,step)]", "1"),
    ] {
        let text = source(&format!(
            r#"
curve gain {{clock={clock};points={points};}}
pattern leaf {{length=1q;note n {{at=0q;dur=2q;pitch=A4;expression e {{kind=gain;curve=&gain;}}}}}}
pattern parent {{length=2q;use u {{pattern=&leaf;at=0q;stretch=2;boundary=cut;}}}}
place play {{pattern=&parent;track=&notes;at=0q;stretch=3;boundary=cut;
override shorter {{event="0/u/0/n";set={{dur=1q;}};}}
}}
"#
        ));
        let before = events(&text);
        let mut document = SourceDocument::parse(text).unwrap();
        let plan = FoundationEditContext
            .prepare_materialize_instance(document.authored(), "play", "copy")
            .unwrap();
        document
            .apply(&plan.transaction, &FoundationEditContext)
            .unwrap();
        let after = events(document.source());
        mapped_event_values(&before, &after, &plan);
        match &after[0].kind {
            EventKind::Note {
                gain_expression: Some(expression),
                ..
            } => assert_eq!(expression.points[1].position.to_string(), expected),
            _ => unreachable!(),
        }
        assert_eq!(after[0].score_off_q.as_ref().unwrap().to_string(), "1");
    }
}

#[test]
fn spill_keeps_full_gates_across_repetitions() {
    let text = source(
        r#"
pattern riff {length=1q;note n {at=3/4q;dur=2q;pitch=A4;}}
place play {pattern=&riff;track=&notes;at=0q;count=2;stretch=2;boundary=spill;}
"#,
    );
    let before = events(&text);
    let mut document = SourceDocument::parse(text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    let after = events(document.source());
    mapped_event_values(&before, &after, &plan);
    assert_eq!(after[0].score_off_q.as_ref().unwrap().to_string(), "11/2");
    assert_eq!(after[1].score_off_q.as_ref().unwrap().to_string(), "15/2");
}

#[test]
fn source_projection_keeps_original_text_and_inverse_restores_authored_identity() {
    let untouched =
        "pattern riff { /* original spacing */ length = 1q; note n { at=0q;dur=1/2q;pitch=C4; } }";
    let text = source(&format!(
        "{untouched}\nplace play {{pattern=&riff;track=&notes;at=0q;label=\"Take one\";}}\n"
    ));
    let mut document = SourceDocument::parse(&text).unwrap();
    let original = document.authored().clone();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    let applied = document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    assert!(document.source().contains(untouched));
    assert!(document.source().starts_with("maac 1;\n// This header"));
    assert!(document.source().contains("label=\"Take one\";"));
    document
        .apply(&applied.inverse, &FoundationEditContext)
        .unwrap();
    assert_eq!(document.authored(), &original);
    assert!(document.source().contains(untouched));
    assert_eq!(events(document.source()), events(&text));
}

#[test]
fn plans_are_deterministic_revision_bound_and_refuse_bad_targets_and_collisions() {
    let text = source(
        r#"
pattern _mi0 {length=1q;}
pattern riff {length=1q;note n {at=0q;dur=1/2q;pitch=C4;}}
place play {pattern=&riff;track=&notes;at=0q;}
"#,
    );
    let mut document = SourceDocument::parse(text).unwrap();
    let context = FoundationEditContext;
    let plan = context
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    assert_eq!(
        plan,
        context
            .prepare_materialize_instance(document.authored(), "play", "copy")
            .unwrap()
    );
    assert!(!plan.mappings[0]
        .new_source_object_path
        .contains(&"_mi0".to_owned()));
    assert_eq!(
        context
            .prepare_materialize_instance(document.authored(), "riff", "copy")
            .unwrap_err()
            .code,
        "E_REFERENCE"
    );
    assert_eq!(
        context
            .prepare_materialize_instance(document.authored(), "play", "riff")
            .unwrap_err()
            .code,
        "E_DUPLICATE_ID"
    );
    assert_eq!(
        context
            .prepare_materialize_instance(document.authored(), "play", "bad.id")
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
    edit(
        &mut document,
        vec!["play".into()],
        vec!["label".into()],
        json!({"t":"string","v":"stale"}),
    );
    let stale_base = document.authored().clone();
    assert_eq!(
        document
            .apply(&plan.transaction, &context)
            .unwrap_err()
            .code,
        "E_CONFLICT"
    );
    assert_eq!(document.authored(), &stale_base);
}

#[test]
fn repeated_empty_patterns_and_extra_wrapper_depth_are_bounded_before_copying() {
    for body in [
        "pattern empty {length=1q;} place play {pattern=&empty;track=&notes;at=0q;count=1000000000;}",
        "pattern empty {length=1q;} pattern nested {length=1q;use u {pattern=&empty;at=0q;count=1024;}} place play {pattern=&nested;track=&notes;at=0q;}",
    ] {
        let document = SourceDocument::parse(source(body)).unwrap();
        assert_eq!(FoundationEditContext.prepare_materialize_instance(document.authored(), "play", "copy").unwrap_err().code, "E_RESOURCE_LIMIT");
    }
    let mut body = "pattern p0 {length=1q;}".to_owned();
    for index in 1..64 {
        body.push_str(&format!(
            "pattern p{index} {{length=1q;use u {{pattern=&p{};at=0q;}}}}",
            index - 1
        ));
    }
    body.push_str("place play {pattern=&p63;track=&notes;at=0q;}");
    let document = SourceDocument::parse(source(&body)).unwrap();
    assert_eq!(
        FoundationEditContext
            .prepare_materialize_instance(document.authored(), "play", "copy")
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn imported_nested_private_refs_overrides_and_inserts_are_local_independent_copies() {
    let leaf = r#"maac 1;
library leaf {version="1";}
tuning tune {period=1200ct;steps=[0ct,700ct];reference_index=0;reference_frequency=220Hz;}
curve gain {clock=normalized;points=[(0,1,linear),(1,2,step)];}
pattern riff {length=1q;note n {at=0q;dur=1/2q;pitch=degree(0,&tune);expression e {kind=gain;curve=&gain;}}}
"#;
    let outer = format!(
        r#"maac 1;
library outer {{version="1";}}
import hidden {{path="leaf.maac";hash="{}";}}
pattern phrase {{length=1q;use u {{pattern=&hidden.riff;at=0q;}}}}
tuning public_tune {{period=1200ct;steps=[0ct,700ct];reference_index=0;reference_frequency=220Hz;}}
curve public_gain {{clock=normalized;points=[(0,1,linear),(1,2,step)];}}
"#,
        sha256_digest(leaf.as_bytes())
    );
    let text = source(&format!(
        r#"
import music {{path="lib/outer.maac";hash="{}";}}
place play {{pattern=&music.phrase;track=&notes;at=0q;count=2;
override final {{event="0/u/0/n";set={{pitch=degree(1,&music.public_tune);}};}}
insert once {{note extra {{at=2q;dur=1/4q;pitch=degree(0,&music.public_tune);expression e {{kind=gain;curve=&music.public_gain;}}}}}}
}}
place other {{pattern=&music.phrase;track=&notes;at=4q;}}
"#,
        sha256_digest(outer.as_bytes())
    ));
    let mut bundle = SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), text.clone()),
            ("lib/outer.maac".into(), outer),
            ("lib/leaf.maac".into(), leaf.into()),
        ]),
        assets: BTreeMap::new(),
    };
    let before = maac::compile_bundle(&bundle).unwrap().events;
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = SourceDocument::parse(text).unwrap();
    let plan = context
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    assert_eq!(
        plan.mappings[0].old_source_object_path,
        vec!["music", "hidden", "riff", "n"]
    );
    document.apply(&plan.transaction, &context).unwrap();
    for operation in &plan.transaction.operations {
        if let Operation::InsertObject {
            id, object_value, ..
        } = operation
        {
            assert!(!id.contains('.'));
            let encoded = serde_json::to_string(object_value).unwrap();
            assert!(
                !encoded.contains("music."),
                "copied subtree retained imported dependency: {encoded}"
            );
        }
    }
    bundle
        .sources
        .insert(bundle.entry.clone(), document.source().into());
    let after = maac::compile_bundle(&bundle).unwrap().events;
    mapped_event_values(&before, &after, &plan);
    let override_tuning = document.authored().tree()["objects"]["play"]["children"]["final"]
        ["fields"]["set"]["fields"]["pitch"]["args"][1]["path"][0]
        .as_str()
        .unwrap();
    assert!(document.authored().tree()["objects"]
        .get(override_tuning)
        .is_some());
    // A later, correctly repinned library edit affects the shared placement
    // while every materialized occurrence still uses its private definitions.
    let changed_leaf = leaf
        .replace("reference_frequency=220Hz", "reference_frequency=110Hz")
        .replace(
            "points=[(0,1,linear),(1,2,step)]",
            "points=[(0,3,linear),(1,4,step)]",
        );
    let original_outer = bundle.sources["lib/outer.maac"].clone();
    let changed_outer = original_outer.replace(
        &sha256_digest(leaf.as_bytes()),
        &sha256_digest(changed_leaf.as_bytes()),
    );
    let changed_entry = document.source().replace(
        &sha256_digest(original_outer.as_bytes()),
        &sha256_digest(changed_outer.as_bytes()),
    );
    bundle.sources.insert("lib/leaf.maac".into(), changed_leaf);
    bundle
        .sources
        .insert("lib/outer.maac".into(), changed_outer);
    bundle.sources.insert(bundle.entry.clone(), changed_entry);
    let after_library_edit = maac::compile_bundle(&bundle).unwrap().events;
    for event in after
        .iter()
        .filter(|event| event.address.starts_with("play/"))
    {
        assert_eq!(
            after_library_edit
                .iter()
                .find(|candidate| candidate.address == event.address)
                .unwrap(),
            event
        );
    }
    let shared = after_library_edit
        .iter()
        .find(|event| event.address.starts_with("other/"))
        .unwrap();
    assert!(
        matches!(&shared.kind, EventKind::Note{pitch_hz:110.0,gain_expression:Some(expression),..} if expression.points[0].gain.to_string()=="3")
    );
}

#[test]
fn messages_preserve_bytes_and_simultaneous_dispatch_class_and_order() {
    use maac::{MessageAdapterCapability, PerformanceDispatchKind};
    let text = source(
        r#"
pattern riff {length=2q;
  note old {at=0q;dur=1q;pitch=C4;order=-100;}
  message msg {at=1q;protocol="midi1";bytes=[176,1,64];order=100;}
  note new {at=1q;dur=1/2q;pitch=E4;order=-200;}
}
place play {pattern=&riff;track=&notes;at=0q;count=2;
override change {event="0/msg";set={bytes=[176,1,127];};}
override gone {event="1/msg";delete=true;}
insert once {message extra {at=3/2q;protocol="midi1";bytes=[144,60,127];}}
}
"#,
    );
    let mut bundle = SourceBundle::new("main.maac", &text);
    let before = artifact_events(&bundle);
    let mut document = SourceDocument::parse(text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    assert_eq!(plan.mappings.len(), 7);
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    bundle
        .sources
        .insert(bundle.entry.clone(), document.source().into());
    mapped_artifact_event_values(&before, &artifact_events(&bundle), &plan);
    let artifact = maac::compiler::compile_bundle_artifact(&bundle).unwrap();
    let dispatches = artifact
        .performance_dispatches(&[MessageAdapterCapability {
            target: maac::plan::PortRef::new("synth", "events").unwrap(),
            protocols: vec!["midi1".into()],
        }])
        .unwrap();
    let same_frame: Vec<_> = dispatches
        .iter()
        .filter(|dispatch| dispatch.frame == 24_000)
        .collect();
    assert_eq!(same_frame.len(), 3);
    assert!(
        matches!(same_frame[0].dispatch, PerformanceDispatchKind::Message {ref bytes,..} if *bytes == vec![176,1,127])
    );
    assert!(matches!(
        same_frame[1].dispatch,
        PerformanceDispatchKind::NoteOff
    ));
    assert!(matches!(
        same_frame[2].dispatch,
        PerformanceDispatchKind::NoteOn { .. }
    ));
    assert_eq!(
        same_frame
            .iter()
            .map(|dispatch| dispatch.order)
            .collect::<Vec<_>>(),
        vec![100, -100, -200]
    );
}

#[test]
fn hits_preserve_overrides_offsets_and_once_only_insert_behavior() {
    let bytes: Vec<u8> = [1.0f32, 0.5]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let text = source(&format!(
        r#"
asset sample {{kind=audio;path="hit.pcm";hash="{}";format="pcm_f32le_interleaved/1";rate=24000Hz;channels=1;frames=2;}}
node kit {{type="core.kit/1";config={{channels=1;samples=[{{key="kick";asset=&sample;}}];}};}}
track drums {{target=&kit:events;}}
pattern leaf {{length=1q;hit h {{at=0q;key="wrong";onset_offset=1ms;order=-2;}}}}
pattern phrase {{length=2q;use u {{pattern=&leaf;at=0q;count=2;transpose=300ct;boundary=cut;}}}}
place play {{pattern=&phrase;track=&drums;at=0q;stretch=2;boundary=cut;
override change {{event="0/u/0/h";set={{key="kick";at=1q;velocity=3/4;}};}}
override gone {{event="0/u/1/h";delete=true;}}
insert once {{hit extra {{at=2q;key="kick";velocity=0;}}}}
}}
"#,
        sha256_digest(&bytes)
    ));
    let mut bundle = SourceBundle::new("main.maac", &text);
    bundle.assets.insert("hit.pcm".into(), bytes);
    let before = artifact_events(&bundle);
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = SourceDocument::parse(text).unwrap();
    let plan = context
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    assert_eq!(plan.mappings.len(), 3);
    document.apply(&plan.transaction, &context).unwrap();
    bundle
        .sources
        .insert(bundle.entry.clone(), document.source().into());
    let after = artifact_events(&bundle);
    mapped_artifact_event_values(&before, &after, &plan);
    assert_eq!(after[0]["on_frame"], 24_048);
    assert_eq!(after[0]["score_on_q"], "1/1");
    assert_eq!(after[0]["kind"]["kind"], "hit");
    assert_eq!(after[0]["kind"]["key"], "kick");
    assert_eq!(after[0]["kind"]["velocity"], "3/4");
    assert_eq!(after[1]["address"], "play/once/extra");
}

#[test]
fn hertz_and_ratio_pitch_keep_fractional_cent_transposes() {
    let text = source(
        r#"
pattern leaf {length=1q;
note hz {at=0q;dur=1/4q;pitch=220Hz;}
note ratio {at=1/2q;dur=1/4q;pitch=ratio(3/2,220Hz);}
}
pattern phrase {length=1q;use u {pattern=&leaf;at=0q;transpose=1/3ct;}}
place play {pattern=&phrase;track=&notes;at=0q;transpose=1/2ct;}
"#,
    );
    let before = events(&text);
    let mut document = SourceDocument::parse(text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    let after = events(document.source());
    mapped_event_values(&before, &after, &plan);
    for (event, base) in after.iter().zip([220.0, 330.0]) {
        let expected = base * 2f64.powf((5.0 / 6.0) / 1200.0);
        assert!(
            matches!(event.kind,EventKind::Note{pitch_hz,..} if (pitch_hz-expected).abs()<1e-10)
        );
    }
}

#[test]
fn retained_leaf_ids_cannot_collide_with_generated_nested_use_ids() {
    let text = source(
        r#"
pattern leaf {length=1q;note n {at=0q;dur=1/4q;pitch=A4;}}
pattern phrase {length=1q;
note _use0 {at=1/2q;dur=1/4q;pitch=C4;}
use u {pattern=&leaf;at=0q;}
}
place play {pattern=&phrase;track=&notes;at=0q;}
"#,
    );
    let before = events(&text);
    let mut document = SourceDocument::parse(text).unwrap();
    let plan = FoundationEditContext
        .prepare_materialize_instance(document.authored(), "play", "copy")
        .unwrap();
    let retained = plan
        .mappings
        .iter()
        .find(|mapping| mapping.old_event_address == "play/0/_use0")
        .unwrap();
    assert!(retained.new_event_address.ends_with("/_use0"));
    let nested = plan
        .mappings
        .iter()
        .find(|mapping| mapping.old_event_address == "play/0/u/0/n")
        .unwrap();
    assert!(nested.new_event_address.contains("/_use1/0/n"));
    document
        .apply(&plan.transaction, &FoundationEditContext)
        .unwrap();
    mapped_event_values(&before, &events(document.source()), &plan);
}

#[test]
fn subtree_bytes_and_exact_rational_growth_fail_without_mutating_source() {
    let huge_label = "x".repeat(300_000);
    let text = source(&format!(
        r#"pattern riff {{length=1q;label="{huge_label}";}} place play {{pattern=&riff;track=&notes;at=0q;count=20;}}"#
    ));
    let document = SourceDocument::parse(text).unwrap();
    let original = document.authored().clone();
    assert_eq!(
        FoundationEditContext
            .prepare_materialize_instance(document.authored(), "play", "copy")
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    assert_eq!(document.authored(), &original);
    let large_stretch = num_bigint::BigInt::from(1u8) << 4095usize;
    let document = SourceDocument::parse(source(&format!("pattern riff {{length=2q;}} place play {{pattern=&riff;track=&notes;at=0q;stretch={large_stretch};}}"))).unwrap();
    assert_eq!(
        FoundationEditContext
            .prepare_materialize_instance(document.authored(), "play", "copy")
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
}
