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
        audio clip {{asset=&sample;at=0q;source=[0frame,3frame];mode=rate;}}
        node room {{ type="fx.reverb/1"; config={{channels=1;}}; params={{mix=0.5;}}; }}
        node compressor {{ type="fx.compressor/1"; config={{channels=1;detector=external;sidechain_channels=1;}}; params={{ratio=2;attack=0s;release=0s;}}; }}
        node master {{ type="core.pan/1"; }}
        connect room_in {{from=&clip:out;to=&room:in;}}
        connect compressor_in {{from=&room:out;to=&compressor:in;}}
        connect detector {{from=&clip:out;to=&compressor:sidechain;}}
        connect master_in {{from=&compressor:out;to=&master:in;}}
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
fn identity(plan: &PlanArtifact) -> Value {
    let value: Value = serde_json::from_slice(&plan.to_json().unwrap()).unwrap();
    value["production"]["execution_identity"].clone()
}
#[test]
fn production_audio_identity_normalizes_defaults_and_tracks_every_transport_input() {
    let source = bundle(48000, 0.5, "");
    let base = identity(&compile_bundle_artifact(&source).unwrap());
    let implicit = "audio clip {asset=&sample;at=0q;source=[0frame,3frame];mode=rate;}";
    let mut explicit = source.clone();
    let text = explicit.sources.get_mut("scores/main.maac").unwrap();
    *text = text.replace(implicit,"audio clip {asset=&sample;at=bar(1,1);source=[0frame,3frame];mode=rate;speed=1;reverse=false;gain=1;fade_in=0ms;fade_out=0s;fade_shape=linear;}");
    assert_eq!(
        base["execution_hash"],
        identity(&compile_bundle_artifact(&explicit).unwrap())["execution_hash"]
    );
    for change in [
        "at=1/100q;",
        "speed=2;",
        "reverse=true;",
        "gain=1/2;",
        "fade_in=1ms;",
        "fade_out=1ms;",
        "fade_shape=equal_power;",
        "source=[1frame,3frame];",
    ] {
        let mut changed = source.clone();
        let field = change.split('=').next().unwrap();
        let mut body = "asset=&sample;at=0q;source=[0frame,3frame];mode=rate;".to_owned();
        if field == "at" {
            body = body.replace("at=0q;", change);
        } else if field == "source" {
            body = body.replace("source=[0frame,3frame];", change);
        } else {
            body.push_str(change);
        }
        let text = changed.sources.get_mut("scores/main.maac").unwrap();
        *text = text.replace(implicit, &format!("audio clip {{{body}}}"));
        assert_ne!(
            base["execution_hash"],
            identity(&compile_bundle_artifact(&changed).unwrap())["execution_hash"],
            "{change}"
        );
    }
    assert_ne!(
        base["execution_hash"],
        identity(&compile_bundle_artifact(&bundle(48000, 0.25, "")).unwrap())["execution_hash"]
    );
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
    assert!(hound::WavReader::open(master).is_ok());
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
