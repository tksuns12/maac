use maac::production_data::{prepare_document, ProductionSettings, SCHEMA_BYTES};
use maac::{parse, SourceBundle};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const EXAMPLE: &str = include_str!("../examples/production.maac");
fn assets() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("production.schema.json".into(), SCHEMA_BYTES.to_vec())])
}
fn prepared(
    source: &str,
) -> Result<(maac::Document, Option<ProductionSettings>), maac::Diagnostics> {
    prepare_document(&parse(source).unwrap(), &assets())
}

#[test]
fn example_resolves_exact_data_and_strips_only_validated_objects() {
    let original = parse(EXAMPLE).unwrap();
    let (document, settings) = prepare_document(&original, &assets()).unwrap();
    let mut settings = settings.unwrap();
    assert_eq!(settings.deliveries["release_cd"].rate, 44100);
    assert_eq!(settings.deliveries["archive"].rate, 96000);
    assert_eq!(settings.deliveries["release_cd"].targets.len(), 3);
    assert_eq!(document.objects.len(), original.objects.len() - 2);
    assert!(!document.objects.contains_key("production_deliveries"));
    assert!(!document.objects.contains_key("production_schema"));
    assert_eq!(document.object("bass_eq"), original.object("bass_eq"));
    assert_eq!(
        document.object("production_demo"),
        original.object("production_demo")
    );
    let plan = maac::compile(&document).unwrap();
    settings.execution_identity =
        Some(maac::production_identity::execution_identity(&original, &plan).unwrap());
    settings.validate(&plan).unwrap();
    assert_eq!(
        plan.audio_output_channels(&settings.deliveries["release_cd"].targets["master"].output)
            .unwrap(),
        2
    );
}

#[test]
fn effects_only_needs_no_extension_and_unknown_objects_survive() {
    let mut document = parse(EXAMPLE).unwrap();
    document.objects.remove("production_deliveries");
    document.objects.remove("production_schema");
    let (unchanged, settings) = prepare_document(&document, &BTreeMap::new()).unwrap();
    assert_eq!(unchanged, document);
    assert!(settings.is_none());
    let extra = format!("{EXAMPLE}\nasset untouched {{ kind = blob; path = \"unused.bin\"; hash = \"sha256:{}\"; }}", "0".repeat(64));
    let (prepared, _) = prepared(&extra).unwrap();
    assert!(prepared.objects.contains_key("untouched"));
    assert!(maac::compile(&prepared).is_err());
}

#[test]
fn invalid_source_data_is_rejected_before_objects_are_removed() {
    for (old, new) in [
        ("requires = [\"maac.production/1\"];", "requires = [];"),
        (
            "namespace = \"maac.production/1\";",
            "namespace = \"maac.production/2\";",
        ),
        ("render_affecting = true;", "render_affecting = false;"),
        ("deliveries = {", "other = 1; deliveries = {"),
        ("rate = 44100Hz;", "rate = 88200Hz;"),
        ("rate = 44100Hz;", "rate = 44100s;"),
        ("role = stem;", "role = master;"),
        ("role = master;", "role = stem;"),
        ("role = master;", "role = \"master\";"),
        ("encoding = wav_pcm16le;", "encoding = flac;"),
        ("encoding = wav_pcm16le;", "encoding = wav_f32le;"),
        ("type = tpdf; seed = 1;", "type = none; seed = 1;"),
        ("type = tpdf; seed = 1;", "type = tpdf; seed = -1;"),
        (
            "type = tpdf; seed = 1;",
            "type = tpdf; seed = 18446744073709551616;",
        ),
        ("type = tpdf; seed = 1;", "type = tpdf; seed = 1/2;"),
        ("unit = LUFS;", "unit = dBFS;"),
        ("min = -24; max = -22;", "min = -20; max = -22;"),
        ("min = -24; max = -22;", ""),
        ("min = -24;", "min = -24dB;"),
        ("schema = &production_schema;", "schema = &missing;"),
        ("kind = descriptor;", "kind = blob;"),
        (
            "path = \"production.schema.json\";",
            "path = \"../production.schema.json\";",
        ),
        (
            "schema = &production_schema;",
            "schema = &production_schema:out;",
        ),
        (
            "output = &ducked_bass:out;",
            "output = &nested.ducked_bass:out;",
        ),
        ("output = &ducked_bass:out;", "output = &ducked_bass;"),
    ] {
        assert!(EXAMPLE.contains(old));
        assert!(
            prepared(&EXAMPLE.replacen(old, new, 1)).is_err(),
            "accepted {new}"
        );
    }
    let giant = format!("min = -{};", "9".repeat(400));
    assert!(prepared(&EXAMPLE.replacen("min = -24;", &giant, 1)).is_err());
}

