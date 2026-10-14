//! `maac analyze --lead`: a named lead's margin over the rest of its mix
//! point, and the `lead_under` warning, on fixtures with known answers and on
//! "Morning Ferry" at the levels of the owner's listening round.

use maac::analyze::{analyze_artifact, AnalysisReport, AnalyzeOptions, LeadSummary, SectionMode};
use maac::compiler::compile_bundle_artifact;
use maac::plan::{PlanLimits, PortRef};
use maac::SourceBundle;

/// Six seconds at 120 bpm. A sine lead plays eight A4 notes in region `verse`
/// and three in region `tag`, into `bus`. A held C5 sine `pad`, in the same
/// octave band, and a held A1 sine `bass`, three octaves lower, join `bus`
/// when given a level. `extra` adds declarations.
fn song(lead: &str, pad: Option<&str>, bass: Option<&str>, extra: &str) -> String {
    let part = |name: &str, level: Option<&str>, pitch: &str| {
        level.map_or(String::new(), |level| {
            format!(
                r#"node {name} {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = {level}; }}; }}
connect {name}_bus {{ from = &{name}:out; to = &bus:in; }}
pattern {name}_hold {{ length = 12q; note hold {{ at = 0q; dur = 12q; pitch = {pitch}; }} }}
track {name}_notes {{ target = &{name}:events; }}
place {name}_play {{ pattern = &{name}_hold; track = &{name}_notes; at = 0q; }}
"#
            )
        })
    };
    let notes = |count: u32| {
        (0..count)
            .map(|n| format!("note n{n} {{ at = {n}q; dur = 3/4q; pitch = A4; }}"))
            .collect::<Vec<_>>()
            .join("\n  ")
    };
    format!(
        r#"maac 1;
project p {{ score = [0q, 12q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
region verse {{ span = [0q, 8q]; }}
region tag {{ span = [8q, 12q]; }}
node lead {{ type = "core.sine/1"; params = {{ attack = 5ms; release = 5ms; level = {lead}; }}; }}
node bus {{ type = "core.sum/1"; config = {{ channels = 1; }}; }}
node out {{ type = "core.gain/1"; config = {{ channels = 1; }}; params = {{ gain = 0.5; }}; }}
connect lead_bus {{ from = &lead:out; to = &bus:in; }}
connect bus_out {{ from = &bus:out; to = &out:in; }}
pattern melody {{ length = 8q;
  {verse}
}}
pattern ending {{ length = 4q;
  {tag}
}}
track tune {{ target = &lead:events; }}
place verse_tune {{ pattern = &melody; track = &tune; at = 0q; }}
place tag_tune {{ pattern = &ending; track = &tune; at = 8q; }}
{pad}{bass}{extra}
"#,
        verse = notes(8),
        tag = notes(3),
        pad = part("pad", pad, "C5"),
        bass = part("bass", bass, "A1"),
    )
}

fn port(text: &str) -> PortRef {
    let (node, port) = text.split_once(':').unwrap();
    PortRef::new(node, port).unwrap()
}

fn options(leads: &[&str]) -> AnalyzeOptions {
    AnalyzeOptions {
        sections: SectionMode::Regions,
        leads: leads.iter().map(|lead| port(lead)).collect(),
        ..AnalyzeOptions::default()
    }
}

fn run(source: &str, options: &AnalyzeOptions) -> Result<AnalysisReport, String> {
    let plan = compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("fixture compiles: {e}"));
    let meter = maac::analyze::meter_of(&maac::parse(source).unwrap());
    analyze_artifact(&plan, meter.as_ref(), options, &PlanLimits::default())
        .map_err(|error| error.code)
}

fn analyze(source: &str, leads: &[&str]) -> AnalysisReport {
    run(source, &options(leads)).unwrap()
}

/// The lead summary of `lead` for `section`, or for the whole piece.
fn summary<'r>(report: &'r AnalysisReport, lead: &str, section: Option<&str>) -> &'r LeadSummary {
    let source = report
        .sources
        .iter()
        .find(|source| source.port == lead)
        .unwrap();
    match section {
        None => source.whole.lead.as_ref(),
        Some(id) => source
            .sections
            .iter()
            .find(|s| s.section == id)
            .and_then(|s| s.measures.lead.as_ref()),
    }
    .unwrap_or_else(|| panic!("a lead summary for {lead} in {section:?}"))
}

fn median(report: &AnalysisReport, lead: &str, section: &str) -> f64 {
    summary(report, lead, Some(section)).median_db.unwrap()
}

fn under(report: &AnalysisReport) -> Vec<(&str, Option<&str>, f64)> {
    report
        .findings
        .iter()
        .filter(|finding| finding.kind == "lead_under")
        .map(|f| (f.source.as_str(), f.section.as_deref(), f.value))
        .collect()
}

