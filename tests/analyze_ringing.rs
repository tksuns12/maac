//! Pitch estimates over a ringing release (F25 of the T3 trial): a legato
//! melody on an instrument with a long release is still checked for pitch.

use maac::analyze::{analyze_artifact, AnalysisReport, AnalyzeOptions, SectionMode};
use maac::compiler::compile_bundle_artifact;
use maac::plan::PlanLimits;
use maac::SourceBundle;

/// Eight legato notes at 120 bpm on a sine whose release is `release`.
fn melody(release: &str) -> AnalysisReport {
    let source = format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; tail = 1s; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node lead {{ type = "core.sine/1"; params = {{ attack = 2ms; release = {release}; level = 0.3; }}; }}
node out {{ type = "core.gain/1"; config = {{ channels = 1; }}; params = {{ gain = 1; }}; }}
connect lead_out {{ from = &lead:out; to = &out:in; }}
pattern tune {{ length = 8q;
  note n0 {{ at = 0q; dur = 1q; pitch = A4; }}
  note n1 {{ at = 1q; dur = 1q; pitch = C5; }}
  note n2 {{ at = 2q; dur = 1q; pitch = E5; }}
  note n3 {{ at = 3q; dur = 1q; pitch = D5; }}
  note n4 {{ at = 4q; dur = 1q; pitch = B4; }}
  note n5 {{ at = 5q; dur = 1q; pitch = G4; }}
  note n6 {{ at = 6q; dur = 1q; pitch = A4; }}
  note n7 {{ at = 7q; dur = 1q; pitch = A5; }}
}}
track notes {{ target = &lead:events; }}
place play {{ pattern = &tune; track = &notes; at = 0q; }}
"#
    );
    let plan = compile_bundle_artifact(&SourceBundle::new("main.maac", &source))
        .unwrap_or_else(|e| panic!("fixture compiles: {e}"));
    let options = AnalyzeOptions {
        sections: SectionMode::Whole,
        notes: true,
        ..AnalyzeOptions::default()
    };
    analyze_artifact(&plan, None, &options, &PlanLimits::default()).unwrap()
}

/// Each note's event name and status.
fn statuses(report: &AnalysisReport) -> Vec<(&str, &str)> {
    report
        .notes
        .iter()
        .map(|note| (note.event.rsplit('/').next().unwrap(), note.status))
        .collect()
}

#[test]
fn a_legato_melody_over_its_own_release_is_pitched() {
    let dry = melody("5ms");
    assert!(statuses(&dry).iter().all(|(_, status)| *status == "ok"));
    // Every note but the first sounds over the release of those before it,
    // and is measured once their pitches are cancelled. `n7` is an octave
    // over `n6`, so cancelling `n6` would cancel `n7` too: it reads
    // unpitched, not mistuned.
    let ringing = melody("400ms");
    let expected: Vec<(&str, &str)> = (0..8)
        .map(|n| {
            (
                ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"][n],
                if n == 7 { "unpitched" } else { "ok" },
            )
        })
        .collect();
    assert_eq!(statuses(&ringing), expected);
    assert!(ringing
        .notes
        .iter()
        .filter_map(|note| note.cents)
        .take(7)
        .all(|cents| cents.abs() < 5.0));
}
