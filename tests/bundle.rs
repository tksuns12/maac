use std::collections::BTreeMap;
use std::fs;

use maac::bundle::{
    normalize_file_reference, sha256_digest, DependencyIdentity, SourceBundle, SourceIdentity,
    MAX_BUNDLE_ASSET_BYTES, MAX_BUNDLE_FILE_BYTES, MAX_BUNDLE_PATH_BYTES, MAX_BUNDLE_SOURCES,
    MAX_BUNDLE_SOURCE_BYTES, MAX_IMPORT_DEPTH, MAX_SYNTAX_OBJECTS,
};
use maac::bundle_fs::load_bundle;
use maac::DiagnosticCode;
use tempfile::tempdir;

fn source_with(body: &str) -> String {
    format!("maac 1;\n{body}\n")
}

fn first_code(bundle: &SourceBundle) -> DiagnosticCode {
    bundle
        .resolve()
        .expect_err("bundle must be rejected")
        .first()
        .expect("diagnostic must be present")
        .code
}

#[test]
fn resolves_hash_pinned_transitive_imports_relative_to_the_declaring_source() {
    let leaf = source_with("node tone { type = \"core.sine/1\"; }");
    let middle = source_with(&format!(
        "import tone {{ path = \"../shared/tone.maac\"; hash = \"{}\"; }}",
        sha256_digest(leaf.as_bytes())
    ));
    let entry = source_with(&format!(
        "import sounds {{ path = \"lib/sounds.maac\"; hash = \"{}\"; }}",
        sha256_digest(middle.as_bytes())
    ));
    let bundle = SourceBundle {
        entry: "main.maac".into(),
        sources: BTreeMap::from([
            ("main.maac".into(), entry),
            ("lib/sounds.maac".into(), middle),
            ("shared/tone.maac".into(), leaf),
        ]),
        assets: BTreeMap::new(),
    };

    let resolved = bundle.resolve().expect("valid imports should resolve");
    assert_eq!(resolved.entry, "main.maac");
    assert_eq!(resolved.documents.len(), 3);
    assert_eq!(resolved.imports["main.maac"]["sounds"], "lib/sounds.maac");
    assert_eq!(
        resolved.imports["lib/sounds.maac"]["tone"],
        "shared/tone.maac"
    );
    assert_eq!(resolved.dependencies.len(), 2);
    assert_eq!(resolved.source_files.len(), 3);
}

#[test]
fn sha256_and_identity_json_are_canonical_and_strict() {
    assert_eq!(
        sha256_digest(b"abc"),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let source = SourceIdentity {
        path: "main.maac".into(),
        hash: sha256_digest(b"maac 1;"),
    };
    assert_eq!(
        serde_json::from_value::<SourceIdentity>(serde_json::to_value(&source).unwrap()).unwrap(),
        source
    );
    assert!(serde_json::from_str::<DependencyIdentity>(
        r#"{"source":"main.maac","alias":"x","path":"x.maac","hash":"sha256:0000000000000000000000000000000000000000000000000000000000000000","extra":true}"#
    )
    .is_err());
}

#[test]
fn rejects_missing_mismatched_and_malformed_imports() {
    let missing = SourceBundle::new(
        "main.maac",
        source_with(&format!(
            "import x {{ path = \"x.maac\"; hash = \"{}\"; }}",
            sha256_digest(b"maac 1;")
        )),
    );
    assert_eq!(first_code(&missing), DiagnosticCode::Reference);

    let mut mismatched = missing.clone();
    mismatched.sources.insert("x.maac".into(), source_with(""));
    assert_eq!(first_code(&mismatched), DiagnosticCode::Hash);

    let malformed = SourceBundle::new(
        "main.maac",
        source_with("import x { path = \"x.maac\"; hash = \"SHA256:bad\"; }"),
    );
    assert_eq!(first_code(&malformed), DiagnosticCode::Hash);

    let extra_field = SourceBundle::new(
        "main.maac",
        source_with(&format!(
            "import x {{ path = \"x.maac\"; hash = \"{}\"; optional = true; }}",
            sha256_digest(b"maac 1;")
        )),
    );
    assert_eq!(first_code(&extra_field), DiagnosticCode::Reference);
}

#[test]
fn rejects_import_cycles_and_project_root_escapes() {
    let zero_pin = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let mut cycle = SourceBundle::new(
        "a.maac",
        source_with(&format!(
            "import b {{ path = \"b.maac\"; hash = \"{zero_pin}\"; }}"
        )),
    );
    cycle.sources.insert(
        "b.maac".into(),
        source_with(&format!(
            "import a {{ path = \"a.maac\"; hash = \"{zero_pin}\"; }}"
        )),
    );
    let diagnostics = cycle.resolve().expect_err("cycle must be rejected");
    assert_eq!(diagnostics.first().unwrap().code, DiagnosticCode::Reference);
    assert!(diagnostics.first().unwrap().message.contains("cycle"));

    for path in ["../x.maac", "/x.maac", r"dir\x.maac", "C:/x.maac"] {
        assert!(
            normalize_file_reference("main.maac", path).is_err(),
            "{path}"
        );
    }
}

#[test]
fn resolves_only_declared_wavetable_assets() {
    let wave = vec![0, 1, 2, 3];
    let source = source_with(&format!(
        r#"wavetable colors {{ path = "tables/colors.wav"; hash = "{}"; cycle_length = 8; }}
asset legacy {{ kind = blob; path = "missing.bin"; hash = "sha256:0000000000000000000000000000000000000000000000000000000000000000"; }}"#,
        sha256_digest(&wave)
    ));
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle
        .assets
        .insert("tables/colors.wav".into(), wave.clone());

    let resolved = bundle.resolve().expect("wavetable bytes should resolve");
    assert_eq!(resolved.assets["tables/colors.wav"], wave);
    assert_eq!(resolved.assets.len(), 1);
}

#[test]
fn rejects_wavetable_hash_mismatches_and_missing_assets() {
    let source = source_with(
        "wavetable colors { path = \"colors.wav\"; hash = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"; cycle_length = 8; }",
    );
    let missing = SourceBundle::new("main.maac", source.clone());
    assert_eq!(first_code(&missing), DiagnosticCode::Asset);

    let mut mismatch = SourceBundle::new("main.maac", source);
    mismatch.assets.insert("colors.wav".into(), vec![1]);
    assert_eq!(first_code(&mismatch), DiagnosticCode::Hash);
}

