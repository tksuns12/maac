use std::collections::BTreeMap;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::editing::{
    apply_transaction, AuthoredDocument, BundleEditContext, EditContext, Operation, Transaction,
};
use serde_json::json;

fn imported_pattern_bundle() -> SourceBundle {
    let library = r#"maac 1;
library sounds { version="1"; }
pattern riff { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }
"#;
    let pin = sha256_digest(library.as_bytes());
    let entry = format!(
        r#"maac 1;
project p {{ score=[0q,2q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
import sounds {{ path="sounds.maac"; hash="{pin}"; }}
node s {{ type="core.sine/1"; }}
track notes {{ target=&s:events; }}
place play {{ pattern=&sounds.riff; track=&notes; at=0q; }}
"#
    );
    SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), entry),
            ("sounds.maac".into(), library.into()),
        ]),
        assets: BTreeMap::new(),
    }
}

fn authored(bundle: &SourceBundle) -> AuthoredDocument {
    let document = maac::parse(&bundle.sources[&bundle.entry]).unwrap();
    AuthoredDocument::from_document(&document).unwrap()
}

#[test]
fn fixed_import_context_validates_entry_edits_against_resolved_patterns() {
    let bundle = imported_pattern_bundle();
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = authored(&bundle);
    let base_revision = document.revision().to_owned();
    let transaction = Transaction::new(
        base_revision.clone(),
        vec![Operation::Set {
            object: vec!["play".into()],
            field: vec!["count".into()],
            value: json!({"t":"number","n":"2","d":"1"}),
            expect: None,
            expect_absent: true,
        }],
    )
    .unwrap();
    let applied = apply_transaction(&mut document, &transaction, &context).unwrap();
    assert_ne!(applied.new_revision, base_revision);
    assert_eq!(
        document.tree()["objects"]["play"]["fields"]["count"],
        json!({"t":"number","n":"2","d":"1"})
    );
}

#[test]
fn import_declaration_changes_are_reresolved_and_repinned_atomically() {
    let mut bundle = imported_pattern_bundle();
    let other = r#"maac 1;
library other { version="2"; }
pattern riff { length=1q; note n { at=0q; dur=1/2q; pitch=D4; } }
"#;
    let other_pin = sha256_digest(other.as_bytes());
    bundle.sources.insert("other.maac".into(), other.into());
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = authored(&bundle);
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![
            Operation::Set {
                object: vec!["sounds".into()],
                field: vec!["path".into()],
                value: json!({"t":"string","v":"other.maac"}),
                expect: None,
                expect_absent: false,
            },
            Operation::Set {
                object: vec!["sounds".into()],
                field: vec!["hash".into()],
                value: json!({"t":"string","v":other_pin}),
                expect: None,
                expect_absent: false,
            },
        ],
    )
    .unwrap();
    let prior_revision = document.revision().to_owned();
    let applied = apply_transaction(&mut document, &transaction, &context).unwrap();
    assert_ne!(applied.new_revision, prior_revision);
    assert_eq!(applied.new_revision, document.revision());
    assert_eq!(
        document.tree()["objects"]["sounds"]["fields"]["path"]["v"],
        "other.maac"
    );
}

#[test]
fn bad_import_repin_is_rejected_atomically() {
    let bundle = imported_pattern_bundle();
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = authored(&bundle);
    let original = document.clone();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["sounds".into()],
            field: vec!["hash".into()],
            value: json!({"t":"string","v":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    let error = apply_transaction(&mut document, &transaction, &context).unwrap_err();
    assert_eq!(error.code, "E_HASH");
    assert_eq!(document, original);
}

#[test]
fn standalone_library_sources_are_editable_with_protocol2() {
    let source = r#"maac 1;
library sounds { version="1"; }
pattern riff { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }
"#;
    let bundle = SourceBundle::new("sounds.maac", source);
    let context = BundleEditContext::for_source(&bundle, "sounds.maac").unwrap();
    let parsed = maac::parse(source).unwrap();
    let mut document = AuthoredDocument::from_document(&parsed).unwrap();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["sounds".into()],
            field: vec!["creator".into()],
            value: json!({"t":"string","v":"Editor"}),
            expect: None,
            expect_absent: true,
        }],
    )
    .unwrap();
    apply_transaction(&mut document, &transaction, &context).unwrap();
    assert_eq!(
        document.tree()["objects"]["sounds"]["fields"]["creator"]["v"],
        "Editor"
    );
}

