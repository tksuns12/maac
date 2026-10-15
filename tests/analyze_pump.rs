//! `maac analyze`'s beat dip and `pump_unheard`: how far the level above
//! 300 Hz dips over the beat, on fixtures with known answers and on
//! "Afterlight".

use maac::analyze::{analyze_artifact, AnalysisReport, AnalyzeOptions, Measures, SectionMode};
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::SourceBundle;

/// Eight quarter notes on `tempo` with regions `first` and `second`, four
/// each, routed by `graph` to `&out:out`.
fn song(tempo: &str, graph: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; tail = 0s; requires = ["maac.production/1"]; }}
tempo clock {{ points = [{tempo}]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
region first {{ span = [0q, 4q]; }}
region second {{ span = [4q, 8q]; }}
region short {{ span = [0q, 3q]; }}
{graph}
"#
    )
}

const STEADY: &str = "(0q, 120bpm, step)";

/// A mono sine node `name` holding `pitch` at `level` over `span`.
fn tone(name: &str, pitch: &str, level: f64, span: (&str, &str)) -> String {
    format!(
        r#"node {name} {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = {level}; }}; }}
track {name}_notes {{ target = &{name}:events; }}
pattern {name}_held {{ length = {length}q; note a {{ at = 0q; dur = {length}q; pitch = {pitch}; }} }}
place {name}_once {{ pattern = &{name}_held; track = &{name}_notes; at = {start}q; }}
"#,
        start = span.0,
        length = span.1,
    )
}

/// A mono gain stage `name` at 1 for the first half of every quarter note
/// and at `low` for the second.
fn gated(name: &str, low: f64) -> String {
    let points: Vec<String> = (0..8)
        .flat_map(|q| {
            [
                format!("({q}q, 1, step)"),
                format!("({}/2q, {low}, step)", 2 * q + 1),
            ]
        })
        .collect();
    format!(
        r#"node {name} {{ type = "core.gain/1"; config = {{ channels = 1; }}; params = {{ gain = 1; }}; }}
curve {name}_steps {{ clock = score; points = [{}]; }}
automation {name}_lane {{ target = &{name}.params.gain; curve = &{name}_steps; at = 0q; }}
"#,
        points.join(", ")
    )
}

fn connect(from: &str, to: &str) -> String {
    let id = format!("{from}_{to}").replace([':', '&'], "_");
    format!("connect {id} {{ from = &{from}; to = &{to}; }}\n")
}

const OUT: &str =
    "node out { type = \"core.gain/1\"; config = { channels = 1; }; params = { gain = 1; }; }\n";
const MIX: &str = "node mix { type = \"core.sum/1\"; config = { channels = 1; }; }\n";

fn analyze_with(source: &str, options: &AnalyzeOptions, limits: &PlanLimits) -> AnalysisReport {
    let plan = compile_bundle_artifact_with_limits(&SourceBundle::new("main.maac", source), limits)
        .unwrap_or_else(|e| panic!("fixture compiles: {e}"));
    let meter = maac::analyze::meter_of(&maac::parse(source).unwrap());
    analyze_artifact(&plan, meter.as_ref(), options, limits).unwrap()
}

fn analyze(source: &str) -> AnalysisReport {
    analyze_with(
        source,
        &AnalyzeOptions {
            sections: SectionMode::Regions,
            ..AnalyzeOptions::default()
        },
        &PlanLimits::default(),
    )
}

fn measures<'r>(report: &'r AnalysisReport, port: &str, section: Option<&str>) -> &'r Measures {
    let source = report
        .sources
        .iter()
        .find(|source| source.port == port)
        .unwrap_or_else(|| panic!("{port} is analysed"));
    match section {
        None => &source.whole,
        Some(id) => {
            &source
                .sections
                .iter()
                .find(|s| s.section == id)
                .unwrap()
                .measures
        }
    }
}

fn dip(report: &AnalysisReport, port: &str, section: Option<&str>) -> Option<f64> {
    measures(report, port, section).beat_dip_db
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} is not within {tolerance} of {expected}"
    );
}

