//! `std/studio/1.0.0`: frozen bytes, catalog, full-range rendering, velocity
//! response and calibrated levels.

use std::collections::BTreeSet;

use maac::analyze::{analyze_artifact, AnalyzeOptions, SectionMode, SourceReport};
use maac::bundle::{sha256_digest, SourceBundle};
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::stdlib::{self, STUDIO_ID, STUDIO_SOURCE};
use maac::{render_artifact, PlanArtifact};

const INSTRUMENTS: [&str; 11] = [
    "electric_piano",
    "bass",
    "pad",
    "kick",
    "snare",
    "clap",
    "closed_hat",
    "open_hat",
    "low_tom",
    "high_tom",
    "crash",
];

const KEYS: [&str; 8] = [
    "kick", "snare", "clap", "hat", "open_hat", "low_tom", "high_tom", "crash",
];

fn composition(quarters: &str, nodes: &str, target: &str, body: &str, groove: bool) -> String {
    let groove_field = if groove { " groove = &lazy;" } else { "" };
    format!(
        r#"maac 1;
project p {{ score = [0q, {quarters}q]; rate = 48000Hz; tail = 2s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 90bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
groove lazy {{ grid = 1/2q; ratio = 2/3; }}
import studio {{ builtin = "std/studio/1.0.0"; }}
{nodes}
track t {{ target = &{target}:events; }}
pattern phrase {{ length = {quarters}q;
{body}
}}
place once {{ pattern = &phrase; track = &t; at = 0q;{groove_field} }}
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
    render_artifact(plan, |frame| {
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
        sha256_digest(STUDIO_SOURCE.as_bytes()),
        include_str!("../stdlib/studio/1.0.0.sha256").trim()
    );
}

#[test]
fn catalog_lists_the_exports_and_every_usage_compiles() {
    let catalog = stdlib::catalog_for(STUDIO_ID).unwrap();
    let names: BTreeSet<_> = catalog
        .instruments
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(names, INSTRUMENTS.into_iter().collect());
    assert_eq!(catalog.kits.len(), 1);
    let kit = &catalog.kits[0];
    assert_eq!(kit.name, "drums");
    let keys: BTreeSet<_> = kit.pieces.iter().map(|piece| piece.key.as_str()).collect();
    assert_eq!(keys, KEYS.into_iter().collect());
    let basic = stdlib::kit_in(stdlib::BASIC_1_1_ID, "drums").unwrap();
    for piece in &basic.pieces {
        let studio = kit
            .pieces
            .iter()
            .find(|candidate| candidate.key == piece.key)
            .unwrap();
        assert_eq!(studio.gate_seconds, piece.gate_seconds, "{}", piece.key);
        assert_eq!(studio.choke, piece.choke, "{}", piece.key);
    }
    for usage in catalog
        .instruments
        .iter()
        .map(|item| &item.usage)
        .chain([&kit.usage])
    {
        assert!(usage.contains(STUDIO_ID));
        // The kit's usage plays hits, which need the artifact compiler.
        let audio = render(&plan(usage));
        assert!(audio.iter().any(|sample| *sample != 0.0), "{usage}");
    }
}

/// Every semitone of the catalog range at velocities 0.2, 0.5 and 1, one
/// note per eighth.
fn range_source(export: &str, low: u8, high: u8, params: &str) -> String {
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
        &format!("node out {{ instrument = &studio.{export}; config = {{ voices = 32; }}; params = {{ {params} }}; }}"),
        "out",
        &notes.join("\n"),
        false,
    )
}

#[test]
fn every_pitched_export_renders_its_catalog_range_at_three_velocities() {
    for (export, params) in [
        ("electric_piano", ""),
        ("bass", ""),
        // A short release keeps the overlapping voices in the test bounded.
        ("pad", "attack = 50ms; release = 200ms;"),
    ] {
        let guidance = stdlib::instrument_in(STUDIO_ID, export).unwrap().guidance;
        let tested: Vec<_> = (guidance.midi_min..=guidance.midi_max).map(pitch).collect();
        assert_eq!(guidance.tested_pitches, tested, "{export}");
        let plan = plan(&range_source(
            export,
            guidance.midi_min,
            guidance.midi_max,
            params,
        ));
        let mut peak: f64 = 0.0;
        render_artifact(&plan, |frame| {
            for sample in frame {
                assert!(sample.is_finite());
                peak = peak.max(sample.abs());
            }
            Ok(())
        })
        .unwrap();
        assert!(peak > 0.0 && peak < 1.0, "{export} peak {peak}");
    }
}

#[test]
fn every_drum_renders_at_its_tested_pitches() {
    for export in &INSTRUMENTS[3..] {
        let tested = stdlib::instrument_in(STUDIO_ID, export)
            .unwrap()
            .guidance
            .tested_pitches;
        let notes: Vec<_> = tested
            .iter()
            .enumerate()
            .map(|(index, pitch)| {
                format!(
                    "note n{index} {{ at = {index}q; dur = 1/8q; pitch = {pitch}; velocity = 1; }}"
                )
            })
            .collect();
        let source = composition(
            &tested.len().to_string(),
            &format!("node out {{ instrument = &studio.{export}; }}"),
            "out",
            &notes.join("\n"),
            false,
        );
        let whole = analysis(&source).whole;
        assert_eq!(whole.clipped_samples, 0, "{export}");
    }
}

/// The band-weighted mean octave of the analyzer's octave-band shares: one
/// unit higher means the energy sits an octave higher.
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
fn harder_notes_and_hits_are_brighter_in_every_export() {
    for (export, pitch) in [("electric_piano", "C4"), ("bass", "E2"), ("pad", "C4")] {
        let at = |velocity: &str| {
            brightness(&composition(
                "4",
                &format!("node out {{ instrument = &studio.{export}; }}"),
                "out",
                &format!("note n {{ at = 0q; dur = 2q; pitch = {pitch}; velocity = {velocity}; }}"),
                false,
            ))
        };
        let (soft, hard) = (at("0.3"), at("1"));
        assert!(hard > soft, "{export}: soft {soft:.4}, hard {hard:.4}");
    }
    for key in KEYS {
        let at = |velocity: &str| {
            brightness(&composition(
                "2",
                "node out { instrument = &studio.drums; }",
                "out",
                &format!(r#"hit h {{ at = 0q; key = "{key}"; velocity = {velocity}; }}"#),
                false,
            ))
        };
        let (soft, hard) = (at("0.3"), at("1"));
        assert!(hard > soft, "{key}: soft {soft:.4}, hard {hard:.4}");
    }
}

const COMPING: [[&str; 4]; 4] = [
    ["A3", "C4", "E4", "G4"],
    ["G3", "B3", "D4", "E4"],
    ["F3", "A3", "C4", "E4"],
    ["E3", "G3", "B3", "D4"],
];

const BASS_LINE: [(&str, &str, &str); 12] = [
    ("0", "19/16", "F2"),
    ("5/2", "3/4", "C3"),
    ("7/2", "1/2", "F2"),
    ("4", "19/16", "E2"),
    ("13/2", "3/4", "B2"),
    ("15/2", "1/2", "E2"),
    ("8", "19/16", "D2"),
    ("21/2", "3/4", "A2"),
    ("23/2", "1/2", "D2"),
    ("12", "19/16", "C2"),
    ("29/2", "3/4", "G2"),
    ("31/2", "1/2", "E2"),
];

const PAD_CHORDS: [[&str; 3]; 4] = [
    ["D3", "F3", "A3"],
    ["C3", "E3", "G3"],
    ["Bb2", "D3", "F3"],
    ["Bb2", "C3", "F3"],
];

/// The "Late Window" parts the calibration uses, every velocity 0.7.
fn reference(export: &str) -> String {
    let melodic = |body: Vec<String>| {
        composition(
            "16",
            &format!("node out {{ instrument = &studio.{export}; config = {{ voices = 24; }}; }}"),
            "out",
            &body.join("\n"),
            true,
        )
    };
    match export {
        "electric_piano" => melodic(
            COMPING
                .iter()
                .enumerate()
                .flat_map(|(bar, pitches)| {
                    let joined = pitches.join(", ");
                    [
                        format!("chord d{bar} {{ at = {}q; dur = 13/10q; pitches = [{joined}]; velocity = 0.7; }}", bar * 4),
                        format!("chord p{bar} {{ at = {}/2q; dur = 13/10q; pitches = [{joined}]; velocity = 0.7; }}", bar * 8 + 5),
                    ]
                })
                .collect(),
        ),
        "bass" => melodic(
            BASS_LINE
                .iter()
                .enumerate()
                .map(|(index, (at, dur, pitch))| {
                    format!("note n{index} {{ at = {at}q; dur = {dur}q; pitch = {pitch}; velocity = 0.7; }}")
                })
                .collect(),
        ),
        "pad" => melodic(
            PAD_CHORDS
                .iter()
                .enumerate()
                .map(|(bar, pitches)| {
                    format!(
                        "chord c{bar} {{ at = {}q; dur = 4q; pitches = [{}]; velocity = 0.7; }}",
                        bar * 4,
                        pitches.join(", ")
                    )
                })
                .collect(),
        ),
        _ => {
            // Four bars of the "Late Window" boom-bap beat.
            let mut hits = Vec::new();
            for bar in 0..4 {
                let base = bar * 4;
                for eighth in 0..8 {
                    hits.push(format!(r#"hit h{bar}_{eighth} {{ at = {}/2q; key = "hat"; velocity = 0.7; }}"#, base * 2 + eighth));
                }
                // Kicks on 1, the last sixteenth of 2 and the and of 3.
                for (index, quarter_fourths) in [0, 7, 10].iter().enumerate() {
                    hits.push(format!(
                        r#"hit k{bar}_{index} {{ at = {}/4q; key = "kick"; velocity = 0.7; }}"#,
                        base * 4 + quarter_fourths
                    ));
                }
                hits.push(format!(r#"hit s{bar}_0 {{ at = {}q; key = "snare"; velocity = 0.7; }}"#, base + 1));
                hits.push(format!(r#"hit s{bar}_1 {{ at = {}q; key = "snare"; velocity = 0.7; }}"#, base + 3));
            }
            composition("16", "node out { instrument = &studio.drums; }", "out", &hits.join("\n"), true)
        }
    }
}

#[test]
fn reference_phrases_measure_minus_20_lufs_at_velocity_0_7() {
    for export in ["electric_piano", "bass", "pad", "drums"] {
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
fn full_velocity_drums_peak_below_minus_one_dbtp() {
    for key in KEYS {
        let whole = analysis(&composition(
            "2",
            "node out { instrument = &studio.drums; }",
            "out",
            &format!(r#"hit h {{ at = 0q; key = "{key}"; velocity = 1; }}"#),
            false,
        ))
        .whole;
        let peak = whole.true_peak_dbtp.unwrap();
        assert!(peak < -1.0, "{key}: {peak} dBTP");
    }
}