#[test]
fn preflights_counts_and_aggregate_bytes_across_unreachable_inputs() {
    let mut too_many = SourceBundle::new("main.maac", source_with(""));
    for index in 0..MAX_BUNDLE_SOURCES {
        too_many
            .sources
            .insert(format!("unused-{index}.maac"), source_with(""));
    }
    assert_eq!(first_code(&too_many), DiagnosticCode::ResourceLimit);

    let mut source_bytes = SourceBundle::new("main.maac", source_with(""));
    let chunk = "x".repeat(MAX_BUNDLE_FILE_BYTES);
    for index in 0..=(MAX_BUNDLE_SOURCE_BYTES / MAX_BUNDLE_FILE_BYTES) {
        source_bytes
            .sources
            .insert(format!("unused-{index}.maac"), chunk.clone());
    }
    assert_eq!(first_code(&source_bytes), DiagnosticCode::ResourceLimit);

    let mut asset_bytes = SourceBundle::new("main.maac", source_with(""));
    let chunk = vec![0; MAX_BUNDLE_FILE_BYTES];
    for index in 0..=(MAX_BUNDLE_ASSET_BYTES / MAX_BUNDLE_FILE_BYTES) {
        asset_bytes
            .assets
            .insert(format!("unused-{index}.wav"), chunk.clone());
    }
    assert_eq!(first_code(&asset_bytes), DiagnosticCode::ResourceLimit);
}

#[test]
fn rejects_aggregate_syntax_objects_and_invalid_unreachable_sources() {
    let mut objects = String::from("maac 1;\n");
    for index in 0..=MAX_SYNTAX_OBJECTS {
        use std::fmt::Write;
        writeln!(&mut objects, "node n{index} {{}}").unwrap();
    }
    assert!(objects.len() <= MAX_BUNDLE_FILE_BYTES);
    let object_bomb = SourceBundle::new("main.maac", objects);
    assert_eq!(first_code(&object_bomb), DiagnosticCode::ResourceLimit);

    let mut invalid_unused = SourceBundle::new("main.maac", source_with(""));
    invalid_unused
        .sources
        .insert("unused.maac".into(), "not a MaaC document".into());
    assert_eq!(first_code(&invalid_unused), DiagnosticCode::Syntax);
}