#[test]
fn descriptor_requires_recognized_exact_bytes_and_matching_pin() {
    let document = parse(EXAMPLE).unwrap();
    assert!(prepare_document(&document, &BTreeMap::new()).is_err());
    let mut bad_assets = assets();
    bad_assets
        .get_mut("production.schema.json")
        .unwrap()
        .push(b'\n');
    assert!(prepare_document(&document, &bad_assets).is_err());
    let alternate = b"{}".to_vec();
    let original_hash = format!("sha256:{:x}", Sha256::digest(SCHEMA_BYTES));
    let alternate_hash = format!("sha256:{:x}", Sha256::digest(&alternate));
    let document = parse(&EXAMPLE.replace(&original_hash, &alternate_hash)).unwrap();
    bad_assets.insert("production.schema.json".into(), alternate);
    assert!(prepare_document(&document, &bad_assets).is_err());
}

#[test]
fn plan_boundary_rejects_dangling_input_and_mismatched_master_ports() {
    let (document, settings) = prepared(EXAMPLE).unwrap();
    let plan = maac::compile(&document).unwrap();
    let mut settings = settings;
    settings.as_mut().unwrap().execution_identity = Some(
        maac::production_identity::execution_identity(&parse(EXAMPLE).unwrap(), &plan).unwrap(),
    );
    let original_settings = settings.clone().unwrap();
    for (node, port) in [
        ("missing", "out"),
        ("bass", "events"),
        ("ducked_bass", "sidechain"),
    ] {
        let mut settings = settings.clone().unwrap();
        let output = &mut settings
            .deliveries
            .get_mut("release_cd")
            .unwrap()
            .targets
            .get_mut("processed_bass")
            .unwrap()
            .output;
        output.node = node.into();
        output.port = port.into();
        let authored = EXAMPLE.replacen(
            "output = &ducked_bass:out;",
            &format!("output = &{node}:{port};"),
            1,
        );
        settings.execution_identity = Some(
            maac::production_identity::execution_identity(&parse(&authored).unwrap(), &plan)
                .unwrap(),
        );
        let code = settings.validate(&plan).unwrap_err().code;
        assert_eq!(
            code,
            if node == "missing" {
                "E_REFERENCE"
            } else {
                "E_PORT_TYPE"
            }
        );
    }
    let mut settings = settings.unwrap();
    settings
        .deliveries
        .get_mut("release_cd")
        .unwrap()
        .targets
        .get_mut("master")
        .unwrap()
        .output
        .node = "bass_room".into();
    let authored = EXAMPLE.replacen(
        "role = master;\n            output = &master:out;",
        "role = master;\n            output = &bass_room:out;",
        1,
    );
    settings.execution_identity = Some(
        maac::production_identity::execution_identity(&parse(&authored).unwrap(), &plan).unwrap(),
    );
    assert_eq!(settings.validate(&plan).unwrap_err().code, "E_REFERENCE");
    let mut plan = plan;
    if let maac::plan::Processor::Compressor { channels, .. } = &mut plan
        .nodes
        .iter_mut()
        .find(|node| node.id == "ducked_bass")
        .unwrap()
        .processor
    {
        *channels = 6;
    }
    assert_eq!(
        original_settings.validate(&plan).unwrap_err().code,
        "E_PORT_TYPE"
    );
}

