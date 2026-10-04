//! `maac analyze`: measurements, findings, sections and images on synthesized
//! fixtures whose answers are known in advance.

use maac::analyze::{analyze_artifact, AnalysisReport, AnalyzeOptions, SectionMode};
use maac::compiler::compile_bundle_artifact;
use maac::plan::PlanLimits;
use maac::{PlanArtifact, SourceBundle};

/// Two 4/4 bars at 480 bpm (one second), one A4 note through `chain`, which
/// routes `&s:out` to the project output `&out:out`.
fn source(chain: &str, extra: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 480bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node s {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = 0.5; }}; }}
{chain}
pattern tone {{ length = 8q; note a {{ at = 0q; dur = 8q; pitch = A4; }} }}
track melody {{ target = &s:events; }}
place play {{ pattern = &tone; track = &melody; at = 0q; }}
{extra}
"#
    )
}

const GAIN: &str = r#"node out { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 1; }; }
connect route { from = &s:out; to = &out:in; }"#;

fn plan(source: &str) -> PlanArtifact {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("fixture compiles: {e}"))
}

fn analyze(source: &str, options: &AnalyzeOptions) -> AnalysisReport {
    let meter = maac::analyze::meter_of(&maac::parse(source).unwrap());
    analyze_artifact(
        &plan(source),
        meter.as_ref(),
        options,
        &PlanLimits::default(),
    )
    .unwrap()
}

fn kinds(report: &AnalysisReport) -> Vec<(&str, &str)> {
    report
        .findings
        .iter()
        .map(|finding| (finding.kind, finding.source.as_str()))
        .collect()
}

#[test]
fn a_sine_measures_at_its_level_and_octave() {
    let report = analyze(&source(GAIN, ""), &AnalyzeOptions::default());
    let output = &report.sources[0];
    assert_eq!(output.port, "out:out");
    assert_eq!(output.role, "output");
    let whole = &output.whole;
    // Amplitude 0.5: peak -6.02 dBFS, RMS 3.01 dB lower.
    let peak = whole.sample_peak_dbfs.unwrap();
    assert!((peak + 6.02).abs() < 0.05, "{peak}");
    assert!(
        (whole.crest_db.unwrap() - 3.01).abs() < 0.05,
        "{:?}",
        whole.crest_db
    );
    // K-weighting is close to 0 dB at 440 Hz.
    let loudness = whole.integrated_lufs.unwrap();
    assert!((loudness - (-0.691 - 9.03)).abs() < 0.3, "{loudness}");
    // 440 Hz falls in the 500 Hz octave.
    let strongest = (0..10)
        .max_by(|&a, &b| {
            whole.bands_db[a]
                .unwrap_or(-999.0)
                .total_cmp(&whole.bands_db[b].unwrap_or(-999.0))
        })
        .unwrap();
    assert_eq!(report.band_centers_hz[strongest], 500.0);
    assert_eq!(whole.clipped_samples, 0);
    assert!(whole.stereo.is_none());
    assert!(report.findings.is_empty(), "{:?}", kinds(&report));
}

#[test]
fn a_panned_source_is_reported_as_imbalanced() {
    let chain = r#"node out { type = "core.pan/1"; params = { pan = -0.5; }; }
connect route { from = &s:out; to = &out:in; }"#;
    let report = analyze(&source(chain, ""), &AnalyzeOptions::default());
    let stereo = report.sources[0].whole.stereo.as_ref().unwrap();
    // Equal-power pan at -0.5: cos(pi/8) against sin(pi/8).
    assert!(
        (stereo.balance_db.unwrap() - 7.66).abs() < 0.05,
        "{stereo:?}"
    );
    assert!(kinds(&report).contains(&("stereo_imbalance", "out:out")));
}

