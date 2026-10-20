//! `std/orchestra/1.0.0`: frozen bytes, catalog, full-range rendering,
//! velocity response, calibrated levels and connected lines.

use std::collections::BTreeSet;

use maac::analyze::{analyze_artifact, AnalyzeOptions, SectionMode, SourceReport};
use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::stdlib::{self, ORCHESTRA_ID, ORCHESTRA_SOURCE};
use maac::{render_artifact_with_limits, PlanArtifact};

const INSTRUMENTS: [&str; 5] = ["violins", "violas", "cellos", "basses", "concert_harp"];
const STRINGS: [&str; 4] = ["violins", "violas", "cellos", "basses"];

fn composition(quarters: &str, nodes: &str, body: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, {quarters}q]; rate = 48000Hz; tail = 3s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 76bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import orchestra {{ builtin = "std/orchestra/1.0.0"; }}
{nodes}
track t {{ target = &out:events; }}
pattern phrase {{ length = {quarters}q;
{body}
}}
place once {{ pattern = &phrase; track = &t; at = 0q; }}
"#
    )
}

fn plan(source: &str) -> PlanArtifact {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source),
        &PlanLimits::song(),
    )
    .unwrap_or_else(|error| panic!("{error:?}\n{source}"))
}

fn analysis(source: &str) -> SourceReport {
    let report = analyze_artifact(
        &plan(source),
        None,
        &AnalyzeOptions {
            sections: SectionMode::Whole,
            ..AnalyzeOptions::default()
        },
        &PlanLimits::song(),
    )
    .unwrap();
    report.sources.into_iter().next().unwrap()
}

fn render(plan: &PlanArtifact) -> Vec<f64> {
    let mut samples = Vec::new();
    render_artifact_with_limits(plan, &PlanLimits::song(), |frame| {
        samples.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    samples
}

fn pitch(midi: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(midi % 12)],
        i32::from(midi / 12) - 1
    )
}

#[test]
fn published_version_keeps_its_exact_source_bytes() {
    assert_eq!(
        sha256_digest(ORCHESTRA_SOURCE.as_bytes()),
        include_str!("../stdlib/orchestra/1.0.0.sha256").trim()
    );
}

#[test]
fn catalog_lists_the_exports_and_every_usage_compiles() {
    let catalog = stdlib::catalog_for(ORCHESTRA_ID).unwrap();
    let names: BTreeSet<_> = catalog
        .instruments
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(names, INSTRUMENTS.into_iter().collect());
    assert!(catalog.kits.is_empty());
    for item in &catalog.instruments {
        assert!(item.usage.contains(ORCHESTRA_ID));
        let audio = render(&plan(&item.usage));
        assert!(audio.iter().any(|sample| *sample != 0.0), "{}", item.name);
    }
}

/// Every semitone of the catalog range at velocities 0.2, 0.5 and 1, one
/// short stroke per half note, with rests between so none connect.
fn range_source(export: &str, low: u8, high: u8) -> String {
    let mut notes = Vec::new();
    for midi in low..=high {
        for velocity in ["0.2", "0.5", "1"] {
            let index = notes.len();
            notes.push(format!(
                "note n{index} {{ at = {index}/2q; dur = 1/4q; pitch = {}; velocity = {velocity}; }}",
                pitch(midi)
            ));
        }
    }
    composition(
        &format!("{}/2", notes.len()),
        &format!(
            "node out {{ instrument = &orchestra.{export}; config = {{ voices = 64; }}; params = {{ release = 300ms; }}; }}"
        ),
        &notes.join("\n"),
    )
}

#[test]
fn every_export_renders_its_catalog_range_at_three_velocities() {
    for export in INSTRUMENTS {
        let guidance = stdlib::instrument_in(ORCHESTRA_ID, export)
            .unwrap()
            .guidance;
        let tested: Vec<_> = (guidance.midi_min..=guidance.midi_max).map(pitch).collect();
        assert_eq!(guidance.tested_pitches, tested, "{export}");
        let source = range_source(export, guidance.midi_min, guidance.midi_max);
        // The harp has no release control: its 8 s ring needs the voices.
        let source = if export == "concert_harp" {
            source.replace(" params = { release = 300ms; };", "")
        } else {
            source
        };
        let samples = render(&plan(&source));
        assert!(samples.iter().all(|sample| sample.is_finite()), "{export}");
        let peak = samples
            .iter()
            .fold(0.0f64, |peak, sample| peak.max(sample.abs()));
        assert!(peak > 0.0 && peak < 1.0, "{export} peak {peak}");
    }
}

/// The spectral centre of a source's octave bands, in octaves.
fn brightness(source: &str) -> f64 {
    let whole = analysis(source).whole;
    let shares: Vec<f64> = whole
        .bands_db
        .iter()
        .map(|db| db.map_or(0.0, |db| 10f64.powf(db / 10.0)))
        .collect();
    let total: f64 = shares.iter().sum();
    shares
        .iter()
        .enumerate()
        .map(|(octave, share)| octave as f64 * share)
        .sum::<f64>()
        / total
}

