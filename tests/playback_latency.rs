//! Not a pass/fail test: how soon an edit is heard while a song plays, on a
//! real song. Run with
//! `MAAC_SONG=afterlight cargo test --release --test playback_latency -- --ignored --nocapture`.
//!
//! A reader pulls the sound at the speed it plays (512 frames at a time) while
//! the test edits the song. For each edit it reports how much sound after the
//! playhead was the old plan's, and how long the producer took. Underruns
//! (reads that came up short) are counted apart: those before the edit are the
//! wait for the first sound at start-up, those after it are the edit's.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{BackgroundSession, CheckpointSet, PlaybackConfig, Reuse, SessionPolicy};
use maac::plan::PlanLimits;
use maac::{PlanArtifact, SourceBundle};

fn limits() -> PlanLimits {
    PlanLimits::song()
}

#[test]
#[ignore]
fn playback_latency_on_a_real_song() {
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
    let second = rate as f64;
    let spacing = rate * 4 + 37;
    let reference = CheckpointSet::without_checkpoints(&base, &limits()).unwrap();

    // Edits of one note's velocity, each with the frame its effect starts at.
    let needle = "velocity = ";
    let mut edits: Vec<(u64, PlanArtifact)> = Vec::new();
    for (index, (at, _)) in text.rmatch_indices(needle).enumerate() {
        let digits = &text[at + needle.len()..];
        let end = digits
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '/'))
            .unwrap_or(0);
        if end == 0 {
            continue;
        }
        let edited = format!(
            "{}{}0.3{}{}",
            &text[..at],
            needle,
            index + 1,
            &text[at + needle.len() + end..]
        );
        let Ok(plan) = compile_text(&edited) else {
            continue;
        };
        if let Ok(Reuse::From(bound)) = reference.reuse_for(&plan) {
            edits.push((bound, plan));
        }
    }
    edits.sort_by_key(|(bound, _)| *bound);
    println!(
        "{song}: {:.0} s of audio, {} velocity edits to choose from",
        total as f64 / second,
        edits.len()
    );

    let bg = BackgroundSession::new(&base, &limits(), SessionPolicy::new(spacing)).unwrap();
    let started = Instant::now();
    assert!(bg.wait_idle(Duration::from_secs(900)));
    println!(
        "the worker filled {} checkpoints in {:.1} s",
        bg.status().checkpoints,
        started.elapsed().as_secs_f64()
    );

    // Where to start playing, and the edit chosen for each case, by where its
    // effect begins relative to the playhead.
    let playhead = total / 4;
    let pick = |low: i64, high: i64| {
        edits.iter().find(|(bound, _)| {
            let ahead = *bound as i64 - playhead as i64;
            (low..high).contains(&ahead)
        })
    };
    let cases: Vec<(&str, Option<&(u64, PlanArtifact)>)> = vec![
        (
            "effect 30 s ahead of the playhead",
            pick(30 * rate as i64, i64::MAX),
        ),
        ("effect 3 s ahead", pick(2 * rate as i64, 6 * rate as i64)),
        ("effect 0.5 s ahead", pick(0, rate as i64)),
        (
            "effect 5 s behind the playhead",
            pick(-10 * rate as i64, -3 * rate as i64),
        ),
        (
            "effect at the very start",
            pick(i64::MIN, -(playhead as i64) + rate as i64),
        ),
    ];

    let only: Option<usize> = std::env::var("MAAC_CASE").ok().and_then(|v| v.parse().ok());
    for (index, (label, chosen)) in cases.into_iter().enumerate() {
        if only.is_some_and(|only| only != index) {
            continue;
        }
        let Some((bound, plan)) = chosen else {
            println!("  {label}: no such edit in this song");
            continue;
        };
        // Back to the base plan, with the checkpoints refilled, so that every
        // case starts alike.
        bg.update(&base);
        assert!(bg.wait_idle(Duration::from_secs(900)));

        let begun = Instant::now();
        let play = bg
            .play(
                playhead,
                PlaybackConfig {
                    pause_prerender: false,
                    // The start is measured by the other test; here the edit.
                    approximate_start: false,
                    ..PlaybackConfig::default()
                },
            )
            .unwrap();
        let channels = play.channels();
        let play = Arc::new(play);
        let stop = Arc::new(AtomicBool::new(false));
        let first_sound = Arc::new(std::sync::Mutex::new(None::<f64>));
        let reader = {
            let (play, stop, first_sound) = (play.clone(), stop.clone(), first_sound.clone());
            std::thread::spawn(move || {
                let period = Duration::from_secs_f64(512.0 / second);
                let mut buffer = vec![0.0; 512 * channels];
                let mut next = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    if play.read(&mut buffer) > 0 {
                        first_sound
                            .lock()
                            .unwrap()
                            .get_or_insert_with(|| begun.elapsed().as_secs_f64());
                    }
                    next += period;
                    if let Some(wait) = next.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                }
            })
        };
        // Play for a while on the base plan.
        std::thread::sleep(Duration::from_secs(4));
        let before = play.status();
        let at_edit = before.position;
        let edited_at = Instant::now();
        bg.update(plan);
        let mut switch = None;
        let mut exact = None;
        let mut stale_for = 0.0f64;
        while edited_at.elapsed() < Duration::from_secs(120) {
            let status = play.status();
            if status.stale {
                stale_for = edited_at.elapsed().as_secs_f64();
            }
            if status.switches.len() > 1 && switch.is_none() {
                switch = Some((status.switches[1].frame, edited_at.elapsed().as_secs_f64()));
            }
            // Done when the sound is exact again.
            if status.switches.len() > 1 && !status.approximate && !status.stale {
                exact = Some((
                    status.switches.last().unwrap().frame,
                    edited_at.elapsed().as_secs_f64(),
                ));
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let after = play.status();
        stop.store(true, Ordering::Release);
        reader.join().unwrap();
        if let (Some((frame, took)), Some((exact_frame, exact_took))) = (switch, exact) {
            println!(
                "  {label} (effect from {:.1} s, playhead {:.1} s): first sound {:.2} s after play; \
                 new plan heard from {:.1} s ({:.2} s after the playhead, {:.2} s after the edit, {}); \
                 exact from {:.1} s ({:.2} s after the edit); underruns {} before the edit (start-up), {} after it",
                *bound as f64 / second,
                at_edit as f64 / second,
                first_sound.lock().unwrap().unwrap_or(f64::NAN),
                frame as f64 / second,
                (frame as f64 - at_edit as f64) / second,
                took,
                if after.switches.get(1).is_some_and(|s| s.approximate_from.is_some()) {
                    "approximately"
                } else {
                    "exactly"
                },
                exact_frame as f64 / second,
                exact_took,
                before.underruns,
                after.underruns - before.underruns,
            );
            drop(play);
            continue;
        }
        match switch {
            Some((frame, took)) => println!(
                "  {label} (effect from {:.1} s, playhead {:.1} s): first sound {:.2} s after play; \
                 new plan from {:.1} s, {:.2} s of old sound after the playhead, producer took {:.2} s{}; \
                 underruns {} before the edit (start-up), {} after it",
                *bound as f64 / second,
                at_edit as f64 / second,
                first_sound.lock().unwrap().unwrap_or(f64::NAN),
                frame as f64 / second,
                (frame as f64 - at_edit as f64) / second,
                took,
                if stale_for > 0.0 {
                    format!(
                        " (catching up for {stale_for:.2} s from {:.1} s)",
                        after.catch_up_from.unwrap_or(0) as f64 / second
                    )
                } else {
                    String::new()
                },
                before.underruns,
                after.underruns - before.underruns,
            ),
            None => println!("  {label}: no switch within 120 s: {after:?}"),
        }
        drop(play);
    }
}

