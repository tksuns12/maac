//! The background session: a worker fills checkpoints while edits and renders
//! go on. Whatever the worker is doing, a render must equal a fresh render of
//! the plan most recently given, bit for bit, and work done for an older plan
//! must never be kept.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use maac::dsp::{
    BackgroundSession, CheckpointSet, DspEngine, Reuse, SessionPolicy, ESTIMATED_CHECKPOINT_BYTES,
};
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
    /// Checkpoints the worker has tried to commit, the held one included.
    commits: Arc<AtomicUsize>,
}

fn gate(bg: &BackgroundSession) -> Gate {
    let (reached_tx, reached) = mpsc::channel();
    let (go, go_rx) = mpsc::channel::<()>();
    let go_rx = Mutex::new(Some(go_rx));
    let commits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&commits);
    bg.set_worker_hook(move |frame| {
        counted.fetch_add(1, Ordering::SeqCst);
        // Only the first checkpoint is held.
        if let Some(go_rx) = go_rx.lock().unwrap().take() {
            reached_tx.send(frame).unwrap();
            let _ = go_rx.recv();
        }
    });
    Gate {
        reached,
        go,
        commits,
    }
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
    // The refused work is tried again for the new plan, by the worker alone.
    assert!(
        bg.status().last_checkpoint + 2 * SPACING >= total,
        "the worker did not carry on: {:?}",
        bg.status()
    );
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

/// A render holds the session for as long as it runs; `update` and `status`
/// must not wait for it.
#[test]
fn update_and_status_do_not_wait_for_a_render() {
    let plan = compile(&base_song(4)).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, SessionPolicy::new(SPACING), false);
    let (inside, inside_rx) = mpsc::channel::<()>();
    let (go, go_rx) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let bg = &bg;
        let render = scope.spawn(move || {
            let mut first = true;
            bg.render(0, total, |_| {
                if std::mem::take(&mut first) {
                    inside.send(()).unwrap();
                    let _ = go_rx.recv();
                }
                Ok(())
            })
            .unwrap();
        });
        inside_rx.recv_timeout(WAIT).unwrap();
        let started = Instant::now();
        for _ in 0..50 {
            bg.update(&plan);
            let _ = bg.status();
        }
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "50 updates and statuses took {:?} during a render",
            started.elapsed()
        );
        go.send(()).unwrap();
        render.join().unwrap();
    });
}

/// An update cuts the worker's stretch short instead of letting it finish the
/// old plan's tail. Counted in checkpoints the worker tried to commit: about
/// one song's worth if it stops, twice that if it does not.
#[test]
fn an_update_stops_the_stretch_in_progress() {
    let song = base_song(8);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, SessionPolicy::new(SPACING), false);
    let gate = gate(&bg);
    bg.prerender();
    gate.reached.recv_timeout(WAIT).unwrap();
    // An edit that changes the song near its start: nothing of the old tail
    // survives it.
    let reference = CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut rng = Rng(11);
    let early = (0..200)
        .find_map(|index| {
            let plan = compile(&edit(&song, &mut rng, 8 + index).0).ok()?;
            matches!(reference.reuse_for(&plan).unwrap(), Reuse::From(bound) if bound < total / 20)
                .then_some(plan)
        })
        .expect("an early edit");
    // With the worker free to carry on after an update, the commits of the
    // old stretch and of the new one add up; they are few only if the update
    // cut the old one short.
    bg.set_auto_prerender(true);
    bg.update(&early);
    gate.go.send(()).unwrap();
    assert!(bg.wait_idle(WAIT));
    let one_song = (total / SPACING) as usize;
    let commits = gate.commits.load(Ordering::SeqCst);
    assert!(
        commits <= one_song + one_song / 4,
        "{commits} commits for a song of {one_song} checkpoints: the old stretch ran on"
    );
    assert_range(&bg, &early, total / 2, total, "after the update");
}

