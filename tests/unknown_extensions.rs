//! Unknown extensions are preserved for inspection and refused everywhere else
//! (evidence row D16, spec §§1.2, 26; the
//! [decision](../docs/unknown-extensions-proposal.md)).

use maac::bundle::sha256_digest;
use maac::editing::{AuthoredDocument, BundleEditContext, EditContext, FoundationEditContext};
use maac::extensions::SUPPORTED;
use maac::production_identity::execution_identity;
use maac::{check_bundle, compile_bundle, Diagnostics, SourceBundle};

const SCHEMA: &[u8] = br#"{"title":"notes"}"#;
const EXTENSION: &str = r#"extension e { namespace = "org.example.notes/1"; schema = &notes_schema; render_affecting = false; data = { comment = "keep me"; }; }"#;

fn source(requires: &str, extension: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; {requires} }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node s {{ type = "core.sine/1"; }}
pattern riff {{ length = 1q; note n {{ at = 0q; dur = 1q; pitch = C4; }} }}
track notes {{ target = &s:events; }}
place play {{ pattern = &riff; track = &notes; at = 0q; }}
asset notes_schema {{ kind = descriptor; path = "notes.json"; hash = "{}"; }}
{extension}
"#,
        sha256_digest(SCHEMA)
    )
}

fn unknown() -> String {
    source(r#"requires = ["org.example.notes/1"];"#, EXTENSION)
}

fn bundle(source: &str) -> SourceBundle {
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle.assets.insert("notes.json".into(), SCHEMA.to_vec());
    bundle
}

/// (code, object path, field path) of each diagnostic.
fn located(diagnostics: &Diagnostics) -> Vec<(String, Vec<String>, Vec<String>)> {
    diagnostics
        .iter()
        .map(|d| {
            (
                d.code.as_str().to_owned(),
                d.object_path.clone(),
                d.field_path.clone(),
            )
        })
        .collect()
}

fn at(code: &str, object: &str, field: &str) -> (String, Vec<String>, Vec<String>) {
    (code.into(), vec![object.into()], vec![field.into()])
}

#[test]
fn unknown_extensions_are_preserved_for_inspection() {
    let source = unknown();
    let parsed = maac::parse(&source).unwrap();
    let tree = parsed.to_syntax_json_value();
    assert_eq!(
        tree["objects"]["e"]["fields"]["data"]["fields"]["comment"]["v"],
        "keep me"
    );
    let authored = AuthoredDocument::from_document(&parsed).unwrap();
    assert_eq!(authored.tree(), &tree);
    let changed = AuthoredDocument::from_document(
        &maac::parse(&source.replace("keep me", "changed")).unwrap(),
    )
    .unwrap();
    assert_ne!(authored.revision(), changed.revision());
}

#[test]
fn unknown_extensions_are_refused_at_their_namespace() {
    let source = unknown();
    let capability = at("E_CAPABILITY", "e", "namespace");

    let validated = maac::semantic::validate_source(&maac::parse(&source).unwrap()).unwrap_err();
    let found = located(&validated);
    assert!(found.contains(&capability), "{found:?}");
    assert!(
        found.contains(&at("E_CAPABILITY", "p", "requires")),
        "{found:?}"
    );
    // The extension is reported only at its namespace.
    assert_eq!(
        found
            .iter()
            .filter(|(_, object, _)| object == &["e"])
            .count(),
        1,
        "{found:?}"
    );

    for refused in [
        check_bundle(&bundle(&source)).unwrap_err(),
        compile_bundle(&bundle(&source)).map(|_| ()).unwrap_err(),
    ] {
        assert_eq!(located(&refused), std::slice::from_ref(&capability));
    }

    let authored = AuthoredDocument::from_document(&maac::parse(&source).unwrap()).unwrap();
    let foundation = FoundationEditContext
        .validate_document(authored.tree())
        .unwrap_err();
    assert_eq!(foundation.code, "E_CAPABILITY");
    assert_eq!(foundation.object_path, ["e"]);
    assert_eq!(foundation.field_path, ["namespace"]);
    let context = BundleEditContext::new(&bundle(&source)).unwrap();
    let edited = context.validate_document(authored.tree()).unwrap_err();
    assert_eq!(edited.code, "E_CAPABILITY");
    assert_eq!(edited.object_path, ["e"]);
    assert_eq!(edited.field_path, ["namespace"]);
}

#[test]
fn unknown_extensions_have_no_execution_identity() {
    let plain = source("", "");
    let plan = compile_bundle(&bundle(&plain.replace("asset notes_schema", "// "))).unwrap();
    let document = maac::parse(&unknown()).unwrap();
    let error = execution_identity(&document, &plan).unwrap_err();
    assert!(error.to_string().contains("org.example.notes/1"), "{error}");
}

#[test]
fn every_namespace_must_be_listed_in_project_requires() {
    let reference = at("E_REFERENCE", "e", "namespace");
    // Unknown namespace, not required: both findings.
    let unlisted = check_bundle(&bundle(&source("", EXTENSION))).unwrap_err();
    assert_eq!(
        located(&unlisted),
        [at("E_CAPABILITY", "e", "namespace"), reference.clone()]
    );
    // Recognized namespaces, with `requires` absent or not naming them.
    for namespace in SUPPORTED {
        let extension = EXTENSION
            .replace("org.example.notes/1", namespace)
            .replace("render_affecting = false", "render_affecting = true");
        for requires in ["", r#"requires = ["org.example.other/1"];"#] {
            let refused = check_bundle(&bundle(&source(requires, &extension))).unwrap_err();
            let found = located(&refused);
            assert!(
                found.contains(&reference),
                "{namespace} {requires}: {found:?}"
            );
            assert!(
                !found.contains(&at("E_CAPABILITY", "e", "namespace")),
                "{namespace}: {found:?}"
            );
        }
    }
}

#[test]
fn the_capability_matrix_states_the_supported_identifiers() {
    let matrix = include_str!("../docs/capabilities.md");
    let line = matrix
        .lines()
        .find(|line| line.starts_with("**Supported extension identifiers (§1.2):**"))
        .expect("the matrix states its supported extension identifiers");
    let mut stated: Vec<&str> = line.split('`').skip(1).step_by(2).collect();
    stated.sort_unstable();
    let mut supported = SUPPORTED.to_vec();
    supported.sort_unstable();
    assert_eq!(stated, supported);
}
