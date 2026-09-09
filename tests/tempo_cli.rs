use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::tempdir;

fn source(shape: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }}
tempo clock {{ points = [(0q, 60bpm, {shape}), (1q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node s {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 1/4; }}; }}
track t {{ target = &s:events; }}
pattern pat {{ length = 1q; note n {{ at = 0q; dur = 1q; pitch = A4; }} }}
place x {{ pattern = &pat; track = &t; at = 0q; }}
curve level {{ clock = seconds; points = [(0s, 1/4, step), (1/10s, 1/2, step)]; }}
automation a {{ target = &s.params.level; curve = &level; at = 1/2q; }}"#
    )
}
fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}
fn success(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn cli_ramps_compile_build_and_source_free_render() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("main.maac");
    let plan = dir.path().join("plan.json");
    fs::write(&input, source("linear")).unwrap();
    let check = success(invoke(&[Path::new("--json"), Path::new("check"), &input]));
    assert_eq!(check["frames"], 33272);
    assert_eq!(check["notes"], 1);
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &input,
        Path::new("-o"),
        &plan,
    ]));
    let json: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    assert_eq!(json["version"], 3);
    for format in ["float32", "pcm16"] {
        let built = dir.path().join(format!("{format}-build.wav"));
        success(invoke(&[
            Path::new("--json"),
            Path::new("build"),
            &input,
            Path::new("-o"),
            &built,
            Path::new("--format"),
            Path::new(format),
        ]));
    }
    fs::remove_file(&input).unwrap();
    for format in ["float32", "pcm16"] {
        let rendered = dir.path().join(format!("{format}-render.wav"));
        success(invoke(&[
            Path::new("--json"),
            Path::new("render"),
            &plan,
            Path::new("-o"),
            &rendered,
            Path::new("--format"),
            Path::new(format),
        ]));
        assert_eq!(
            fs::read(dir.path().join(format!("{format}-build.wav"))).unwrap(),
            fs::read(&rendered).unwrap()
        );
        assert_eq!(hound::WavReader::open(rendered).unwrap().duration(), 33272);
    }
}

#[test]
fn forged_plans_and_late_pcm_failure_preserve_destination() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("main.maac");
    let plan = dir.path().join("plan.json");
    let wav = dir.path().join("existing.wav");
    fs::write(&input, source("linear")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &input,
        Path::new("-o"),
        &plan,
    ]));
    let original: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    for field in ["version", "frame"] {
        let mut forged = original.clone();
        if field == "version" {
            forged["version"] = 99.into();
        } else {
            forged["events"][0]["on_frame"] = 1.into();
        }
        fs::write(&plan, serde_json::to_vec(&forged).unwrap()).unwrap();
        fs::write(&wav, b"keep me").unwrap();
        let output = invoke(&[
            Path::new("--json"),
            Path::new("render"),
            &plan,
            Path::new("-o"),
            &wav,
            Path::new("--force"),
        ]);
        assert!(!output.status.success());
        assert_eq!(fs::read(&wav).unwrap(), b"keep me");
    }
    fs::write(
        &input,
        source("linear").replace("(1/10s, 1/2, step)", "(1/10s, 2, step)"),
    )
    .unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &input,
        Path::new("-o"),
        &plan,
        Path::new("--force"),
    ]));
    let before = fs::read_dir(dir.path()).unwrap().count();
    let output = invoke(&[
        Path::new("--json"),
        Path::new("render"),
        &plan,
        Path::new("-o"),
        &wav,
        Path::new("--format"),
        Path::new("pcm16"),
        Path::new("--force"),
    ]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["code"], "E_PCM16_RANGE");
    assert_eq!(fs::read(&wav).unwrap(), b"keep me");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), before);
}

#[test]
fn step_cli_plan_matches_legacy_bundle_bytes() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("main.maac");
    let output = dir.path().join("plan.json");
    fs::write(&input, source("step")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &input,
        Path::new("-o"),
        &output,
    ]));
    let bundle = maac::bundle_fs::load_bundle(&input, dir.path()).unwrap();
    assert_eq!(
        fs::read(&output).unwrap(),
        maac::compile_bundle(&bundle).unwrap().to_json().unwrap()
    );
    assert!(maac::cli::read_plan(&output).is_ok());
}

