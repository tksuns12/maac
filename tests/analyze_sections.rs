//! Section edges in `maac analyze`: a section's measures cover its own frames
//! and never take in the start of the next section (F24 of the T3 trial).

use maac::analyze::{analyze_artifact, AnalysisReport, AnalyzeOptions, Measures, SectionMode};
use maac::compiler::compile_bundle_artifact;
use maac::plan::PlanLimits;
use maac::SourceBundle;

/// At 110 bpm a quarter note is 26181.8 frames, so the edge between regions
/// `a` and `b` at 4q falls inside a 100 ms block. `steady` plays through both
/// regions, `late` only in `b`, and `hit` plays one 8.5 ms loud note at the
/// start of `b`.
const SOURCE: &str = r#"maac 1;
project p { score = [0q, 8q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; tail = 0s; }
tempo clock { points = [(0q, 110bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
region a { span = [0q, 4q]; }
region b { span = [4q, 8q]; }
node steady { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.1; }; }
node late { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.1; }; }
node hit { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.5; }; }
node bus { type = "core.sum/1"; config = { channels = 1; }; }
node out { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 1; }; }
connect steady_bus { from = &steady:out; to = &bus:in; }
connect late_bus { from = &late:out; to = &bus:in; }
connect hit_bus { from = &hit:out; to = &bus:in; }
connect bus_out { from = &bus:out; to = &out:in; }
pattern held { length = 4q; note n { at = 0q; dur = 4q; pitch = A4; } }
pattern blip { length = 4q; note n { at = 0q; dur = 1/64q; pitch = 880Hz; } }
track steady_notes { target = &steady:events; }
track late_notes { target = &late:events; }
track hit_notes { target = &hit:events; }
place steady_a { pattern = &held; track = &steady_notes; at = 0q; }
place steady_b { pattern = &held; track = &steady_notes; at = 4q; }
place late_b { pattern = &held; track = &late_notes; at = 4q; }
place hit_b { pattern = &blip; track = &hit_notes; at = 4q; }
"#;

fn report() -> AnalysisReport {
    let plan = compile_bundle_artifact(&SourceBundle::new("main.maac", SOURCE))
        .unwrap_or_else(|e| panic!("fixture compiles: {e}"));
    let meter = maac::analyze::meter_of(&maac::parse(SOURCE).unwrap());
    let options = AnalyzeOptions {
        sections: SectionMode::Regions,
        ..AnalyzeOptions::default()
    };
    analyze_artifact(&plan, meter.as_ref(), &options, &PlanLimits::default()).unwrap()
}

fn measures<'r>(report: &'r AnalysisReport, port: &str, section: &str) -> &'r Measures {
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

#[test]
fn a_part_that_rests_through_a_section_reads_silent_there() {
    let report = report();
    let edge = report.sections[1].start_frame;
    assert_ne!(
        edge % report.block_frames,
        0,
        "the edge falls inside a block"
    );
    for port in ["late:out", "hit:out"] {
        let rest = measures(&report, port, "a");
        assert_eq!(rest.sample_peak_dbfs, None, "{port}");
        assert_eq!(rest.rms_dbfs, None, "{port}");
        assert_eq!(rest.integrated_lufs, None, "{port}");
        assert_eq!(rest.max_short_term_lufs, None, "{port}");
        let shares = rest.master_share.as_ref().unwrap();
        assert!(shares.iter().flatten().all(|share| *share == 0.0), "{port}");
    }
    // So no stem dominates it in `a`, where `steady` plays alone.
    assert!(
        report
            .findings
            .iter()
            .all(|finding| finding.kind != "band_dominance"
                || finding.section.as_deref() != Some("a"))
    );
}

#[test]
fn a_section_keeps_the_peak_at_its_first_frames() {
    let report = report();
    let whole = &report
        .sources
        .iter()
        .find(|source| source.port == "hit:out")
        .unwrap()
        .whole;
    let first = measures(&report, "hit:out", "b");
    assert!(whole.sample_peak_dbfs.unwrap() > -6.1);
    assert_eq!(first.sample_peak_dbfs, whole.sample_peak_dbfs);
    assert_eq!(first.true_peak_dbtp, whole.true_peak_dbtp);
    // The output's sections add up to the whole piece's peak and energy.
    let output = |section| measures(&report, "out:out", section);
    assert_eq!(
        output("b").sample_peak_dbfs,
        report.sources[0].whole.sample_peak_dbfs
    );
}
