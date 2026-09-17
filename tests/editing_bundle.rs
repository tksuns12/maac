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
    apply_transaction(&mut document, &transaction, &context).unwrap();
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
