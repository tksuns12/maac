//! Rendering a span of frames: exact from frame 0, a preview from later.

use maac::analyze::{analyze_artifact, AnalyzeOptions, SectionMode, Window};
use maac::compiler::compile_bundle_artifact;
use maac::dsp::{
    render_ports_artifact_span_with_limits, render_ports_artifact_with_limits, RenderSpan,
};
use maac::plan::{PlanLimits, PortRef};
use maac::{PlanArtifact, SourceBundle};

/// Four bars at 120 bpm: a long pad note through bars 1-3, a short note in
/// bar 1, and a note in each of bars 3 and 4, through a reverb.
const SOURCE: &str = r#"maac 1;
project p {
  score = [0q, 16q]; rate = 48000Hz; tail = 1s;
  tempo = &clock; meter = &metre; output = &room:out;
  requires = ["maac.production/1"];
}
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
region first { span = [bar(1, 1), bar(3, 1)]; }
region last { span = [bar(3, 1), bar(5, 1)]; }
import studio { builtin = "std/studio/1.0.0"; }
node keys { instrument = &studio.electric_piano; config = { voices = 8; }; }
node room { type = "fx.reverb/1"; config = { channels = 2; predelay = 10ms; damping = 0.5; }; params = { mix = 0.3; decay = 1.5s; }; }
connect keys_room { from = &keys:out; to = &room:in; }
pattern part { length = 16q;
  note held { at = 0q; dur = 11q; pitch = C3; velocity = 0.6; }
  note short { at = 1q; dur = 1/2q; pitch = E4; velocity = 0.7; }
  note third { at = 8q; dur = 2q; pitch = G4; velocity = 0.5; }
  note fourth { at = 12q; dur = 2q; pitch = A4; velocity = 0.8; }
}
track t { target = &keys:events; }
place once { pattern = &part; track = &t; at = 0q; }
"#;

fn plan() -> PlanArtifact {
    compile_bundle_artifact(&SourceBundle::new("main.maac", SOURCE)).unwrap()
}

fn output() -> PortRef {
    PortRef::new("room", "out").unwrap()
}

fn render(plan: &PlanArtifact, span: Option<RenderSpan>) -> Vec<Vec<f64>> {
    let limits = PlanLimits::song();
    let mut frames = Vec::new();
    let capture = |outputs: &[Vec<f64>]| {
        frames.push(outputs[0].clone());
        Ok(())
    };
    match span {
        Some(span) => {
            render_ports_artifact_span_with_limits(plan, &limits, &[output()], span, capture)
        }
        None => render_ports_artifact_with_limits(plan, &limits, &[output()], capture),
    }
    .unwrap();
    frames
}

// 120 bpm: a bar is two seconds, 96,000 frames.
const BAR: u64 = 96_000;

#[test]
fn a_span_from_frame_zero_is_the_full_render_up_to_its_end() {
    let plan = plan();
    let full = render(&plan, None);
    let span = render(
        &plan,
        Some(RenderSpan {
            start: 0,
            end: 2 * BAR + 1234,
        }),
    );
    assert_eq!(span.len() as u64, 2 * BAR + 1234);
    let bits = |frames: &[Vec<f64>]| -> Vec<Vec<u64>> {
        frames
            .iter()
            .map(|f| f.iter().map(|x| x.to_bits()).collect())
            .collect()
    };
    assert_eq!(bits(&span), bits(&full[..span.len()]));
}

#[test]
fn a_preview_restarts_notes_still_sounding_and_drops_finished_ones() {
    let plan = plan();
    // From bar 3: the held note sounds through it; the short note ended in bar 1.
    let preview = render(
        &plan,
        Some(RenderSpan {
            start: 2 * BAR,
            end: 2 * BAR + 4800,
        }),
    );
    assert_eq!(preview.len(), 4800);
    let peak = preview
        .iter()
        .flatten()
        .fold(0.0_f64, |peak, x| peak.max(x.abs()));
    assert!(
        peak > 1e-4,
        "the held note restarts at the preview's first frame"
    );
    // A preview starting where nothing sounds and nothing starts is silent.
    let silent = render(
        &plan,
        Some(RenderSpan {
            start: 16 * BAR / 4 + 24_000,
            end: 16 * BAR / 4 + 24_100,
        }),
    );
    assert!(silent.iter().flatten().all(|x| *x == 0.0));
}