#[test]
fn an_inverted_pair_cancels_in_mono() {
    let chain = r#"node out { type = "core.matrix/1"; config = { inputs = 1; outputs = 2; coefficients = [[1], [-1]]; }; }
connect route { from = &s:out; to = &out:in; }"#;
    let report = analyze(&source(chain, ""), &AnalyzeOptions::default());
    let stereo = report.sources[0].whole.stereo.as_ref().unwrap();
    assert_eq!(stereo.correlation, Some(-1.0));
    assert!(stereo.mono_loss_db.unwrap() < -100.0, "{stereo:?}");
    assert!(kinds(&report).contains(&("mono_cancellation", "out:out")));
}

#[test]
fn clipping_is_found_at_its_bar() {
    // The note starts in bar 2 and is boosted past full scale.
    let source = source(GAIN, "")
        .replace("gain = 1;", "gain = 4;")
        .replace("at = 0q; dur = 8q;", "at = 4q; dur = 4q;");
    let report = analyze(&source, &AnalyzeOptions::default());
    let clipping = report
        .findings
        .iter()
        .find(|finding| finding.kind == "clipping")
        .expect("a clipping finding");
    assert_eq!(clipping.severity, "error");
    let at = clipping.at.as_ref().unwrap();
    assert_eq!(at.bar, Some(2));
    assert_eq!(at.q, "4");
    assert!(report.sources[0].whole.clipped_samples > 0);
    assert!(kinds(&report).contains(&("true_peak_over", "out:out")));
}

#[test]
fn regions_and_bar_blocks_become_sections() {
    let regions = analyze(
        &source(
            GAIN,
            "region verse { span = [0q, 4q]; label = \"Verse\"; }\nregion chorus { span = [4q, 8q]; }",
        ),
        &AnalyzeOptions::default(),
    );
    let ids: Vec<&str> = regions.sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["verse", "chorus"]);
    assert_eq!(regions.sections[0].label.as_deref(), Some("Verse"));
    assert_eq!(regions.sections[1].start_bar, Some(2));
    assert_eq!(regions.sections[1].start_frame, 24_000);
    assert_eq!(regions.sources[0].sections.len(), 2);

    let bars = analyze(
        &source(GAIN, ""),
        &AnalyzeOptions {
            sections: SectionMode::Bars(1),
            ..AnalyzeOptions::default()
        },
    );
    let ids: Vec<&str> = bars.sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["bars-1-1", "bars-2-2"]);
    let whole = analyze(
        &source(GAIN, ""),
        &AnalyzeOptions {
            sections: SectionMode::Whole,
            ..AnalyzeOptions::default()
        },
    );
    assert!(whole.sections.is_empty() && whole.sources[0].sections.is_empty());
}

#[test]
fn sources_reaching_the_output_are_found_and_silent_ones_reported() {
    let chain = r#"node quiet { type = "core.sine/1"; params = { level = 0; }; }
node out { type = "core.sum/1"; config = { channels = 1; }; }
connect a { from = &s:out; to = &out:in; }
connect b { from = &quiet:out; to = &out:in; }"#;
    let source = source(chain, "track hush { target = &quiet:events; }\nplace silent { pattern = &tone; track = &hush; at = 0q; }");
    let report = analyze(&source, &AnalyzeOptions::default());
    let ports: Vec<(&str, &str)> = report
        .sources
        .iter()
        .map(|source| (source.port.as_str(), source.role))
        .collect();
    assert_eq!(
        ports,
        [
            ("out:out", "output"),
            ("quiet:out", "stem"),
            ("s:out", "stem")
        ]
    );
    // Nothing below the -70 LUFS absolute floor is reported.
    for source in &report.sources {
        for section in &source.sections {
            for value in [
                section.measures.max_momentary_lufs,
                section.measures.max_short_term_lufs,
            ]
            .into_iter()
            .flatten()
            {
                assert!(value > -70.0, "{} {value}", source.port);
            }
        }
    }
    assert!(kinds(&report).contains(&("silent_source", "quiet:out")));
    let share = report.sources[2].whole.master_share.as_ref().unwrap();
    assert_eq!(share[4], Some(1.0));

    let explicit = analyze(
        &source,
        &AnalyzeOptions {
            sources: vec![maac::plan::PortRef::new("s", "out").unwrap()],
            ..AnalyzeOptions::default()
        },
    );
    assert_eq!(explicit.sources.len(), 2);
}