#[test]
fn a_lead_level_with_its_accompaniment_raises_nothing() {
    let report = analyze(&song("0.25", Some("0.25"), None, ""), &["lead:out"]);
    assert_eq!(report.profile, "maac.analyze.default/8");
    assert_eq!(report.leads, ["lead:out"]);
    let verse = summary(&report, "lead:out", Some("verse"));
    assert_eq!(
        (verse.mix_point.as_str(), verse.notes, verse.clear),
        ("bus", 8, 0)
    );
    // Two equal sines a third apart in one octave band: about level.
    assert!(verse.median_db.unwrap().abs() < 2.0, "{verse:?}");
    assert!(under(&report).is_empty(), "{:?}", under(&report));
}

#[test]
fn a_lead_under_its_accompaniment_is_reported_per_section() {
    let level = analyze(&song("0.25", Some("0.25"), None, ""), &["lead:out"]);
    let quiet = analyze(&song("0.025", Some("0.25"), None, ""), &["lead:out"]);
    // Twenty dB down moves the margin by twenty dB.
    let drop = median(&quiet, "lead:out", "verse") - median(&level, "lead:out", "verse");
    assert!((drop + 20.0).abs() < 0.3, "{drop}");

    let found = under(&quiet);
    assert_eq!(found.len(), 1, "{found:?}");
    let (source, section, value) = found[0];
    assert_eq!((source, section), ("lead:out", Some("verse")));
    assert_eq!(value, median(&quiet, "lead:out", "verse"));
    let finding = quiet
        .findings
        .iter()
        .find(|f| f.kind == "lead_under")
        .unwrap();
    assert_eq!((finding.severity, finding.threshold), ("warning", -8.5));
    assert!(
        finding
            .event
            .as_deref()
            .unwrap()
            .starts_with("verse_tune/0/"),
        "{:?}",
        finding.event
    );
    assert!(
        finding.message.contains("the rest of `bus`"),
        "{}",
        finding.message
    );

    // Three notes are too few to judge `tag`, though all are under.
    let tag = summary(&quiet, "lead:out", Some("tag"));
    assert_eq!((tag.notes, tag.under), (3, 3));
    let whole = summary(&quiet, "lead:out", None);
    assert_eq!((whole.notes, whole.under), (11, 11));

    // Three dB down is still a lead.
    let near = analyze(&song("0.177", Some("0.25"), None, ""), &["lead:out"]);
    assert!(under(&near).is_empty(), "{:?}", under(&near));
}

#[test]
fn a_lead_is_judged_in_its_own_band() {
    // The bass is 10 dB louder, but three octaves below the lead.
    let report = analyze(&song("0.1", None, Some("0.316"), ""), &["lead:out"]);
    let rms = |port: &str| {
        let source = report.sources.iter().find(|s| s.port == port).unwrap();
        source.whole.rms_dbfs.unwrap()
    };
    assert!(rms("bass:out") - rms("lead:out") > 9.0);
    assert!(median(&report, "lead:out", "verse") > 5.0, "{report:?}");
    assert!(under(&report).is_empty(), "{:?}", under(&report));
}

#[test]
fn two_leads_are_each_compared_with_the_accompaniment() {
    // A quiet E5 organ shares the octave band with the lead and the pad.
    let organ = r#"node organ { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.05; }; }
connect organ_bus { from = &organ:out; to = &bus:in; }
pattern chord_tone { length = 12q; note hold { at = 0q; dur = 12q; pitch = E5; } }
track organ_notes { target = &organ:events; }
place organ_play { pattern = &chord_tone; track = &organ_notes; at = 0q; }"#;
    let source = song("0.25", Some("0.25"), None, organ);
    let one = analyze(&source, &["lead:out"]);
    let two = analyze(&source, &["lead:out", "pad:out"]);
    assert_eq!(two.leads, ["lead:out", "pad:out"]);
    // With the pad named too, only the quiet organ is left to cover the lead.
    let alone = median(&one, "lead:out", "verse");
    let with_pad = median(&two, "lead:out", "verse");
    assert!(with_pad > alone + 5.0, "{alone} {with_pad}");
    assert!(summary(&two, "pad:out", None).notes >= 1);

    // With nothing else in the mix, every note is clear.
    let bare = analyze(
        &song("0.25", Some("0.25"), None, ""),
        &["lead:out", "pad:out"],
    );
    let verse = summary(&bare, "lead:out", Some("verse"));
    assert_eq!((verse.notes, verse.clear, verse.median_db), (8, 8, None));
    assert!(under(&bare).is_empty());
}

