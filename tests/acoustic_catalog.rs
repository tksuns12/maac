use std::collections::BTreeSet;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::library::LibrarySet;
use maac::stdlib::{
    self, ACOUSTIC_ID, ACOUSTIC_SOURCE, ACOUSTIC_SOURCE_PATH, BASIC_ID, BASIC_SOURCE,
    BASIC_SOURCE_PATH,
};

#[test]
fn registry_lists_exact_sources_and_basic_wrappers_keep_their_defaults() {
    let listed = stdlib::libraries();
    assert_eq!(listed.len(), 2);
    for (id, path, source) in [
        (BASIC_ID, BASIC_SOURCE_PATH, BASIC_SOURCE),
        (ACOUSTIC_ID, ACOUSTIC_SOURCE_PATH, ACOUSTIC_SOURCE),
    ] {
        let library = listed.iter().find(|library| library.library == id).unwrap();
        assert_eq!(library.source_path, path);
        assert_eq!(library.source_hash, sha256_digest(source.as_bytes()));
        let embedded = stdlib::lookup(id).unwrap();
        assert_eq!(embedded.path, path);
        assert_eq!(embedded.source, source);
    }
    assert_eq!(
        stdlib::catalog().unwrap(),
        stdlib::catalog_for(BASIC_ID).unwrap()
    );
    assert_eq!(
        stdlib::instrument("nylon_guitar").unwrap(),
        stdlib::instrument_in(BASIC_ID, "nylon_guitar").unwrap()
    );
    for unknown in [
        "std/acoustic/latest",
        "std/acoustic/9.0.0",
        "std/basic/1.0.1",
        "unknown",
    ] {
        assert!(stdlib::lookup(unknown).is_none());
        assert_eq!(
            stdlib::catalog_for(unknown)
                .unwrap_err()
                .first()
                .unwrap()
                .code,
            maac::DiagnosticCode::Reference
        );
        assert_eq!(
            stdlib::instrument_in(unknown, "nylon_guitar")
                .unwrap_err()
                .first()
                .unwrap()
                .code,
            maac::DiagnosticCode::Reference
        );
    }
    assert_eq!(
        stdlib::instrument_in(ACOUSTIC_ID, "mellow_piano")
            .unwrap_err()
            .first()
            .unwrap()
            .code,
        maac::DiagnosticCode::Reference
    );
}

#[test]
fn overlapping_exports_coexist_under_distinct_aliases_and_deduplicate_sources() {
    let bundle = SourceBundle::new(
        "mixed.maac",
        format!(
            r#"maac 1;
library mixed {{ version = "1"; }}
import basic {{ builtin = "{BASIC_ID}"; }}
import acoustic {{ builtin = "{ACOUSTIC_ID}"; }}
import repeated {{ builtin = "{ACOUSTIC_ID}"; }}
"#
        ),
    );
    let resolved = bundle.resolve().unwrap();
    assert_eq!(resolved.documents.len(), 3);
    assert_eq!(resolved.source_files.len(), 3);
    assert_eq!(resolved.dependencies.len(), 3);
    assert_eq!(resolved.imports["mixed.maac"]["basic"], BASIC_SOURCE_PATH);
    assert_eq!(
        resolved.imports["mixed.maac"]["acoustic"],
        ACOUSTIC_SOURCE_PATH
    );
    assert_eq!(
        resolved.imports["mixed.maac"]["repeated"],
        ACOUSTIC_SOURCE_PATH
    );
    let library = LibrarySet::resolve(&resolved).unwrap();
    assert_eq!(library.programs.len(), 27);
    let guitars = library
        .programs
        .iter()
        .filter(|program| program.source.object == "nylon_guitar")
        .collect::<Vec<_>>();
    assert_eq!(guitars.len(), 2);
    assert_ne!(guitars[0].source.file, guitars[1].source.file);
    assert_ne!(guitars[0].id, guitars[1].id);
    let nodes = maac::parse("maac 1; node old { instrument = &basic.nylon_guitar; } node new { instrument = &acoustic.nylon_guitar; } node same { instrument = &repeated.nylon_guitar; }").unwrap();
    let old = library
        .resolve_instance("mixed.maac", &nodes.objects["old"])
        .unwrap();
    let new = library
        .resolve_instance("mixed.maac", &nodes.objects["new"])
        .unwrap();
    let repeated = library
        .resolve_instance("mixed.maac", &nodes.objects["same"])
        .unwrap();
    assert_ne!(old.program_id, new.program_id);
    assert_eq!(new.program_id, repeated.program_id);
}

#[test]
fn both_embedded_sources_count_once_toward_the_bundle_limit() {
    let mut bundle = SourceBundle::new("main.maac", format!("maac 1; import basic {{ builtin = \"{BASIC_ID}\"; }} import acoustic {{ builtin = \"{ACOUSTIC_ID}\"; }} import repeated {{ builtin = \"{ACOUSTIC_ID}\"; }}"));
    for index in 3..maac::bundle::MAX_BUNDLE_SOURCES {
        bundle
            .sources
            .insert(format!("unused-{index}.maac"), "maac 1;".into());
    }
    assert!(bundle.resolve().is_ok());
    bundle
        .sources
        .insert("excess.maac".into(), "maac 1;".into());
    assert_eq!(
        bundle.resolve().unwrap_err().first().unwrap().code,
        maac::DiagnosticCode::ResourceLimit
    );
}

#[test]
fn acoustic_catalog_exposes_only_its_three_exports_and_derives_all_control_specs() {
    let catalog = stdlib::catalog_for(ACOUSTIC_ID).unwrap();
    assert_eq!(catalog.library, ACOUSTIC_ID);
    assert_eq!(catalog.source_path, ACOUSTIC_SOURCE_PATH);
    assert_eq!(
        catalog.source_hash,
        sha256_digest(ACOUSTIC_SOURCE.as_bytes())
    );
    assert_eq!(
        catalog
            .instruments
            .iter()
            .map(|item| item.name.as_str())
            .collect::<BTreeSet<_>>(),
        ["nylon_guitar", "steel_guitar", "muted_guitar"]
            .into_iter()
            .collect()
    );
    let bundle = SourceBundle::new("acoustic.maac", format!("maac 1; library test {{ version = \"1\"; }} import acoustic {{ builtin = \"{ACOUSTIC_ID}\"; }}"));
    let library = LibrarySet::resolve(&bundle.resolve().unwrap()).unwrap();
    for item in catalog.instruments {
        assert_eq!(item.channels, 2);
        assert_eq!(
            item.controls
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            [
                "level",
                "brightness",
                "release",
                "pan",
                "bend_ratio",
                "vibrato_amount"
            ]
            .into_iter()
            .collect()
        );
        let program = library
            .programs
            .iter()
            .find(|program| program.source.object == item.name)
            .unwrap();
        for (name, info) in &item.controls {
            let spec = program.control_spec(name).unwrap();
            assert_eq!(info.default, program.controls[name].default);
            assert_eq!(info.unit, spec.unit);
            assert_eq!(info.rate, spec.rate);
            assert_eq!(info.min, spec.min);
            assert_eq!(info.max, spec.max);
            assert_eq!(info.min_open, spec.min_open);
            assert_eq!(info.max_open, spec.max_open);
        }
        assert!(item.usage.contains(ACOUSTIC_ID));
        assert!(!item.usage.contains(BASIC_ID));
        let plan = maac::compile_bundle(&SourceBundle::new("demo.maac", item.usage)).unwrap();
        assert_eq!(plan.events.len(), 1);
        assert_eq!(maac::load_plan(&plan.to_json().unwrap()).unwrap(), plan);
    }
}