/// Update, let the worker take it up on its own, then render: the worker's
/// adoption must have dropped what the edit invalidated.
#[test]
fn the_worker_takes_up_an_edit_by_itself() {
    let song = base_song(9);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, SessionPolicy::new(SPACING), true);
    assert!(bg.wait_idle(WAIT));
    let reference = CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut rng = Rng(13);
    let (early, bound) = (0..200)
        .find_map(|index| {
            let plan = compile(&edit(&song, &mut rng, 9 + index).0).ok()?;
            match reference.reuse_for(&plan).unwrap() {
                Reuse::From(bound) if bound < total / 20 => Some((plan, bound)),
                _ => None,
            }
        })
        .expect("an early edit");
    bg.update(&early);
    assert!(bg.wait_idle(WAIT));
    assert!(bg.status().last_error.is_none(), "{:?}", bg.status());
    // Ranges that begin just after the edit: a checkpoint of the old plan
    // there would be wrong for the new one.
    for start in [bound + 1, bound + SPACING, bound + 3 * SPACING] {
        assert_range(
            &bg,
            &early,
            start,
            start + 30_000,
            "worker adopted the edit",
        );
    }
    assert_range(&bg, &early, total * 3 / 4, total, "late range");
}

/// A set at its budget does not render the same tail again at every wake.
#[test]
fn a_set_at_its_budget_is_left_alone() {
    let plan = compile(&base_song(6)).unwrap();
    let policy = SessionPolicy::new(SPACING).with_budget_bytes(3 * ESTIMATED_CHECKPOINT_BYTES);
    let bg = session(&plan, policy, true);
    let gate = gate(&bg);
    gate.go.send(()).unwrap(); // nothing is held
    assert!(bg.wait_idle(WAIT));
    // Below its budget the set is filled in further; once it is full, asking
    // again renders nothing.
    let mut counts = Vec::new();
    for _ in 0..6 {
        bg.prerender();
        assert!(bg.wait_idle(WAIT));
        counts.push(gate.commits.load(Ordering::SeqCst));
    }
    assert!(
        counts[4] == counts[5] && counts[3] == counts[5],
        "the worker kept rendering with nothing to add: {counts:?}"
    );
    assert!(bg.status().checkpoints <= 3, "{:?}", bg.status());
}

#[test]
fn the_worker_keeps_to_the_budget_while_adding() {
    let plan = compile(&base_song(7)).unwrap();
    let total = plan.output().total_frames;
    let policy = SessionPolicy::new(SPACING).with_budget_bytes(3 * ESTIMATED_CHECKPOINT_BYTES);
    let bg = session(&plan, policy, false);
    // A render of the first quarter leaves a few checkpoints; the worker then
    // adds more than the budget allows.
    let mut sink = 0usize;
    bg.render(0, total / 4, |_| {
        sink += 1;
        Ok(())
    })
    .unwrap();
    bg.prerender();
    assert!(bg.wait_idle(WAIT));
    assert!(bg.status().checkpoints <= 3, "{:?}", bg.status());
    assert_range(&bg, &plan, total / 2, total, "thinned by the worker");
}

/// A panic in a render's callback must not leave the worker stopped for good.
#[test]
fn a_panicking_callback_does_not_stop_the_worker() {
    let plan = compile(&base_song(12)).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, SessionPolicy::new(SPACING), false);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = bg.render(0, 64, |_| panic!("callback panic"));
    }));
    assert!(outcome.is_err());
    bg.prerender();
    assert!(bg.wait_idle(WAIT), "the worker never ran after the panic");
    assert!(bg.status().checkpoints > 10, "{:?}", bg.status());
    assert_range(&bg, &plan, total / 2, total, "after the panic");
}

/// A panic in the worker is reported, and the session carries on.
#[test]
fn a_panicking_worker_is_reported_and_the_session_goes_on() {
    let plan = compile(&base_song(13)).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, SessionPolicy::new(SPACING), false);
    let fired = AtomicUsize::new(0);
    bg.set_worker_hook(move |_| {
        if fired.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("worker panic");
        }
    });
    bg.prerender();
    assert!(bg.wait_idle(WAIT), "wait_idle hung after a worker panic");
    let status = bg.status();
    assert!(
        status
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("panicked")),
        "{status:?}"
    );
    assert_range(&bg, &plan, total / 2, total, "after the worker panic");
}

/// Dropping a busy session returns only once the worker is gone: nothing the
/// worker held is alive afterwards.
#[test]
fn dropping_a_session_joins_its_worker() {
    let plan = compile(&base_song(14)).unwrap();
    for _ in 0..3 {
        let bg = session(&plan, SessionPolicy::new(SPACING), true);
        let token = Arc::new(());
        let weak = Arc::downgrade(&token);
        bg.set_worker_hook(move |_| {
            let _keep = &token;
            std::thread::sleep(Duration::from_millis(5));
        });
        std::thread::sleep(Duration::from_millis(30));
        drop(bg);
        assert!(weak.upgrade().is_none(), "the worker outlived its session");
    }
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
