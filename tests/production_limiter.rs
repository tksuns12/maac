//! `fx.limiter/1`: the lookahead true-peak limiter of the production extension.

use maac::compiler::compile_bundle_artifact;
use maac::production_analysis::Analyzer;
use maac::{render_artifact, DiagnosticCode, PlanArtifact, SourceBundle};

/// One second at 120 bpm; `chain` routes `&src:out` to `&out:out`.
fn source(signal: &str, chain: &str) -> String {
    format!(
        r#"maac 1;
project p {{ score = [0q, 2q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &out:out; tail = 50ms; requires = ["maac.production/1"]; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
{signal}
{chain}
"#
    )
}

const TONE: &str = r#"node src { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.9; }; }
pattern tone { length = 2q; note a { at = 0q; dur = 2q; pitch = 11025Hz; } }
track t { target = &src:events; }
place play { pattern = &tone; track = &t; at = 0q; }"#;

const NOISE: &str = r#"node src { type = "core.noise/1"; config = { channels = 2; seed = 7; }; }"#;

fn limiter(config: &str, params: &str) -> String {
    format!(
        r#"node out {{ type = "fx.limiter/1"; config = {{ {config} }}; params = {{ {params} }}; }}
connect route {{ from = &src:out; to = &out:in; }}"#
    )
}

fn plan(source: &str) -> PlanArtifact {
    compile_bundle_artifact(&SourceBundle::new("main.maac", source))
        .unwrap_or_else(|e| panic!("compiles: {e}"))
}

fn frames(source: &str) -> Vec<Vec<f64>> {
    let mut frames = Vec::new();
    render_artifact(&plan(source), |frame| {
        frames.push(frame.to_vec());
        Ok(())
    })
    .unwrap();
    frames
}

/// (sample peak, true peak) in dB, through the delivery analyzer.
fn peaks(frames: &[Vec<f64>]) -> (f64, f64) {
    let mut analyzer = Analyzer::new(48000, frames[0].len() as u8).unwrap();
    for frame in frames {
        analyzer.push_frame(frame).unwrap();
    }
    let analysis = analyzer.finish().unwrap();
    (
        analysis.sample_peak.value.unwrap(),
        analysis.true_peak.value.unwrap(),
    )
}

#[test]
fn driven_noise_never_passes_the_ceiling() {
    let ceiling = 10.0_f64.powf(-1.0 / 20.0);
    let out = frames(&source(
        NOISE,
        &limiter("channels = 2;", "gain = 12dB; ceiling = -1dB;"),
    ));
    let sample_peak = out
        .iter()
        .flatten()
        .fold(0.0_f64, |peak, x| peak.max(x.abs()));
    assert!(sample_peak <= ceiling, "{sample_peak}");
    let (_, true_peak) = peaks(&out);
    println!("driven noise true peak: {true_peak} dBTP");
    assert!(true_peak <= -1.0 + 0.1, "{true_peak}");
}

#[test]
fn an_inter_sample_peak_is_caught() {
    // A tone near a quarter of the sample rate, whose samples miss its
    // crests: the true peak is above the sample peak.
    let out = frames(&source(
        TONE,
        &limiter("channels = 1;", "gain = 6dB; ceiling = -1dB;"),
    ));
    let (sample_peak, true_peak) = peaks(&out);
    println!("tone sample peak {sample_peak} dBFS, true peak {true_peak} dBTP");
    assert!(sample_peak <= -1.0 + 1e-9, "{sample_peak}");
    assert!(true_peak <= -1.0 + 0.1, "{true_peak}");
}

#[test]
fn quiet_input_is_only_delayed_by_the_declared_latency() {
    let dry = source(
        TONE,
        r#"node out { type = "core.gain/1"; config = { channels = 1; }; params = { gain = 0.1; }; }
connect route { from = &src:out; to = &out:in; }"#,
    );
    let limited = dry
        .replace(
            "node out { type = \"core.gain/1\"",
            "node quiet { type = \"core.gain/1\"",
        )
        .replace("connect route { from = &src:out; to = &out:in; }", "")
        + r#"node out { type = "fx.limiter/1"; config = { channels = 1; lookahead = 0.25ms; }; }
connect to_quiet { from = &src:out; to = &quiet:in; }
connect to_out { from = &quiet:out; to = &out:in; }"#;
    let dry = frames(&dry);
    let limited = frames(&limited);
    assert_eq!(dry.len(), limited.len());
    // 0.25 ms is 12 frames; nothing reaches the -1 dB default ceiling.
    for n in 12..dry.len() {
        assert_eq!(limited[n], dry[n - 12], "frame {n}");
    }
    assert!(limited[..12].iter().all(|frame| frame[0] == 0.0));
    let wire: serde_json::Value = serde_json::from_slice(
        &plan(&source(TONE, &limiter("channels = 1;", "")))
            .to_json()
            .unwrap(),
    )
    .unwrap();
    let node = wire["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "out")
        .unwrap();
    assert_eq!(node["processor"]["kind"], "fx.limiter/1");
    assert_eq!(node["processor"]["lookahead_frames"], 72);
}

#[test]
fn the_gain_recovers_after_a_peak() {
    // A loud first half and a quiet second half: the quiet half returns to
    // unity gain within a few release time constants.
    let source = source(
        r#"node src { type = "core.sine/1"; params = { attack = 0s; release = 0s; level = 0.9; }; }
pattern tone { length = 2q;
  note a { at = 0q; dur = 1q; pitch = 440Hz; velocity = 1; }
  note b { at = 1q; dur = 1q; pitch = 440Hz; velocity = 0.1; }
}
track t { target = &src:events; }
place play { pattern = &tone; track = &t; at = 0q; }"#,
        &limiter(
            "channels = 1;",
            "gain = 6dB; ceiling = -6dB; release = 20ms;",
        ),
    );
    let out = frames(&source);
    // Frames 36000..48000 lie 250 ms (12 release constants) into the quiet half.
    let quiet_peak = out[36000..48000]
        .iter()
        .fold(0.0_f64, |peak, f| peak.max(f[0].abs()));
    let expected = 0.9 * 0.1 * 10.0_f64.powf(6.0 / 20.0);
    assert!(
        (quiet_peak - expected).abs() < 1e-3,
        "{quiet_peak} against {expected}"
    );
}

#[test]
fn configuration_and_parameters_are_validated() {
    let code = |chain: String| {
        compile_bundle_artifact(&SourceBundle::new("main.maac", source(TONE, &chain)))
            .unwrap_err()
            .iter()
            .next()
            .unwrap()
            .code
    };
    assert_eq!(
        code(limiter("channels = 1; lookahead = 0.2ms;", "")),
        DiagnosticCode::Range
    );
    assert_eq!(
        code(limiter("channels = 1; lookahead = 11ms;", "")),
        DiagnosticCode::Range
    );
    assert_eq!(
        code(limiter("channels = 1; knee = 1dB;", "")),
        DiagnosticCode::UnknownField
    );
    assert_eq!(
        code(limiter("channels = 1;", "ceiling = 1dB;")),
        DiagnosticCode::Range
    );
    assert_eq!(
        code(limiter("channels = 1;", "gain = -1dB;")),
        DiagnosticCode::Range
    );
    assert_eq!(
        code(limiter("channels = 1;", "release = 0s;")),
        DiagnosticCode::Range
    );
    assert_eq!(code(limiter("channels = 3;", "")), DiagnosticCode::Range);
    let without_capability = source(TONE, &limiter("channels = 1;", ""))
        .replace(r#" requires = ["maac.production/1"];"#, "");
    assert_eq!(
        compile_bundle_artifact(&SourceBundle::new("main.maac", without_capability))
            .unwrap_err()
            .iter()
            .next()
            .unwrap()
            .code,
        DiagnosticCode::Capability
    );
}
