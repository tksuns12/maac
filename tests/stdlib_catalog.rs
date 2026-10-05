use std::collections::BTreeSet;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::library::LibrarySet;
use maac::stdlib::{self, BASIC_ID, BASIC_SOURCE, BASIC_SOURCE_PATH};

const EXPECTED: [&str; 24] = [
    "kick",
    "snare",
    "clap",
    "closed_hat",
    "open_hat",
    "low_tom",
    "high_tom",
    "crash",
    "mellow_piano",
    "bright_piano",
    "electric_piano",
    "organ",
    "nylon_guitar",
    "steel_guitar",
    "muted_guitar",
    "finger_bass",
    "pick_bass",
    "sub_bass",
    "synth_bass",
    "strings",
    "flute",
    "bell",
    "warm_pad",
    "synth_lead",
];

#[test]
fn published_version_keeps_its_exact_source_bytes() {
    assert_eq!(
        sha256_digest(BASIC_SOURCE.as_bytes()),
        include_str!("../stdlib/basic/1.0.0.sha256").trim()
    );
}

#[test]
fn catalog_covers_exactly_the_published_exports_and_editorial_records() {
    let catalog = stdlib::catalog().unwrap();
    assert_eq!(catalog.library, BASIC_ID);
    assert_eq!(catalog.source_path, BASIC_SOURCE_PATH);
    assert_eq!(catalog.source_hash, sha256_digest(BASIC_SOURCE.as_bytes()));
    let names: BTreeSet<_> = catalog
        .instruments
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(names, EXPECTED.into_iter().collect());
    assert_eq!(catalog.instruments.len(), EXPECTED.len());
    let metadata: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../stdlib/basic/1.0.0.json")).unwrap();
    assert_eq!(metadata.len(), names.len());
    assert_eq!(
        metadata
            .iter()
            .map(|item| item["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        names
    );
    for item in &catalog.instruments {
        assert_eq!(item.channels, 2);
        assert!(!item.family.is_empty());
        assert!(!item.description.is_empty());
        assert!(item.guidance.midi_min <= item.guidance.midi_max);
        assert!(item.guidance.example_duration_seconds >= item.guidance.duration_min_seconds);
        assert!(item.guidance.example_duration_seconds <= item.guidance.duration_max_seconds);
        assert_eq!(stdlib::instrument(&item.name).unwrap(), *item);
    }
    assert_eq!(
        stdlib::instrument("nonexistent")
            .unwrap_err()
            .first()
            .unwrap()
            .code,
        maac::DiagnosticCode::Reference
    );
}

#[test]
fn catalog_control_metadata_matches_validated_programs_without_copied_defaults() {
    let source = format!(
        "maac 1; library test {{ version = \"1\"; }} import basic {{ builtin = \"{BASIC_ID}\"; }}"
    );
    let bundle = SourceBundle::new("test.maac", source).resolve().unwrap();
    let library = LibrarySet::resolve(&bundle).unwrap();
    let catalog = stdlib::catalog().unwrap();
    for item in &catalog.instruments {
        let program = library
            .programs
            .iter()
            .find(|program| program.source.object == item.name)
            .unwrap();
        assert_eq!(
            item.controls
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            ["level", "brightness", "release", "pan"]
                .into_iter()
                .collect()
        );
        for (name, info) in &item.controls {
            let spec = program.control_spec(name).unwrap();
            assert_eq!(info.default, program.controls[name].default);
            assert_eq!(info.unit, spec.unit);
            assert_eq!(info.rate, spec.rate);
            assert_eq!(info.min, spec.min);
            assert_eq!(info.max, spec.max);
            assert_eq!(info.min_open, spec.min_open);
            assert_eq!(info.max_open, spec.max_open);
            spec.validate(&info.default).unwrap();
        }
    }
}

#[test]
fn every_catalog_usage_is_a_complete_offline_composition() {
    for item in stdlib::catalog().unwrap().instruments {
        let bundle = SourceBundle::new("demo.maac", &item.usage);
        let plan = maac::compile_bundle(&bundle)
            .unwrap_or_else(|error| panic!("{}: {error:?}", item.name));
        assert_eq!(plan.version, 2);
        assert_eq!(plan.events.len(), 1);
        let bytes = plan.to_json().unwrap();
        assert_eq!(maac::load_plan(&bytes).unwrap(), plan);
    }
}

#[test]
fn basic_1_1_keeps_every_1_0_export_and_adds_the_drums_kit() {
    use maac::stdlib::{BASIC_1_1_ID, BASIC_1_1_SOURCE};
    assert_eq!(
        sha256_digest(BASIC_1_1_SOURCE.as_bytes()),
        include_str!("../stdlib/basic/1.1.0.sha256").trim()
    );
    let old = stdlib::catalog_for(BASIC_ID).unwrap();
    let new = stdlib::catalog_for(BASIC_1_1_ID).unwrap();
    assert!(old.kits.is_empty());
    // The instruments are the same exports with the same controls and
    // guidance; only their usage examples name the newer import.
    assert_eq!(old.instruments.len(), new.instruments.len());
    for (before, after) in old.instruments.iter().zip(&new.instruments) {
        assert_eq!(before.name, after.name);
        assert_eq!(before.controls, after.controls);
        assert_eq!(before.guidance, after.guidance);
    }
    let [kit] = new.kits.as_slice() else {
        panic!("one kit export")
    };
    assert_eq!(kit.name, "drums");
    let keys: Vec<_> = kit.pieces.iter().map(|piece| piece.key.as_str()).collect();
    assert_eq!(
        keys,
        ["clap", "crash", "hat", "high_tom", "kick", "low_tom", "open_hat", "snare"]
    );
    let chokes: Vec<_> = kit
        .pieces
        .iter()
        .filter(|piece| piece.choke.as_deref() == Some("hats"))
        .map(|piece| piece.instrument.as_str())
        .collect();
    assert_eq!(chokes, ["closed_hat", "open_hat"]);
    // Each piece control, named by the piece's key, defaults to its drum's
    // own control default.
    for piece in &kit.pieces {
        let drum = new
            .instruments
            .iter()
            .find(|instrument| instrument.name == piece.instrument)
            .unwrap();
        for control in ["level", "pan", "brightness"] {
            let name = format!("{}_{control}", piece.key);
            assert_eq!(
                kit.controls[&name].default, drum.controls[control].default,
                "{name}"
            );
        }
    }
    assert_eq!(stdlib::kit_in(BASIC_1_1_ID, "drums").unwrap().name, "drums");
    assert!(stdlib::kit_in(BASIC_ID, "drums").is_err());
}