/// A880 at 0.3, 10 dB down for the second half of every quarter note.
fn gated_tone(tempo: &str) -> String {
    song(
        tempo,
        &[
            tone("tone", "A5", 0.3, ("0", "8")),
            gated("gate", 0.316_227_766),
            OUT.into(),
            connect("tone:out", "gate:in"),
            connect("gate:out", "out:in"),
        ]
        .concat(),
    )
}

#[test]
fn a_level_that_drops_for_half_of_each_beat_dips_by_the_drop() {
    let report = analyze(&gated_tone(STEADY));
    assert_eq!(report.profile, "maac.analyze.default/9");
    for section in [None, Some("first"), Some("second")] {
        close(dip(&report, "out:out", section).unwrap(), 10.0, 0.1);
        close(dip(&report, "tone:out", section).unwrap(), 0.0, 0.05);
    }
}

#[test]
fn the_beat_follows_the_tempo() {
    // At 90 bpm from 4q, the second region's quarter notes are longer, and
    // the gate still drops for the second half of each.
    let report = analyze(&gated_tone("(0q, 120bpm, step), (4q, 90bpm, step)"));
    for section in ["first", "second"] {
        close(dip(&report, "out:out", Some(section)).unwrap(), 10.0, 0.1);
    }
}

#[test]
fn a_dipping_sub_does_not_dip_the_level_above_300_hz() {
    // G1 at 0.5 drops 10 dB for half of each beat under a steady A5 at 0.3.
    let source = song(
        STEADY,
        &[
            tone("sub", "G1", 0.5, ("0", "8")),
            gated("gate", 0.316_227_766),
            tone("tone", "A5", 0.3, ("0", "8")),
            MIX.into(),
            OUT.into(),
            connect("sub:out", "gate:in"),
            connect("gate:out", "mix:in"),
            connect("tone:out", "mix:in"),
            connect("mix:out", "out:in"),
        ]
        .concat(),
    );
    let report = analyze(&source);
    // The sub's level drops by 10 dB, but the output above 300 Hz barely
    // dips...
    assert!(dip(&report, "out:out", None).unwrap() < 0.5, "{report:#?}");
    // ...though the sub is the louder part.
    assert!(
        measures(&report, "sub:out", None).rms_dbfs.unwrap()
            > measures(&report, "tone:out", None).rms_dbfs.unwrap()
    );
}

#[test]
fn short_or_silent_spans_have_no_dip() {
    // `short` holds three whole quarter notes; `rest` stops at 2q.
    let source = song(
        STEADY,
        &[
            tone("tone", "A5", 0.3, ("0", "8")),
            tone("rest", "E5", 0.3, ("0", "2")),
            MIX.into(),
            OUT.into(),
            connect("tone:out", "mix:in"),
            connect("rest:out", "mix:in"),
            connect("mix:out", "out:in"),
        ]
        .concat(),
    );
    let report = analyze(&source);
    assert_eq!(dip(&report, "out:out", Some("short")), None);
    assert!(dip(&report, "out:out", Some("first")).is_some());
    assert!(dip(&report, "rest:out", Some("first")).is_some());
    assert_eq!(dip(&report, "rest:out", Some("second")), None);
}

/// A pad that `duck` ducks on every beat, keyed by a kick that only reaches
/// its sidechain, and a lead in `first` that does not go through it; then a
/// glue compressor that is not keyed.
fn pumped() -> String {
    let kick = r#"node kick { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.5; }; }
track kick_notes { target = &kick:events; }
pattern kick_beat { length = 1q; note a { at = 0q; dur = 1/4q; pitch = A1; } }
place kick_beats { pattern = &kick_beat; track = &kick_notes; at = 0q; count = 8; }
node duck { type = "fx.compressor/1"; config = { channels = 1; detector = external; sidechain_channels = 1; }; params = { threshold = -30dB; ratio = 10; knee = 0dB; attack = 0s; release = 100ms; }; }
node glue { type = "fx.compressor/1"; config = { channels = 1; }; params = { threshold = -30dB; ratio = 2; knee = 0dB; attack = 10ms; release = 100ms; }; }
"#;
    song(
        STEADY,
        &[
            tone("pad", "A4", 0.2, ("0", "8")),
            tone("lead", "E5", 0.5, ("0", "4")),
            kick.into(),
            MIX.into(),
            OUT.into(),
            connect("pad:out", "duck:in"),
            connect("kick:out", "duck:sidechain"),
            connect("duck:out", "mix:in"),
            connect("lead:out", "mix:in"),
            connect("mix:out", "glue:in"),
            connect("glue:out", "out:in"),
        ]
        .concat(),
    )
}

