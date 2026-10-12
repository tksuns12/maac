//! The draft `piano` in scripts/piano/piano.maac, before it joins
//! std/studio/1.1.0: full-range rendering, velocity response, pitch, the
//! calibrated level and the work a three-minute part costs.

use maac::analyze::{analyze_artifact, AnalysisReport, AnalyzeOptions, Measures, SectionMode};
use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::{render_artifact, PlanArtifact};

const DRAFT: &str = include_str!("../scripts/piano/piano.maac");

fn composition(quarters: &str, body: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, {quarters}q]; rate = 48000Hz; tail = 3s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 72bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
{DRAFT}
node out {{ instrument = &piano; config = {{ voices = 24; }}; }}
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
    .unwrap_or_else(|error| panic!("{error:?}"))
}

fn report(source: &str, notes: bool) -> AnalysisReport {
    analyze_artifact(
        &plan(source),
        None,
        &AnalyzeOptions {
            sections: SectionMode::Whole,
            notes,
            ..AnalyzeOptions::default()
        },
        &PlanLimits::song(),
    )
    .unwrap()
}

fn whole(source: &str) -> Measures {
    report(source, false)
        .sources
        .into_iter()
        .next()
        .unwrap()
        .whole
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
fn every_semitone_from_a0_to_c8_renders_at_three_velocities() {
    let mut notes = Vec::new();
    for midi in 21..=108 {
        for velocity in ["0.2", "0.5", "1"] {
            let index = notes.len();
            notes.push(format!(
                "note n{index} {{ at = {index}/2q; dur = 1/4q; pitch = {}; velocity = {velocity}; }}",
                pitch(midi)
            ));
        }
    }
    let source = composition(&format!("{}/2", notes.len()), &notes.join("\n"));
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
fn harder_notes_are_brighter() {
    for pitch in ["C2", "C4", "C6"] {
        let at = |velocity: &str| {
            brightness(&composition(
                "4",
                &format!("note n {{ at = 0q; dur = 2q; pitch = {pitch}; velocity = {velocity}; }}"),
            ))
        };
        let (soft, hard) = (at("0.3"), at("1"));
        assert!(hard > soft, "{pitch}: soft {soft:.4}, hard {hard:.4}");
    }
}

#[test]
fn every_octave_reads_as_pitched_within_50_cents() {
    let notes: Vec<_> = ["A0", "C1", "C2", "C3", "C4", "C5", "C6", "C7"]
        .iter()
        .enumerate()
        .map(|(index, pitch)| {
            format!(
                "note n{index} {{ at = {}q; dur = 2q; pitch = {pitch}; velocity = 0.6; }}",
                index * 3
            )
        })
        .collect();
    let measured = report(&composition("24", &notes.join("\n")), true).notes;
    assert_eq!(measured.len(), notes.len());
    for note in &measured {
        let value = serde_json::to_value(note).unwrap();
        assert_eq!(value["status"], "ok", "{value}");
        let cents = value["cents"].as_f64().unwrap();
        assert!(cents.abs() <= 50.0, "{value}");
    }
}

/// "Morning Ferry" verse chords as ballad accompaniment, every note at
/// `velocity`: a low root and its octave, then the triad broken in eighths.
fn ballad(bars: usize, velocity: &str) -> String {
    const BARS: [(&str, &str, [&str; 3]); 4] = [
        ("G1", "G2", ["B3", "D4", "G4"]),
        ("D2", "D3", ["A3", "D4", "F#4"]),
        ("E2", "E3", ["B3", "E4", "G4"]),
        ("C2", "C3", ["C4", "E4", "G4"]),
    ];
    let mut lines = Vec::new();
    for bar in 0..bars {
        let (root, octave, triad) = BARS[bar % 4];
        lines.push(format!(
            "chord l{bar} {{ at = {}q; dur = 4q; pitches = [{root}, {octave}]; velocity = {velocity}; }}",
            bar * 4
        ));
        for (step, index) in [0, 1, 2, 1, 0, 1, 2, 1].iter().enumerate() {
            lines.push(format!(
                "note r{bar}_{step} {{ at = {}/2q; dur = 1/2q; pitch = {}; velocity = {velocity}; }}",
                bar * 8 + step,
                triad[*index]
            ));
        }
    }
    composition(&(bars * 4).to_string(), &lines.join("\n"))
}

#[test]
fn the_reference_phrase_measures_minus_20_lufs_with_headroom() {
    let measures = whole(&ballad(4, "0.7"));
    let loudness = measures.integrated_lufs.unwrap();
    assert!((loudness + 20.0).abs() <= 1.0, "{loudness} LUFS");
    let peak = measures.true_peak_dbtp.unwrap();
    assert!(peak < -1.0, "{peak} dBTP");
}

fn minimum_budget(source: &SourceBundle) -> u64 {
    let limits = |max_execution_work| PlanLimits {
        max_execution_work,
        ..PlanLimits::song()
    };
    let artifact = compile_bundle_artifact_with_limits(source, &PlanLimits::song()).unwrap();
    let (mut low, mut high) = (0, PlanLimits::song().max_execution_work);
    while low < high {
        let middle = low + (high - low) / 2;
        if artifact.validate_with_limits(&limits(middle)).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}

#[test]
fn a_three_minute_ballad_fits_the_song_profile_beside_morning_ferry() {
    // 54 bars at 72 bpm is three minutes.
    let piano = minimum_budget(&SourceBundle::new("main.maac", ballad(54, "0.6")));
    let ferry = minimum_budget(&SourceBundle::new(
        "ferry.maac",
        include_str!("../examples/ferry/ferry.maac"),
    ));
    let budget = PlanLimits::song().max_execution_work;
    assert!(
        piano + ferry <= budget,
        "piano {piano} + ferry {ferry} > {budget}"
    );
}
