use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::tempdir;
fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}
fn invoke_in_dir(cwd: &Path, args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}
fn success(result: Output) -> Value {
    assert!(result.status.success(), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
    serde_json::from_slice(&result.stdout).unwrap()
}
fn copy_example(root: &Path) -> std::path::PathBuf {
    fs::create_dir_all(root.join("examples/sounds/core-kit")).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in [
        "examples/core-modulation.maac",
        "examples/sounds/core-kit/kick.pcm",
    ] {
        fs::copy(repo.join(path), root.join(path)).unwrap();
    }
    root.join("examples/core-modulation.maac")
}
#[test]
fn authored_example_survives_source_free_cli_replay_in_both_encodings() {
    let d = tempdir().unwrap();
    let root = d.path();
    let input = copy_example(root);
    let saved = root.join("saved.json");
    for action in ["check", "compile"] {
        let mut args = vec![
            Path::new("--json"),
            Path::new(action),
            &input,
            Path::new("--project-root"),
            root,
        ];
        if action == "compile" {
            args.extend([Path::new("-o"), saved.as_path()]);
        }
        let report = success(invoke(&args));
        assert_eq!(report["notes"], 1);
        assert_eq!(report["hits"], 1);
        assert_eq!(report["audio_clips"], 3);
    }
    let plan: Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    assert_eq!(plan["version"], 7);
    assert_eq!(plan["modulations"].as_array().unwrap().len(), 4);
    let mut audio = Vec::new();
    for format in ["float32", "pcm16"] {
        let out = root.join(format!("source-{format}.wav"));
        success(invoke(&[
            Path::new("--json"),
            Path::new("build"),
            &input,
            Path::new("--project-root"),
            root,
            Path::new("-o"),
            &out,
            Path::new("--format"),
            Path::new(format),
        ]));
        let mut wav = hound::WavReader::open(&out).unwrap();
        // Each two-quarter ramp lasts 4*ln(5/4) s; tail is exactly 1/2 s.
        let expected = ((8.0_f64 * (1.25_f64).ln() + 0.5) * 48000.).ceil() as u32;
        assert_eq!(expected, 109688);
        assert_eq!(wav.duration(), expected);
        assert_eq!(wav.spec().sample_rate, 48000);
        assert_eq!(wav.spec().channels, 2);
        let peak = if format == "float32" {
            assert_eq!(wav.spec().bits_per_sample, 32);
            wav.samples::<f32>()
                .map(|x| {
                    let x = x.unwrap();
                    assert!(x.is_finite());
                    x.abs() as f64
                })
                .fold(0., f64::max)
        } else {
            assert_eq!(wav.spec().bits_per_sample, 16);
            wav.samples::<i16>()
                .map(|x| f64::from(x.unwrap()).abs() / 32768.)
                .fold(0., f64::max)
        };
        assert!(peak > 0.01 && peak < 1., "{format}: {peak}");
        audio.push(fs::read(&out).unwrap());
    }
    fs::remove_dir_all(root.join("examples")).unwrap();
    let empty = tempdir().unwrap();
    assert!(!empty.path().join("examples").exists());
    assert!(!empty.path().join("examples/core-modulation.maac").exists());
    assert!(!empty
        .path()
        .join("examples/sounds/core-kit/kick.pcm")
        .exists());
    for (index, format) in ["float32", "pcm16"].iter().enumerate() {
        let out = root.join(format!("retained-{format}.wav"));
        assert!(saved.is_absolute());
        assert!(out.is_absolute());
        success(invoke_in_dir(
            empty.path(),
            &[
                Path::new("--json"),
                Path::new("render"),
                &saved,
                Path::new("-o"),
                &out,
                Path::new("--format"),
                Path::new(format),
            ],
        ));
        assert_eq!(fs::read(out).unwrap(), audio[index]);
    }
}
fn minimal(extra: &str) -> String {
    format!(
        r#"maac 1; project p {{score=[0q,1/100q];tail=0s;rate=48000Hz;tempo=&t;meter=&m;output=&sound:out;}} tempo t {{points=[(0q,120bpm,step)];}} meter m {{points=[(0q,4,4)];}} node sound {{type="core.sine/1";}} node c {{type="core.constant/1";}} {extra}"#
    )
}
#[test]
fn invalid_source_and_retained_controls_preserve_existing_and_absent_outputs() {
    let d = tempdir().unwrap();
    let root = d.path();
    let source = root.join("main.maac");
    let saved = root.join("saved.json");
    for (extra, code) in [
        (
            "modulate bad {from=&c:out;target=&sound.params.attack;amount=1;}",
            "E_UNIT",
        ),
        ("connect bad {from=&c:out;to=&sound:events;}", ""),
    ] {
        fs::write(&source, minimal(extra)).unwrap();
        for exists in [false, true] {
            let out = root.join("keep.wav");
            if exists {
                fs::write(&out, b"original").unwrap();
            }
            let result = invoke(&[
                Path::new("--json"),
                Path::new("build"),
                &source,
                Path::new("-o"),
                &out,
                Path::new("--force"),
            ]);
            assert!(!result.status.success());
            if !code.is_empty() {
                assert!(String::from_utf8(result.stdout).unwrap().contains(code));
            }
            if exists {
                assert_eq!(fs::read(&out).unwrap(), b"original");
                fs::remove_file(&out).unwrap();
            } else {
                assert!(!out.exists());
            }
        }
    }
    fs::write(&source, minimal("")).unwrap();
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &source,
        Path::new("-o"),
        &saved,
    ]));
    let original: Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
    for kind in ["malformed", "control_output", "nonfinite"] {
        let mut plan = original.clone();
        match kind {
            "malformed" => {
                plan["modulations"] = serde_json::json!([{"id":"bad"}]);
            }
            "control_output" => {
                plan["output"]["output"]["node"] = serde_json::json!("c");
            }
            _ => {
                // Start from a canonical source recipe to preserve the typed processor shape.
                fs::write(&source,minimal("node d {type=\"core.constant/1\";} modulate overflow {from=&c:out;target=&d.params.value;amount=2;}")).unwrap();
                success(invoke(&[
                    Path::new("--json"),
                    Path::new("compile"),
                    &source,
                    Path::new("-o"),
                    &saved,
                    Path::new("--force"),
                ]));
                plan = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
                let c = plan["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|n| n["id"] == "c")
                    .unwrap();
                c["params"]["value"] = serde_json::json!(format!("1{}/1", "0".repeat(308)));
            }
        }
        fs::write(&saved, serde_json::to_vec(&plan).unwrap()).unwrap();
        for exists in [false, true] {
            let out = root.join("keep.wav");
            if exists {
                fs::write(&out, b"original").unwrap();
            }
            let result = invoke(&[
                Path::new("--json"),
                Path::new("render"),
                &saved,
                Path::new("-o"),
                &out,
                Path::new("--force"),
            ]);
            assert!(!result.status.success(), "{kind}");
            if kind == "nonfinite" {
                assert!(String::from_utf8(result.stdout)
                    .unwrap()
                    .contains("E_NONFINITE"));
            }
            if exists {
                assert_eq!(fs::read(&out).unwrap(), b"original");
                fs::remove_file(out).unwrap();
            } else {
                assert!(!out.exists());
            }
        }
    }
}

