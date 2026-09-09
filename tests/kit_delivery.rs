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
        project p {{ score=[0q,4q]; rate=48000Hz; tempo=&clock; meter=&metre; output=&master:out; tail=1/100s; requires=["maac.production/1"]; }}
        tempo clock {{ points=[(0q,120bpm,linear),(4q,240bpm,step)]; }}
        meter metre {{ points=[(0q,4,4)]; }}
        asset schema {{ kind=descriptor; path="production.schema.json"; hash="{}"; }}
        asset sample {{ kind=audio; path="sample.pcm"; hash="{}"; format="pcm_f32le_interleaved/1"; rate=24000Hz; channels=1; frames=3; }}
        node kit {{ type="core.kit/1"; config={{channels=1;samples=[{{key="";asset=&sample;}}];}}; }}
        node room {{ type="fx.reverb/1"; config={{channels=1;}}; params={{mix=0.5;}}; }}
        node master {{ type="core.pan/1"; }}
        connect room_in {{from=&kit:out;to=&room:in;}}
        connect master_in {{from=&room:out;to=&master:in;}}
        track t {{target=&kit:events;}}
        pattern pat {{length=4q; hit h {{at=0q;key="";}} hit again {{at=1q;key="";}}}}
        place place {{pattern=&pat;track=&t;at=0q;}}
        extension deliveries {{namespace="maac.production/1";schema=&schema;render_affecting=true;data={{deliveries={{release={{rate={rate}Hz;resampler="maac.src.kaiser/1";targets={{
            master={{role=master;output=&master:out;encoding=wav_f32le;dither={{type=none;}};{checks}}};
            stem={{role=stem;output=&kit:out;encoding=wav_f32le;dither={{type=none;}};}};
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
fn production_kit_identity_normalizes_defaults_and_tracks_samples_and_hits() {
    let source = bundle(48000, 0.5, "");
    let plan = compile_bundle_artifact(&source).unwrap();
    let base = identity(&plan);
    let mut explicit = source.clone();
    let text = explicit.sources.get_mut("scores/main.maac").unwrap();
    *text = text
        .replace(
            "config={channels=1;samples",
            "params={level=1;};config={voices=64;channels=1;samples",
        )
        .replace(
            "hit h {at=0q;key=\"\";}",
            "hit h {at=0q;key=\"\";velocity=1;onset_offset=0s;order=0;}",
        );
    assert_eq!(
        base["execution_hash"],
        identity(&compile_bundle_artifact(&explicit).unwrap())["execution_hash"]
    );
    assert_ne!(
        base["execution_hash"],
        identity(&compile_bundle_artifact(&bundle(48000, 0.25, "")).unwrap())["execution_hash"]
    );
    let mut moved = source;
    let text = moved.sources.get_mut("scores/main.maac").unwrap();
    *text = text.replace("hit again {at=1q", "hit again {at=2q");
    assert_ne!(
        base["execution_hash"],
        identity(&compile_bundle_artifact(&moved).unwrap())["execution_hash"]
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
    // Exact duration is 2*ln(2)+1/100 seconds; independently calculated ceilings.
    for (rate, expected) in [(44100, 61577), (48000, 67023), (96000, 134045)] {
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
        assert_eq!(direct.engine_frames, 67023);
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
                ["master", "kit", "room"].map(|id| maac::plan::PortRef::new(id, "out").unwrap());
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
fn legacy_artifact_delivery_preserves_manifest_identity_and_samples() {
    let mut source = bundle(48000, 0.5, "");
    let text = source.sources.get_mut("scores/main.maac").unwrap();
    *text = text
        .lines()
        .filter(|line| !line.contains("asset sample "))
        .collect::<Vec<_>>()
        .join("\n")
        .replace(
            "points=[(0q,120bpm,linear),(4q,240bpm,step)]",
            "points=[(0q,120bpm,step)]",
        )
        .replace(
            r#"type="core.kit/1"; config={channels=1;samples=[{key="";asset=&sample;}];}"#,
            r#"type="core.sine/1""#,
        )
        .replace(
            r#"hit h {at=0q;key="";}"#,
            "note h {at=0q;dur=1/4q;pitch=A4;}",
        )
        .replace(
            r#"hit again {at=1q;key="";}"#,
            "note again {at=1q;dur=1/4q;pitch=A4;}",
        );
    let artifact = compile_bundle_artifact(&source).unwrap();
    let versioned = maac::compiler::compile_bundle_versioned(&source).unwrap();
    assert!(artifact.version() < 3);
    let direct_dir = tempfile::tempdir().unwrap();
    let artifact_dir = tempfile::tempdir().unwrap();
    let mut direct_options = options(direct_dir.path());
    direct_options.targets = vec!["stem".into()];
    let mut artifact_options = options(artifact_dir.path());
    artifact_options.targets = vec!["stem".into()];
    let direct = maac::production_delivery::deliver_versioned(
        &versioned,
        &direct_options,
        &Default::default(),
    )
    .unwrap();
    let artifact = maac::production_delivery::deliver_artifact(
        &artifact,
        &artifact_options,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(artifact.schema, "maac.production.delivery-manifest/1");
    assert_eq!(direct.render_key, artifact.render_key);
    assert_eq!(direct.interval, artifact.interval);
    assert_eq!(direct.execution_identity, artifact.execution_identity);
    assert_eq!(
        direct.targets["stem"].file_hash,
        artifact.targets["stem"].file_hash
    );
}
