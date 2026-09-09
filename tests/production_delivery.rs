use std::fs;

use maac::bundle::sha256_digest;
use maac::plan::{PlanLimits, PortRef};
use maac::production_data::SCHEMA_BYTES;
use maac::production_delivery::{deliver, DeliveryLimits, DeliveryOptions};
use maac::{compile_bundle, Plan, SourceBundle};
use tempfile::tempdir;

fn source(rate: u32, master_limits: &str, extra_targets: &str) -> String {
    format!(
        r#"maac 1;
project p {{score=[0q,1/100q];rate=48000Hz;tempo=&clock;meter=&metre;output=&master:out;tail=1/20s;requires=["maac.production/1"];}}
tempo clock {{points=[(0q,120bpm,step)];}}
meter metre {{points=[(0q,4,4)];}}
asset schema {{kind=descriptor;path="production.schema.json";hash="{}";}}
node sine {{type="core.sine/1";params={{attack=0s;release=0s;level=0.2;}};}}
node detector {{type="core.sine/1";params={{attack=0s;release=0s;level=0.8;}};}}
node compressed {{type="fx.compressor/1";config={{channels=1;detector=external;sidechain_channels=1;}};params={{threshold=-30dB;ratio=10;knee=0dB;attack=0s;release=0s;}};}}
node room {{type="fx.reverb/1";config={{channels=1;}};params={{mix=0.5;}};}}
node master {{type="core.pan/1";params={{pan=0;}};}}
node boost {{type="core.gain/1";config={{channels=1;}};params={{gain=10;}};}}
connect c1 {{from=&sine:out;to=&compressed:in;}}
connect c2 {{from=&detector:out;to=&compressed:sidechain;}}
connect c3 {{from=&compressed:out;to=&room:in;}}
connect c4 {{from=&room:out;to=&master:in;}}
connect c5 {{from=&sine:out;to=&boost:in;}}
pattern phrase {{length=1/100q;note n {{at=0q;dur=1/100q;pitch=A4;velocity=1;}}}}
track melody {{target=&sine:events;}}
track duck {{target=&detector:events;}}
place melody_notes {{pattern=&phrase;track=&melody;at=0q;}}
place detector_notes {{pattern=&phrase;track=&duck;at=0q;}}
extension deliveries {{namespace="maac.production/1";schema=&schema;render_affecting=true;data={{deliveries={{release={{rate={rate}Hz;resampler="maac.src.kaiser/1";targets={{
master={{role=master;output=&master:out;encoding=wav_f32le;dither={{type=none;}};{master_limits}}};
stem={{role=stem;output=&compressed:out;encoding=wav_f32le;dither={{type=none;}};}};
{extra_targets}
}};}};}};}};}}
"#,
        sha256_digest(SCHEMA_BYTES)
    )
}

fn plan(source: &str) -> Plan {
    let mut bundle = SourceBundle::new("main.maac", source);
    bundle
        .assets
        .insert("production.schema.json".into(), SCHEMA_BYTES.to_vec());
    compile_bundle(&bundle).unwrap()
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
fn aligned_stems_preserve_sidechain_tail_and_retained_plan_replay() {
    let plan = plan(&source(48000, "", ""));
    let full_dir = tempdir().unwrap();
    let full = deliver(&plan, &options(full_dir.path()), &PlanLimits::default()).unwrap();
    assert!(full.ok);
    assert_eq!(full.engine_frames, 2640);
    assert_eq!(full.frames, 2640);
    let ports = [
        PortRef::new("master", "out").unwrap(),
        PortRef::new("compressed", "out").unwrap(),
    ];
    let mut expected = [Vec::new(), Vec::new()];
    maac::dsp::render_ports_with_limits(&plan, &PlanLimits::default(), &ports, |frames| {
        for (i, frame) in frames.iter().enumerate() {
            expected[i].extend(frame.iter().map(|v| *v as f32));
        }
        Ok(())
    })
    .unwrap();
    for (i, id) in ["master", "stem"].iter().enumerate() {
        let path = full_dir.path().join(format!("release.{id}.wav"));
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 18);
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(bytes[36..38].try_into().unwrap()), 0); // WAVEFORMATEX.cbSize
        assert_eq!(&bytes[38..42], b"fact");
        assert_eq!(u32::from_le_bytes(bytes[42..46].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(bytes[46..50].try_into().unwrap()), 2640);
        assert_eq!(&bytes[50..54], b"data");
        assert_eq!(
            u32::from_le_bytes(bytes[54..58].try_into().unwrap()) as usize,
            expected[i].len() * 4
        );
        let mut wav = hound::WavReader::open(path).unwrap();
        assert_eq!(wav.duration(), 2640);
        let actual = wav.samples::<f32>().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(actual, expected[i]);
    }
    assert!(
        expected[0][3000..].iter().any(|v| *v != 0.0),
        "reverb tail must continue"
    );
    let retained = Plan::from_json(&plan.to_json().unwrap()).unwrap();
    let replay_dir = tempdir().unwrap();
    let replay = deliver(
        &retained,
        &options(replay_dir.path()),
        &PlanLimits::default(),
    )
    .unwrap();
    assert_eq!(full.render_key, replay.render_key);
    for id in ["master", "stem"] {
        assert_eq!(full.targets[id].file_hash, replay.targets[id].file_hash);
    }
    let selected_dir = tempdir().unwrap();
    let mut selected = options(selected_dir.path());
    selected.targets = vec!["stem".into()];
    let solo = deliver(&plan, &selected, &PlanLimits::default()).unwrap();
    assert_eq!(
        solo.targets["stem"].file_hash,
        full.targets["stem"].file_hash
    );
    assert!(!selected_dir.path().join("release.master.wav").exists());
    assert_ne!(solo.render_key, full.render_key);
}