#[test]
fn strict_standalone_settings_reject_unknown_fields_and_noncanonical_limits() {
    let (_, settings) = prepared(EXAMPLE).unwrap();
    let json = serde_json::to_value(settings.unwrap()).unwrap();
    let roundtrip: ProductionSettings = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(roundtrip.deliveries.len(), 2);
    let mut cases = Vec::new();
    let mut value = json.clone();
    value["extra"] = true.into();
    cases.push(value);
    let mut value = json.clone();
    value["deliveries"]["release_cd"]["extra"] = true.into();
    cases.push(value);
    let mut value = json.clone();
    value["deliveries"]["release_cd"]["targets"]["master"]["limits"]["integrated_loudness"]
        ["min"] = "-48/2".into();
    cases.push(value);
    let mut value = json.clone();
    value["deliveries"]["release_cd"]["targets"]["master"]["limits"]["true_peak"]["unit"] =
        "dBFS".into();
    cases.push(value);
    let mut value = json.clone();
    value["deliveries"]["release_cd"]["rate"] = 88200.into();
    cases.push(value);
    let mut value = json.clone();
    value["deliveries"]["release_cd"]["targets"]["master"]["dither"]["extra"] = 1.into();
    cases.push(value);
    let mut value = json.clone();
    value["deliveries"]["archive"]["targets"]["master"]["dither"]["seed"] = 1.into();
    cases.push(value);
    let mut value = json.clone();
    value["schema_hash"] = "sha256:00".into();
    cases.push(value);
    let mut value = json.clone();
    value["extension_id"] = "../bad".into();
    cases.push(value);
    for value in cases {
        assert!(serde_json::from_value::<ProductionSettings>(value).is_err());
    }
}

#[test]
fn package_root_descriptor_is_loaded_even_for_nested_entry() {
    let mut bundle = SourceBundle::new("examples/production.maac", EXAMPLE);
    bundle.assets = assets();
    let resolved = bundle.resolve().unwrap();
    assert_eq!(resolved.assets["production.schema.json"], SCHEMA_BYTES);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("examples")).unwrap();
    std::fs::write(dir.path().join("examples/production.maac"), EXAMPLE).unwrap();
    std::fs::write(dir.path().join("production.schema.json"), SCHEMA_BYTES).unwrap();
    let loaded =
        maac::bundle_fs::load_bundle(std::path::Path::new("examples/production.maac"), dir.path())
            .unwrap();
    assert_eq!(loaded.assets["production.schema.json"], SCHEMA_BYTES);
    std::fs::remove_file(dir.path().join("production.schema.json")).unwrap();
    assert!(maac::bundle_fs::load_bundle(
        std::path::Path::new("examples/production.maac"),
        dir.path()
    )
    .is_err());
}

#[test]
fn duplicate_standalone_ids_and_bounded_maps_are_rejected() {
    let (_, settings) = prepared(EXAMPLE).unwrap();
    let settings = settings.unwrap();
    let delivery = serde_json::to_string(&settings.deliveries["release_cd"]).unwrap();
    let duplicate = format!("{{\"schema_hash\":\"{}\",\"extension_id\":\"production_deliveries\",\"deliveries\":{{\"same\":{delivery},\"same\":{delivery}}}}}", settings.schema_hash);
    assert!(serde_json::from_str::<ProductionSettings>(&duplicate).is_err());
    let target =
        serde_json::to_string(&settings.deliveries["release_cd"].targets["master"]).unwrap();
    let duplicate = format!("{{\"rate\":44100,\"resampler\":\"maac.src.kaiser/1\",\"targets\":{{\"same\":{target},\"same\":{target}}}}}");
    assert!(serde_json::from_str::<maac::production_data::Delivery>(&duplicate).is_err());
    let mut oversized = settings.clone();
    oversized.deliveries.clear();
    for index in 0..=maac::production_data::MAX_DELIVERIES {
        oversized.deliveries.insert(
            format!("delivery_{index}"),
            settings.deliveries["release_cd"].clone(),
        );
    }
    assert!(oversized.validate_structure().is_err());
    assert!(
        serde_json::from_value::<ProductionSettings>(serde_json::to_value(oversized).unwrap())
            .is_err()
    );
}

#[test]
fn complete_plan_requires_identity_and_rejects_stale_unselected_definitions() {
    let (document, settings) = prepared(EXAMPLE).unwrap();
    let plan = maac::compile(&document).unwrap();
    let mut settings = settings.unwrap();
    assert_eq!(settings.validate(&plan).unwrap_err().code, "E_HASH");
    settings.execution_identity = Some(
        maac::production_identity::execution_identity(&parse(EXAMPLE).unwrap(), &plan).unwrap(),
    );
    settings.validate(&plan).unwrap();
    let original = settings.clone();
    settings
        .deliveries
        .get_mut("archive")
        .unwrap()
        .targets
        .get_mut("master")
        .unwrap()
        .encoding = maac::production_data::Encoding::Pcm24;
    assert_eq!(settings.validate(&plan).unwrap_err().code, "E_HASH");
    let mut settings = original.clone();
    settings
        .deliveries
        .get_mut("release_cd")
        .unwrap()
        .targets
        .get_mut("master")
        .unwrap()
        .limits
        .as_mut()
        .unwrap()
        .integrated_loudness
        .as_mut()
        .unwrap()
        .min = Some(maac::Rational::from_integer((-25).into()));
    assert_eq!(settings.validate(&plan).unwrap_err().code, "E_HASH");
    let mut settings = original;
    settings.execution_identity.as_mut().unwrap().execution_hash =
        format!("sha256:{}", "0".repeat(64));
    assert_eq!(settings.validate(&plan).unwrap_err().code, "E_HASH");
}

