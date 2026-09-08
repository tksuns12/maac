use maac::bundle::{
    sha256_digest, SourceBundle, MAX_BUNDLE_FILE_BYTES, MAX_BUNDLE_SOURCES,
    MAX_BUNDLE_SOURCE_BYTES, MAX_IMPORT_DEPTH, MAX_SYNTAX_OBJECTS,
};
use maac::DiagnosticCode;

const IMPORT: &str = "import basic { builtin = \"std/basic/1.0.0\"; }";
const BUILTIN_PATH: &str = "@builtin/std/basic/1.0.0.maac";

fn entry(body: &str) -> SourceBundle {
    SourceBundle::new("main.maac", format!("maac 1;\n{body}"))
}

#[test]
fn embeds_builtin_once_with_an_identity_for_each_alias() {
    let bundle = entry(&format!(
        "{IMPORT}\nimport second {{ builtin = \"std/basic/1.0.0\"; }}"
    ));
    let resolved = bundle.resolve().unwrap();
    assert_eq!(bundle.sources.len(), 1);
    assert_eq!(resolved.documents.len(), 2);
    assert_eq!(resolved.source_files.len(), 2);
    assert_eq!(resolved.dependencies.len(), 2);
    let builtin = resolved
        .source_files
        .iter()
        .find(|source| source.path == BUILTIN_PATH)
        .unwrap();
    assert_eq!(
        builtin.hash,
        sha256_digest(maac::stdlib::BASIC_SOURCE.as_bytes())
    );
    for alias in ["basic", "second"] {
        assert_eq!(resolved.imports["main.maac"][alias], BUILTIN_PATH);
        let dependency = resolved
            .dependencies
            .iter()
            .find(|dependency| dependency.alias == alias)
            .unwrap();
        assert_eq!(dependency.path, BUILTIN_PATH);
        assert_eq!(dependency.hash, builtin.hash);
    }
}

#[test]
fn resolves_builtin_imports_inside_pinned_local_libraries() {
    let library = format!("maac 1; library local {{ version = \"1\"; }} {IMPORT}");
    let mut bundle = entry(&format!(
        "import local {{ path = \"lib/local.maac\"; hash = \"{}\"; }}",
        sha256_digest(library.as_bytes())
    ));
    bundle.sources.insert("lib/local.maac".into(), library);
    let resolved = bundle.resolve().unwrap();
    assert_eq!(resolved.documents.len(), 3);
    assert_eq!(resolved.imports["lib/local.maac"]["basic"], BUILTIN_PATH);
}

#[test]
fn rejects_unknown_builtin_ids_and_nonexclusive_import_fields() {
    for body in [
        "import x { builtin = \"std/basic/9.0.0\"; }",
        "import x { builtin = \"std/unknown/1.0.0\"; }",
        "import x { builtin = 1; }",
        "import x { builtin = \"std/basic/1.0.0\"; path = \"x.maac\"; }",
        "import x { builtin = \"std/basic/1.0.0\"; hash = \"ignored\"; }",
        "import x { builtin = \"std/basic/1.0.0\"; extra = true; }",
        "import x { builtin = \"std/basic/1.0.0\"; node child {} }",
    ] {
        let diagnostics = entry(body).resolve().unwrap_err();
        assert_eq!(
            diagnostics.first().unwrap().code,
            DiagnosticCode::Reference,
            "{body}"
        );
    }
    assert!(entry("import x { builtin = \"std/basic/9.0.0\"; }")
        .resolve()
        .unwrap_err()
        .first()
        .unwrap()
        .message
        .contains("unknown built-in"));
}

#[test]
fn reserves_embedded_namespace_for_sources_assets_entry_and_local_references() {
    let mut source = entry(IMPORT);
    source.sources.insert(BUILTIN_PATH.into(), "maac 1;".into());
    let mut asset = entry(IMPORT);
    asset.assets.insert(BUILTIN_PATH.into(), vec![]);
    let reserved_entry = SourceBundle::new(BUILTIN_PATH, "maac 1;");
    for bundle in [source, asset, reserved_entry] {
        assert_eq!(
            bundle.resolve().unwrap_err().first().unwrap().code,
            DiagnosticCode::Reference
        );
    }
    for path in [
        BUILTIN_PATH,
        "./@builtin/std/basic/1.0.0.maac",
        "lib/../@builtin/std/basic/1.0.0.maac",
    ] {
        assert!(
            maac::bundle::normalize_file_reference("main.maac", path).is_err(),
            "{path}"
        );
    }
}

