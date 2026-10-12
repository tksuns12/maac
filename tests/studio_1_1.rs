//! `std/studio/1.1.0`: frozen bytes, every 1.0.0 export unchanged, and the
//! catalog of the two new exports. tests/studio_electric_guitar.rs and
//! tests/studio_piano.rs check how the new exports sound.

use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::stdlib::{self, STUDIO_1_1_ID, STUDIO_1_1_SOURCE, STUDIO_ID, STUDIO_SOURCE};
use maac::{render_artifact, PlanArtifact};

fn plan(source: &str) -> PlanArtifact {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source),
        &PlanLimits::song(),
    )
    .unwrap_or_else(|error| panic!("{error:?}\n{source}"))
}

fn render(plan: &PlanArtifact) -> Vec<f64> {
    let mut samples = Vec::new();
    render_artifact(plan, |frame| {
        samples.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    samples
}

#[test]
fn published_version_keeps_its_exact_source_bytes() {
    assert_eq!(
        sha256_digest(STUDIO_1_1_SOURCE.as_bytes()),
        include_str!("../stdlib/studio/1.1.0.sha256").trim()
    );
}

#[test]
fn the_source_is_1_0_0_with_two_exports_appended() {
    let previous = STUDIO_SOURCE.replacen("version = \"1.0.0\";", "version = \"1.1.0\";", 1);
    assert!(STUDIO_1_1_SOURCE.starts_with(&previous));
    let added = &STUDIO_1_1_SOURCE[previous.len()..];
    let exports: Vec<_> = added
        .lines()
        .filter_map(|line| line.strip_prefix("instrument "))
        .collect();
    assert_eq!(exports, ["electric_guitar {", "piano {"]);
}

#[test]
fn every_1_0_0_export_keeps_its_catalog_entry() {
    let old = stdlib::catalog_for(STUDIO_ID).unwrap();
    let new = stdlib::catalog_for(STUDIO_1_1_ID).unwrap();
    // Only the usage examples differ: they name the newer import.
    for before in &old.instruments {
        let after = new
            .instruments
            .iter()
            .find(|item| item.name == before.name)
            .unwrap();
        assert_eq!(after.family, before.family);
        assert_eq!(after.description, before.description);
        assert_eq!(after.channels, before.channels);
        assert_eq!(after.controls, before.controls);
        assert_eq!(after.guidance, before.guidance);
        assert_eq!(
            after.usage,
            before.usage.replace(STUDIO_ID, STUDIO_1_1_ID),
            "{}",
            before.name
        );
    }
    assert_eq!(old.kits.len(), 1);
    assert_eq!(new.kits.len(), 1);
    assert_eq!(new.kits[0].pieces, old.kits[0].pieces);
    assert_eq!(new.kits[0].controls, old.kits[0].controls);
    let added: Vec<_> = new
        .instruments
        .iter()
        .map(|item| item.name.as_str())
        .filter(|name| old.instruments.iter().all(|item| item.name != *name))
        .collect();
    assert_eq!(added, ["electric_guitar", "piano"]);
}

#[test]
fn shared_exports_render_bit_identically_in_both_versions() {
    let old = stdlib::catalog_for(STUDIO_ID).unwrap();
    for item in &old.instruments {
        let before = render(&plan(&item.usage));
        let after = render(&plan(&item.usage.replace(STUDIO_ID, STUDIO_1_1_ID)));
        assert_eq!(before, after, "{}", item.name);
    }
}

#[test]
fn the_new_exports_list_their_full_range_and_their_usage_plays() {
    let catalog = stdlib::catalog_for(STUDIO_1_1_ID).unwrap();
    for (name, family, low, high) in [
        ("electric_guitar", "guitars", 40, 88),
        ("piano", "keys", 21, 108),
    ] {
        let item = catalog
            .instruments
            .iter()
            .find(|item| item.name == name)
            .unwrap();
        assert_eq!(item.family, family);
        assert_eq!(item.channels, 2);
        assert_eq!(
            (item.guidance.midi_min, item.guidance.midi_max),
            (low, high)
        );
        assert_eq!(
            item.guidance.tested_pitches.len(),
            usize::from(high - low) + 1
        );
        assert!(item.usage.contains(STUDIO_1_1_ID));
        let audio = render(&plan(&item.usage));
        assert!(audio.iter().all(|sample| sample.is_finite()));
        assert!(audio.iter().any(|sample| *sample != 0.0), "{name}");
    }
    let guitar = stdlib::instrument_in(STUDIO_1_1_ID, "electric_guitar").unwrap();
    let controls: Vec<_> = guitar.controls.keys().map(String::as_str).collect();
    assert_eq!(
        controls,
        ["drive", "level", "mute", "pan", "pickup", "release", "tone", "vibrato"]
    );
    let piano = stdlib::instrument_in(STUDIO_1_1_ID, "piano").unwrap();
    let controls: Vec<_> = piano.controls.keys().map(String::as_str).collect();
    assert_eq!(
        controls,
        [
            "brightness",
            "hammer",
            "level",
            "release",
            "tuning_spread",
            "width"
        ]
    );
}
