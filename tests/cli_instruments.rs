use std::process::{Command, Output};
use tempfile::tempdir;

fn invoke(args: &[&str]) -> Output {
    let directory = tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(directory.path())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn lists_and_describes_embedded_instruments_outside_the_repository() {
    let list = invoke(&["instruments"]);
    assert!(list.status.success(), "{list:?}");
    let human = String::from_utf8(list.stdout).unwrap();
    assert!(human.contains("keys:"));
    assert!(human.contains("mellow_piano"));
    let detail = invoke(&["instruments", "mellow_piano"]);
    assert!(detail.status.success(), "{detail:?}");
    let human = String::from_utf8(detail.stdout).unwrap();
    for required in [
        "level",
        "brightness",
        "release",
        "pan",
        "maac 1;",
        "std/basic/1.0.0",
        "mellow_piano",
    ] {
        assert!(human.contains(required), "missing {required}");
    }
}

#[test]
fn json_list_and_detail_have_exact_control_metadata_and_unknown_names_fail() {
    let list = invoke(&["--json", "instruments"]);
    assert!(list.status.success(), "{list:?}");
    let json: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(json["command"], "instruments");
    assert_eq!(json["catalog"]["instruments"].as_array().unwrap().len(), 24);
    let detail = invoke(&["instruments", "mellow_piano", "--json"]);
    assert!(detail.status.success(), "{detail:?}");
    let json: serde_json::Value = serde_json::from_slice(&detail.stdout).unwrap();
    assert_eq!(json["instrument"]["name"], "mellow_piano");
    assert_eq!(json["instrument"]["controls"]["release"]["unit"], "seconds");
    assert!(json["instrument"]["controls"]["release"]["default"].is_string());
    assert!(json["instrument"]["controls"]["brightness"]["min_open"]
        .as_bool()
        .unwrap());
    assert!(json.get("catalog").is_none());
    let unknown = invoke(&["--json", "instruments", "missing"]);
    assert!(!unknown.status.success());
    let json: serde_json::Value = serde_json::from_slice(&unknown.stdout).unwrap();
    assert_eq!(json["code"], "E_REFERENCE");
    assert!(json["message"].as_str().unwrap().contains("missing"));
    assert!(unknown.stderr.is_empty());
}
