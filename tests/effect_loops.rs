//! Feedback inside effects: a cycle in an effect's graph may close through a
//! delay line's `in`, and `synth.width/1` may mirror the sides.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::plan::PlanLimits;
use maac::{render_artifact_with_limits, Diagnostic, DiagnosticCode, PlanArtifact, SourceBundle};

/// A short A4 at `src`, with `desk` written beside it; the project's output
/// is `fx`'s `out`.
fn song(desk: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 4q]; rate = 48000Hz; tail = 1s; tempo = &clock; meter = &metre; output = &fx:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
node src {{ type = "core.sine/1"; params = {{ level = 0.5; }}; }}
pattern c {{ length = 4q; note a {{ at = 0q; dur = 1/4q; pitch = A4; velocity = 0.8; }} }}
track t {{ target = &src:events; }}
place pl {{ pattern = &c; track = &t; at = 0q; }}
{desk}
"#
    )
}

fn compile(source: &str) -> Result<PlanArtifact, maac::Diagnostics> {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("main.maac", source),
        &PlanLimits::song(),
    )
}

fn render(source: &str) -> Vec<f64> {
    let plan = compile(source).unwrap_or_else(|error| panic!("{error:?}"));
    let mut out = Vec::new();
    render_artifact_with_limits(&plan, &PlanLimits::song(), |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    out
}

/// The one error a document fails with.
fn failure(source: &str) -> Diagnostic {
    let diagnostics = compile(source).expect_err("the document should fail");
    let mut all = diagnostics.iter();
    let first = all.next().expect("one diagnostic").clone();
    assert!(all.next().is_none(), "{diagnostics:?}");
    first
}

/// A mono echo in one effect: the line feeds back to its own `in` through a
/// low-pass and a gain. `line` is the delay node's parameters.
fn tape(line: &str, regen: &str) -> String {
    format!(
        r#"effect tape {{
  channels = 1;
  shared main {{
    channels = 1; output = &line:out;
    node feed {{ type = "synth.mix/1"; config = {{ channels = 1; }}; }}
    node line {{ type = "synth.delay/1"; config = {{ channels = 1; max = 1s; }}; params = {{ {line} }}; }}
    node tone {{ type = "synth.onepole/1"; config = {{ channels = 1; }}; params = {{ cutoff = 2000Hz; }}; }}
    node regen {{ type = "synth.gain/1"; config = {{ channels = 1; }}; params = {{ level = {regen}; }}; }}
    connect in_feed {{ from = &input:out; to = &feed:in; }}
    connect feed_line {{ from = &feed:out; to = &line:in; }}
    connect line_tone {{ from = &line:out; to = &tone:in; }}
    connect tone_regen {{ from = &tone:out; to = &regen:in; }}
    connect regen_feed {{ from = &regen:out; to = &feed:in; }}
  }}
}}
node fx {{ effect = &tape; }}
connect src_fx {{ from = &src:out; to = &fx:in; }}"#
    )
}

#[test]
fn a_loop_in_an_effect_renders_as_the_project_graph_loop_one_frame_later() {
    // Today's way: the loop runs through the project graph, where one frame
    // of core.delay/1 breaks it, so each pass takes the line's 375 frames
    // plus one.
    let project = song(
        r#"effect line_only {
  channels = 1;
  shared main {
    channels = 1; output = &line:out;
    node line { type = "synth.delay/1"; config = { channels = 1; max = 1s; }; params = { time = 1/128s; }; }
    connect in_line { from = &input:out; to = &line:in; }
  }
}
effect darken {
  channels = 1;
  shared main {
    channels = 1; output = &tone:out;
    node tone { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 2000Hz; }; }
    connect in_tone { from = &input:out; to = &tone:in; }
  }
}
node fx { effect = &line_only; }
node worn { effect = &darken; }
node gap { type = "core.delay/1"; config = { channels = 1; frames = 1; }; }
node regen { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 0.9; }; }
connect a_src { from = &src:out; to = &fx:in; }
connect fx_worn { from = &fx:out; to = &worn:in; }
connect worn_gap { from = &worn:out; to = &gap:in; }
connect gap_regen { from = &gap:out; to = &regen:in; }
connect z_loop { from = &regen:out; to = &fx:in; }"#,
    );
    // In one effect the line is 376 frames long and holds that frame itself.
    let effect = song(&tape("time = 47/6000s;", "0.9"));
    let before = render(&project);
    let after = render(&effect);
    assert_eq!(before.len(), after.len());
    assert_eq!(after[0], 0.0);
    assert_eq!(&after[1..], &before[..before.len() - 1]);
    // The loop really repeats: there is sound 125 ms after the note ends.
    assert!(peak(&after, 12_000, 14_400) > 1e-3);
}

/// The largest absolute sample in `samples[start..end]`.
fn peak(samples: &[f64], start: usize, end: usize) -> f64 {
    samples[start..end]
        .iter()
        .fold(0.0, |peak, s| s.abs().max(peak))
}