#[test]
fn embedded_source_counts_toward_source_and_depth_limits() {
    let mut bundle = entry(IMPORT);
    for index in 1..MAX_BUNDLE_SOURCES {
        bundle
            .sources
            .insert(format!("unused-{index}.maac"), "maac 1;".into());
    }
    assert_eq!(
        bundle.resolve().unwrap_err().first().unwrap().code,
        DiagnosticCode::ResourceLimit
    );
    bundle.sources.remove("unused-1.maac");
    bundle
        .sources
        .get_mut("main.maac")
        .unwrap()
        .push_str("\nimport second { builtin = \"std/basic/1.0.0\"; }");
    assert!(bundle.resolve().is_ok());

    let mut chain = SourceBundle::new(
        format!("s{MAX_IMPORT_DEPTH}.maac"),
        format!("maac 1; {IMPORT}"),
    );
    let mut child = chain.sources[&chain.entry].clone();
    for index in (0..MAX_IMPORT_DEPTH).rev() {
        let source = format!(
            "maac 1; import next {{ path = \"s{}.maac\"; hash = \"{}\"; }}",
            index + 1,
            sha256_digest(child.as_bytes())
        );
        chain
            .sources
            .insert(format!("s{index}.maac"), source.clone());
        child = source;
    }
    chain.entry = "s0.maac".into();
    assert_eq!(
        chain.resolve().unwrap_err().first().unwrap().code,
        DiagnosticCode::ResourceLimit
    );
    chain.entry = "s1.maac".into();
    assert!(chain.resolve().is_ok());
}

#[test]
fn parsed_document_api_still_requires_a_bundle_for_builtin_imports() {
    let document = maac::parse(&format!("maac 1; {IMPORT}")).unwrap();
    assert!(maac::check(&document).is_err());
    assert!(maac::compile(&document).is_err());
}

#[test]
fn embedded_bytes_count_at_the_aggregate_boundary() {
    let mut bundle = entry(IMPORT);
    let mut remaining = MAX_BUNDLE_SOURCE_BYTES
        - maac::stdlib::BASIC_SOURCE.len()
        - bundle.sources["main.maac"].len();
    let mut index = 0;
    while remaining > 0 {
        let bytes = remaining.min(MAX_BUNDLE_FILE_BYTES);
        let source = format!("maac 1;{}", " ".repeat(bytes - 7));
        bundle
            .sources
            .insert(format!("padding-{index}.maac"), source);
        remaining -= bytes;
        index += 1;
    }
    assert!(bundle.resolve().is_ok());
    bundle.sources.get_mut("main.maac").unwrap().push(' ');
    assert_eq!(
        bundle.resolve().unwrap_err().first().unwrap().code,
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn embedded_objects_count_at_the_aggregate_boundary() {
    fn objects(object: &maac::Object) -> usize {
        1 + object.children.values().map(objects).sum::<usize>()
    }
    let embedded = maac::parse(maac::stdlib::BASIC_SOURCE).unwrap();
    let embedded_count: usize = embedded.objects.values().map(objects).sum();
    let mut bundle = entry(IMPORT);
    let mut source = String::from("maac 1;\n");
    for index in 0..MAX_SYNTAX_OBJECTS - embedded_count - 1 {
        use std::fmt::Write;
        writeln!(&mut source, "node n{index} {{}}").unwrap();
    }
    bundle.sources.insert("objects.maac".into(), source);
    assert!(bundle.resolve().is_ok());
    bundle
        .sources
        .get_mut("objects.maac")
        .unwrap()
        .push_str("node excess {}\n");
    assert_eq!(
        bundle.resolve().unwrap_err().first().unwrap().code,
        DiagnosticCode::ResourceLimit
    );
}
