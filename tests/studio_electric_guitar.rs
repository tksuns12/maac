//! `std/studio/1.1.0`'s `electric_guitar`: full-range rendering, velocity
//! response, the calibrated level and the level held across `drive`.

use maac::analyze::{analyze_artifact, AnalyzeOptions, Measures, SectionMode};
use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::{render_artifact, PlanArtifact};

fn composition(quarters: &str, params: &str, body: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, {quarters}q]; rate = 48000Hz; tail = 2s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 72bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import studio {{ builtin = "std/studio/1.1.0"; }}
node out {{ instrument = &studio.electric_guitar; config = {{ voices = 24; }}; params = {{ {params} }}; }}
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

fn whole(source: &str) -> Measures {
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
    report.sources.into_iter().next().unwrap().whole
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
fn every_semitone_from_e2_to_e6_renders_at_three_velocities() {
    let mut notes = Vec::new();
    for midi in 40..=88 {
        for velocity in ["0.2", "0.5", "1"] {
            let index = notes.len();
            notes.push(format!(
                "note n{index} {{ at = {index}/2q; dur = 1/4q; pitch = {}; velocity = {velocity}; }}",
                pitch(midi)
            ));
        }
    }
    let source = composition(&format!("{}/2", notes.len()), "", &notes.join("\n"));
    let mut peak: f64 = 0.0;
    render_artifact(&plan(&source), |frame| {
        for sample in frame {
            assert!(sample.is_finite());
            peak = peak.max(sample.abs());
        }
        Ok(())
    })
    .unwrap();
    assert!(peak > 0.0 && peak < 1.0, "peak {peak}");
}

/// The band-weighted mean octave of the analyzer's octave-band shares.
fn brightness(source: &str) -> f64 {
    let shares: Vec<f64> = whole(source)
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
fn harder_picks_are_brighter() {
    for pitch in ["E2", "E3", "E4"] {
        let at = |velocity: &str| {
            brightness(&composition(
                "4",
                "",
                &format!("note n {{ at = 0q; dur = 2q; pitch = {pitch}; velocity = {velocity}; }}"),
            ))
        };
        let (soft, hard) = (at("0.3"), at("1"));
        assert!(hard > soft, "{pitch}: soft {soft:.4}, hard {hard:.4}");
    }
}

/// The "Morning Ferry" power-chord chorus, every chord at velocity 0.7: a G5
/// bar's rhythm on C5, G5, D5 and E5.
fn reference(params: &str) -> String {
    // Onsets in eighths from the bar start, and durations.
    const RHYTHM: [(usize, &str); 5] = [(0, "3/2"), (3, "1/2"), (4, "1"), (6, "1/2"), (7, "1/2")];
    const SHAPES: [[&str; 3]; 4] = [
        ["C3", "G3", "C4"],
        ["G2", "D3", "G3"],
        ["D3", "A3", "D4"],
        ["E2", "B2", "E3"],
    ];
    let mut chords = Vec::new();
    for (bar, shape) in SHAPES.iter().enumerate() {
        for (beat, (at, dur)) in RHYTHM.iter().enumerate() {
            chords.push(format!(
                "chord c{bar}_{beat} {{ at = {}/2q; dur = {dur}q; pitches = [{}]; velocity = 0.7; }}",
                bar * 8 + at,
                shape.join(", ")
            ));
        }
    }
    composition("16", params, &chords.join("\n"))
}

#[test]
fn the_reference_phrase_measures_minus_20_lufs_and_holds_across_drive() {
    let measures = whole(&reference(""));
    let loudness = measures.integrated_lufs.unwrap();
    assert!((loudness + 20.0).abs() <= 1.0, "{loudness} LUFS");
    assert!(measures.true_peak_dbtp.unwrap() < -1.0);
    let sweep: Vec<f64> = [1, 4, 16]
        .iter()
        .map(|drive| {
            whole(&reference(&format!("drive = {drive};")))
                .integrated_lufs
                .unwrap()
        })
        .collect();
    let span = sweep.iter().cloned().fold(f64::MIN, f64::max)
        - sweep.iter().cloned().fold(f64::MAX, f64::min);
    assert!(span <= 2.0, "{sweep:?}");
}
