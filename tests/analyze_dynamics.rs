//! `maac analyze`'s `dynamics` objects and master chain: the gain each
//! compressor or limiter takes from each section, on fixtures with known
//! answers and on "Morning Ferry".

use maac::analyze::{analyze_artifact, AnalysisReport, AnalyzeOptions, Dynamics, SectionMode};
use maac::compiler::compile_bundle_artifact;
use maac::plan::{PlanLimits, PortRef};
use maac::SourceBundle;

/// Four seconds at 120 bpm with regions `first` and `second`, two beats
/// each, routed by `graph` to `&out:out`.
fn song(graph: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; tail = 0s; requires = ["maac.production/1"]; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
region first {{ span = [0q, 4q]; }}
region second {{ span = [4q, 8q]; }}
pattern held {{ length = 4q; note a {{ at = 0q; dur = 4q; pitch = A4; }} }}
{graph}
"#
    )
}

/// A sine node `name` at `level`, playing A4 through the regions in `at`.
fn tone(name: &str, level: f64, at: &[&str]) -> String {
    let mut text = format!(
        r#"node {name} {{ type = "core.sine/1"; params = {{ attack = 0s; release = 0s; level = {level}; }}; }}
track {name}_notes {{ target = &{name}:events; }}
"#
    );
    for (index, start) in at.iter().enumerate() {
        text.push_str(&format!(
            "place {name}_{index} {{ pattern = &held; track = &{name}_notes; at = {start}; }}\n"
        ));
    }
    text
}

fn compressor(name: &str, params: &str, sidechain: bool) -> String {
    let sidechain = if sidechain {
        " detector = external; sidechain_channels = 1;"
    } else {
        ""
    };
    format!(
        r#"node {name} {{ type = "fx.compressor/1"; config = {{ channels = 1;{sidechain} }}; params = {{ {params} }}; }}
"#
    )
}

fn connect(from: &str, to: &str) -> String {
    let id = format!("{from}_{to}").replace([':', '&'], "_");
    format!("connect {id} {{ from = &{from}; to = &{to}; }}\n")
}

fn options() -> AnalyzeOptions {
    AnalyzeOptions {
        sections: SectionMode::Regions,
        ..AnalyzeOptions::default()
    }
}

fn analyze(source: &str, options: &AnalyzeOptions) -> AnalysisReport {
    let plan = compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("fixture compiles: {e}"));
    let meter = maac::analyze::meter_of(&maac::parse(source).unwrap());
    analyze_artifact(&plan, meter.as_ref(), options, &PlanLimits::default()).unwrap()
}

/// The `dynamics` of `port` in `section`, or over the whole piece.
fn dynamics<'r>(report: &'r AnalysisReport, port: &str, section: Option<&str>) -> &'r Dynamics {
    let source = report
        .sources
        .iter()
        .find(|source| source.port == port)
        .unwrap_or_else(|| panic!("{port} is analysed"));
    match section {
        None => source.whole.dynamics.as_ref(),
        Some(id) => source
            .sections
            .iter()
            .find(|s| s.section == id)
            .and_then(|s| s.measures.dynamics.as_ref()),
    }
    .unwrap_or_else(|| panic!("dynamics for {port} in {section:?}"))
}

fn measure<'r>(
    report: &'r AnalysisReport,
    port: &str,
    section: &str,
) -> &'r maac::analyze::Measures {
    &report
        .sources
        .iter()
        .find(|source| source.port == port)
        .unwrap()
        .sections
        .iter()
        .find(|s| s.section == section)
        .unwrap()
        .measures
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} is not within {tolerance} of {expected}"
    );
}

/// A sine at −30 dBFS in `first` and −10 dBFS in `second`, into a hard-knee
/// compressor at −20 dB and ratio 2 that attacks at once and holds its
/// reduction, then a gain stage.
fn steps(makeup: &str) -> String {
    song(&[
        tone("soft", 0.0316, &["0q"]),
        tone("hard", 0.316, &["4q"]),
        "node bus { type = \"core.sum/1\"; config = { channels = 1; }; }\n".into(),
        compressor(
            "comp",
            &format!("threshold = -20dB; ratio = 2; knee = 0dB; attack = 0s; release = 30s; makeup = {makeup};"),
            false,
        ),
        "node out { type = \"core.gain/1\"; config = { channels = 1; }; params = { gain = 1; }; }\n".into(),
        connect("soft:out", "bus:in"),
        connect("hard:out", "bus:in"),
        connect("bus:out", "comp:in"),
        connect("comp:out", "out:in"),
    ]
    .concat())
}