#[test]
fn descriptor_unknown_fields_and_traversal_are_rejected() {
    for source in [
        EXAMPLE.replacen(
            "kind = descriptor;",
            "kind = descriptor; executable = true;",
            1,
        ),
        EXAMPLE.replacen(
            "path = \"production.schema.json\";",
            "path = \"../../production.schema.json\";",
            1,
        ),
        EXAMPLE.replacen(
            "path = \"production.schema.json\";",
            "path = \"https://example.com/schema.json\";",
            1,
        ),
    ] {
        assert!(prepared(&source).is_err());
    }
    let source = EXAMPLE.replacen(
        "path = \"production.schema.json\";",
        "path = \"../production.schema.json\";",
        1,
    );
    let mut bundle = SourceBundle::new("nested/main.maac", source);
    bundle.assets = assets();
    assert!(bundle.resolve().is_err());
}

#[cfg(unix)]
#[test]
fn descriptor_symlink_cannot_escape_package_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("main.maac"), EXAMPLE).unwrap();
    std::fs::write(outside.path().join("schema.json"), SCHEMA_BYTES).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("schema.json"),
        root.path().join("production.schema.json"),
    )
    .unwrap();
    assert!(maac::bundle_fs::load_bundle(std::path::Path::new("main.maac"), root.path()).is_err());
}

#[test]
fn optional_source_labels_survive_the_generic_object_contract() {
    let source = EXAMPLE
        .replacen(
            "kind = descriptor;",
            "kind = descriptor; label = \"Delivery schema\";",
            1,
        )
        .replacen(
            "render_affecting = true;",
            "render_affecting = true; label = \"Release formats\";",
            1,
        );
    let (document, settings) = prepared(&source).unwrap();
    let plan = maac::compile(&document).unwrap();
    let mut settings = settings.unwrap();
    settings.execution_identity = Some(
        maac::production_identity::execution_identity(&parse(&source).unwrap(), &plan).unwrap(),
    );
    settings.validate(&plan).unwrap();
}

#[test]
fn caller_limits_cover_rationals_ids_metadata_and_plan_aggregates() {
    let source = EXAMPLE.replacen(
        "min = -24; max = -22;",
        "min = -1/340282366920938463463374607431768211457; max = 0;",
        1,
    );
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle.assets = assets();
    let limits = maac::plan::PlanLimits {
        max_rational_bits: 128,
        ..Default::default()
    };
    assert!(maac::compile_bundle_with_limits(&bundle, &limits).is_err());
    let plan = maac::compile_bundle(&bundle).unwrap();
    assert_eq!(
        plan.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
    let settings = plan.production.as_ref().unwrap();
    let defaults = maac::plan::PlanLimits::default();
    let usage = settings.resource_usage(&defaults).unwrap();
    for limits in [
        maac::plan::PlanLimits {
            max_id_bytes: 8,
            ..defaults
        },
        maac::plan::PlanLimits {
            max_string_bytes: 16,
            ..defaults
        },
        maac::plan::PlanLimits {
            max_json_bytes: settings
                .execution_identity
                .as_ref()
                .unwrap()
                .normalized_source_json
                .len()
                - 1,
            ..defaults
        },
        maac::plan::PlanLimits {
            max_total_string_bytes: usage.string_bytes - 1,
            ..defaults
        },
        maac::plan::PlanLimits {
            max_objects: usage.objects - 1,
            ..defaults
        },
    ] {
        assert!(settings.resource_usage(&limits).is_err());
    }
    // Each side alone fits; the combined aggregate must still be charged.
    let limits = maac::plan::PlanLimits {
        max_total_string_bytes: usage.string_bytes,
        ..defaults
    };
    settings.resource_usage(&limits).unwrap();
    assert_eq!(
        plan.validate_with_limits(&limits).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
}