#[test]
fn final_pcm24_dither_is_metered_from_bytes_at_delivery_rate() {
    let text = source(44100, "", "").replace(
        "encoding=wav_f32le;dither={type=none;}",
        "encoding=wav_pcm24le;dither={type=tpdf;seed=7;}",
    );
    let plan = plan(&text);
    let directory = tempdir().unwrap();
    let report = deliver(&plan, &options(directory.path()), &PlanLimits::default()).unwrap();
    assert!(report.ok);
    assert_eq!(report.frames, 2426);
    for id in ["master", "stem"] {
        let path = directory.path().join(format!("release.{id}.wav"));
        let wav = hound::WavReader::open(&path).unwrap();
        assert_eq!(wav.spec().bits_per_sample, 24);
        assert_eq!(wav.spec().sample_rate, 44100);
        assert_eq!(wav.duration(), 2426);
        let measured = maac::production_analysis::analyze_wav(&path).unwrap();
        assert_eq!(
            serde_json::to_value(&measured).unwrap(),
            serde_json::to_value(report.targets[id].measurements.as_ref().unwrap()).unwrap()
        );
        assert_eq!(
            report.targets[id].file_hash.as_ref().unwrap(),
            &sha256_digest(&fs::read(path).unwrap())
        );
    }
}

#[test]
fn delivery_length_uses_exact_duration_and_pcm24_odd_data_is_padded() {
    let source = source(96000, "", "")
        .lines()
        .filter(|line| {
            !["pattern ", "track ", "place "]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("score=[0q,1/100q]", "score=[0q,2/96001q]")
        .replace("tail=1/20s", "tail=0s")
        .replace("encoding=wav_f32le", "encoding=wav_pcm24le");
    let plan = plan(&source);
    let directory = tempdir().unwrap();
    let report = deliver(&plan, &options(directory.path()), &PlanLimits::default()).unwrap();
    assert_eq!(report.engine_frames, 1);
    assert_eq!(
        report.frames, 1,
        "rescaling the rounded engine count would incorrectly yield two frames"
    );
    let bytes = fs::read(directory.path().join("release.stem.wav")).unwrap();
    assert_eq!(bytes.len(), 48); // 44-byte PCM header, three data bytes, one pad.
    assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 3);
    assert_eq!(
        hound::WavReader::new(bytes.as_slice()).unwrap().duration(),
        1
    );
    assert_eq!(
        report.targets["stem"].pcm_hash.as_ref().unwrap(),
        &sha256_digest(&[0, 0, 0])
    );
}

#[test]
fn failed_loudness_checks_retain_complete_audio_and_manifest() {
    let plan = plan(&source(
        48000,
        "limits={integrated_loudness={unit=LUFS;min=-20;};};",
        "",
    ));
    let directory = tempdir().unwrap();
    let report = deliver(&plan, &options(directory.path()), &PlanLimits::default()).unwrap();
    assert!(!report.ok);
    assert_eq!(report.artifact_status, "complete");
    assert_eq!(report.manifest_status, "complete");
    assert_eq!(report.check_status, "fail");
    assert_eq!(report.targets["master"].checks[0].reason, "unmeasurable");
    assert!(directory.path().join("release.master.wav").is_file());
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("release.manifest.json")).unwrap())
            .unwrap();
    assert_eq!(stored["targets"]["master"]["artifact_status"], "complete");
    assert_eq!(stored["check_status"], "fail");
}

#[test]
fn failed_integer_target_does_not_delete_completed_master() {
    let plan = plan(&source(48000, "", "z_overload={role=stem;output=&boost:out;encoding=wav_pcm16le;dither={type=none;};limits={sample_peak={unit=dBFS;max=-1;};};};"));
    let directory = tempdir().unwrap();
    let report = deliver(&plan, &options(directory.path()), &PlanLimits::default()).unwrap();
    assert!(!report.ok);
    assert_eq!(report.manifest_status, "partial");
    assert_eq!(report.artifact_status, "partial");
    assert_eq!(report.targets["z_overload"].artifact_status, "failed");
    assert_eq!(report.targets["z_overload"].check_status, "fail");
    assert_eq!(
        report.targets["z_overload"].error.as_ref().unwrap().code,
        "E_OVERLOAD"
    );
    assert!(directory.path().join("release.master.wav").is_file());
    assert!(!directory.path().join("release.z_overload.wav").exists());
    assert!(directory.path().join("release.manifest.json").is_file());
}

