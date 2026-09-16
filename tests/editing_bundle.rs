use std::collections::BTreeMap;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::editing::{
    apply_transaction, AuthoredDocument, BundleEditContext, Operation, Transaction,
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
fn import_declaration_changes_require_dependency_reresolution() {
    let bundle = imported_pattern_bundle();
    let context = BundleEditContext::new(&bundle).unwrap();
    let mut document = authored(&bundle);
    let original = document.clone();
    let transaction = Transaction::new(
        document.revision().to_owned(),
        vec![Operation::Set {
            object: vec!["sounds".into()],
            field: vec!["path".into()],
            value: json!({"t":"string","v":"other.maac"}),
            expect: None,
            expect_absent: false,
        }],
    )
    .unwrap();
    let error = apply_transaction(&mut document, &transaction, &context).unwrap_err();
    assert_eq!(error.code, "E_CAPABILITY");
    assert_eq!(document, original);
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
