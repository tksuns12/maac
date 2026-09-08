use std::process::{Command, Output};

use tempfile::tempdir;

const ACOUSTIC: &str = "std/acoustic/1.0.0";

fn invoke(args: &[&str]) -> Output {
    let directory = tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(directory.path())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn every_selected_usage_compiles_with_only_its_entry_file() {
    let catalog = json_success(&["--json", "instruments", "--library", ACOUSTIC]);
    for instrument in catalog["catalog"]["instruments"].as_array().unwrap() {
        let directory = tempdir().unwrap();
        std::fs::write(
            directory.path().join("demo.maac"),
            instrument["usage"].as_str().unwrap(),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_maac"))
            .current_dir(directory.path())
            .args(["--json", "compile", "demo.maac", "-o", "demo.plan.json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let bytes = std::fs::read(directory.path().join("demo.plan.json")).unwrap();
        let plan = maac::load_plan(&bytes).unwrap();
        assert_eq!(plan.events.len(), 1);
    }
}

fn json_success(args: &[&str]) -> serde_json::Value {
    let output = invoke(args);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn exact_library_selection_and_listing_work_outside_the_repository() {
    let libraries = json_success(&["--json", "instruments", "--libraries"]);
    assert_eq!(libraries["command"], "instruments");
    assert_eq!(libraries["input"], "@builtin");
    let entries = libraries["libraries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| entry["library"] == ACOUSTIC));
    assert!(entries
        .iter()
        .any(|entry| entry["library"] == "std/basic/1.0.0"));
    assert!(libraries.get("catalog").is_none());
    assert!(libraries.get("instrument").is_none());

    let catalog = json_success(&["--json", "instruments", "--library", ACOUSTIC]);
    assert_eq!(catalog["library"], ACOUSTIC);
    assert_eq!(catalog["catalog"]["library"], ACOUSTIC);
    assert_eq!(
        catalog["catalog"]["instruments"].as_array().unwrap().len(),
        3
    );
    let detail = json_success(&[
        "instruments",
        "nylon_guitar",
        "--library",
        ACOUSTIC,
        "--json",
    ]);
    assert_eq!(detail["library"], ACOUSTIC);
    assert_eq!(detail["instrument"]["name"], "nylon_guitar");
    assert!(detail["instrument"]["usage"]
        .as_str()
        .unwrap()
        .contains(ACOUSTIC));
    assert!(detail["instrument"]["controls"]["bend_ratio"].is_object());

    let human = invoke(&["instruments", "--libraries"]);
    assert!(human.status.success(), "{human:?}");
    assert!(String::from_utf8(human.stdout).unwrap().contains(ACOUSTIC));
    let human = invoke(&["instruments", "--library", ACOUSTIC, "nylon_guitar"]);
    assert!(human.status.success(), "{human:?}");
    assert!(String::from_utf8(human.stdout).unwrap().contains(ACOUSTIC));
}

#[test]
fn default_catalog_payload_stays_basic_and_new_fields_are_omitted() {
    let default = json_success(&["--json", "instruments"]);
    let explicit = json_success(&["--json", "instruments", "--library", "std/basic/1.0.0"]);
    assert_eq!(default["catalog"], explicit["catalog"]);
    assert_eq!(
        default["catalog"]["instruments"].as_array().unwrap().len(),
        24
    );
    assert!(default.get("library").is_none());
    assert!(default.get("libraries").is_none());
    let detail = json_success(&["--json", "instruments", "nylon_guitar"]);
    assert!(detail.get("library").is_none());
    assert!(detail.get("libraries").is_none());
    assert!(detail["instrument"]["usage"]
        .as_str()
        .unwrap()
        .contains("std/basic/1.0.0"));
}

#[test]
fn unknown_ids_names_and_conflicting_selectors_fail_explicitly() {
    for args in [
        vec!["--json", "instruments", "--library", "std/acoustic/9.0.0"],
        vec!["--json", "instruments", "--library", "std/acoustic/latest"],
        vec![
            "--json",
            "instruments",
            "--library",
            ACOUSTIC,
            "mellow_piano",
        ],
    ] {
        let output = invoke(&args);
        assert!(!output.status.success());
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["code"], "E_REFERENCE", "{json}");
        let message = json["message"].as_str().unwrap();
        assert!(message.contains(args[3]), "missing library context: {json}");
        if let Some(name) = args.get(4) {
            assert!(message.contains(name), "missing instrument context: {json}");
        }
    }
    for args in [
        vec!["--json", "instruments", "--libraries", "nylon_guitar"],
        vec![
            "--json",
            "instruments",
            "--libraries",
            "--library",
            ACOUSTIC,
        ],
    ] {
        let output = invoke(&args);
        assert!(!output.status.success());
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["code"], "E_USAGE", "{json}");
    }
}
