use std::fs;

use maac::bundle::{sha256_digest, SourceBundle};
use maac::plan::{PlanLimits, PortRef};
use maac::plan_v3::VersionedPlan;
use maac::production_data::SCHEMA_BYTES;
use maac::production_delivery::{deliver_versioned, DeliveryLimits, DeliveryOptions};
use tempfile::tempdir;

fn source(rate: u32, master_limits: &str) -> String {
    format!(
        r#"maac 1;
project p {{score=[0q,4q];rate=48000Hz;tempo=&clock;meter=&metre;output=&master:out;tail=1/100s;requires=["maac.production/1"];}}
tempo clock {{points=[(0q,120bpm,linear),(4q,240bpm,step)];}}
meter metre {{points=[(0q,4,4)];}}
asset schema {{kind=descriptor;path="production.schema.json";hash="{}";}}
node sine {{type="core.sine/1";params={{attack=0s;release=0s;level=0.2;}};}}
node detector {{type="core.sine/1";params={{attack=0s;release=0s;level=0.8;}};}}
node compressed {{type="fx.compressor/1";config={{channels=1;detector=external;sidechain_channels=1;}};params={{threshold=-30dB;ratio=10;knee=0dB;attack=0s;release=0s;}};}}
node room {{type="fx.reverb/1";config={{channels=1;}};params={{mix=0.5;}};}}
node master {{type="core.pan/1";params={{pan=0;}};}}
connect c1 {{from=&sine:out;to=&compressed:in;}}
connect c2 {{from=&detector:out;to=&compressed:sidechain;}}
connect c3 {{from=&compressed:out;to=&room:in;}}
connect c4 {{from=&room:out;to=&master:in;}}
pattern phrase {{length=4q;note n {{at=0q;dur=4q;pitch=A4;velocity=1;}}}}
track melody {{target=&sine:events;}}
track duck {{target=&detector:events;}}
place melody_notes {{pattern=&phrase;track=&melody;at=0q;}}
place detector_notes {{pattern=&phrase;track=&duck;at=0q;}}
extension deliveries {{namespace="maac.production/1";schema=&schema;render_affecting=true;data={{deliveries={{release={{rate={rate}Hz;resampler="maac.src.kaiser/1";targets={{
master={{role=master;output=&master:out;encoding=wav_f32le;dither={{type=none;}};{master_limits}}};
stem={{role=stem;output=&compressed:out;encoding=wav_f32le;dither={{type=none;}};}};
}};}};}};}};}}
"#,
        sha256_digest(SCHEMA_BYTES)
    )
}

fn plan(rate: u32, master_limits: &str) -> VersionedPlan {
    let mut bundle = SourceBundle::new("main.maac", source(rate, master_limits));
    bundle
        .assets
        .insert("production.schema.json".into(), SCHEMA_BYTES.to_vec());
    let plan = maac::compile_bundle_versioned(&bundle).unwrap();
    assert!(matches!(plan, VersionedPlan::V3(_)));
    plan
}

fn options(path: &std::path::Path) -> DeliveryOptions {
    DeliveryOptions {
        delivery_id: "release".into(),
        targets: Vec::new(),
        output_dir: path.into(),
        overwrite: false,
        limits: DeliveryLimits::default(),
    }
}

#[test]
fn ramp_duration_is_independently_certified_at_every_delivery_rate() {
    // The exact recipe is 2*ln(2) + 1/100 seconds. These constants are the
    // independently calculated mathematical ceilings at the supported rates.
    for (rate, expected) in [(44_100, 61_577), (48_000, 67_023), (96_000, 134_045)] {
        let plan = plan(rate, "");
        let directory = tempdir().unwrap();
        let report =
            deliver_versioned(&plan, &options(directory.path()), &PlanLimits::default()).unwrap();

        assert!(report.ok);
        assert_eq!(report.schema, "maac.production.delivery-manifest/2");
        assert_eq!(report.engine_rate, 48_000);
        assert_eq!(report.engine_frames, 67_023);
        assert_eq!(report.rate, rate);
        assert_eq!(report.frames, expected);
        assert_eq!(report.interval["score_start_q"], "0");
        assert_eq!(report.interval["score_end_q"], "4");
        assert_eq!(report.interval["tail_seconds"], "1/100");
        assert_eq!(report.interval["engine_frames"], 67_023);
        assert_eq!(report.interval["delivery_frames"], expected);
        assert!(report.interval.get("duration_seconds").is_none());
        let VersionedPlan::V3(plan) = &plan else {
            unreachable!()
        };
        assert_eq!(
            report.interval["tempo"],
            serde_json::to_value(&plan.tempo).unwrap()
        );
        for id in ["master", "stem"] {
            let wav =
                hound::WavReader::open(directory.path().join(format!("release.{id}.wav"))).unwrap();
            assert_eq!(wav.spec().sample_rate, rate);
            assert_eq!(u64::from(wav.duration()), expected);
        }
    }
}