#[test]
fn unrepresentable_retained_lfo_preserves_destinations() {
    let d = tempdir().unwrap();
    let saved = d.path().join("bad.json");
    let b = maac::SourceBundle::new(
        "score.maac",
        minimal("node motion {type=\"core.lfo/1\";config={period=1s;};}")
            .replace("score=[0q,1/100q];tail=0s", "score=[0q,1q];tail=1s")
            .replace("120bpm", "60bpm"),
    );
    let artifact = maac::compile_bundle_artifact(&b).unwrap();
    let mut plan: Value = serde_json::from_slice(&artifact.to_json().unwrap()).unwrap();
    let denominator = num_bigint::BigInt::from(1) << 1200;
    let numerator = (&denominator / 4) + 1;
    let motion = plan["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["id"] == "motion")
        .unwrap();
    motion["processor"]["config"]["period"] =
        serde_json::json!(format!("{numerator}/{denominator}"));
    fs::write(&saved, serde_json::to_vec(&plan).unwrap()).unwrap();
    for exists in [false, true] {
        let out = d.path().join("keep.wav");
        if exists {
            fs::write(&out, b"original").unwrap();
        }
        let result = invoke(&[
            Path::new("--json"),
            Path::new("render"),
            &saved,
            Path::new("-o"),
            &out,
            Path::new("--force"),
        ]);
        assert!(!result.status.success());
        assert!(String::from_utf8(result.stdout)
            .unwrap()
            .contains("E_TIME_PRECISION"));
        if exists {
            assert_eq!(fs::read(out).unwrap(), b"original");
        } else {
            assert!(!out.exists());
        }
    }
}