#[test]
fn versioned_writer_preserves_legacy_bytes_and_validates_before_header() {
    use maac::export::{
        write_wav, write_wav_versioned, write_wav_versioned_with_limits, WavFormat,
    };
    use std::io::Cursor;
    let document = maac::parse(&source("step")).unwrap();
    let legacy = maac::compile(&document).unwrap();
    let versioned = maac::compile_versioned(&document).unwrap();
    for format in [WavFormat::Float32, WavFormat::Pcm16] {
        let mut old = Cursor::new(Vec::new());
        let mut new = Cursor::new(Vec::new());
        write_wav(&mut old, &legacy, format).unwrap();
        write_wav_versioned(&mut new, &versioned, format).unwrap();
        assert_eq!(old.into_inner(), new.into_inner());
    }
    let ramp = maac::compile_versioned(&maac::parse(&source("linear")).unwrap()).unwrap();
    let mut sink = Cursor::new(Vec::new());
    let limits = maac::plan::PlanLimits {
        max_events: 0,
        ..Default::default()
    };
    assert!(
        write_wav_versioned_with_limits(&mut sink, &ramp, WavFormat::Float32, &limits).is_err()
    );
    assert!(sink.into_inner().is_empty());
}

#[test]
fn renderer_budget_exhaustion_leaves_generic_wav_sink_empty() {
    let document = maac::parse(
        r#"maac 1;
project p { score = [0q, 4q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &s:out; }
tempo clock { points = [(0q, 120bpm, linear), (4q, 240bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node s { type = "core.sine/1"; config = { voices = 1; }; }"#,
    )
    .unwrap();
    let plan = maac::compile_versioned(&document).unwrap();
    let limits = maac::plan::PlanLimits {
        max_work: 256,
        ..Default::default()
    };
    // Import validation can spend exactly 256 terms; renderer preparation
    // requires further work and must share that allowance before writing.
    let maac::VersionedPlan::V3(v3) = &plan else {
        panic!()
    };
    v3.validate_with_limits(&limits).unwrap();
    let mut sink = std::io::Cursor::new(Vec::new());
    let error = maac::export::write_wav_versioned_with_limits(
        &mut sink,
        &plan,
        maac::export::WavFormat::Float32,
        &limits,
    )
    .unwrap_err();
    assert_eq!(error.code(), "E_RESOURCE_LIMIT");
    assert!(sink.into_inner().is_empty());
}

#[test]
fn cli_delivers_ramps_from_source_and_retained_plan() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("main.maac");
    let plan = dir.path().join("plan.json");
    let schema = dir.path().join("schema.json");
    let direct = dir.path().join("direct");
    let retained = dir.path().join("retained");
    fs::write(&schema, maac::production_data::SCHEMA_BYTES).unwrap();
    let text = source("linear").replace(
        "score = [0q, 1q];",
        "score = [0q, 1q]; requires = [\"maac.production/1\"];",
    ) + &format!(
        r#"
asset schema {{ kind = descriptor; path = "schema.json"; hash = "{}"; }}
extension deliveries {{ namespace = "maac.production/1"; schema = &schema; render_affecting = true;
  data = {{ deliveries = {{ release = {{ rate = 44100Hz; resampler = "maac.src.kaiser/1";
    targets = {{ master = {{ role = master; output = &s:out; encoding = wav_f32le; dither = {{ type = none; }}; }}; }};
  }}; }}; }};
}}"#,
        maac::bundle::sha256_digest(maac::production_data::SCHEMA_BYTES)
    );
    fs::write(&input, text).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &input,
        Path::new("-o"),
        &plan,
    ]));
    let first = success(invoke(&[
        Path::new("--json"),
        Path::new("deliver"),
        &input,
        Path::new("--delivery"),
        Path::new("release"),
        Path::new("--output-dir"),
        &direct,
    ]));
    fs::remove_file(&input).unwrap();
    fs::remove_file(&schema).unwrap();
    let second = success(invoke(&[
        Path::new("--json"),
        Path::new("deliver"),
        &plan,
        Path::new("--delivery"),
        Path::new("release"),
        Path::new("--output-dir"),
        &retained,
    ]));
    assert_eq!(
        first["delivery"]["schema"],
        "maac.production.delivery-manifest/2"
    );
    assert_eq!(first["delivery"]["frames"], 30568);
    assert_eq!(
        first["delivery"]["render_key"],
        second["delivery"]["render_key"]
    );
    assert_eq!(
        fs::read(direct.join("release.master.wav")).unwrap(),
        fs::read(retained.join("release.master.wav")).unwrap()
    );
}