#[test]
fn reports_and_images_are_deterministic() {
    let source = source(GAIN, "region verse { span = [0q, 4q]; }");
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let with_images = |dir: &std::path::Path| AnalyzeOptions {
        images: Some(dir.to_path_buf()),
        ..AnalyzeOptions::default()
    };
    let first = analyze(&source, &with_images(first_dir.path()));
    let second = analyze(&source, &with_images(second_dir.path()));
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    let files: Vec<&str> = first
        .images
        .iter()
        .map(|image| image.file.as_str())
        .collect();
    assert_eq!(
        files,
        [
            "spectrogram-out-out.png",
            "spectrogram-s-out.png",
            "piano-roll.png"
        ]
    );
    for image in &first.images {
        let bytes = std::fs::read(first_dir.path().join(&image.file)).unwrap();
        assert_eq!(
            bytes,
            std::fs::read(second_dir.path().join(&image.file)).unwrap()
        );
        assert_eq!(&bytes[1..4], b"PNG");
        assert_eq!(
            u32::from_be_bytes(bytes[16..20].try_into().unwrap()) as usize,
            image.width
        );
        assert_eq!(
            u32::from_be_bytes(bytes[20..24].try_into().unwrap()) as usize,
            image.height
        );
        assert_eq!(image.sections[0].x0, 0);
    }
    // The A4 note is the piano roll's only key.
    let roll = &first.images[2];
    assert_eq!((roll.min_key, roll.max_key), (Some(68), Some(70)));
    assert_eq!(roll.legend.len(), 1);
    assert_eq!(roll.legend[0].target, "s");

    // Existing images are kept unless forced.
    let meter = maac::analyze::meter_of(&maac::parse(&source).unwrap());
    let refused = analyze_artifact(
        &plan(&source),
        meter.as_ref(),
        &with_images(first_dir.path()),
        &PlanLimits::default(),
    )
    .unwrap_err();
    assert_eq!(refused.code, "E_IO");
}

#[test]
fn the_cli_prints_the_report_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.maac");
    std::fs::write(&path, source(GAIN, "")).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["analyze", "--json", "--section", "bars:1"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["command"], "analyze");
    assert_eq!(json["analysis"]["schema"], "maac.analysis-report/1");
    assert_eq!(json["analysis"]["sections"].as_array().unwrap().len(), 2);

    let bad = std::process::Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["analyze", "--json", "--section", "bars:0"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!bad.status.success());
    let json: serde_json::Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(json["code"], "E_USAGE");
}

#[test]
fn a_generator_before_its_gain_is_a_chain_not_a_stem() {
    let chain = r#"node noise { type = "core.noise/1"; config = { channels = 1; }; }
node hiss { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 0.001; }; }
node out { type = "core.sum/1"; config = { channels = 1; }; }
connect a { from = &s:out; to = &out:in; }
connect b { from = &noise:out; to = &hiss:in; }
connect c { from = &hiss:out; to = &out:in; }"#;
    let report = analyze(&source(chain, ""), &AnalyzeOptions::default());
    let roles: Vec<(&str, &str)> = report
        .sources
        .iter()
        .map(|source| (source.port.as_str(), source.role))
        .collect();
    assert_eq!(
        roles,
        [
            ("out:out", "output"),
            ("hiss:out", "stem"),
            ("noise:out", "chain"),
            ("s:out", "stem")
        ]
    );
    assert!(
        report.findings.iter().all(|f| f.source != "noise:out"),
        "{:?}",
        kinds(&report)
    );
}

