//! The background session: a worker fills checkpoints while edits and renders
//! go on. Whatever the worker is doing, a render must equal a fresh render of
//! the plan most recently given, bit for bit, and work done for an older plan
//! must never be kept.

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use maac::dsp::{BackgroundSession, DspEngine, SessionPolicy, ESTIMATED_CHECKPOINT_BYTES};
use maac::plan::PlanLimits;
use maac::PlanArtifact;

#[path = "common/song_edits.rs"]
mod song_edits;
use song_edits::*;

const SPACING: u64 = 4_801;
const WAIT: Duration = Duration::from_secs(120);

fn limits() -> PlanLimits {
    PlanLimits::song()
}

fn bits(frames: &[f64]) -> Vec<u64> {
    frames.iter().map(|sample| sample.to_bits()).collect()
}

fn render_full(plan: &PlanArtifact) -> Vec<f64> {
    let mut out = Vec::new();
    DspEngine::new_artifact_with_limits(plan, &limits())
        .unwrap()
        .render(|frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    out
}

/// `start..end` of the session's render equals that range of a fresh render
/// of `plan`, which the test holds itself.
fn assert_range(session: &BackgroundSession, plan: &PlanArtifact, start: u64, end: u64, why: &str) {
    let channels = usize::from(plan.output().channels);
    let mut got = Vec::new();
    let report = session
        .render(start, end, |frame| {
            got.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    let fresh = render_full(plan);
    assert_eq!(
        bits(&fresh[start as usize * channels..end as usize * channels]),
        bits(&got),
        "{why}: range {start}..{end}, {report:?}"
    );
}

fn session(plan: &PlanArtifact, policy: SessionPolicy, auto: bool) -> BackgroundSession {
    BackgroundSession::new_with(plan, &limits(), policy, auto).unwrap()
}

#[test]
fn the_worker_fills_the_checkpoints_and_renders_stay_exact() {
    let plan = compile(&base_song(1)).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, SessionPolicy::new(SPACING), true);
    assert!(bg.wait_idle(WAIT), "the worker never finished");
    let status = bg.status();
    assert!(status.last_error.is_none(), "{status:?}");
    assert!(
        status.last_checkpoint + 2 * SPACING >= total,
        "the worker stopped at {} of {total}: {status:?}",
        status.last_checkpoint
    );
    let frames = bg.checkpoint_frames();
    assert!(frames.windows(2).all(|pair| pair[0] < pair[1]));
    assert_range(&bg, &plan, total * 3 / 4, total, "late range");
    assert_range(&bg, &plan, 12_345, 100_000, "middle range");
}

#[test]
fn without_auto_prerender_the_worker_waits_to_be_asked() {
    let plan = compile(&base_song(2)).unwrap();
    let bg = session(&plan, SessionPolicy::new(SPACING), false);
    assert!(bg.wait_idle(WAIT));
    assert_eq!(bg.status().checkpoints, 0);
    bg.prerender();
    assert!(bg.wait_idle(WAIT));
    assert!(bg.status().checkpoints > 10, "{:?}", bg.status());
}

/// Holds the worker at its first checkpoint until told to go on.
struct Gate {
    reached: mpsc::Receiver<u64>,
    go: mpsc::Sender<()>,
}

fn gate(bg: &BackgroundSession) -> Gate {
    let (reached_tx, reached) = mpsc::channel();
    let (go, go_rx) = mpsc::channel::<()>();
    let go_rx = Mutex::new(Some(go_rx));
    bg.set_worker_hook(move |frame| {
        // Only the first checkpoint is held.
        if let Some(go_rx) = go_rx.lock().unwrap().take() {
            reached_tx.send(frame).unwrap();
            let _ = go_rx.recv();
        }
    });
    Gate { reached, go }
}

/// A render finishing late must not put a checkpoint of the old plan into the
/// set. The worker is held just before committing its first checkpoint; an
/// early edit is given and rendered; then the worker is let go.
#[test]
fn work_for_an_older_plan_is_refused() {
    let song = base_song(3);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, SessionPolicy::new(SPACING), false);
    let gate = gate(&bg);
    bg.prerender();
    let held_at = gate
        .reached
        .recv_timeout(WAIT)
        .expect("worker reached a checkpoint");
    assert!(held_at > 0);

    // An edit that changes the sound before the held checkpoint, so that a
    // checkpoint of the old plan at that frame would be wrong for the new one.
    let channels = usize::from(plan.output().channels);
    let before = render_full(&plan);
    let mut rng = Rng(7);
    let mut plan2 = None;
    for index in 0..200 {
        let (next, _) = edit(&song, &mut rng, 3 + index);
        let Ok(p) = compile(&next) else { continue };
        let after = render_full(&p);
        let upto = held_at as usize * channels;
        if after.len() >= upto && bits(&after[..upto]) != bits(&before[..upto]) {
            plan2 = Some(p);
            break;
        }
    }
    let plan2 = plan2.expect("an edit that changes the sound before the held frame");
    bg.update(&plan2);
    // Applies the edit and renders a short stretch: no checkpoints come of it.
    let mut sink = Vec::new();
    bg.render(0, 64, |frame| {
        sink.extend_from_slice(frame);
        Ok(())
    })
    .unwrap();
    gate.go.send(()).unwrap();
    assert!(bg.wait_idle(WAIT));
    assert!(bg.status().last_error.is_none(), "{:?}", bg.status());
    // Late and middle ranges resume from whatever checkpoints are held.
    assert_range(&bg, &plan2, total * 3 / 4, total, "after the late commit");
    assert_range(
        &bg,
        &plan2,
        held_at,
        held_at + 20_000,
        "from the held frame",
    );
}

#[test]
fn update_does_not_wait_for_the_worker() {
    let plan = compile(&base_song(4)).unwrap();
    let bg = session(&plan, SessionPolicy::new(SPACING), false);
    let gate = gate(&bg);
    bg.prerender();
    gate.reached.recv_timeout(WAIT).unwrap();
    let started = Instant::now();
    for _ in 0..50 {
        bg.update(&plan);
    }
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "50 updates took {:?} with the worker busy",
        started.elapsed()
    );
    gate.go.send(()).unwrap();
    assert!(bg.wait_idle(WAIT));
}

