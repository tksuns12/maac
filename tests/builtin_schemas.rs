//! A descriptor asset may name a built-in schema instead of pinning a copied
//! file (§14.1). Both forms must compile to the same plan, need no bundle
//! bytes in the built-in form, and record the embedded schema's identity.

use std::collections::BTreeMap;
use std::path::Path;

use maac::bundle::{sha256_digest, SourceIdentity};
use maac::extensions::{builtin_schema, SCHEMAS, SUPPORTED};
use maac::production_data::{prepare_document, SCHEMA_BYTES};
use maac::{parse, SourceBundle};
use serde_json::Value;

const EXAMPLE: &str = include_str!("../examples/production.maac");
const COPIED: &str = r#"asset production_schema {
  kind = descriptor;
  path = "schemas/production.schema.json";
  hash = "sha256:1f9f3623515e645834fd97ecb300cdf00c5e11f4dd260f5bd85a92d3af91568d";
}"#;
const BUILTIN: &str =
    r#"asset production_schema { kind = descriptor; builtin = "maac.production/1"; }"#;

fn builtin_example() -> String {
    assert!(EXAMPLE.contains(COPIED));
    EXAMPLE.replace(COPIED, BUILTIN)
}

fn copied_bundle() -> SourceBundle {
    let mut bundle = SourceBundle::new("main.maac", EXAMPLE);
    bundle.assets.insert(
        "schemas/production.schema.json".into(),
        SCHEMA_BYTES.to_vec(),
    );
    bundle
}

/// A plan without what depends on the source text: spans, source hashes
/// and the execution identity. Everything left is render input.
fn comparable(plan: &impl serde::Serialize) -> Value {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for key in ["span", "source_files", "execution_identity"] {
                    map.remove(key);
                }
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(plan).unwrap();
    strip(&mut value);
    value
}

#[test]
fn every_supported_namespace_embeds_its_published_schema_file() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        "schemas/production.schema.json",
        "schemas/takes.schema.json",
        "schemas/takes-v2.schema.json",
    ];
    assert_eq!(SCHEMAS.len(), SUPPORTED.len());
    for ((schema, namespace), file) in SCHEMAS.iter().zip(SUPPORTED).zip(files) {
        assert_eq!(schema.namespace, namespace);
        assert_eq!(schema.path, format!("@builtin/schema/{namespace}.json"));
        assert_eq!(schema.bytes, std::fs::read(root.join(file)).unwrap());
        assert_eq!(builtin_schema(namespace), Some(*schema));
    }
    for unknown in [
        "maac.production/2",
        "maac.production",
        "",
        "@builtin/schema/maac.production/1.json",
    ] {
        assert_eq!(builtin_schema(unknown), None, "{unknown}");
    }
}

#[test]
fn builtin_form_prepares_the_same_document_and_plan_as_the_copied_file() {
    let copied = parse(EXAMPLE).unwrap();
    let builtin = parse(&builtin_example()).unwrap();
    let (copied_document, copied_settings) =
        prepare_document(&copied, &copied_bundle().assets).unwrap();
    let (builtin_document, builtin_settings) =
        prepare_document(&builtin, &BTreeMap::new()).unwrap();
    // Only the descriptor's text differs, and preparation removes it.
    assert_eq!(
        builtin_document.objects.keys().collect::<Vec<_>>(),
        copied_document.objects.keys().collect::<Vec<_>>()
    );
    assert!(!builtin_document.objects.contains_key("production_schema"));
    assert_eq!(builtin_settings, copied_settings);
    assert_eq!(
        builtin_settings.unwrap().schema_hash,
        sha256_digest(SCHEMA_BYTES)
    );

    let copied_plan = maac::compile_bundle(&copied_bundle()).unwrap();
    let builtin_plan =
        maac::compile_bundle(&SourceBundle::new("main.maac", builtin_example())).unwrap();
    // Original-source evidence accepts the built-in form.
    let settings = builtin_plan.production.as_ref().unwrap();
    assert!(settings.execution_identity.is_some());
    settings.validate(&builtin_plan).unwrap();
    assert_eq!(comparable(&builtin_plan), comparable(&copied_plan));
}