#[test]
fn retained_plan_replays_shared_graph_stems_and_tail_byte_for_byte() {
    let plan = plan(48_000, "");
    let direct_dir = tempdir().unwrap();
    let direct =
        deliver_versioned(&plan, &options(direct_dir.path()), &PlanLimits::default()).unwrap();

    let plan_bytes = plan.to_json().unwrap();
    let retained = VersionedPlan::from_json(&plan_bytes).unwrap();
    let retained_dir = tempdir().unwrap();
    let replay = deliver_versioned(
        &retained,
        &options(retained_dir.path()),
        &PlanLimits::default(),
    )
    .unwrap();
    assert_eq!(direct.render_key, replay.render_key);
    assert_eq!(direct.execution_identity, replay.execution_identity);
    for id in ["master", "stem"] {
        assert_eq!(direct.targets[id].file_hash, replay.targets[id].file_hash);
        assert_eq!(
            fs::read(direct_dir.path().join(format!("release.{id}.wav"))).unwrap(),
            fs::read(retained_dir.path().join(format!("release.{id}.wav"))).unwrap()
        );
    }

    let ports = [
        PortRef::new("master", "out").unwrap(),
        PortRef::new("compressed", "out").unwrap(),
    ];
    let mut rendered = [Vec::new(), Vec::new()];
    maac::dsp::render_ports_versioned_with_limits(
        &plan,
        &PlanLimits::default(),
        &ports,
        |frames| {
            for (index, frame) in frames.iter().enumerate() {
                rendered[index].extend(frame.iter().map(|sample| *sample as f32));
            }
            Ok(())
        },
    )
    .unwrap();
    for (index, id) in ["master", "stem"].iter().enumerate() {
        let actual = hound::WavReader::open(direct_dir.path().join(format!("release.{id}.wav")))
            .unwrap()
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(actual, rendered[index]);
    }
    assert!(rendered[0][2 * 66_543..]
        .iter()
        .any(|sample| *sample != 0.0));

    let selected_dir = tempdir().unwrap();
    let mut selected = options(selected_dir.path());
    selected.targets = vec!["stem".into()];
    let selected = deliver_versioned(&plan, &selected, &PlanLimits::default()).unwrap();
    assert_eq!(
        selected.targets["stem"].file_hash,
        direct.targets["stem"].file_hash
    );
    assert!(!selected_dir.path().join("release.master.wav").exists());
    assert_ne!(selected.render_key, direct.render_key);
}

#[test]
fn ramp_delivery_preserves_destinations_and_retains_audio_when_checks_fail() {
    let plan = plan(48_000, "limits={integrated_loudness={unit=LUFS;min=0;};};");
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("release.manifest.json"),
        b"existing manifest",
    )
    .unwrap();
    let opts = options(directory.path());
    let error = deliver_versioned(&plan, &opts, &PlanLimits::default()).unwrap_err();
    assert_eq!(error.code, "E_OUTPUT_EXISTS");
    assert_eq!(
        fs::read(directory.path().join("release.manifest.json")).unwrap(),
        b"existing manifest"
    );
    assert!(!directory.path().join("release.master.wav").exists());

    let mut opts = opts;
    opts.overwrite = true;
    let report = deliver_versioned(&plan, &opts, &PlanLimits::default()).unwrap();
    assert!(!report.ok);
    assert_eq!(report.schema, "maac.production.delivery-manifest/2");
    assert_eq!(report.artifact_status, "complete");
    assert_eq!(report.manifest_status, "complete");
    assert_eq!(report.check_status, "fail");
    assert!(directory.path().join("release.master.wav").is_file());
    assert!(directory.path().join("release.stem.wav").is_file());
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("release.manifest.json")).unwrap())
            .unwrap();
    assert_eq!(stored["targets"]["master"]["artifact_status"], "complete");
    assert_eq!(stored["check_status"], "fail");
}

#[test]
fn versioned_legacy_dispatch_preserves_manifest_key_and_audio_bytes() {
    let text = source(48_000, "").replace("120bpm,linear", "120bpm,step");
    let mut bundle = SourceBundle::new("main.maac", text);
    bundle
        .assets
        .insert("production.schema.json".into(), SCHEMA_BYTES.to_vec());
    let legacy = maac::compile_bundle(&bundle).unwrap();
    let versioned = maac::compile_bundle_versioned(&bundle).unwrap();
    assert_eq!(versioned, VersionedPlan::Legacy(legacy.clone()));

    let legacy_dir = tempdir().unwrap();
    let legacy_report = maac::production_delivery::deliver(
        &legacy,
        &options(legacy_dir.path()),
        &PlanLimits::default(),
    )
    .unwrap();
    let versioned_dir = tempdir().unwrap();
    let versioned_report = deliver_versioned(
        &versioned,
        &options(versioned_dir.path()),
        &PlanLimits::default(),
    )
    .unwrap();

    assert_eq!(legacy_report.schema, "maac.production.delivery-manifest/1");
    assert_eq!(legacy_report.render_key, versioned_report.render_key);
    assert_eq!(legacy_report.interval, versioned_report.interval);
    assert!(legacy_report.interval.get("duration_seconds").is_some());
    for id in ["master", "stem"] {
        assert_eq!(
            fs::read(legacy_dir.path().join(format!("release.{id}.wav"))).unwrap(),
            fs::read(versioned_dir.path().join(format!("release.{id}.wav"))).unwrap()
        );
    }
}