#[test]
fn edits_and_renders_interleaved_with_the_worker_stay_exact() {
    for seed in 1..=3u64 {
        let mut song = base_song(10 + seed);
        let plan = compile(&song).unwrap();
        let total = plan.output().total_frames;
        let bg = session(&plan, SessionPolicy::new(SPACING), true);
        let mut rng = Rng(seed * 77);
        for index in 0..6 {
            // Let the worker get somewhere, a different distance each time.
            std::thread::sleep(Duration::from_millis(rng.below(40)));
            let (next, kind) = edit(&song, &mut rng, seed + index);
            let Ok(plan) = compile(&next) else { continue };
            song = next;
            bg.update(&plan);
            let start = rng.below(total - 1);
            let end = (start + 1 + rng.below(total)).min(total);
            assert_range(
                &bg,
                &plan,
                start,
                end,
                &format!("seed {seed} edit {index} {kind}"),
            );
            if index % 2 == 1 {
                // And again with no edit between.
                let again = rng.below(total - 1);
                assert_range(&bg, &plan, again, total, "again");
            }
        }
        assert!(bg.wait_idle(WAIT));
        assert!(bg.status().last_error.is_none(), "{:?}", bg.status());
    }
}

#[test]
fn dropping_a_busy_session_stops_its_worker() {
    let plan = compile(&base_song(5)).unwrap();
    let started = Instant::now();
    for _ in 0..5 {
        let bg = session(&plan, SessionPolicy::new(SPACING), true);
        std::thread::sleep(Duration::from_millis(20));
        drop(bg);
    }
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_tight_budget_holds_in_the_background_too() {
    let plan = compile(&base_song(6)).unwrap();
    let total = plan.output().total_frames;
    let policy = SessionPolicy::new(SPACING).with_budget_bytes(3 * ESTIMATED_CHECKPOINT_BYTES);
    let bg = session(&plan, policy, true);
    assert!(bg.wait_idle(WAIT));
    assert!(bg.status().checkpoints <= 3, "{:?}", bg.status());
    assert_range(&bg, &plan, total / 2, total, "thinned");
}

/// Not a pass/fail test: how long until an edit can be heard when the worker
/// has filled the checkpoints, and when it is still busy. Run with
/// `--ignored --nocapture --release`; `MAAC_SONG` picks the song.
#[test]
#[ignore]
fn background_latency_on_a_real_song() {
    use maac::compiler::compile_bundle_artifact_with_limits;
    use maac::dsp::{CheckpointSet, Reuse};
    use maac::SourceBundle;

    let song = std::env::var("MAAC_SONG").unwrap_or_else(|_| "afterlight".into());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(&song)
        .join(format!("{song}.maac"));
    let text = std::fs::read_to_string(path).unwrap();
    let compile_text = |text: &str| {
        compile_bundle_artifact_with_limits(
            &SourceBundle::new("standalone.maac", text.to_string()),
            &limits(),
        )
    };
    let base = compile_text(&text).unwrap();
    let total = base.output().total_frames;
    let rate = u64::from(base.output().sample_rate_hz);
    let spacing = rate * 4 + 37;
    let started = Instant::now();
    let bg = session(&base, SessionPolicy::new(spacing), true);
    assert!(bg.wait_idle(Duration::from_secs(900)));
    println!(
        "{song}: {:.0} s of audio; the worker filled {} checkpoints in {:.1} s",
        total as f64 / rate as f64,
        bg.status().checkpoints,
        started.elapsed().as_secs_f64()
    );
    // Edits pile up, as in a session: each is made on the text the last one
    // left, and measured against the plan the session holds.
    let mut text = text;
    let mut previous = base.clone();
    let needle = "velocity = ";
    let mut tried = 0;
    let offsets: Vec<usize> = text.rmatch_indices(needle).map(|(i, _)| i).collect();
    for index in offsets {
        let digits = &text[index + needle.len()..];
        let end = digits
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '/'))
            .unwrap_or(0);
        if end == 0 {
            continue;
        }
        let edited = format!(
            "{}{}0.3{}{}",
            &text[..index],
            needle,
            tried + 1,
            &text[index + needle.len() + end..]
        );
        let Ok(plan) = compile_text(&edited) else {
            continue;
        };
        let reference = CheckpointSet::without_checkpoints(&previous, &limits()).unwrap();
        let Reuse::From(bound) = reference.reuse_for(&plan).unwrap() else {
            continue;
        };
        // Every other edit is made while the worker is still busy with the
        // last one.
        let busy = bg.status().busy;
        bg.update(&plan);
        let started = Instant::now();
        let mut first_sound = None;
        let end_frame = (bound + rate * 2).min(total);
        let report = bg
            .render(bound, end_frame, |_| {
                first_sound.get_or_insert_with(|| started.elapsed().as_secs_f64());
                Ok(())
            })
            .unwrap();
        let window = started.elapsed().as_secs_f64();
        let waited = Instant::now();
        if tried % 2 == 0 {
            assert!(bg.wait_idle(Duration::from_secs(900)));
        }
        println!(
            "  edit at {:.1} s (worker {}): first sound {:.2} s, 2 s window {window:.2} s, started at {:.1} s{}",
            bound as f64 / rate as f64,
            if busy { "busy" } else { "idle" },
            first_sound.unwrap_or(f64::NAN),
            report.started_at as f64 / rate as f64,
            if tried % 2 == 0 {
                format!("; worker idle again after {:.1} s", waited.elapsed().as_secs_f64())
            } else {
                String::new()
            },
        );
        text = edited;
        previous = plan;
        tried += 1;
        if tried >= 6 {
            break;
        }
    }
}
