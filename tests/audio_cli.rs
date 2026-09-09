use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::tempdir;
fn source(bytes: &[u8]) -> String {
    format!(
        r#"maac 1;
project p {{ score=[0q,1/3000q]; tail=1/12000s; rate=48000Hz; tempo=&clock; meter=&metre; output=&clip:out; }}
tempo clock {{ points=[(0q,120bpm,step)]; }} meter metre {{ points=[(0q,4,4)]; }}
asset sample {{ kind=audio; path="samples/hit.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=24000Hz; channels=1; frames=2; }}
audio clip {{asset=&sample;at=0q;source=[0frame,2frame];mode=rate;}}"#,
        maac::bundle::sha256_digest(bytes)
    )
}
fn setup(root: &Path, values: &[f32]) -> (PathBuf, PathBuf) {
    fs::create_dir_all(root.join("scores")).unwrap();
    fs::create_dir_all(root.join("samples")).unwrap();
    let input = root.join("scores/main.maac");
    let asset = root.join("samples/hit.pcm");
    let bytes: Vec<u8> = values.iter().flat_map(|x| x.to_le_bytes()).collect();
    fs::write(&input, source(&bytes)).unwrap();
    fs::write(&asset, bytes).unwrap();
    (input, asset)
}
fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}
fn success(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn nested_bundle_build_and_retained_render_match_both_wav_formats() {
    let d = tempdir().unwrap();
    let root = d.path();
    let (input, asset) = setup(root, &[0.25, 0.5]);
    let saved = root.join("plan.json");
    let checked = success(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &input,
        Path::new("--project-root"),
        root,
    ]));
    assert_eq!(checked["notes"], 0);
    assert_eq!(checked["audio_clips"], 1);
    let compiled = success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &input,
        Path::new("--project-root"),
        root,
        Path::new("-o"),
        &saved,
    ]));
    assert_eq!(compiled["audio_clips"], 1);
    assert_eq!(compiled["notes"], 0);
    let plan_bytes = fs::read(&saved).unwrap();
    assert!(maac::load_plan_versioned(&plan_bytes).is_err());
    let mut built = vec![];
    for format in ["float32", "pcm16"] {
        let path = root.join(format!("built-{format}.wav"));
        let result = success(invoke(&[
            Path::new("--json"),
            Path::new("build"),
            &input,
            Path::new("--project-root"),
            root,
            Path::new("-o"),
            &path,
            Path::new("--format"),
            Path::new(format),
        ]));
        assert_eq!(result["audio_clips"], 1);
        assert_eq!(result["notes"], 0);
        built.push(fs::read(path).unwrap());
    }
    fs::remove_file(input).unwrap();
    fs::remove_file(asset).unwrap();
    for (index, format) in ["float32", "pcm16"].into_iter().enumerate() {
        let path = root.join(format!("retained-{format}.wav"));
        let result = success(invoke(&[
            Path::new("--json"),
            Path::new("render"),
            &saved,
            Path::new("-o"),
            &path,
            Path::new("--format"),
            Path::new(format),
        ]));
        assert_eq!(result["audio_clips"], 1);
        assert_eq!(fs::read(path).unwrap(), built[index]);
    }
}
#[test]
fn failed_force_exports_and_invalid_assets_preserve_destination() {
    for kind in ["missing", "hash", "nonfinite", "overload"] {
        let d = tempdir().unwrap();
        let values = match kind {
            "nonfinite" => [f32::NAN, 0.5],
            "overload" => [2., 2.],
            _ => [0.25, 0.5],
        };
        let (input, asset) = setup(d.path(), &values);
        let output = d.path().join("keep.wav");
        fs::write(&output, b"original").unwrap();
        match kind {
            "missing" => fs::remove_file(asset).unwrap(),
            "hash" => fs::write(asset, [0u8; 8]).unwrap(),
            _ => {}
        }
        let result = invoke(&[
            Path::new("--json"),
            Path::new("build"),
            &input,
            Path::new("--project-root"),
            d.path(),
            Path::new("-o"),
            &output,
            Path::new("--force"),
            Path::new("--format"),
            Path::new("pcm16"),
        ]);
        assert!(!result.status.success(), "{kind}");
        assert_eq!(fs::read(&output).unwrap(), b"original");
        let error: Value = serde_json::from_slice(&result.stdout).unwrap();
        if kind == "overload" {
            assert_eq!(error["code"], "E_PCM16_RANGE");
        }
    }
}