#[test]
fn harder_strokes_are_louder_and_harder_plucks_brighter() {
    let note = |export: &str, pitch: &str, velocity: &str| {
        composition(
            "4",
            &format!("node out {{ instrument = &orchestra.{export}; }}"),
            &format!("note n {{ at = 0q; dur = 2q; pitch = {pitch}; velocity = {velocity}; }}"),
        )
    };
    // A harder stroke moves the bow faster and presses it harder: louder,
    // with the tone nearly unchanged.
    for (export, pitch) in [
        ("violins", "A5"),
        ("violas", "D4"),
        ("cellos", "D3"),
        ("basses", "D2"),
    ] {
        let level = |velocity: &str| {
            analysis(&note(export, pitch, velocity))
                .whole
                .integrated_lufs
                .unwrap()
        };
        let (soft, hard) = (level("0.3"), level("1"));
        assert!(
            hard > soft + 6.0,
            "{export}: soft {soft:.1}, hard {hard:.1} LUFS"
        );
    }
    let (soft, hard) = (
        brightness(&note("concert_harp", "D4", "0.3")),
        brightness(&note("concert_harp", "D4", "1")),
    );
    assert!(hard > soft, "concert_harp: soft {soft:.4}, hard {hard:.4}");
}

/// Four bars of each export's reference phrase at velocity 0.7.
fn reference(export: &str) -> String {
    let body = match export {
        "violins" => [
            ("0", "2", "D5", ""),
            ("2", "2", "A5", ""),
            ("4", "3/2", "Bb5", ""),
            ("11/2", "1/2", "A5", " slur = true;"),
            ("6", "1", "G5", ""),
            ("7", "1", "F5", " slur = true;"),
            ("8", "3", "A5", ""),
            ("11", "1/2", "F5", ""),
            ("23/2", "1/2", "G5", " slur = true;"),
            ("12", "2", "G5", ""),
            ("14", "1", "E5", ""),
            ("15", "1", "C5", " slur = true;"),
        ]
        .iter()
        .enumerate()
        .map(|(index, (at, dur, pitch, extra))| {
            format!("note m{index} {{ at = {at}q; dur = {dur}q; pitch = {pitch}; velocity = 0.7;{extra} }}")
        })
        .collect::<Vec<_>>()
        .join("\n"),
        "violas" => [["A3", "D4", "F4"], ["Bb3", "D4", "F4"], ["A3", "C4", "F4"], ["G3", "C4", "E4"]]
            .iter()
            .enumerate()
            .map(|(bar, pitches)| {
                format!(
                    "chord c{bar} {{ at = {}q; dur = 4q; pitches = [{}]; velocity = 0.7; }}",
                    bar * 4,
                    pitches.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "cellos" | "basses" => {
            let pitches = if export == "cellos" {
                ["D3", "Bb2", "F3", "C3"]
            } else {
                ["D2", "Bb1", "F2", "C2"]
            };
            pitches
                .iter()
                .enumerate()
                .map(|(bar, pitch)| {
                    format!(
                        "note c{bar} {{ at = {}q; dur = 4q; pitch = {pitch}; velocity = 0.7; }}",
                        bar * 4
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => ["D3", "A3", "D4", "F4", "A4", "F4", "D4", "A3"]
            .iter()
            .cycle()
            .take(32)
            .enumerate()
            .map(|(index, pitch)| {
                format!("note h{index} {{ at = {index}/2q; dur = 1/2q; pitch = {pitch}; velocity = 0.7; }}")
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let voices = if export == "concert_harp" { 64 } else { 12 };
    composition(
        "16",
        &format!(
            "node out {{ instrument = &orchestra.{export}; config = {{ voices = {voices}; }}; }}"
        ),
        &body,
    )
}

#[test]
fn reference_phrases_measure_minus_20_lufs_at_velocity_0_7() {
    for export in INSTRUMENTS {
        let whole = analysis(&reference(export)).whole;
        let loudness = whole.integrated_lufs.unwrap();
        assert!((loudness + 20.0).abs() <= 1.0, "{export}: {loudness} LUFS");
        assert!(
            whole.true_peak_dbtp.unwrap() < -1.0,
            "{export}: {:?}",
            whole.true_peak_dbtp
        );
    }
}

#[test]
fn every_string_section_plays_connected_lines() {
    for export in STRINGS {
        let source = composition(
            "4",
            &format!("node out {{ instrument = &orchestra.{export}; }}"),
            "note a { at = 0q; dur = 2q; pitch = D4; velocity = 0.6; }\nnote b { at = 2q; dur = 2q; pitch = E4; velocity = 0.6; slur = true; slide = 80ms; }",
        );
        let wire: serde_json::Value =
            serde_json::from_slice(&plan(&source).to_json().unwrap()).unwrap();
        assert_eq!(wire["events"][1]["join"]["from"], "once/0/a", "{export}");
    }
}