#[test]
fn spans_must_be_nonempty_and_inside_the_render() {
    let plan = plan();
    let limits = PlanLimits::song();
    for span in [
        RenderSpan { start: 10, end: 10 },
        RenderSpan { start: 10, end: 5 },
        RenderSpan {
            start: 0,
            end: u64::MAX,
        },
    ] {
        let error =
            render_ports_artifact_span_with_limits(&plan, &limits, &[output()], span, |_| Ok(()))
                .unwrap_err();
        assert_eq!(error.code(), "E_RANGE", "{span:?}");
    }
}

fn analyze(
    options: &AnalyzeOptions,
) -> Result<maac::analyze::AnalysisReport, maac::analyze::AnalyzeError> {
    let meter = maac::analyze::meter_of(&maac::parse(SOURCE).unwrap());
    analyze_artifact(&plan(), meter.as_ref(), options, &PlanLimits::song())
}

#[test]
fn a_region_window_is_measured_exactly() {
    let report = analyze(&AnalyzeOptions {
        window: Some(Window::Region("last".into())),
        ..AnalyzeOptions::default()
    })
    .unwrap();
    let window = report.window.as_ref().unwrap();
    assert_eq!((window.start_frame, window.end_frame), (2 * BAR, 4 * BAR));
    assert_eq!(window.render_start_frame, 0);
    assert!(!window.approximate);
    assert_eq!(report.frames, 2 * BAR);
    // Only the overlapping region remains as a section.
    let ids: Vec<_> = report.sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["last"]);
    assert!(report.sources[0].whole.integrated_lufs.is_some());
}

#[test]
fn a_bars_window_clips_sections_and_a_preroll_is_marked_approximate() {
    let report = analyze(&AnalyzeOptions {
        sections: SectionMode::Regions,
        window: Some(Window::Bars(2, 3)),
        preroll_seconds: Some(1.0),
        ..AnalyzeOptions::default()
    })
    .unwrap();
    let window = report.window.as_ref().unwrap();
    assert_eq!((window.start_frame, window.end_frame), (BAR, 3 * BAR));
    assert_eq!(window.render_start_frame, BAR - 48_000);
    assert!(window.approximate);
    let sections: Vec<_> = report
        .sections
        .iter()
        .map(|s| (s.id.as_str(), s.start_frame, s.end_frame))
        .collect();
    assert_eq!(
        sections,
        [("first", BAR, 2 * BAR), ("last", 2 * BAR, 3 * BAR)]
    );
}

#[test]
fn window_errors_are_explicit() {
    for (window, code) in [
        (Window::Region("missing".into()), "E_REFERENCE"),
        (Window::Bars(9, 12), "E_RANGE"),
    ] {
        let error = analyze(&AnalyzeOptions {
            window: Some(window),
            ..AnalyzeOptions::default()
        })
        .unwrap_err();
        assert_eq!(error.code, code);
    }
    let error = analyze(&AnalyzeOptions {
        preroll_seconds: Some(2.0),
        ..AnalyzeOptions::default()
    })
    .unwrap_err();
    assert_eq!(error.code, "E_USAGE");
    for text in ["bars:3-2", "bars:0-2", "bars:x-2", "region:", "whole"] {
        assert!(Window::parse(text).is_err(), "{text}");
    }
    assert_eq!(Window::parse("bars:2-5").unwrap(), Window::Bars(2, 5));
    assert_eq!(
        Window::parse("region:b").unwrap(),
        Window::Region("b".into())
    );
}
