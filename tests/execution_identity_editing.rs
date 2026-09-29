use maac::bundle::SourceBundle;
use maac::editing::{BundleEditContext, EditContext, Operation, SourceDocument, Transaction};
use maac::production_identity::execution_identity;
use serde_json::json;

const SOURCE: &str = r#"maac 1;
project demo {
  score=[0q,4q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&room:out;
  tail=1/10s;
  requires=["maac.production/1"];
}
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
curve rise { clock=normalized; points=[(0,0ct,linear),(1,100ct,step)]; }
node sine { type="core.sine/1"; config={voices=4;}; }
node room { type="fx.reverb/1"; config={channels=1;}; params={mix=0.2;}; }
connect input { from=&sine:out; to=&room:in; }
pattern phrase {
  length=4q;
  note tone {
    label="source label"; at=0q; dur=3q; pitch=C4;
    expression bend { kind=pitch; curve=&rise; }
  }
}
track melody { target=&sine:events; }
place play { pattern=&phrase; track=&melody; at=0q; }
"#;

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

fn context() -> BundleEditContext {
    let bundle = SourceBundle::new("main.maac", SOURCE);
    BundleEditContext::new(&bundle).unwrap()
}

#[test]
fn production_event_normalization_allows_revision_bound_source_edits_atomically() {
    let context = context();
    let mut document = SourceDocument::parse(SOURCE).unwrap();
    let base_revision = document.revision().to_owned();

    let note = &document.authored().tree()["objects"]["phrase"]["children"]["tone"];
    let normalized = context
        .normalize_object(document.authored().tree(), &path(&["phrase", "tone"]), note)
        .unwrap();
    assert_eq!(normalized["children"]["bend"]["kind"], "expression");

    // The FX node makes this resolve through BundleEditContext's artifact
    // normalizer; its compiled reverb defaults are absent from the source.
    let room = &document.authored().tree()["objects"]["room"];
    let normalized_room = context
        .normalize_object(document.authored().tree(), &path(&["room"]), room)
        .unwrap();
    assert_eq!(
        normalized_room["fields"]["config"]["fields"]["predelay"],
        json!({"t":"quantity","n":"0","d":"1","u":"s"})
    );
    assert_eq!(
        normalized_room["fields"]["config"]["fields"]["damping"],
        json!({"t":"number","n":"1","d":"2"})
    );

    let parsed = maac::parse(document.source()).unwrap();
    let plan = maac::compile(&parsed).unwrap();
    let before_identity = execution_identity(&parsed, &plan).unwrap();
    before_identity.validate().unwrap();

    let transaction = Transaction::new(
        base_revision.clone(),
        vec![Operation::Set {
            object: path(&["phrase", "tone"]),
            field: path(&["label"]),
            value: json!({"t":"string","v":"edited label"}),
            expect: Some(json!({"t":"string","v":"source label"})),
            expect_absent: false,
        }],
    )
    .unwrap();
    let applied = document.apply(&transaction, &context).unwrap();

    assert_ne!(document.revision(), base_revision);
    assert_eq!(applied.new_revision, document.revision());
    assert!(document.source().contains("label=\"edited label\""));
    let reparsed = maac::parse(document.source()).unwrap();
    let edited_plan = maac::compile(&reparsed).unwrap();
    let after_identity = execution_identity(&reparsed, &edited_plan).unwrap();
    after_identity.validate().unwrap();
    assert_eq!(
        before_identity.execution_hash,
        after_identity.execution_hash
    );
    assert_ne!(
        before_identity.source_input_hash,
        after_identity.source_input_hash
    );

    let prior_source = document.source().to_owned();
    let prior_revision = document.revision().to_owned();
    let invalid = Transaction::new(
        prior_revision.clone(),
        vec![Operation::Set {
            object: path(&["phrase", "tone"]),
            field: path(&["at"]),
            value: json!({"t":"quantity","n":"-1","d":"1","u":"q"}),
            expect: Some(json!({"t":"quantity","n":"0","d":"1","u":"q"})),
            expect_absent: false,
        }],
    )
    .unwrap();
    let error = document.apply(&invalid, &context).unwrap_err();
    assert_eq!(error.code, "E_RANGE");
    assert_eq!(document.source(), prior_source);
    assert_eq!(document.revision(), prior_revision);
}