/// How soon sound starts when playing from a point some way past a checkpoint,
/// with the approximate start on and off, and how long until it is exact.
/// Run with `MAAC_SONG=homeward cargo test --release --test playback_latency
/// playback_start -- --ignored --nocapture`.
#[test]
#[ignore]
fn playback_start_on_a_real_song() {
    let song = std::env::var("MAAC_SONG").unwrap_or_else(|_| "afterlight".into());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(&song)
        .join(format!("{song}.maac"));
    let text = std::fs::read_to_string(path).unwrap();
    let base =
        compile_bundle_artifact_with_limits(&SourceBundle::new("standalone.maac", text), &limits())
            .unwrap();
    let rate = u64::from(base.output().sample_rate_hz);
    let second = rate as f64;
    let spacing = rate * 4 + 37;
    let bg = BackgroundSession::new(&base, &limits(), SessionPolicy::new(spacing)).unwrap();
    assert!(bg.wait_idle(Duration::from_secs(900)));
    println!("{song}: checkpoints every {:.1} s", spacing as f64 / second);
    let checkpoint = 5 * spacing;
    for offset_ms in [200u64, 500, 1_000, 2_000, 3_500] {
        let start = checkpoint + offset_ms * rate / 1_000;
        for approximate in [false, true] {
            let begun = Instant::now();
            let play = bg
                .play(
                    start,
                    PlaybackConfig {
                        approximate_start: approximate,
                        ..PlaybackConfig::default()
                    },
                )
                .unwrap();
            let channels = play.channels();
            let mut buffer = vec![0.0; 512 * channels];
            let mut first_sound = None;
            let mut exact = None;
            let period = Duration::from_secs_f64(512.0 / second);
            let mut next = Instant::now();
            while begun.elapsed() < Duration::from_secs(60) {
                if play.read(&mut buffer) > 0 && first_sound.is_none() {
                    first_sound = Some(begun.elapsed().as_secs_f64());
                }
                let status = play.status();
                if first_sound.is_some() && !status.approximate && exact.is_none() {
                    exact = Some(begun.elapsed().as_secs_f64());
                    break;
                }
                next += period;
                if let Some(wait) = next.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
            let status = play.status();
            println!(
                "  {:.1} s past a checkpoint, approximate start {approximate}: first sound {:.2} s, \
                 exact after {:.2} s, underruns {}, switches {}",
                offset_ms as f64 / 1_000.0,
                first_sound.unwrap_or(f64::NAN),
                exact.unwrap_or(f64::NAN),
                status.underruns,
                status.switches.len(),
            );
        }
    }
}