#[test]
fn a_compressor_reports_what_it_takes_per_section() {
    let report = analyze(&steps("0dB"), &options());
    assert_eq!(report.profile, "maac.analyze.default/8");
    // Under the threshold, nothing; 10 dB over it at ratio 2, 5 dB.
    let first = dynamics(&report, "comp:out", Some("first"));
    assert_eq!(first.reduction_db, Some(0.0));
    assert_eq!(first.max_reduction_db, 0.0);
    let peak = measure(&report, "bus:out", "second")
        .sample_peak_dbfs
        .unwrap();
    let expected = (peak + 20.0) / 2.0;
    close(expected, 5.0, 0.02);
    let second = dynamics(&report, "comp:out", Some("second"));
    close(second.reduction_db.unwrap(), expected, 0.05);
    close(second.max_reduction_db, expected, 0.02);
    // Over the whole piece, each section counts by its output power.
    let power = |section| {
        10.0_f64.powf(
            measure(&report, "comp:out", section)
                .integrated_lufs
                .unwrap()
                / 10.0,
        )
    };
    let (quiet, loud) = (power("first"), power("second"));
    let combined =
        10.0 * ((quiet + loud * 10.0_f64.powf(expected / 10.0)) / (quiet + loud)).log10();
    let whole = dynamics(&report, "comp:out", None);
    close(whole.reduction_db.unwrap(), combined, 0.05);
    assert!(whole.reduction_db.unwrap() < expected - 0.05);
    assert_eq!(whole.max_reduction_db, second.max_reduction_db);
    // Only the processor gets a dynamics object.
    assert!(report
        .sources
        .iter()
        .filter(|source| source.port != "comp:out")
        .all(|source| source.whole.dynamics.is_none()
            && source
                .sections
                .iter()
                .all(|s| s.measures.dynamics.is_none())));
    // The chain is walked through the gain stage and stops at the sum.
    assert_eq!(report.master_chain, ["comp"]);
}

#[test]
fn makeup_is_not_a_reduction() {
    let plain = analyze(&steps("0dB"), &options());
    let lifted = analyze(&steps("6dB"), &options());
    for section in [None, Some("first"), Some("second")] {
        assert_eq!(
            dynamics(&plain, "comp:out", section),
            dynamics(&lifted, "comp:out", section)
        );
    }
    let level = |report: &AnalysisReport| {
        measure(report, "out:out", "second")
            .integrated_lufs
            .unwrap()
    };
    close(level(&lifted) - level(&plain), 6.0, 0.01);
}

#[test]
fn a_limiter_reports_how_far_it_is_driven_past_its_ceiling() {
    // A sine at −6 dBFS, driven 6 dB into a −6 dB ceiling: about 6 dB.
    let source = song(&[
        tone("tone", 0.5, &["0q", "4q"]),
        r#"node out { type = "fx.limiter/1"; config = { channels = 1; }; params = { gain = 6dB; ceiling = -6dB; release = 1s; }; }
"#
        .into(),
        connect("tone:out", "out:in"),
    ]
    .concat());
    let report = analyze(&source, &options());
    let peak = measure(&report, "tone:out", "second")
        .true_peak_dbtp
        .unwrap();
    let expected = peak + 12.0;
    close(expected, 6.0, 0.05);
    for section in ["first", "second"] {
        let found = dynamics(&report, "out:out", Some(section));
        close(found.reduction_db.unwrap(), expected, 0.05);
        close(found.max_reduction_db, expected, 0.05);
    }
    assert_eq!(report.master_chain, ["out"]);
}

/// A pad through `padcomp` into `duck`, which a tone in `second` ducks
/// through its sidechain, then a gain stage.
fn ducking() -> String {
    song(&[
        tone("pad", 0.1, &["0q", "4q"]),
        tone("kick", 0.5, &["4q"]),
        compressor(
            "padcomp",
            "threshold = -30dB; ratio = 2; knee = 0dB; attack = 0s; release = 1s;",
            false,
        ),
        compressor(
            "duck",
            "threshold = -30dB; ratio = 10; knee = 0dB; attack = 0s; release = 100ms;",
            true,
        ),
        "node out { type = \"core.gain/1\"; config = { channels = 1; }; params = { gain = 1; }; }\n".into(),
        connect("pad:out", "padcomp:in"),
        connect("padcomp:out", "duck:in"),
        connect("kick:out", "duck:sidechain"),
        connect("duck:out", "out:in"),
    ]
    .concat())
}

#[test]
fn a_sidechain_reduction_follows_its_detector_and_is_not_a_chain_input() {
    let report = analyze(&ducking(), &options());
    // The pad is steady, so `padcomp` takes the same from both sections.
    let steady = |section| {
        dynamics(&report, "padcomp:out", Some(section))
            .reduction_db
            .unwrap()
    };
    close(steady("first"), steady("second"), 0.05);
    assert!(steady("first") > 3.0);
    // `duck` takes nothing until the tone reaches its sidechain.
    let first = dynamics(&report, "duck:out", Some("first"));
    assert_eq!(first.max_reduction_db, 0.0);
    let second = dynamics(&report, "duck:out", Some("second"));
    assert!(second.reduction_db.unwrap() > 10.0, "{second:?}");
    // The sidechain is not a main input, so the walk goes on to `padcomp`.
    assert_eq!(report.master_chain, ["padcomp", "duck"]);
}