#[test]
fn a_loop_through_a_drive_runs_away_and_the_drive_holds_it() {
    let echo = |regen: &str| {
        render(&song(&format!(
            r#"effect tape {{
  channels = 1;
  shared main {{
    channels = 1; output = &heat:out;
    node feed {{ type = "synth.mix/1"; config = {{ channels = 1; }}; }}
    node line {{ type = "synth.delay/1"; config = {{ channels = 1; max = 1s; }}; params = {{ time = 50ms; }}; }}
    node heat {{ type = "synth.drive/1"; config = {{ channels = 1; }}; params = {{ drive = 2; reference = 1/4; }}; }}
    node thin {{ type = "synth.highpass/1"; config = {{ channels = 1; }}; params = {{ cutoff = 100Hz; }}; }}
    node regen {{ type = "synth.gain/1"; config = {{ channels = 1; }}; params = {{ level = {regen}; }}; }}
    connect in_feed {{ from = &input:out; to = &feed:in; }}
    connect feed_line {{ from = &feed:out; to = &line:in; }}
    connect line_heat {{ from = &line:out; to = &heat:in; }}
    connect heat_thin {{ from = &heat:out; to = &thin:in; }}
    connect thin_regen {{ from = &thin:out; to = &regen:in; }}
    connect regen_feed {{ from = &regen:out; to = &feed:in; }}
  }}
}}
node fx {{ effect = &tape; }}
connect src_fx {{ from = &src:out; to = &fx:in; }}"#
        )))
    };
    // The note lasts 1/4q, 125 ms; each pass takes 50 ms, so the first
    // echo alone sounds from 50 ms to 100 ms.
    let first_echo = |samples: &[f64]| peak(samples, 2_400, 4_800);
    let late = |samples: &[f64]| peak(samples, 48_000 * 2, 48_000 * 3);
    let dying = echo("0.5");
    assert!(late(&dying) < first_echo(&dying) / 1_000.0);
    let runaway = echo("1.25");
    assert!(late(&runaway) > first_echo(&runaway));
    assert!(runaway
        .iter()
        .all(|sample| sample.is_finite() && sample.abs() < 4.0));
}

#[test]
fn nothing_limits_a_loop_that_grows_without_bound() {
    // A loop gain of 16 through a one-frame line overflows within a few
    // hundred frames of the note, and the render stops there.
    let plan = compile(&song(&tape("time = 1/48000s;", "16"))).unwrap();
    let error = render_artifact_with_limits(&plan, &PlanLimits::song(), |_| Ok(())).unwrap_err();
    assert!(
        format!("{error:?}").to_lowercase().contains("nonfinite"),
        "{error:?}"
    );
}

#[test]
fn a_cycle_that_passes_through_no_delay_line_is_refused_and_named() {
    let without_line = song(
        r#"effect ring {
  channels = 1;
  shared main {
    channels = 1; output = &regen:out;
    node feed { type = "synth.mix/1"; config = { channels = 1; }; }
    node regen { type = "synth.gain/1"; config = { channels = 1; }; params = { level = 0.5; }; }
    connect in_feed { from = &input:out; to = &feed:in; }
    connect feed_regen { from = &feed:out; to = &regen:in; }
    connect regen_feed { from = &regen:out; to = &feed:in; }
  }
}
node fx { effect = &ring; }
connect src_fx { from = &src:out; to = &fx:in; }"#,
    );
    let error = failure(&without_line);
    assert_eq!(error.code, DiagnosticCode::AlgebraicLoop);
    assert!(
        error
            .message
            .contains("same-sample cycle feed -> regen -> feed"),
        "{}",
        error.message
    );

    // A line's length still depends on what modulates it in the same frame.
    let through_time = song(
        r#"effect wobble {
  channels = 1;
  shared main {
    channels = 1; output = &line:out;
    node line { type = "synth.delay/1"; config = { channels = 1; max = 1s; }; params = { time = 10ms; }; }
    node tone { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 2000Hz; }; }
    connect in_line { from = &input:out; to = &line:in; }
    connect line_tone { from = &line:out; to = &tone:in; }
    modulate sway { from = &tone:out; to = &line.params.time; depth = 1ms; }
  }
}
node fx { effect = &wobble; }
connect src_fx { from = &src:out; to = &fx:in; }"#,
    );
    let error = failure(&through_time);
    assert_eq!(error.code, DiagnosticCode::AlgebraicLoop);
    assert!(
        error
            .message
            .contains("same-sample cycle line -> tone -> line"),
        "{}",
        error.message
    );
}

/// A stereo source, panned left of centre, through one `synth.width/1`.
fn wide(width: &str) -> String {
    song(&format!(
        r#"node place {{ type = "core.pan/1"; params = {{ pan = -0.4; }}; }}
connect src_place {{ from = &src:out; to = &place:in; }}
effect field {{
  channels = 2;
  shared main {{
    channels = 2; output = &wide:out;
    node wide {{ type = "synth.width/1"; params = {{ width = {width}; }}; }}
    connect in_wide {{ from = &input:out; to = &wide:in; }}
  }}
}}
node fx {{ effect = &field; }}
connect place_fx {{ from = &place:out; to = &fx:in; }}"#
    ))
}

#[test]
fn a_negative_width_mirrors_the_sides() {
    let dry = render(&wide("1"));
    let swapped = render(&wide("-1"));
    assert!(dry.chunks(2).any(|frame| frame[0] != frame[1]));
    for (dry, swapped) in dry.chunks(2).zip(swapped.chunks(2)) {
        assert!((dry[0] - swapped[1]).abs() < 1e-15);
        assert!((dry[1] - swapped[0]).abs() < 1e-15);
    }
    // Minus a half sends three quarters of each side across.
    let crossed = render(&wide("-1/2"));
    for (dry, crossed) in dry.chunks(2).zip(crossed.chunks(2)) {
        assert!((crossed[0] - (dry[0] / 4.0 + 3.0 * dry[1] / 4.0)).abs() < 1e-15);
        assert!((crossed[1] - (3.0 * dry[0] / 4.0 + dry[1] / 4.0)).abs() < 1e-15);
    }
    render(&wide("-4"));
    assert_eq!(failure(&wide("-5")).code, DiagnosticCode::Range);
    assert_eq!(failure(&wide("5")).code, DiagnosticCode::Range);
}