#[test]
fn rejects_oversized_bundle_paths_even_when_inputs_are_unreachable() {
    let oversized = "x".repeat(MAX_BUNDLE_PATH_BYTES + 1);

    let oversized_entry = SourceBundle::new(oversized.clone(), source_with(""));
    assert_eq!(first_code(&oversized_entry), DiagnosticCode::ResourceLimit);

    let mut oversized_source = SourceBundle::new("main.maac", source_with(""));
    oversized_source
        .sources
        .insert(oversized.clone(), source_with(""));
    assert_eq!(first_code(&oversized_source), DiagnosticCode::ResourceLimit);

    let mut oversized_asset = SourceBundle::new("main.maac", source_with(""));
    oversized_asset.assets.insert(oversized, Vec::new());
    assert_eq!(first_code(&oversized_asset), DiagnosticCode::ResourceLimit);
}

#[test]
fn rejects_oversized_declared_and_joined_file_references() {
    let oversized = "x".repeat(MAX_BUNDLE_PATH_BYTES + 1);
    let valid_pin = sha256_digest(b"maac 1;\n\n");
    let import = SourceBundle::new(
        "main.maac",
        source_with(&format!(
            "import dep {{ path = \"{oversized}\"; hash = \"{valid_pin}\"; }}"
        )),
    );
    assert_eq!(first_code(&import), DiagnosticCode::ResourceLimit);

    let wavetable = SourceBundle::new(
        "main.maac",
        source_with(&format!(
            "wavetable wave {{ path = \"{oversized}\"; hash = \"{}\"; cycle_length = 8; }}",
            sha256_digest(b"wave")
        )),
    );
    assert_eq!(first_code(&wavetable), DiagnosticCode::ResourceLimit);

    let base = format!("{}/main.maac", "a".repeat(3_000));
    let reference = format!("{}.maac", "b".repeat(2_000));
    let diagnostics = normalize_file_reference(&base, &reference).unwrap_err();
    assert_eq!(
        diagnostics.first().unwrap().code,
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn accepts_bundle_paths_and_joined_references_at_the_exact_limit() {
    let exact_source_path = "s".repeat(MAX_BUNDLE_PATH_BYTES);
    let dependency = source_with("");
    let mut bundle = SourceBundle::new(
        "main.maac",
        source_with(&format!(
            "import dep {{ path = \"{exact_source_path}\"; hash = \"{}\"; }}",
            sha256_digest(dependency.as_bytes())
        )),
    );
    bundle.sources.insert(exact_source_path, dependency);
    bundle
        .resolve()
        .expect("an exact-limit import path should resolve");

    let exact_entry = "e".repeat(MAX_BUNDLE_PATH_BYTES);
    SourceBundle::new(exact_entry, source_with(""))
        .resolve()
        .expect("an exact-limit entry and source key should resolve");

    let exact_asset_path = "a".repeat(MAX_BUNDLE_PATH_BYTES);
    let wave = b"wave";
    let mut asset_bundle = SourceBundle::new(
        "main.maac",
        source_with(&format!(
            "wavetable wave {{ path = \"{exact_asset_path}\"; hash = \"{}\"; cycle_length = 8; }}",
            sha256_digest(wave)
        )),
    );
    asset_bundle.assets.insert(exact_asset_path, wave.to_vec());
    asset_bundle
        .resolve()
        .expect("an exact-limit wavetable path and asset key should resolve");

    let base = format!("{}/main.maac", "a".repeat(2_047));
    let reference = format!("{}.maac", "b".repeat(2_043));
    let normalized = normalize_file_reference(&base, &reference)
        .expect("an exact-limit joined reference should normalize");
    assert_eq!(normalized.len(), MAX_BUNDLE_PATH_BYTES);
}

#[test]
fn rejects_import_depth_in_memory_and_during_filesystem_discovery() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("d")).unwrap();

    let last = MAX_IMPORT_DEPTH + 1;
    let mut sources = BTreeMap::new();
    let mut next = source_with("");
    sources.insert(format!("d/{last}.maac"), next.clone());
    fs::write(root.join(format!("d/{last}.maac")), &next).unwrap();
    for index in (0..last).rev() {
        let source = source_with(&format!(
            "import next {{ path = \"{}.maac\"; hash = \"{}\"; }}",
            index + 1,
            sha256_digest(next.as_bytes())
        ));
        sources.insert(format!("d/{index}.maac"), source.clone());
        fs::write(root.join(format!("d/{index}.maac")), &source).unwrap();
        next = source;
    }
    let bundle = SourceBundle {
        entry: "d/0.maac".into(),
        sources,
        assets: BTreeMap::new(),
    };
    assert_eq!(first_code(&bundle), DiagnosticCode::ResourceLimit);

    let diagnostics = load_bundle(std::path::Path::new("d/0.maac"), root).unwrap_err();
    assert_eq!(
        diagnostics.first().unwrap().code,
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn filesystem_loader_reads_only_explicit_files_and_resolves_wavetables() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("lib/tables")).unwrap();
    let wave = b"wave";
    fs::write(root.join("lib/tables/colors.wav"), wave).unwrap();
    let dep = source_with(&format!(
        "wavetable colors {{ path = \"tables/colors.wav\"; hash = \"{}\"; cycle_length = 8; }}",
        sha256_digest(wave)
    ));
    fs::write(root.join("lib/sounds.maac"), &dep).unwrap();
    let entry = source_with(&format!(
        "import sounds {{ path = \"lib/sounds.maac\"; hash = \"{}\"; }}",
        sha256_digest(dep.as_bytes())
    ));
    fs::write(root.join("main.maac"), entry).unwrap();
    fs::write(root.join("unreachable.maac"), "not maac").unwrap();

    let bundle = load_bundle(std::path::Path::new("main.maac"), root).unwrap();
    assert_eq!(bundle.sources.len(), 2);
    assert_eq!(bundle.assets["lib/tables/colors.wav"], wave);
    assert!(!bundle.sources.contains_key("unreachable.maac"));
}