#[test]
fn library_comment_edits_change_import_pin_without_changing_authored_revision() {
    let first = r#"maac 1;
// first source comment
library sounds { version="1"; }
pattern riff { length=1q; note n { at=0q; dur=1/2q; pitch=C4; } }
"#;
    let second = first.replace("first source comment", "edited source comment");
    let first_authored = AuthoredDocument::from_document(&maac::parse(first).unwrap()).unwrap();
    let second_authored = AuthoredDocument::from_document(&maac::parse(&second).unwrap()).unwrap();

    assert_ne!(
        sha256_digest(first.as_bytes()),
        sha256_digest(second.as_bytes())
    );
    assert_eq!(first_authored.revision(), second_authored.revision());
    assert_eq!(first_authored, second_authored);
}

fn imported_instrument_bundle() -> SourceBundle {
    let library = r#"maac 1;
library sounds { version="1"; }
instrument lead {
  channels=1;
  voice v {
    channels=1; amplitude=&amp; output=&osc:out;
    node amp { type="synth.adsr/1"; }
    node osc { type="synth.sine/1"; }
  }
}
"#;
    let pin = sha256_digest(library.as_bytes());
    let entry = format!(
        r#"maac 1;
project p {{ score=[0q,1q]; rate=48000Hz; tempo=&t; meter=&m; output=&lead:out; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
import sounds {{ path="sounds.maac"; hash="{pin}"; }}
node lead {{ instrument=&sounds.lead; }}
pattern riff {{ length=1q; note n {{ at=0q; dur=1/2q; pitch=C4; }} }}
track notes {{ target=&lead:events; }}
place play {{ pattern=&riff; track=&notes; at=0q; }}
"#
    );
    SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), entry),
            ("sounds.maac".into(), library.into()),
        ]),
        assets: BTreeMap::new(),
    }
}

#[test]
fn imported_instrument_descriptors_participate_in_candidate_validation() {
    let bundle = imported_instrument_bundle();
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = authored(&bundle);
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["p".into()],
            field: vec!["label".into()],
            value: json!({"t":"string","v":"Imported composition"}),
            expect: None,
            expect_absent: true,
        }],
    )
    .unwrap();
    let applied = apply_transaction(&mut document, &transaction, &context).unwrap();
    assert!(!applied.impact.full_render_invalidated);
}

#[test]
fn library_preconditions_expand_synth_defaults_without_a_project() {
    let source = r#"maac 1;
library sounds { version="1"; }
instrument lead {
  channels=1;
  voice v {
    channels=1; amplitude=&amp; output=&osc:out;
    node amp { type="synth.adsr/1"; }
    node osc { type="synth.sine/1"; }
  }
}
"#;
    let bundle = SourceBundle::new("sounds.maac", source);
    let context = BundleEditContext::for_source(&bundle, "sounds.maac").unwrap();
    let parsed = maac::parse(source).unwrap();
    let authored = AuthoredDocument::from_document(&parsed).unwrap();
    let node = &authored.tree()["objects"]["lead"]["children"]["v"]["children"]["osc"];
    let normalized = context
        .normalize_object(
            authored.tree(),
            &["lead".into(), "v".into(), "osc".into()],
            node,
        )
        .unwrap();
    assert_eq!(
        normalized["fields"]["params"]["fields"]["ratio"],
        json!({"t":"number","n":"1","d":"1"})
    );
    assert_eq!(
        normalized["fields"]["params"]["fields"]["frequency"],
        json!({"t":"quantity","n":"0","d":"1","u":"Hz"})
    );
    assert!(normalized["fields"].get("config").is_none());
}

