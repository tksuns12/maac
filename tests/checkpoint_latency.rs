//! How long an edit takes to hear with checkpoints, on a real song. Not a
//! pass/fail test: run with `--ignored --nocapture --release` and read the
//! numbers. `MAAC_SONG` picks the song (default afterlight).

use std::path::Path;
use std::time::Instant;

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{CheckpointSet, DspEngine, RenderSpan, Reuse};
use maac::plan::PlanLimits;
use maac::{PlanArtifact, SourceBundle};

fn compile(text: &str) -> Result<PlanArtifact, String> {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("standalone.maac", text.to_owned()),
        &PlanLimits::song(),
    )
    .map_err(|error| format!("{error:?}"))
}

#[test]
#[ignore]
fn edit_to_hear_latency_on_a_real_song() {
    let song = std::env::var("MAAC_SONG").unwrap_or_else(|_| "afterlight".into());
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(&song)
        .join(format!("{song}.maac"));
    let text = std::fs::read_to_string(path).unwrap();
    let limits = PlanLimits::song();

    let started = Instant::now();
    let base = compile(&text).unwrap();
    let compile_secs = started.elapsed().as_secs_f64();
    let total = base.output().total_frames;
    let rate = u64::from(base.output().sample_rate_hz);
    let spacing = rate * 4 + 37;

    let started = Instant::now();
    let set = CheckpointSet::render(&base, &limits, spacing, |_| Ok(())).unwrap();
    let render_secs = started.elapsed().as_secs_f64();
    println!(
        "{song}: {:.0} s of audio; compile {compile_secs:.2} s; full render with {} checkpoints {render_secs:.1} s",
        total as f64 / rate as f64,
        set.checkpoints().len()
    );

    // Edits: change the last few velocities written in the text.
    let needle = "velocity = ";
    let mut tried = 0;
    for (index, _) in text.rmatch_indices(needle) {
        let digits = &text[index + needle.len()..];
        let end = digits
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '/'))
            .unwrap_or(0);
        if end == 0 {
            continue;
        }
        let edited = format!(
            "{}{}0.31{}",
            &text[..index],
            needle,
            &text[index + needle.len() + end..]
        );
        let started = Instant::now();
        let Ok(plan) = compile(&edited) else {
            continue;
        };
        let compile_new = started.elapsed().as_secs_f64();

        let started = Instant::now();
        let reuse = set.reuse_for(&plan).unwrap();
        let diff_secs = started.elapsed().as_secs_f64();
        let Reuse::From(bound) = reuse else {
            println!("  edit at text offset {index}: {reuse:?}");
            continue;
        };

        let started = Instant::now();
        let mut resumed = set.resume(&plan).unwrap();
        let resume_secs = started.elapsed().as_secs_f64();
        let start = resumed.start_frame;
        // Hear a 2 s window that starts at the first affected frame.
        let end = (bound + rate * 2).min(total);
        let started = Instant::now();
        resumed
            .engine
            .render_span(RenderSpan { start, end }, |_| Ok(()))
            .unwrap();
        let window_secs = started.elapsed().as_secs_f64();

        // For comparison, once: the same window by rendering from frame 0.
        let from_zero = if tried == 0 {
            let started = Instant::now();
            DspEngine::new_artifact_with_limits(&plan, &limits)
                .unwrap()
                .render_span(RenderSpan { start: 0, end }, |_| Ok(()))
                .unwrap();
            started.elapsed().as_secs_f64()
        } else {
            f64::NAN
        };
        println!(
            "  bound {:.1} s (checkpoint {:.1} s): compile {compile_new:.2} s, diff {diff_secs:.2} s (builds an engine), resume {resume_secs:.2} s, render window {window_secs:.2} s; from frame 0 {from_zero:.1} s",
            bound as f64 / rate as f64,
            start as f64 / rate as f64,
        );
        tried += 1;
        if tried >= 5 {
            break;
        }
    }
}

/// What differs in the plan, outside its notes and automation, after an edit.
#[test]
#[ignore]
fn what_an_edit_changes_in_a_real_plan() {
    fn walk(path: String, a: &serde_json::Value, b: &serde_json::Value, out: &mut Vec<String>) {
        use serde_json::Value;
        if a == b {
            return;
        }
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                for key in x.keys().chain(y.keys()) {
                    walk(
                        format!("{path}.{key}"),
                        x.get(key).unwrap_or(&Value::Null),
                        y.get(key).unwrap_or(&Value::Null),
                        out,
                    );
                }
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    walk(format!("{path}[{i}]"), p, q, out);
                }
            }
            _ => out.push(format!(
                "{path}: {} vs {}",
                a.to_string().chars().take(90).collect::<String>(),
                b.to_string().chars().take(90).collect::<String>()
            )),
        }
    }
    let song = std::env::var("MAAC_SONG").unwrap_or_else(|_| "afterlight".into());
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(&song)
        .join(format!("{song}.maac"));
    let text = std::fs::read_to_string(path).unwrap();
    let (mut edited, mut found) = (String::new(), false);
    for (index, _) in text.rmatch_indices("velocity = ") {
        let rest = &text[index + 11..];
        let Some(end) = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')) else {
            continue;
        };
        if end == 0 {
            continue;
        }
        edited = format!("{}velocity = 0.31{}", &text[..index], &rest[end..]);
        if compile(&edited).is_ok() {
            found = true;
            break;
        }
    }
    assert!(found, "no compilable velocity edit");
    let json = |t: &str| -> serde_json::Value {
        serde_json::from_slice(
            &compile(t)
                .unwrap()
                .to_json_with_limits(&PlanLimits::song())
                .unwrap(),
        )
        .unwrap()
    };
    let mut out = Vec::new();
    walk("plan".into(), &json(&text), &json(&edited), &mut out);
    out.sort();
    out.dedup();
    for line in out.iter().filter(|l| {
        !l.starts_with("plan.events")
            && !l.starts_with("plan.automation")
            && !l.starts_with("plan.source_mappings")
    }) {
        println!("DIFF {line}");
    }
    println!("({} differences in total)", out.len());
}
