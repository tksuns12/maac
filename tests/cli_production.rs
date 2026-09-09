use std::process::Command;
use std::{fs, path::Path};

use maac::bundle::sha256_digest;
use maac::production_data::SCHEMA_BYTES;
use serde_json::Value;
use tempfile::tempdir;

fn fixture(directory: &Path, fail_limit: bool) -> std::path::PathBuf {
    fs::write(directory.join("production.schema.json"), SCHEMA_BYTES).unwrap();
    let limit = if fail_limit {
        "limits={integrated_loudness={unit=LUFS;min=-20;};};"
    } else {
        ""
    };
    let source = format!(
        r#"maac 1;
project p {{score=[0q,1/100q];rate=48000Hz;tempo=&clock;meter=&metre;output=&master:out;tail=0s;requires=["maac.production/1"];}}
tempo clock {{points=[(0q,120bpm,step)];}}
meter metre {{points=[(0q,4,4)];}}
asset schema {{kind=descriptor;path="production.schema.json";hash="{}";}}
node sine {{type="core.sine/1";params={{attack=0s;release=0s;level=0.2;}};}}
node master {{type="core.gain/1";config={{channels=1;}};params={{gain=1;}};}}
connect main {{from=&sine:out;to=&master:in;}}
pattern phrase {{length=1/100q;note n {{at=0q;dur=1/100q;pitch=A4;velocity=1;}}}}
track melody {{target=&sine:events;}}
place notes {{pattern=&phrase;track=&melody;at=0q;}}
extension deliveries {{namespace="maac.production/1";schema=&schema;render_affecting=true;data={{deliveries={{release={{rate=48000Hz;resampler="maac.src.kaiser/1";targets={{
master={{role=master;output=&master:out;encoding=wav_f32le;dither={{type=none;}};{limit}}};
stem={{role=stem;output=&sine:out;encoding=wav_pcm16le;dither={{type=tpdf;seed=1;}};}};
}};}};}};}};}}
"#,
        sha256_digest(SCHEMA_BYTES)
    );
    let path = directory.join("main.maac");
    fs::write(&path, source).unwrap();
    path
}

fn invoke(arguments: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(arguments)
        .output()
        .unwrap()
}

fn result(output: &std::process::Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn delivery_command_is_exposed_in_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["deliver", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8(output.stdout).unwrap();
    for option in [
        "--delivery",
        "--output-dir",
        "--target",
        "--force",
        "--project-root",
        "--profile",
    ] {
        assert!(help.contains(option), "missing {option}");
    }
}

#[test]
fn source_and_retained_plan_deliver_identical_selected_artifact_bytes() {
    let directory = tempdir().unwrap();
    let source = fixture(directory.path(), false);
    let plan = directory.path().join("retained.json");
    let first_dir = directory.path().join("source-output");
    let retained_dir = directory.path().join("plan-output");
    let compile = invoke(&[
        "--json".as_ref(),
        "compile".as_ref(),
        source.as_os_str(),
        "--output".as_ref(),
        plan.as_os_str(),
    ]);
    assert!(compile.status.success(), "{:?}", result(&compile));
    for (input, output) in [(&source, &first_dir), (&plan, &retained_dir)] {
        let delivery = invoke(&[
            "--json".as_ref(),
            "deliver".as_ref(),
            input.as_os_str(),
            "--delivery".as_ref(),
            "release".as_ref(),
            "--output-dir".as_ref(),
            output.as_os_str(),
            "--target".as_ref(),
            "stem".as_ref(),
        ]);
        let json = result(&delivery);
        assert!(delivery.status.success(), "{json}");
        assert_eq!(json["command"], "deliver");
        assert_eq!(json["ok"], true);
        assert_eq!(json["frames"], 240);
        assert_eq!(json["delivery"]["targets"].as_object().unwrap().len(), 1);
        assert!(!output.join("release.master.wav").exists());
    }
    assert_eq!(
        fs::read(first_dir.join("release.stem.wav")).unwrap(),
        fs::read(retained_dir.join("release.stem.wav")).unwrap()
    );
}

#[test]
fn failed_checks_return_nonzero_json_and_keep_audio_with_overwrite_protection() {
    let directory = tempdir().unwrap();
    let source = fixture(directory.path(), true);
    let output = directory.path().join("out");
    let arguments = [
        "--json".as_ref(),
        "deliver".as_ref(),
        source.as_os_str(),
        "--delivery".as_ref(),
        "release".as_ref(),
        "--output-dir".as_ref(),
        output.as_os_str(),
    ];
    let delivered = invoke(&arguments);
    let json = result(&delivered);
    assert!(!delivered.status.success());
    assert_eq!(json["ok"], false);
    assert_eq!(json["delivery"]["artifact_status"], "complete");
    assert_eq!(json["delivery"]["manifest_status"], "complete");
    assert_eq!(json["delivery"]["check_status"], "fail");
    assert!(output.join("release.manifest.json").is_file());
    let original = fs::read(output.join("release.master.wav")).unwrap();
    let duplicate = invoke(&arguments);
    assert!(!duplicate.status.success());
    assert_eq!(result(&duplicate)["code"], "E_OUTPUT_EXISTS");
    assert_eq!(
        fs::read(output.join("release.master.wav")).unwrap(),
        original
    );
    let mut forced = arguments.to_vec();
    forced.push("--force".as_ref());
    let forced = invoke(&forced);
    let json = result(&forced);
    assert!(!forced.status.success());
    assert_eq!(json["delivery"]["check_status"], "fail");
    assert_eq!(
        fs::read(output.join("release.master.wav")).unwrap(),
        original
    );
}