#[test]
fn a_keyed_compressor_covered_by_an_unducked_part_is_reported() {
    let report = analyze(&pumped());
    // The ducked pad alone dips far more than the limit in both regions.
    for section in ["first", "second"] {
        assert!(dip(&report, "duck:out", Some(section)).unwrap() > 15.0);
    }
    // With the lead over it, the output barely dips; without, it pumps.
    let covered = dip(&report, "out:out", Some("first")).unwrap();
    assert!(covered < 5.0, "{covered}");
    assert!(dip(&report, "out:out", Some("second")).unwrap() > 15.0);
    let found: Vec<_> = report
        .findings
        .iter()
        .filter(|finding| finding.kind == "pump_unheard")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let finding = found[0];
    assert_eq!(finding.source, "duck:out");
    assert_eq!(finding.section.as_deref(), Some("first"));
    assert_eq!(finding.severity, "warning");
    assert_eq!(finding.value, covered);
    assert_eq!(finding.threshold, 8.0);
    assert!(
        finding.message.contains("`duck:out` alone dips"),
        "{}",
        finding.message
    );
    // The glue compressor takes from the mix but is not keyed.
    let glue = measures(&report, "glue:out", Some("first"))
        .dynamics
        .as_ref()
        .unwrap();
    assert!(glue.reduction_db.unwrap() >= 3.0, "{glue:?}");
}

#[test]
fn the_cli_shows_the_dip_of_each_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.maac");
    std::fs::write(&path, pumped()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["analyze", "--section", "regions"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("\n  warning pump_unheard duck:out [first, bar 1]: "),
        "{text}"
    );
    let second = text
        .lines()
        .find(|line| line.starts_with("  second: "))
        .expect(&text);
    assert!(second.contains(", beat dip "), "{text}");
}

/// The first eight bars of "Afterlight"'s first drop, from two seconds of
/// preroll: as approved, with the lead through the pump, and as in version 1,
/// with the lead joining the mix after it. The owner heard a pump in the
/// first and none in the second.
#[test]
fn afterlight_pumps_only_with_its_lead_through_the_pump() {
    let approved = include_str!("../examples/afterlight/afterlight.maac");
    let lead_after = approved
        .replace(
            "connect lead_bus { from = &lead:out; to = &music:in; }",
            "connect lead_bus { from = &lead:out; to = &bus:in; }",
        )
        .replace(
            "connect lead_high_bus { from = &lead_high:out; to = &music:in; }",
            "connect lead_high_bus { from = &lead_high:out; to = &bus:in; }",
        );
    assert_ne!(lead_after, approved);
    let options = AnalyzeOptions {
        window: Some(maac::analyze::Window::parse("bars:33-40").unwrap()),
        preroll_seconds: Some(2.0),
        sections: SectionMode::Regions,
        ..AnalyzeOptions::default()
    };
    let unheard = |report: &AnalysisReport| {
        report
            .findings
            .iter()
            .filter(|finding| finding.kind == "pump_unheard")
            .map(|finding| (finding.source.clone(), finding.value))
            .collect::<Vec<_>>()
    };
    let report = analyze_with(approved, &options, &PlanLimits::song());
    let heard = dip(&report, "master:out", Some("drop1")).unwrap();
    assert!(heard > 10.0, "{heard}");
    assert_eq!(unheard(&report), []);
    let report = analyze_with(&lead_after, &options, &PlanLimits::song());
    let covered = dip(&report, "master:out", Some("drop1")).unwrap();
    assert!(covered < 7.0, "{covered}");
    assert_eq!(unheard(&report), [("pump:out".to_owned(), covered)]);
}