#[test]
fn destinations_and_aggregate_budgets_fail_before_rendering() {
    let plan = plan(&source(48000, "", ""));
    let directory = tempdir().unwrap();
    let output = directory.path().join("out");
    let mut opts = options(&output);
    opts.limits.max_spool_bytes = 0;
    assert_eq!(
        deliver(&plan, &opts, &PlanLimits::default())
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
    assert!(!output.exists());
    opts.limits = DeliveryLimits::default();
    fs::create_dir(&output).unwrap();
    fs::write(output.join("release.manifest.json"), b"existing manifest").unwrap();
    assert_eq!(
        deliver(&plan, &opts, &PlanLimits::default())
            .unwrap_err()
            .code,
        "E_OUTPUT_EXISTS"
    );
    assert_eq!(
        fs::read(output.join("release.manifest.json")).unwrap(),
        b"existing manifest"
    );
    assert!(!output.join("release.master.wav").exists());
    opts.overwrite = true;
    assert!(deliver(&plan, &opts, &PlanLimits::default()).unwrap().ok);
    opts.targets = vec!["../escape".into()];
    assert_eq!(
        deliver(&plan, &opts, &PlanLimits::default())
            .unwrap_err()
            .code,
        "E_REFERENCE"
    );
}

#[test]
fn long_and_case_distinct_ids_get_bounded_selection_independent_filenames() {
    let delivery_id = "d".repeat(128);
    let target_id = "t".repeat(128);
    let text = source(48000, "", "")
        .replace("release={", &format!("{delivery_id}={{"))
        .replace(
            "master={role=master;",
            &format!("{target_id}={{role=master;"),
        );
    let long_plan = plan(&text);
    let directory = tempdir().unwrap();
    let mut opts = options(directory.path());
    opts.delivery_id = delivery_id;
    let report = deliver(&long_plan, &opts, &PlanLimits::default()).unwrap();
    assert!(report.ok);
    let filename = &report.targets[&target_id].filename;
    assert!(filename.len() <= 255);
    assert_eq!(filename.len(), 230);
    assert!(directory.path().join(filename).is_file());

    let case_plan = plan(&source(
        48000,
        "",
        "MASTER={role=stem;output=&compressed:out;encoding=wav_pcm16le;dither={type=none;};};",
    ));
    let full_dir = tempdir().unwrap();
    let mut opts = options(full_dir.path());
    opts.overwrite = true;
    let full = deliver(&case_plan, &opts, &PlanLimits::default()).unwrap();
    assert!(full.ok);
    assert_ne!(
        full.targets["master"].filename.to_ascii_lowercase(),
        full.targets["MASTER"].filename.to_ascii_lowercase()
    );
    for id in ["master", "MASTER"] {
        let artifact = &full.targets[id];
        assert_eq!(
            artifact.file_hash.as_ref().unwrap(),
            &sha256_digest(&fs::read(full_dir.path().join(&artifact.filename)).unwrap())
        );
    }
    let solo_dir = tempdir().unwrap();
    opts.output_dir = solo_dir.path().into();
    opts.targets = vec!["MASTER".into()];
    let solo = deliver(&case_plan, &opts, &PlanLimits::default()).unwrap();
    assert_eq!(
        solo.targets["MASTER"].filename,
        full.targets["MASTER"].filename
    );
    assert_eq!(
        solo.targets["MASTER"].file_hash,
        full.targets["MASTER"].file_hash
    );

    let uppercase_plan = plan(&source(48000, "", "").replace("release={", "RELEASE={"));
    let directory = tempdir().unwrap();
    let mut opts = options(directory.path());
    opts.delivery_id = "RELEASE".into();
    let uppercase = deliver(&uppercase_plan, &opts, &PlanLimits::default()).unwrap();
    assert_ne!(
        uppercase.manifest_file.to_ascii_lowercase(),
        "release.manifest.json"
    );
    assert!(directory.path().join(&uppercase.manifest_file).is_file());
}

#[cfg(unix)]
#[test]
fn dangling_destination_symlinks_are_protected() {
    let plan = plan(&source(48000, "", ""));
    let directory = tempdir().unwrap();
    let destination = directory.path().join("release.master.wav");
    std::os::unix::fs::symlink("missing.wav", &destination).unwrap();
    assert_eq!(
        deliver(&plan, &options(directory.path()), &PlanLimits::default())
            .unwrap_err()
            .code,
        "E_OUTPUT_EXISTS"
    );
    assert_eq!(
        fs::read_link(destination).unwrap(),
        std::path::PathBuf::from("missing.wav")
    );
}