/// Two tones into `bus`, one through the stem compressor `stemcomp`, then a
/// glue compressor and a limiter.
fn stems() -> String {
    song(&[
        tone("lead", 0.3, &["0q", "4q"]),
        tone("pad", 0.2, &["0q", "4q"]),
        compressor(
            "stemcomp",
            "threshold = -30dB; ratio = 4; knee = 0dB; attack = 0s; release = 100ms;",
            false,
        ),
        "node bus { type = \"core.sum/1\"; config = { channels = 1; }; }\n".into(),
        compressor(
            "glue",
            "threshold = -20dB; ratio = 2; knee = 0dB; attack = 10ms; release = 100ms;",
            false,
        ),
        r#"node out { type = "fx.limiter/1"; config = { channels = 1; }; params = { gain = 0dB; ceiling = -1dB; release = 100ms; }; }
"#
        .into(),
        connect("lead:out", "stemcomp:in"),
        connect("stemcomp:out", "bus:in"),
        connect("pad:out", "bus:in"),
        connect("bus:out", "glue:in"),
        connect("glue:out", "out:in"),
    ]
    .concat())
}

#[test]
fn a_stem_compressor_is_reported_but_not_in_the_master_chain() {
    let report = analyze(&stems(), &options());
    assert_eq!(report.master_chain, ["glue", "out"]);
    assert!(
        dynamics(&report, "stemcomp:out", None)
            .reduction_db
            .unwrap()
            > 3.0
    );
    assert!(dynamics(&report, "glue:out", None).reduction_db.is_some());
    assert!(dynamics(&report, "out:out", None).reduction_db.is_some());
}

#[test]
fn the_master_chain_is_analysed_with_named_sources() {
    let report = analyze(
        &stems(),
        &AnalyzeOptions {
            sources: vec![PortRef::new("pad", "out").unwrap()],
            ..options()
        },
    );
    let ports: Vec<&str> = report.sources.iter().map(|s| s.port.as_str()).collect();
    assert_eq!(ports, ["out:out", "glue:out", "pad:out"]);
    assert!(dynamics(&report, "glue:out", Some("first"))
        .reduction_db
        .is_some());
}

#[test]
fn without_a_compressor_or_limiter_nothing_is_added() {
    let source = song(&[
        tone("tone", 0.5, &["0q", "4q"]),
        "node out { type = \"core.gain/1\"; config = { channels = 1; }; params = { gain = 1; }; }\n".into(),
        connect("tone:out", "out:in"),
    ]
    .concat());
    let report = analyze(&source, &options());
    assert!(report.master_chain.is_empty());
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("master_chain").is_none());
    assert!(!json.to_string().contains("dynamics"));
}

#[test]
fn the_cli_reports_each_processor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.maac");
    std::fs::write(&path, stems()).unwrap();
    let run = |json: bool| {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_maac"));
        command.args(["analyze", "--section", "regions"]);
        if json {
            command.arg("--json");
        }
        let output = command.arg(&path).output().unwrap();
        assert!(output.status.success());
        output.stdout
    };
    let json: serde_json::Value = serde_json::from_slice(&run(true)).unwrap();
    assert_eq!(
        json["analysis"]["master_chain"],
        serde_json::json!(["glue", "out"])
    );
    let text = String::from_utf8(run(false)).unwrap();
    let glue = text
        .find("\nreduction glue:out (master chain): ")
        .expect(&text);
    let limiter = text
        .find("\nreduction out:out (master chain): ")
        .expect(&text);
    let stem = text.find("\nreduction stemcomp:out: ").expect(&text);
    assert!(glue < limiter && limiter < stem, "{text}");
    assert!(
        text[glue..].lines().nth(1).unwrap().contains("; first "),
        "{text}"
    );
}

/// The last bar of "Morning Ferry"'s first verse and the first of its first
/// chorus, from two seconds of preroll.
#[test]
fn morning_ferry_reports_its_glue_and_limiter() {
    let source = include_str!("../examples/ferry/ferry.maac");
    let limits = PlanLimits::song();
    let plan = maac::compiler::compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source),
        &limits,
    )
    .unwrap_or_else(|e| panic!("Morning Ferry compiles: {e}"));
    let meter = maac::analyze::meter_of(&maac::parse(source).unwrap());
    let options = AnalyzeOptions {
        window: Some(maac::analyze::Window::parse("bars:12-13").unwrap()),
        preroll_seconds: Some(2.0),
        ..options()
    };
    let report = analyze_artifact(&plan, meter.as_ref(), &options, &limits).unwrap();
    assert_eq!(report.master_chain, ["glue", "master"]);
    let sections: Vec<&str> = report.sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(sections, ["verse1", "chorus1"]);
    for port in ["glue:out", "master:out"] {
        for section in ["verse1", "chorus1"] {
            let found = dynamics(&report, port, Some(section));
            assert!(found.reduction_db.is_some(), "{port} {section}");
        }
    }
    // The glue takes more from the chorus than from the verse.
    let glue = |section| {
        dynamics(&report, "glue:out", Some(section))
            .reduction_db
            .unwrap()
    };
    assert!(glue("chorus1") > glue("verse1"));
}
