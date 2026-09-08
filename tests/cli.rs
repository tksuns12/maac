use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use hound::{SampleFormat, WavReader};
use serde_json::Value;
use tempfile::tempdir;

const ONE_NOTE_SOURCE: &str = r#"maac 1;
project p {
  score = [0q, 1q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &sum:out;
  tail = 0s;
}
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
pattern riff {
  length = 1q;
  note n { at = 0q; dur = 1/2q; pitch = C4; velocity = 1; }
}
track t { target = &sine:events; }
place main { pattern = &riff; track = &t; at = 0q; }
node sine {
  type = "core.sine/1";
  config = { voices = 4; };
  params = { attack = 0s; release = 0s; level = 0.5; };
}
node sum {
  type = "core.sum/1";
  config = { channels = 1; };
}
connect sine_sum { from = &sine:out; to = &sum:in; }
"#;

fn invoke(args: &[&Path]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_maac"));
    for argument in args {
        command.arg(argument);
    }
    command.output().expect("maac process starts")
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("command emits JSON")
}

#[test]
fn build_compile_and_render_commands_share_the_same_one_note_pipeline() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("one.maac");
    let plan = directory.path().join("one.performance.json");
    let build_wav = directory.path().join("build.wav");
    let render_wav = directory.path().join("render.wav");
    let pcm_wav = directory.path().join("render.pcm16.wav");
    fs::write(&source, ONE_NOTE_SOURCE).unwrap();

    let source_arg = source.as_path();
    let check = invoke(&[Path::new("--json"), Path::new("check"), source_arg]);
    assert!(check.status.success(), "check failed: {:?}", check);
    let check_json = json_stdout(&check);
    assert_eq!(check_json["ok"], true);
    assert_eq!(check_json["command"], "check");
    assert_eq!(check_json["notes"], 1);
    assert_eq!(check_json["frames"], 24_000);

    let compile = invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        source_arg,
        Path::new("-o"),
        plan.as_path(),
    ]);
    assert!(compile.status.success(), "compile failed: {:?}", compile);
    let compile_json = json_stdout(&compile);
    assert_eq!(compile_json["ok"], true);
    assert!(plan.is_file());

    let render = invoke(&[
        Path::new("--json"),
        Path::new("render"),
        plan.as_path(),
        Path::new("-o"),
        render_wav.as_path(),
    ]);
    assert!(render.status.success(), "render failed: {:?}", render);
    let render_json = json_stdout(&render);
    assert_eq!(render_json["ok"], true);
    assert_eq!(render_json["frames"], 24_000);

    let pcm_render = invoke(&[
        Path::new("--json"),
        Path::new("render"),
        plan.as_path(),
        Path::new("-o"),
        pcm_wav.as_path(),
        Path::new("--format"),
        Path::new("pcm16"),
    ]);
    assert!(
        pcm_render.status.success(),
        "PCM16 render failed: {:?}",
        pcm_render
    );
    let pcm_json = json_stdout(&pcm_render);
    assert_eq!(pcm_json["format"], "pcm16");

    let build = invoke(&[
        Path::new("--json"),
        Path::new("build"),
        source_arg,
        Path::new("-o"),
        build_wav.as_path(),
    ]);
    assert!(build.status.success(), "build failed: {:?}", build);
    let build_json = json_stdout(&build);
    assert_eq!(build_json["ok"], true);
    assert_eq!(build_json["frames"], 24_000);

    for path in [&build_wav, &render_wav] {
        let mut reader = WavReader::open(path).unwrap();
        assert_eq!(reader.duration(), 24_000);
        assert!(reader
            .samples::<f32>()
            .any(|sample| sample.unwrap().abs() > 0.0));
    }
    let reader = WavReader::open(&pcm_wav).unwrap();
    assert_eq!(reader.spec().sample_format, SampleFormat::Int);
    assert_eq!(reader.duration(), 24_000);
}

#[test]
fn json_errors_preserve_stable_codes_and_overwrite_protection() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("one.maac");
    let output = directory.path().join("out.wav");
    fs::write(&source, ONE_NOTE_SOURCE).unwrap();

    let first = invoke(&[
        Path::new("build"),
        source.as_path(),
        Path::new("-o"),
        output.as_path(),
    ]);
    assert!(first.status.success(), "initial build failed: {:?}", first);
    let original = fs::read(&output).unwrap();

    let second = invoke(&[
        Path::new("--json"),
        Path::new("build"),
        source.as_path(),
        Path::new("-o"),
        output.as_path(),
    ]);
    assert!(!second.status.success());
    let error = json_stdout(&second);
    assert_eq!(error["ok"], false);
    assert_eq!(error["code"], "E_OUTPUT_EXISTS");
    assert_eq!(fs::read(&output).unwrap(), original);

    fs::write(&source, "maac 1; project {").unwrap();
    let malformed = invoke(&[Path::new("--json"), Path::new("check"), source.as_path()]);
    assert!(!malformed.status.success());
    let malformed_json = json_stdout(&malformed);
    assert_eq!(malformed_json["ok"], false);
    assert_eq!(malformed_json["code"], "E_SYNTAX");
}

#[test]
fn help_is_an_informational_success_even_through_try_parse() {
    let output = invoke(&[Path::new("--help")]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("MaaC (Music as a Code)"));
    assert!(output.stderr.is_empty());
}