#[test]
fn imported_instrument_instance_preconditions_use_resolved_defaults() {
    let bundle = imported_instrument_bundle();
    let context = BundleEditContext::new(&bundle).unwrap();
    let document = authored(&bundle);
    let node = &document.tree()["objects"]["lead"];
    let normalized = context
        .normalize_object(document.tree(), &["lead".into()], node)
        .unwrap();
    assert_eq!(
        normalized["fields"]["config"]["fields"]["voices"],
        json!({"t":"number","n":"64","d":"1"})
    );
    assert!(normalized["fields"]["params"]["fields"].is_object());
}

fn production_bundle() -> SourceBundle {
    let schema = maac::production_data::SCHEMA_BYTES;
    let source = format!(
        r#"maac 1;
project p {{ score=[0q,1q]; rate=48000Hz; tempo=&t; meter=&m; output=&room:out; requires=["maac.production/1"]; }}
tempo t {{ points=[(0q,120bpm,step)]; }}
meter m {{ points=[(0q,4,4)]; }}
asset schema {{ kind=descriptor; path="production.schema.json"; hash="{}"; }}
node sine {{ type="core.sine/1"; }}
node room {{ type="fx.reverb/1"; config={{channels=1;}}; params={{mix=0.5;}}; }}
connect input {{ from=&sine:out; to=&room:in; }}
extension delivery {{ namespace="maac.production/1"; schema=&schema; render_affecting=true; data={{deliveries={{release={{rate=48000Hz;resampler="maac.src.kaiser/1";targets={{master={{role=master;output=&room:out;encoding=wav_f32le;dither={{type=none;}};}};}};}};}};}}; }}
"#,
        sha256_digest(schema)
    );
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle
        .assets
        .insert("production.schema.json".into(), schema.to_vec());
    bundle
}

#[test]
fn production_preconditions_share_fx_execution_defaults() {
    let bundle = production_bundle();
    maac::compiler::check_bundle_artifact(&bundle).unwrap();
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = authored(&bundle);
    let tx = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["room".into()],
            field: vec!["config".into(), "predelay".into()],
            value: json!({"t":"quantity","n":"1","d":"100","u":"s"}),
            expect: Some(json!({"t":"quantity","n":"0","d":"1","u":"ms"})),
            expect_absent: false,
        }],
    )
    .unwrap();
    apply_transaction(&mut document, &tx, &context).unwrap();
    assert_eq!(
        document.tree()["objects"]["room"]["fields"]["config"]["fields"]["predelay"],
        json!({"t":"quantity","n":"1","d":"100","u":"s"})
    );
}

#[test]
fn copied_production_schema_can_become_the_builtin_form() {
    let bundle = production_bundle();
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = authored(&bundle);
    let pin = sha256_digest(maac::production_data::SCHEMA_BYTES);
    let tx = Transaction::new(
        document.revision().to_owned(),
        vec![
            Operation::Unset {
                object: vec!["schema".into()],
                field: vec!["path".into()],
                expect: Some(json!({"t":"string","v":"production.schema.json"})),
            },
            Operation::Unset {
                object: vec!["schema".into()],
                field: vec!["hash".into()],
                expect: Some(json!({"t":"string","v":pin})),
            },
            Operation::Set {
                object: vec!["schema".into()],
                field: vec!["builtin".into()],
                value: json!({"t":"string","v":"maac.production/1"}),
                expect: None,
                expect_absent: true,
            },
        ],
    )
    .unwrap();
    apply_transaction(&mut document, &tx, &context).unwrap();
    let fields = &document.tree()["objects"]["schema"]["fields"];
    assert_eq!(
        fields,
        &json!({
            "kind": {"t":"symbol","v":"descriptor"},
            "builtin": {"t":"string","v":"maac.production/1"},
        })
    );

    // The built-in form edits without any schema bytes in the bundle.
    let source = bundle.sources["main.maac"].replace(
        &format!(
            "asset schema {{ kind=descriptor; path=\"production.schema.json\"; hash=\"{pin}\"; }}"
        ),
        "asset schema { kind=descriptor; builtin=\"maac.production/1\"; }",
    );
    let builtin = SourceBundle::new("main.maac", source);
    maac::compiler::check_bundle_artifact(&builtin).unwrap();
    let context = BundleEditContext::new(&builtin).unwrap();
    let mut document = authored(&builtin);
    let tx = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["room".into()],
            field: vec!["params".into(), "mix".into()],
            value: json!({"t":"number","n":"1","d":"4"}),
            expect: Some(json!({"t":"number","n":"1","d":"2"})),
            expect_absent: false,
        }],
    )
    .unwrap();
    apply_transaction(&mut document, &tx, &context).unwrap();
}