#[test]
fn lead_notes_are_listed_on_request() {
    let source = song("0.025", Some("0.25"), None, "");
    let quiet = analyze(&source, &["lead:out"]);
    assert!(quiet.lead_notes.is_empty());
    let listed = run(
        &source,
        &AnalyzeOptions {
            notes: true,
            ..options(&["lead:out"])
        },
    )
    .unwrap();
    assert_eq!(listed.lead_notes.len(), 11);
    let first = &listed.lead_notes[0];
    assert_eq!(
        (first.event.as_str(), first.lead.as_str(), first.status),
        ("verse_tune/0/n0", "lead:out", "under")
    );
    assert_eq!(first.section.as_deref(), Some("verse"));
    assert!(first.margin_db.unwrap() < -8.5);
}

#[test]
fn without_a_lead_nothing_is_added() {
    let report = run(
        &song("0.025", Some("0.25"), None, ""),
        &AnalyzeOptions {
            sections: SectionMode::Regions,
            ..AnalyzeOptions::default()
        },
    )
    .unwrap();
    assert!(report.leads.is_empty());
    assert!(under(&report).is_empty());
    assert!(report
        .sources
        .iter()
        .all(|source| source.whole.lead.is_none()
            && source.sections.iter().all(|s| s.measures.lead.is_none())));
}

#[test]
fn a_lead_must_be_a_part_that_plays_notes() {
    let hiss = r#"node hiss { type = "core.noise/1"; config = { channels = 1; }; }
node hiss_level { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 0.001; }; }
connect hiss_gain { from = &hiss:out; to = &hiss_level:in; }
connect hiss_bus { from = &hiss_level:out; to = &bus:in; }"#;
    let source = song("0.25", Some("0.25"), None, hiss);
    let code = |leads: &[&str]| run(&source, &options(leads)).unwrap_err();
    assert_eq!(code(&["out:out"]), "E_USAGE");
    assert_eq!(code(&["hiss_level:out"]), "E_USAGE");
    assert_eq!(code(&["lead:out", "bus:out"]), "E_USAGE");
    assert_eq!(code(&["missing:out"]), "E_REFERENCE");
    // Naming one lead twice is naming it once.
    let report = run(&source, &options(&["lead:out", "lead:out"])).unwrap();
    assert_eq!(report.leads, ["lead:out"]);
}

#[test]
fn the_cli_takes_leads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.maac");
    std::fs::write(&path, song("0.025", Some("0.25"), None, "")).unwrap();
    let analyze = |lead: &str| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_maac"))
            .args(["analyze", "--json", "--section", "regions", "--lead", lead])
            .arg(&path)
            .output()
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        (output.status.success(), json)
    };
    let (ok, json) = analyze("lead:out");
    assert!(ok, "{json}");
    assert_eq!(json["analysis"]["leads"][0], "lead:out");
    let (ok, json) = analyze("lead");
    assert!(!ok);
    assert_eq!(json["code"], "E_USAGE");

    let text = std::process::Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(["analyze", "--section", "regions", "--lead", "lead:out"])
        .arg(&path)
        .output()
        .unwrap();
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.contains("warning lead_under lead:out [verse"),
        "{text}"
    );
    assert!(
        text.contains("lead lead:out against the rest of bus"),
        "{text}"
    );
}

/// The first two bars of "Morning Ferry"'s first verse with its lead at
/// `level` instead of the released 4.2, from two seconds of preroll.
fn ferry_verse(level: &str) -> AnalysisReport {
    let released = include_str!("../examples/ferry/ferry.maac");
    let source = released.replacen(
        "params = { level = 4.2; drive = 3;",
        &format!("params = {{ level = {level}; drive = 3;"),
        1,
    );
    assert_ne!(source, released);
    let limits = PlanLimits::song();
    let plan = maac::compiler::compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", &source),
        &limits,
    )
    .unwrap_or_else(|e| panic!("Morning Ferry compiles: {e}"));
    let meter = maac::analyze::meter_of(&maac::parse(&source).unwrap());
    let options = AnalyzeOptions {
        window: Some(maac::analyze::Window::parse("bars:5-6").unwrap()),
        preroll_seconds: Some(2.0),
        ..options(&["lead:out"])
    };
    analyze_artifact(&plan, meter.as_ref(), &options, &limits).unwrap()
}

#[test]
fn the_listening_round_sets_the_limit() {
    // The owner heard the lead 6 dB down as still leading and 9 dB down as
    // buried.
    let (leading, buried) = std::thread::scope(|scope| {
        let leading = scope.spawn(|| ferry_verse("2.1050"));
        let buried = scope.spawn(|| ferry_verse("1.4902"));
        (leading.join().unwrap(), buried.join().unwrap())
    });
    assert!(under(&leading).is_empty(), "{:?}", under(&leading));
    let found = under(&buried);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].0, found[0].1), ("lead:out", Some("verse1")));
}