#[test]
fn panned_stems_are_choices_but_an_unbalanced_mix_is_not() {
    let chain = r#"node t { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.5; }; }
node left { type = "core.pan/1"; params = { pan = -0.8; }; }
node right { type = "core.pan/1"; params = { pan = 0.8; }; }
node out { type = "core.sum/1"; config = { channels = 2; }; }
connect a { from = &s:out; to = &left:in; }
connect b { from = &t:out; to = &right:in; }
connect c { from = &left:out; to = &out:in; }
connect d { from = &right:out; to = &out:in; }"#;
    let extra = "track other { target = &t:events; }\nplace again { pattern = &tone; track = &other; at = 0q; }";
    let report = analyze(&source(chain, extra), &AnalyzeOptions::default());
    assert!(
        report.findings.iter().all(|f| f.kind != "stereo_imbalance"),
        "{:?}",
        kinds(&report)
    );
    // With only the left source playing, the mix itself is unbalanced.
    let lopsided = source(chain, "");
    let report = analyze(&lopsided, &AnalyzeOptions::default());
    assert_eq!(
        report
            .findings
            .iter()
            .filter(|f| f.kind == "stereo_imbalance")
            .map(|f| f.source.as_str())
            .collect::<Vec<_>>(),
        ["out:out"]
    );
}

/// Two parts: `s` plays `s_beats`, `t` plays `t_beats`, eighth-note gates.
fn groove(s_beats: &str, t_beats: &str) -> String {
    let notes = |beats: &str, prefix: &str| {
        beats
            .split(',')
            .enumerate()
            .map(|(i, at)| {
                format!(
                    "note {prefix}{i} {{ at = {at}q; dur = 1/6q; pitch = A4; velocity = 0.5; }}\n"
                )
            })
            .collect::<String>()
    };
    let chain = r#"node t { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.5; }; }
node out { type = "core.sum/1"; config = { channels = 1; }; }
connect a { from = &s:out; to = &out:in; }
connect b { from = &t:out; to = &out:in; }"#;
    source(chain, "")
        .replace(
            "pattern tone { length = 8q; note a { at = 0q; dur = 8q; pitch = A4; } }",
            &format!(
                "pattern tone {{ length = 8q; {} }}\npattern other {{ length = 8q; {} }}",
                notes(s_beats, "s"),
                notes(t_beats, "t")
            ),
        )
        .replace(
            "place play {",
            "track second { target = &t:events; }\nplace both { pattern = &other; track = &second; at = 0q; }\nplace play {",
        )
}

#[test]
fn mixed_swing_is_reported_and_one_grid_is_not() {
    // `s` swings its off-beat eighths; `t` plays them straight.
    let mixed = groove("0,2/3,1,5/3,2,8/3,3,11/3", "1/2,3/2,5/2,7/2");
    let dir = tempfile::tempdir().unwrap();
    let report = analyze(
        &mixed,
        &AnalyzeOptions {
            images: Some(dir.path().to_path_buf()),
            ..AnalyzeOptions::default()
        },
    );
    let finding = report
        .findings
        .iter()
        .find(|f| f.kind == "groove_mismatch")
        .expect("a groove mismatch");
    assert_eq!(finding.severity, "warning");
    // The part on the grid with fewer onsets is named; on a tie, the swung one.
    assert_eq!(finding.source, "s:events");
    assert!(finding.message.contains("`s` (4)") && finding.message.contains("`t` (4)"));
    // Both parts strike one pitch, so the piano roll draws them in lanes.
    let roll = report
        .images
        .iter()
        .find(|i| i.kind == "piano_roll")
        .unwrap();
    assert_eq!((roll.min_key, roll.max_key), (None, None));

    let swung = groove("0,2/3,1,5/3,2,8/3,3,11/3", "2/3,5/3,8/3,11/3");
    let report = analyze(&swung, &AnalyzeOptions::default());
    assert!(report.findings.iter().all(|f| f.kind != "groove_mismatch"));
}
