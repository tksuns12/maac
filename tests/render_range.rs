use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hound::WavReader;
use maac::cli::{execute_artifact_with_range, Command as CliCommand, FormatArg, ProfileArg};
use maac::export::FrameRange;
use serde_json::Value;
use tempfile::tempdir;

const OVERLOAD_SOURCE: &str = r#"maac 1;
project overload {
  score = [0q, 1q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &sine:out;
  tail = 0s;
}
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
pattern phrase {
  length = 1q;
  note impulse { at = 0q; dur = 1/2q; pitch = A4; velocity = 1; }
}
track melody { target = &sine:events; }
place notes { pattern = &phrase; track = &melody; at = 0q; }
node sine {
  type = "core.sine/1";
  params = { attack = 0s; release = 0s; level = 2; };
}
"#;

fn run(args: &[&Path]) -> Output {
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

fn render_plan(directory: &Path, plan: &Path, output_name: &str, extra: &[&Path]) -> Value {
    let output = directory.join(output_name);
    let mut args = vec![
        Path::new("--json"),
        Path::new("render"),
        plan,
        Path::new("-o"),
        output.as_path(),
    ];
    args.extend_from_slice(extra);
    let result = run(&args);
    assert!(result.status.success(), "render failed: {:?}", result);
    json_stdout(&result)
}

fn float_samples(path: &Path) -> Vec<f32> {
    let mut reader = WavReader::open(path).expect("WAV opens");
    reader.samples::<f32>().map(Result::unwrap).collect()
}

fn float_sample_bits(path: &Path) -> Vec<u32> {
    float_samples(path).into_iter().map(f32::to_bits).collect()
}

fn pcm16_samples(path: &Path) -> Vec<i16> {
    let mut reader = WavReader::open(path).expect("WAV opens");
    reader.samples::<i16>().map(Result::unwrap).collect()
}

#[test]
fn stateful_delay_range_is_an_exact_reset_origin_slice_in_both_formats() {
    let directory = tempdir().unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/core-delay.maac");
    let plan = directory.path().join("delay.json");
    let compile = run(&[
        Path::new("--json"),
        Path::new("compile"),
        source.as_path(),
        Path::new("-o"),
        plan.as_path(),
    ]);
    assert!(compile.status.success(), "compile failed: {:?}", compile);

    let full = render_plan(directory.path(), &plan, "full.wav", &[]);
    assert_eq!(full["frames"], 38_400);

    let start = Path::new("--start-frame");
    let start_value = Path::new("2400");
    let end = Path::new("--end-frame");
    let end_value = Path::new("7200");
    let crop = render_plan(
        directory.path(),
        &plan,
        "crop.wav",
        &[start, start_value, end, end_value],
    );
    assert_eq!(crop["frames"], 4_800);
    assert_eq!(crop["start_frame"], 2_400);
    assert_eq!(crop["end_frame"], 7_200);

    let full_samples = float_sample_bits(&directory.path().join("full.wav"));
    let crop_samples = float_sample_bits(&directory.path().join("crop.wav"));
    assert!(
        crop_samples.iter().any(|bits| bits & 0x7fff_ffff != 0),
        "delay excerpt should contain rendered signal"
    );
    assert_eq!(crop_samples, full_samples[2_400..7_200]);

    let full_pcm = render_plan(
        directory.path(),
        &plan,
        "full.pcm16.wav",
        &[Path::new("--format"), Path::new("pcm16")],
    );
    assert_eq!(full_pcm["frames"], 38_400);
    let crop_pcm = render_plan(
        directory.path(),
        &plan,
        "crop.pcm16.wav",
        &[
            Path::new("--format"),
            Path::new("pcm16"),
            start,
            start_value,
            end,
            end_value,
        ],
    );
    assert_eq!(crop_pcm["frames"], 4_800);
    let full_pcm_samples = pcm16_samples(&directory.path().join("full.pcm16.wav"));
    let crop_pcm_samples = pcm16_samples(&directory.path().join("crop.pcm16.wav"));
    assert_eq!(crop_pcm_samples, full_pcm_samples[2_400..7_200]);
}

#[test]
fn empty_and_full_ranges_report_metadata_without_changing_default_result_fields() {
    let directory = tempdir().unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/tempo-ramps.maac");
    let plan = directory.path().join("tempo.json");
    let compile = run(&[
        Path::new("--json"),
        Path::new("compile"),
        source.as_path(),
        Path::new("-o"),
        plan.as_path(),
    ]);
    assert!(compile.status.success(), "compile failed: {:?}", compile);

    let default_result = render_plan(directory.path(), &plan, "default.wav", &[]);
    assert_eq!(default_result["frames"], 208_158);
    assert!(default_result.get("start_frame").is_none());
    assert!(default_result.get("end_frame").is_none());

    let full_result = render_plan(
        directory.path(),
        &plan,
        "explicit-full.wav",
        &[
            Path::new("--start-frame"),
            Path::new("0"),
            Path::new("--end-frame"),
            Path::new("208158"),
        ],
    );
    assert_eq!(full_result["frames"], 208_158);
    assert_eq!(full_result["start_frame"], 0);
    assert_eq!(full_result["end_frame"], 208_158);
    assert_eq!(
        float_sample_bits(&directory.path().join("explicit-full.wav")),
        float_sample_bits(&directory.path().join("default.wav"))
    );

    let empty_result = render_plan(
        directory.path(),
        &plan,
        "empty.wav",
        &[
            Path::new("--start-frame"),
            Path::new("100"),
            Path::new("--end-frame"),
            Path::new("100"),
        ],
    );
    assert_eq!(empty_result["frames"], 0);
    assert_eq!(empty_result["start_frame"], 100);
    assert_eq!(empty_result["end_frame"], 100);
    assert!(float_samples(&directory.path().join("empty.wav")).is_empty());
}

#[test]
fn invalid_range_and_missing_pair_preserve_destinations() {
    let directory = tempdir().unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/core-delay.maac");
    let plan = directory.path().join("delay.json");
    let compile = run(&[
        Path::new("--json"),
        Path::new("compile"),
        source.as_path(),
        Path::new("-o"),
        plan.as_path(),
    ]);
    assert!(compile.status.success(), "compile failed: {:?}", compile);

    let destination = directory.path().join("existing.wav");
    fs::write(&destination, b"keep this destination").unwrap();
    let invalid = run(&[
        Path::new("--json"),
        Path::new("render"),
        plan.as_path(),
        Path::new("-o"),
        destination.as_path(),
        Path::new("--force"),
        Path::new("--start-frame"),
        Path::new("7200"),
        Path::new("--end-frame"),
        Path::new("2400"),
    ]);
    assert!(!invalid.status.success());
    let error = json_stdout(&invalid);
    assert_eq!(error["ok"], false);
    assert_eq!(error["code"], "E_RANGE");
    assert_eq!(fs::read(&destination).unwrap(), b"keep this destination");

    let missing_pair = run(&[
        Path::new("--json"),
        Path::new("render"),
        plan.as_path(),
        Path::new("-o"),
        destination.as_path(),
        Path::new("--force"),
        Path::new("--start-frame"),
        Path::new("100"),
    ]);
    assert!(!missing_pair.status.success());
    let error = json_stdout(&missing_pair);
    assert_eq!(error["ok"], false);
    assert_eq!(error["code"], "E_USAGE");
    assert_eq!(fs::read(&destination).unwrap(), b"keep this destination");

    let missing_end_pair = run(&[
        Path::new("--json"),
        Path::new("render"),
        plan.as_path(),
        Path::new("-o"),
        destination.as_path(),
        Path::new("--force"),
        Path::new("--end-frame"),
        Path::new("100"),
    ]);
    assert!(!missing_end_pair.status.success());
    let error = json_stdout(&missing_end_pair);
    assert_eq!(error["ok"], false);
    assert_eq!(error["code"], "E_USAGE");
    assert_eq!(fs::read(&destination).unwrap(), b"keep this destination");

    let out_of_bounds = run(&[
        Path::new("--json"),
        Path::new("render"),
        plan.as_path(),
        Path::new("-o"),
        destination.as_path(),
        Path::new("--force"),
        Path::new("--start-frame"),
        Path::new("0"),
        Path::new("--end-frame"),
        Path::new("38401"),
    ]);
    assert!(!out_of_bounds.status.success());
    let error = json_stdout(&out_of_bounds);
    assert_eq!(error["ok"], false);
    assert_eq!(error["code"], "E_RANGE");
    assert_eq!(fs::read(&destination).unwrap(), b"keep this destination");

    let overload_source = directory.path().join("overload.maac");
    let overload_plan = directory.path().join("overload.json");
    fs::write(&overload_source, OVERLOAD_SOURCE).unwrap();
    let compile = run(&[
        Path::new("--json"),
        Path::new("compile"),
        overload_source.as_path(),
        Path::new("-o"),
        overload_plan.as_path(),
    ]);
    assert!(compile.status.success(), "compile failed: {:?}", compile);
    let overload_destination = directory.path().join("overload.wav");
    fs::write(&overload_destination, b"preserve on crop validation error").unwrap();
    let overload = run(&[
        Path::new("--json"),
        Path::new("render"),
        overload_plan.as_path(),
        Path::new("-o"),
        overload_destination.as_path(),
        Path::new("--force"),
        Path::new("--format"),
        Path::new("pcm16"),
        Path::new("--start-frame"),
        Path::new("0"),
        Path::new("--end-frame"),
        Path::new("1"),
    ]);
    assert!(!overload.status.success());
    let error = json_stdout(&overload);
    assert_eq!(error["code"], "E_PCM16_RANGE");
    assert_eq!(
        fs::read(&overload_destination).unwrap(),
        b"preserve on crop validation error"
    );

    let before_crop_destination = directory.path().join("overload-before-crop.wav");
    fs::write(
        &before_crop_destination,
        b"preserve on before-crop validation error",
    )
    .unwrap();
    let before_crop = run(&[
        Path::new("--json"),
        Path::new("render"),
        overload_plan.as_path(),
        Path::new("-o"),
        before_crop_destination.as_path(),
        Path::new("--force"),
        Path::new("--format"),
        Path::new("pcm16"),
        Path::new("--start-frame"),
        Path::new("20000"),
        Path::new("--end-frame"),
        Path::new("20001"),
    ]);
    assert!(!before_crop.status.success());
    let error = json_stdout(&before_crop);
    assert_eq!(error["code"], "E_PCM16_RANGE");
    assert_eq!(
        fs::read(&before_crop_destination).unwrap(),
        b"preserve on before-crop validation error"
    );
}

#[test]
fn newest_artifact_audio_and_modulation_range_matches_full_payload() {
    let directory = tempdir().unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/core-modulation.maac");
    let plan = directory.path().join("modulation.json");
    let compile = run(&[
        Path::new("--json"),
        Path::new("compile"),
        source.as_path(),
        Path::new("--project-root"),
        Path::new("."),
        Path::new("-o"),
        plan.as_path(),
    ]);
    assert!(compile.status.success(), "compile failed: {:?}", compile);
    let plan_json: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    assert_eq!(plan_json["version"], 7);

    let full = render_plan(directory.path(), &plan, "mod-full.wav", &[]);
    assert_eq!(full["frames"], 109_688);
    let crop = render_plan(
        directory.path(),
        &plan,
        "mod-crop.wav",
        &[
            Path::new("--start-frame"),
            Path::new("48000"),
            Path::new("--end-frame"),
            Path::new("72000"),
        ],
    );
    assert_eq!(crop["frames"], 24_000);
    let full_samples = float_sample_bits(&directory.path().join("mod-full.wav"));
    let crop_samples = float_sample_bits(&directory.path().join("mod-crop.wav"));
    assert!(
        crop_samples.iter().any(|bits| bits & 0x7fff_ffff != 0),
        "modulation excerpt should contain rendered signal"
    );
    assert_eq!(crop_samples, full_samples[96_000..144_000]);
}

#[test]
fn range_helper_rejects_non_render_commands_before_publication() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("must-not-exist.wav");
    let command = CliCommand::Build {
        input: Some(PathBuf::from("examples/core-delay.maac")),
        output: output.clone(),
        format: FormatArg::Float32,
        force: false,
        project_root: None,
        profile: ProfileArg::Default,
    };
    let error = execute_artifact_with_range(&command, Some(FrameRange::new(0, 1))).unwrap_err();
    assert_eq!(error.code, "E_USAGE");
    assert!(!output.exists());
}

#[test]
fn render_help_documents_the_paired_range_flags() {
    let help = run(&[Path::new("render"), Path::new("--help")]);
    assert!(help.status.success(), "help failed: {:?}", help);
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("--start-frame"));
    assert!(stdout.contains("--end-frame"));
}