#[test]
fn builtin_form_reads_no_bytes_and_records_the_embedded_identity() {
    let resolved = SourceBundle::new("main.maac", builtin_example())
        .resolve()
        .unwrap();
    assert!(resolved.assets.is_empty());
    assert_eq!(
        resolved.builtin_schemas,
        [SourceIdentity {
            path: "@builtin/schema/maac.production/1.json".into(),
            hash: sha256_digest(SCHEMA_BYTES),
        }]
    );
    // An unused copy in the bundle is not loaded.
    let mut bundle = SourceBundle::new("main.maac", builtin_example());
    bundle
        .assets
        .insert("schemas/production.schema.json".into(), b"{}".to_vec());
    assert!(bundle.resolve().unwrap().assets.is_empty());
    // The copied form records no built-in schema.
    assert!(copied_bundle()
        .resolve()
        .unwrap()
        .builtin_schemas
        .is_empty());
}

#[test]
fn reserved_builtin_schema_paths_cannot_be_supplied() {
    let path = "@builtin/schema/maac.production/1.json";
    let mut asset = SourceBundle::new("main.maac", builtin_example());
    asset.assets.insert(path.into(), SCHEMA_BYTES.to_vec());
    assert!(asset.resolve().is_err());
    let mut source = SourceBundle::new("main.maac", builtin_example());
    source.sources.insert(path.into(), "maac 1;".into());
    assert!(source.resolve().is_err());
    // Nor can the copied form point its path at the built-in identity.
    let mut copied = SourceBundle::new(
        "main.maac",
        EXAMPLE.replace(
            "path = \"schemas/production.schema.json\";",
            &format!("path = \"{path}\";"),
        ),
    );
    copied.assets.insert(path.into(), SCHEMA_BYTES.to_vec());
    assert!(copied.resolve().is_err());
}

#[test]
fn take_examples_compile_identically_in_both_forms() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (example, copied, builtin, file) in [
        (
            "examples/take-comp.maac",
            "asset takes_schema {\n  kind = descriptor;\n  path = \"schemas/takes.schema.json\";\n  hash = \"sha256:11cd8e70cf09aff986f821f88f46ad97056cfb5514a4344eeaa38a6d2e432960\";\n}",
            "asset takes_schema { kind = descriptor; builtin = \"maac.takes/1\"; }",
            "schemas/takes.schema.json",
        ),
        (
            "examples/grouped-takes.maac",
            "asset microphone_schema {\n  kind = descriptor; path = \"schemas/takes-v2.schema.json\";\n  hash = \"sha256:4a98b2d72733cd9ba6f90e7c121ea7d67ce79e7d274305a722bbad25ee420e79\";\n}",
            "asset microphone_schema { kind = descriptor; builtin = \"maac.takes/2\"; }",
            "schemas/takes-v2.schema.json",
        ),
    ] {
        let bundle = maac::bundle_fs::load_bundle(&root.join(example), root).unwrap();
        let mut converted = bundle.clone();
        let entry = converted.sources.get_mut(&converted.entry).unwrap();
        assert!(entry.contains(copied), "{example}");
        *entry = entry.replace(copied, builtin);
        assert!(converted.assets.remove(file).is_some(), "{example}");
        let artifact = |bundle: &SourceBundle| {
            let json = maac::compile_bundle_artifact(bundle).unwrap().to_json().unwrap();
            comparable(&serde_json::from_slice::<Value>(&json).unwrap())
        };
        assert_eq!(artifact(&converted), artifact(&bundle), "{example}");
    }
}

#[test]
fn builtin_descriptor_is_only_an_extension_schema() {
    for extra in [
        // Unreferenced, like an unreferenced copied descriptor.
        r#"asset spare { kind = descriptor; builtin = "maac.production/1"; }"#,
        // Played as audio.
        "audio clip { asset = &production_schema; at = 0q; source = [0frame, 1frame]; mode = rate; }",
    ] {
        let source = builtin_example().replace(
            "pattern bass_phrase {",
            &format!("{extra}\npattern bass_phrase {{"),
        );
        let bundle = SourceBundle::new("main.maac", source);
        assert!(
            maac::compiler::check_bundle_artifact(&bundle).is_err(),
            "accepted {extra}"
        );
    }
}