#[test]
fn additive_result_api_reports_actual_mixed_counts_and_clip_human_summary() {
    use maac::cli::{execute_artifact, format_human_artifact, Command as CliCommand, ProfileArg};
    let d = tempdir().unwrap();
    let (input, _) = setup(d.path(), &[0.25, 0.5]);
    let command = CliCommand::Check {
        input: Some(input.clone()),
        project_root: Some(d.path().into()),
        profile: ProfileArg::Default,
    };
    let pure = execute_artifact(&command).unwrap();
    assert_eq!(pure.base().notes, Some(0));
    assert_eq!(pure.hits(), 0);
    assert_eq!(pure.audio_clips(), 1);
    assert_eq!(serde_json::to_value(&pure).unwrap()["audio_clips"], 1);
    assert!(format_human_artifact(&pure).contains("1 audio clips"));
    let mut text = fs::read_to_string(&input).unwrap();
    text.push_str(
        r#"
node sine {type="core.sine/1";}
node kit {type="core.kit/1";config={channels=1;samples=[{key="k";asset=&sample;}];};}
track n {target=&sine:events;} track h {target=&kit:events;}
pattern np {length=1/3000q;note note {at=0q;dur=1/24000q;pitch=A4;}}
pattern hp {length=1/3000q;hit hit {at=0q;key="k";}}
place pn {pattern=&np;track=&n;at=0q;} place ph {pattern=&hp;track=&h;at=0q;}
"#,
    );
    fs::write(&input, text).unwrap();
    let mixed = execute_artifact(&command).unwrap();
    assert_eq!(mixed.base().notes, Some(1));
    assert_eq!(mixed.hits(), 1);
    assert_eq!(mixed.audio_clips(), 1);
    assert_eq!(serde_json::to_value(&mixed).unwrap()["audio_clips"], 1);
    assert!(format_human_artifact(&mixed).contains("1 notes, 1 hits, 1 audio clips"));
}

#[test]
fn artifact_export_preparation_budget_failure_leaves_sink_empty() {
    let bytes: Vec<u8> = [0.25f32, 0.5]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let text = source(&bytes)
        .split("track track")
        .next()
        .unwrap()
        .replace("score=[0q,1/3000q]", "score=[0q,4q]")
        .replace("tail=1/12000s", "tail=0s")
        .replace(
            "points=[(0q,120bpm,step)]",
            "points=[(0q,120bpm,linear),(4q,240bpm,step)]",
        );
    let mut bundle = maac::SourceBundle::new("scores/main.maac", text);
    bundle.assets.insert("samples/hit.pcm".into(), bytes);
    let artifact = maac::compile_bundle_artifact(&bundle).unwrap();
    let limits = maac::plan::PlanLimits {
        max_work: 0,
        ..Default::default()
    };
    let mut sink = std::io::Cursor::new(Vec::new());
    let error = maac::write_wav_artifact_with_limits(
        &mut sink,
        &artifact,
        maac::export::WavFormat::Float32,
        &limits,
    )
    .unwrap_err();
    assert_eq!(error.code(), "E_RESOURCE_LIMIT");
    assert!(sink.into_inner().is_empty());
}

#[test]
fn deliver_routes_source_and_retained_audio_artifacts() {
    let d = tempdir().unwrap();
    let root = d.path();
    let (input, asset) = setup(root, &[0.25, 0.5]);
    let schema = maac::production_data::SCHEMA_BYTES;
    fs::write(root.join("production.schema.json"), schema).unwrap();
    let mut text = fs::read_to_string(&input).unwrap().replace(
        "output=&clip:out;",
        "output=&clip:out; requires=[\"maac.production/1\"];",
    );
    text.push_str(&format!(r#"
asset schema {{kind=descriptor;path="production.schema.json";hash="{}";}}
extension deliveries {{namespace="maac.production/1";schema=&schema;render_affecting=true;data={{deliveries={{release={{rate=48000Hz;resampler="maac.src.kaiser/1";targets={{master={{role=master;output=&clip:out;encoding=wav_f32le;dither={{type=none;}};}};}};}};}};}};}}
"#,maac::bundle::sha256_digest(schema)));
    fs::write(&input, text).unwrap();
    let saved = root.join("saved.json");
    success(invoke(&[
        Path::new("--json"),
        Path::new("compile"),
        &input,
        Path::new("--project-root"),
        root,
        Path::new("-o"),
        &saved,
    ]));
    let out = root.join("source-delivery");
    fs::create_dir(&out).unwrap();
    let first = success(invoke(&[
        Path::new("--json"),
        Path::new("deliver"),
        &input,
        Path::new("--project-root"),
        root,
        Path::new("--delivery"),
        Path::new("release"),
        Path::new("--output-dir"),
        &out,
    ]));
    assert_eq!(first["audio_clips"], 1);
    assert_eq!(first["notes"], 0);
    let audio = fs::read(
        out.join(
            first["delivery"]["targets"]["master"]["filename"]
                .as_str()
                .unwrap(),
        ),
    )
    .unwrap();
    fs::remove_file(input).unwrap();
    fs::remove_file(asset).unwrap();
    fs::remove_file(root.join("production.schema.json")).unwrap();
    let out = root.join("retained-delivery");
    fs::create_dir(&out).unwrap();
    let second = success(invoke(&[
        Path::new("--json"),
        Path::new("deliver"),
        &saved,
        Path::new("--delivery"),
        Path::new("release"),
        Path::new("--output-dir"),
        &out,
    ]));
    assert_eq!(second["audio_clips"], 1);
    assert_eq!(
        audio,
        fs::read(
            out.join(
                second["delivery"]["targets"]["master"]["filename"]
                    .as_str()
                    .unwrap()
            )
        )
        .unwrap()
    );
}