#[test]
fn checking_compile_song_limits_apply_to_fx_projects() {
    let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/ferry/ferry.maac");
    let root = entry.parent().unwrap();
    let bundle = maac::bundle_fs::load_bundle(&entry, root).unwrap();
    let mut doc = maac::editing::SourceDocument::parse(&bundle.sources[&bundle.entry]).unwrap();
    let context = BundleEditContext::new(&bundle)
        .unwrap()
        .checking_compile(maac::plan::PlanLimits::song());
    let first_note_path = doc
        .authored()
        .tree()
        .get("objects")
        .and_then(|v| v.as_object())
        .and_then(|objs| {
            objs.iter().find_map(|(pat_id, pat)| {
                pat.get("children")
                    .and_then(|v| v.as_object())
                    .and_then(|children| {
                        children.iter().find_map(|(child_id, child)| {
                            (child.get("kind").and_then(|k| k.as_str()) == Some("note"))
                                .then(|| vec![pat_id.clone(), child_id.clone()])
                        })
                    })
            })
        })
        .expect("ferry.maac should contain at least one note");
    let tx = Transaction::new(
        doc.revision().to_owned(),
        vec![Operation::Set {
            object: first_note_path,
            field: vec!["pitch".into()],
            value: json!({"t":"symbol","v":"C5"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    doc.apply(&tx, &context)
        .expect("song-limit BundleEditContext should accept fx song edits");
    assert!(doc.source().contains("Morning Ferry"));
}

#[test]
fn take_compiled_artifact_matches_fresh_compile_bit_for_bit() {
    let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/ferry/ferry.maac");
    let root = entry.parent().unwrap();
    let bundle = maac::bundle_fs::load_bundle(&entry, root).unwrap();
    let limits = maac::plan::PlanLimits::song();
    let base_plan = maac::compiler::compile_bundle_artifact_with_limits(&bundle, &limits).unwrap();
    let mut doc = maac::editing::SourceDocument::parse(&bundle.sources[&bundle.entry]).unwrap();

    let context = BundleEditContext::new(&bundle)
        .unwrap()
        .checking_compile(limits)
        .with_compiled_base(doc.authored(), base_plan)
        .unwrap();

    let tx = Transaction::new(
        doc.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["melody_chorus".into(), "f3".into()],
            field: vec!["pitch".into()],
            value: json!({"t":"symbol","v":"A5"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    doc.apply(&tx, &context).unwrap();
    let reused = context
        .take_compiled_artifact(doc.authored())
        .expect("compiled artifact should be reusable after checking_compile apply");
    // Second take returns None (moved out).
    assert!(context.take_compiled_artifact(doc.authored()).is_none());

    let mut updated_bundle = bundle.clone();
    updated_bundle
        .sources
        .insert(updated_bundle.entry.clone(), doc.source().to_owned());
    let fresh =
        maac::compiler::compile_bundle_artifact_with_limits(&updated_bundle, &limits).unwrap();

    let span = maac::dsp::RenderSpan {
        start: 0,
        end: 2_048,
    };
    let mut bits_reused = Vec::new();
    maac::dsp::DspEngine::new_artifact_with_limits(&reused, &limits)
        .unwrap()
        .render_span(span, |frame| {
            bits_reused.extend(frame.iter().map(|s| s.to_bits()));
            Ok(())
        })
        .unwrap();
    let mut bits_fresh = Vec::new();
    maac::dsp::DspEngine::new_artifact_with_limits(&fresh, &limits)
        .unwrap()
        .render_span(span, |frame| {
            bits_fresh.extend(frame.iter().map(|s| s.to_bits()));
            Ok(())
        })
        .unwrap();
    assert_eq!(bits_reused, bits_fresh);
}
