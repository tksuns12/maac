use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle_artifact;
use maac::production_data::SCHEMA_BYTES;
use maac::PlanArtifact;
use serde_json::Value;

fn bundle(rate: u32, sample: f32, checks: &str) -> SourceBundle {
    let bytes: Vec<u8> = [sample, sample / 2., 0.]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let source = format!(
        r#"maac 1;
        project p {{ score=[0q,1/25q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&master:out; tail=1/100s; requires=["maac.production/1"]; }}
        tempo clock {{ points=[(0q,120bpm,linear),(1/25q,240bpm,step)]; }}
        meter metre {{ points=[(0q,4,4)]; }}
        asset schema {{ kind=descriptor; path="production.schema.json"; hash="{}"; }}
        asset sample {{ kind=audio; path="sample.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=24000Hz; channels=1; frames=3; }}
        audio clip {{asset=&sample;at=0q;source=[0frame,3frame];mode=warp_rate;warp=[(0q,0frame),(1/100q,1frame),(1/50q,3frame)];}}
        audio rateclip {{asset=&sample;at=0q;source=[0frame,3frame];mode=rate;gain=1/4;}}
        node mix {{type="core.sum/1";config={{channels=1;}};}}
        node sine {{type="core.sine/1";params={{level=1/100;}};}}
        node kit {{type="core.kit/1";config={{channels=1;samples=[{{key="k";asset=&sample;}}];}};params={{level=1/10;}};}}
        track n {{target=&sine:events;}} track h {{target=&kit:events;}}
        pattern np {{length=1/25q;note note {{at=0q;dur=1/100q;pitch=A4;}}}}
        pattern hp {{length=1/25q;hit hit {{at=0q;key="k";}}}}
        place pn {{pattern=&np;track=&n;at=0q;}} place ph {{pattern=&hp;track=&h;at=0q;}}
        connect warp_mix {{from=&clip:out;to=&mix:in;}}
        connect rate_mix {{from=&rateclip:out;to=&mix:in;}}
        connect sine_mix {{from=&sine:out;to=&mix:in;}}
        connect kit_mix {{from=&kit:out;to=&mix:in;}}
        node room {{ type="fx.reverb/1"; config={{channels=1;}}; params={{mix=0.5;}}; }}
        node compressor {{ type="fx.compressor/1"; config={{channels=1;detector=external;sidechain_channels=1;}}; params={{ratio=2;attack=0s;release=0s;}}; }}
        node master {{ type="core.pan/1"; }}
        connect room_in {{from=&mix:out;to=&room:in;}}
        connect compressor_in {{from=&room:out;to=&compressor:in;}}
        connect detector {{from=&clip:out;to=&compressor:sidechain;}}
        connect master_in {{from=&compressor:out;to=&master:in;}}
        node score_motion {{type="core.lfo/1";config={{period=1/50q;wave=triangle;}};}}
        node seconds_motion {{type="core.lfo/1";config={{period=1/100s;}};}}
        node depth {{type="core.constant/1";}}
        curve shape {{clock=score;points=[(0q,0,linear),(1/25q,1/4,step)];}}
        automation lane {{target=&depth.params.value;curve=&shape;at=0q;}}
        modulate chain {{from=&score_motion:out;target=&depth.params.value;amount=1/4;}}
        modulate level {{from=&depth:out;target=&sine.params.level;amount=1/100;}}
        modulate pan {{from=&seconds_motion:out;target=&master.params.pan;amount=1/2;}}
        extension deliveries {{namespace="maac.production/1";schema=&schema;render_affecting=true;data={{deliveries={{release={{rate={rate}Hz;resampler="maac.src.kaiser/1";targets={{
            master={{role=master;output=&master:out;encoding=wav_f32le;dither={{type=none;}};{checks}}};
            stem={{role=stem;output=&clip:out;encoding=wav_f32le;dither={{type=none;}};}};
            shared={{role=stem;output=&room:out;encoding=wav_f32le;dither={{type=none;}};}};
        }};}};}};}};}}
    "#,
        sha256_digest(SCHEMA_BYTES),
        sha256_digest(&bytes)
    );
    let mut bundle = SourceBundle::new("scores/main.maac", source);
    bundle
        .assets
        .insert("production.schema.json".into(), SCHEMA_BYTES.to_vec());
    bundle.assets.insert("sample.pcm".into(), bytes);
    bundle
}
fn options(path: &std::path::Path) -> maac::production_delivery::DeliveryOptions {
    maac::production_delivery::DeliveryOptions {
        delivery_id: "release".into(),
        targets: Vec::new(),
        output_dir: path.into(),
        overwrite: false,
        limits: Default::default(),
    }
}
#[test]
fn source_and_retained_deliver_all_ports_at_exact_delivery_rates() {
    // Exact duration is ln(2)/50+1/100 seconds; independently calculated ceilings.
    for (rate, expected) in [(44100, 1053), (48000, 1146), (96000, 2291)] {
        let source = bundle(rate, 0.5, "");
        let plan = compile_bundle_artifact(&source).unwrap();
        assert_eq!(plan.version(), 7);
        assert_eq!(plan.audio_clip_count(), 2);
        assert_eq!(plan.event_count(), 2);
        let retained = PlanArtifact::from_json(&plan.to_json().unwrap()).unwrap();
        drop(source);
        let direct_dir = tempfile::tempdir().unwrap();
        let retained_dir = tempfile::tempdir().unwrap();
        let limits = maac::plan::PlanLimits::default();
        let direct = maac::production_delivery::deliver_artifact(
            &plan,
            &options(direct_dir.path()),
            &limits,
        )
        .unwrap();
        let replay = maac::production_delivery::deliver_artifact(
            &retained,
            &options(retained_dir.path()),
            &limits,
        )
        .unwrap();
        assert!(direct.ok && replay.ok);
        assert_eq!(direct.schema, "maac.production.delivery-manifest/2");
        assert_eq!(direct.frames, expected);
        assert_eq!(direct.engine_frames, 1146);
        assert!(direct.interval.get("duration_seconds").is_none());
        assert_eq!(
            direct.interval["tempo"]["points"].as_array().unwrap().len(),
            2
        );
        assert_eq!(direct.render_key, replay.render_key);
        assert_eq!(direct.execution_identity, replay.execution_identity);
        for id in ["master", "stem", "shared"] {
            assert_eq!(direct.targets[id].file_hash, replay.targets[id].file_hash);
            let path = direct_dir.path().join(&direct.targets[id].filename);
            assert_eq!(
                std::fs::read(&path).unwrap(),
                std::fs::read(retained_dir.path().join(&replay.targets[id].filename)).unwrap()
            );
            assert_eq!(
                u64::from(hound::WavReader::open(path).unwrap().duration()),
                expected
            );
        }
        if rate == 48000 {
            let ports =
                ["master", "clip", "room"].map(|id| maac::plan::PortRef::new(id, "out").unwrap());
            let mut rendered = [Vec::new(), Vec::new(), Vec::new()];
            maac::dsp::render_ports_artifact_with_limits(&plan, &limits, &ports, |frames| {
                for (index, frame) in frames.iter().enumerate() {
                    rendered[index].extend(frame.iter().map(|sample| *sample as f32));
                }
                Ok(())
            })
            .unwrap();
            for (index, id) in ["master", "stem", "shared"].iter().enumerate() {
                let actual =
                    hound::WavReader::open(direct_dir.path().join(&direct.targets[*id].filename))
                        .unwrap()
                        .samples::<f32>()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap();
                assert_eq!(actual, rendered[index]);
            }
        }
    }
}
#[test]
fn destination_failure_is_atomic_and_failed_checks_retain_complete_audio() {
    let plan = compile_bundle_artifact(&bundle(
        48000,
        0.5,
        "limits={integrated_loudness={unit=LUFS;min=0;};};",
    ))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let master = directory.path().join("release.master.wav");
    let manifest = directory.path().join("release.manifest.json");
    std::fs::write(&master, b"existing master").unwrap();
    std::fs::write(&manifest, b"existing manifest").unwrap();
    let mut opts = options(directory.path());
    let failure =
        maac::production_delivery::deliver_artifact(&plan, &opts, &Default::default()).unwrap_err();
    assert_eq!(failure.code, "E_OUTPUT_EXISTS");
    assert_eq!(std::fs::read(&master).unwrap(), b"existing master");
    assert_eq!(std::fs::read(&manifest).unwrap(), b"existing manifest");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    opts.overwrite = true;
    let report =
        maac::production_delivery::deliver_artifact(&plan, &opts, &Default::default()).unwrap();
    assert!(!report.ok);
    assert_eq!(report.artifact_status, "complete");
    assert_eq!(report.check_status, "fail");
    assert_eq!(hound::WavReader::open(master).unwrap().duration(), 1146);
    let stored: Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    assert_eq!(stored["check_status"], "fail");
    assert_eq!(stored["artifact_status"], "complete");
}

#[test]
fn delivery_targets_reject_wrong_ports_events_and_non_audio_objects() {
    for target in [
        "clip:missing",
        "clip:events",
        "sample:out",
        "group:out",
        "receiver:events",
        "score_motion:out",
        "depth:out",
    ] {
        let mut source = bundle(48000, 0.5, "");
        let text = source.sources.get_mut("scores/main.maac").unwrap();
        let old = "stem={role=stem;output=&clip:out;";
        assert!(text.contains(old));
        *text = text.replace(old, &format!("stem={{role=stem;output=&{target};"));
        text.push_str("node receiver {type=\"core.sine/1\";} track group {}");
        assert!(compile_bundle_artifact(&source).is_err(), "{target}");
    }
}

#[test]
fn named_cli_deliveries_replay_without_source_or_assets() {
    fn run(args: &[&std::path::Path]) -> Value {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_maac"))
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        serde_json::from_slice(&result.stdout).unwrap()
    }
    use std::{fs, path::Path};
    for (rate, frames) in [(44100, 1053), (48000, 1146), (96000, 2291)] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut b = bundle(rate, 0.5, "");
        // Both delivery encodings cross the real CLI boundary; master/shared remain float.
        let source = b.sources.get_mut("scores/main.maac").unwrap();
        *source = source.replace(
            "stem={role=stem;output=&clip:out;encoding=wav_f32le",
            "stem={role=stem;output=&clip:out;encoding=wav_pcm16le",
        );
        fs::create_dir(root.join("scores")).unwrap();
        for (path, source) in &b.sources {
            fs::write(root.join(path), source).unwrap();
        }
        for (path, bytes) in &b.assets {
            fs::write(root.join(path), bytes).unwrap();
        }
        let input = root.join("scores/main.maac");
        let saved = root.join("saved.json");
        run(&[
            Path::new("--json"),
            Path::new("compile"),
            &input,
            Path::new("--project-root"),
            root,
            Path::new("-o"),
            &saved,
        ]);
        let direct = root.join("direct");
        let retained = root.join("retained");
        fs::create_dir(&direct).unwrap();
        fs::create_dir(&retained).unwrap();
        let first = run(&[
            Path::new("--json"),
            Path::new("deliver"),
            &input,
            Path::new("--project-root"),
            root,
            Path::new("--delivery"),
            Path::new("release"),
            Path::new("--output-dir"),
            &direct,
        ]);
        for path in b.sources.keys().chain(b.assets.keys()) {
            fs::remove_file(root.join(path)).unwrap();
        }
        drop(b);
        let second = run(&[
            Path::new("--json"),
            Path::new("deliver"),
            &saved,
            Path::new("--delivery"),
            Path::new("release"),
            Path::new("--output-dir"),
            &retained,
        ]);
        for report in [&first, &second] {
            assert_eq!(report["notes"], 1);
            assert_eq!(report["hits"], 1);
            assert_eq!(report["audio_clips"], 2);
            assert_eq!(report["delivery"]["frames"], frames);
        }
        assert_eq!(
            first["delivery"]["execution_identity"],
            second["delivery"]["execution_identity"]
        );
        assert_eq!(
            first["delivery"]["render_key"],
            second["delivery"]["render_key"]
        );
        for id in ["master", "stem", "shared"] {
            let name = first["delivery"]["targets"][id]["filename"]
                .as_str()
                .unwrap();
            assert_eq!(
                fs::read(direct.join(name)).unwrap(),
                fs::read(retained.join(name)).unwrap()
            );
            let wav = hound::WavReader::open(direct.join(name)).unwrap();
            assert_eq!(wav.duration(), frames);
            assert_eq!(wav.spec().sample_rate, rate);
            assert_eq!(
                wav.spec().bits_per_sample,
                if id == "stem" { 16 } else { 32 }
            );
        }
    }
}