#[test]
fn filesystem_loader_checks_import_pins_before_parsing_dependencies() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("dep.maac"), "not a MaaC document").unwrap();
    fs::write(
        root.join("main.maac"),
        source_with(
            "import dep { path = \"dep.maac\"; hash = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"; }",
        ),
    )
    .unwrap();

    let diagnostics = load_bundle(std::path::Path::new("main.maac"), root).unwrap_err();
    assert_eq!(diagnostics.first().unwrap().code, DiagnosticCode::Hash);
}

#[cfg(unix)]
#[test]
fn filesystem_loader_accepts_relative_and_absolute_symlinks_within_the_root() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let project = directory.path().join("project");
    fs::create_dir_all(project.join("real")).unwrap();
    let relative_source = source_with("node relative { type = \"core.sine/1\"; }");
    let absolute_source = source_with("node absolute { type = \"core.sine/1\"; }");
    fs::write(project.join("real/relative.maac"), &relative_source).unwrap();
    fs::write(project.join("real/absolute.maac"), &absolute_source).unwrap();
    symlink("real/relative.maac", project.join("relative.maac")).unwrap();
    symlink(
        project.join("real/absolute.maac"),
        project.join("absolute.maac"),
    )
    .unwrap();
    fs::write(
        project.join("main.maac"),
        source_with(&format!(
            "import relative {{ path = \"relative.maac\"; hash = \"{}\"; }}\n\
             import absolute {{ path = \"absolute.maac\"; hash = \"{}\"; }}",
            sha256_digest(relative_source.as_bytes()),
            sha256_digest(absolute_source.as_bytes())
        )),
    )
    .unwrap();

    let bundle = load_bundle(std::path::Path::new("main.maac"), &project).unwrap();
    assert_eq!(bundle.sources["relative.maac"], relative_source);
    assert_eq!(bundle.sources["absolute.maac"], absolute_source);
}

#[cfg(unix)]
#[test]
fn filesystem_loader_rejects_source_and_asset_symlink_escapes() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let project = directory.path().join("project");
    fs::create_dir(&project).unwrap();
    let outside_source = source_with("");
    fs::write(directory.path().join("outside.maac"), &outside_source).unwrap();
    symlink("../outside.maac", project.join("escape.maac")).unwrap();
    fs::write(
        project.join("main.maac"),
        source_with(&format!(
            "import x {{ path = \"escape.maac\"; hash = \"{}\"; }}",
            sha256_digest(outside_source.as_bytes())
        )),
    )
    .unwrap();
    let diagnostics = load_bundle(std::path::Path::new("main.maac"), &project).unwrap_err();
    assert_eq!(diagnostics.first().unwrap().code, DiagnosticCode::Reference);

    fs::write(directory.path().join("outside.wav"), b"wave").unwrap();
    symlink("../outside.wav", project.join("escape.wav")).unwrap();
    fs::write(
        project.join("main.maac"),
        source_with(&format!(
            "wavetable x {{ path = \"escape.wav\"; hash = \"{}\"; cycle_length = 8; }}",
            sha256_digest(b"wave")
        )),
    )
    .unwrap();
    let diagnostics = load_bundle(std::path::Path::new("main.maac"), &project).unwrap_err();
    assert_eq!(diagnostics.first().unwrap().code, DiagnosticCode::Asset);
}
