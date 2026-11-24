//! Thread-count configuration, seamless exact looping (`loop_range` / `set_loop`),
//! and rapid-edit debouncing (`edit_debounce`) on sessions and playbacks.

use std::time::{Duration, Instant};

use maac::dsp::{
    BackgroundSession, DspEngine, Playback, PlaybackConfig, RenderSession, RenderSpan,
    SessionPolicy,
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

fn read_exactly(play: &Playback, frames: usize, size: usize) -> Vec<f64> {
    let channels = play.channels();
    let mut out = Vec::with_capacity(frames * channels);
    let mut buffer = vec![0.0; size * channels];
    let started = Instant::now();
    while out.len() < frames * channels {
        let want = size.min(frames - out.len() / channels);
        let got = play.read(&mut buffer[..want * channels]);
        out.extend_from_slice(&buffer[..got * channels]);
        if got == 0 {
            assert!(started.elapsed() < WAIT, "the playback stalled");
            assert!(play.status().last_error.is_none(), "{:?}", play.status());
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    out
}

fn read_to_end(play: &Playback, size: usize) -> Vec<f64> {
    let channels = play.channels();
    let mut out = Vec::new();
    let mut buffer = vec![0.0; size * channels];
    let started = Instant::now();
    loop {
        let got = play.read(&mut buffer);
        out.extend_from_slice(&buffer[..got * channels]);
        if got == 0 {
            let status = play.status();
            assert!(status.last_error.is_none(), "{status:?}");
            if status.ended {
                return out;
            }
            assert!(started.elapsed() < WAIT, "the playback stalled: {status:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn base_config() -> PlaybackConfig {
    PlaybackConfig {
        lead_frames: 4_096,
        chunk_frames: 1_000,
        pause_prerender: false,
        preview_preroll_frames: 0,
        approximate_start: false,
        ..PlaybackConfig::default()
    }
}

// ---- render_threads / prerender_threads ----------------------------------

#[test]
fn session_and_playback_thread_configs_are_bit_identical() {
    let plan = compile(&base_song(3)).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);

    for (fg, bg_threads, play_threads) in [
        (Some(1), Some(1), Some(1)),
        (Some(4), Some(1), Some(2)),
        (Some(0), Some(0), Some(0)), // 0 clamps to 1
        (Some(2), None, None),
    ] {
        let mut policy = SessionPolicy::new(SPACING);
        if let Some(n) = fg {
            policy = policy.with_render_threads(n);
        }
        if let Some(n) = bg_threads {
            policy = policy.with_prerender_threads(n);
        }
        assert_eq!(policy.render_threads, fg);
        assert_eq!(policy.prerender_threads, bg_threads);

        let mut session = RenderSession::new(&plan, &limits(), policy).unwrap();
        let mut rendered = Vec::new();
        session
            .render(0, total.min(20_000), |frame| {
                rendered.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            bits(&fresh[..rendered.len()]),
            bits(&rendered),
            "RenderSession threads fg={fg:?}"
        );

        let bg = BackgroundSession::new_with(&plan, &limits(), policy, true).unwrap();
        assert!(bg.wait_idle(WAIT));
        assert!(bg.status().checkpoints > 0);
        let play = bg
            .play(
                0,
                PlaybackConfig {
                    render_threads: play_threads,
                    ..base_config()
                },
            )
            .unwrap();
        let got = read_exactly(&play, 12_000, 500);
        assert_eq!(
            bits(&fresh[..12_000 * channels]),
            bits(&got),
            "Playback threads play={play_threads:?}"
        );
    }
}

// ---- seamless exact looping ----------------------------------------------

#[test]
fn invalid_loop_ranges_are_refused() {
    let plan = compile(&base_song(1)).unwrap();
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    for bad in [
        RenderSpan {
            start: 100,
            end: 100,
        },
        RenderSpan {
            start: 500,
            end: 200,
        },
    ] {
        assert!(bg
            .play(
                0,
                PlaybackConfig {
                    loop_range: Some(bad),
                    ..base_config()
                }
            )
            .is_err());
    }
    let play = bg.play(0, base_config()).unwrap();
    assert!(play
        .set_loop(Some(RenderSpan {
            start: 100,
            end: 100
        }))
        .is_err());
    assert!(play
        .set_loop(Some(RenderSpan {
            start: 500,
            end: 200
        }))
        .is_err());
    assert_eq!(play.status().loop_range, None);
}

#[test]
fn loop_passes_are_bit_identical_from_any_start_position() {
    let plan = compile(&base_song(7)).unwrap();
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();

    // Cases: loop from 0, start before loop, start at loop.start, start inside loop.
    let cases = [
        (
            0u64,
            RenderSpan {
                start: 0,
                end: 5_333,
            },
            3usize,
        ),
        (
            1_200,
            RenderSpan {
                start: 4_000,
                end: 9_123,
            },
            3,
        ),
        (
            4_000,
            RenderSpan {
                start: 4_000,
                end: 9_123,
            },
            4,
        ),
        (
            6_500,
            RenderSpan {
                start: 4_000,
                end: 9_123,
            },
            3,
        ),
    ];

    for (start, span, passes) in cases {
        let play = bg
            .play(
                start,
                PlaybackConfig {
                    loop_range: Some(span),
                    ..base_config()
                },
            )
            .unwrap();
        assert_eq!(play.status().loop_range, Some(span));

        // Build exact expected sequence: initial stretch [start..span.end),
        // then (passes - 1) full passes of [span.start..span.end).
        let mut expected = Vec::new();
        expected.extend_from_slice(&fresh[start as usize * channels..span.end as usize * channels]);
        for _ in 1..passes {
            expected.extend_from_slice(
                &fresh[span.start as usize * channels..span.end as usize * channels],
            );
        }
        let total_frames = expected.len() / channels;
        let got = read_exactly(&play, total_frames, 333);
        let status = play.status();
        assert!(!status.ended, "{status:?}");
        assert!(status.last_error.is_none(), "{status:?}");
        assert_eq!(
            bits(&expected),
            bits(&got),
            "start={start}, span={span:?}, passes={passes}"
        );
        // After reading exactly to `span.end`, the next frame is `span.start`
        // (once the next chunk is queued) or `span.end` (if momentarily empty).
        assert!(
            status.position == span.start || status.position == span.end,
            "unexpected position after wrap: {status:?}"
        );
    }
}

#[test]
fn position_wraps_accurately_when_a_single_read_crosses_the_loop_boundary() {
    let plan = compile(&base_song(7)).unwrap();
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    let span = RenderSpan {
        start: 2_000,
        end: 5_000,
    };
    let play = bg
        .play(
            span.start,
            PlaybackConfig {
                loop_range: Some(span),
                ..base_config()
            },
        )
        .unwrap();
    // Wait until more than one loop pass is buffered.
    wait_for("loop buffered", || play.status().buffered_frames >= 3_500);
    // Read 3500 frames in one shot: crosses 5000 -> 2000 and lands at 2500.
    let mut buf = vec![0.0; 3_500 * channels];
    let got = play.read(&mut buf);
    assert_eq!(got, 3_500);
    assert_eq!(play.status().position, 2_500);
    let mut expected = Vec::new();
    expected.extend_from_slice(&fresh[2_000 * channels..5_000 * channels]);
    expected.extend_from_slice(&fresh[2_000 * channels..2_500 * channels]);
    assert_eq!(bits(&expected), bits(&buf));
}

#[test]
fn starting_past_loop_end_plays_linearly_to_end() {
    let plan = compile(&base_song(1)).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    let start = total - 8_000;
    let play = bg
        .play(
            start,
            PlaybackConfig {
                loop_range: Some(RenderSpan {
                    start: 1_000,
                    end: 4_000,
                }),
                ..base_config()
            },
        )
        .unwrap();
    let got = read_to_end(&play, 500);
    assert!(play.status().ended);
    assert_eq!(bits(&fresh[start as usize * channels..]), bits(&got));
}

#[test]
fn set_loop_dynamically_enables_and_disables_looping_seamlessly() {
    let plan = compile(&base_song(7)).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    let play = bg.play(0, base_config()).unwrap();

    // Let the producer fill well past frame 6000 while reader is still at 1000.
    let mut got = read_exactly(&play, 1_000, 500);
    wait_for("lead filled past 4500", || {
        play.status().position + play.status().buffered_frames > 4_500
    });

    // Enable loop [2000..4000) while reader is at 1000 and buffer already went past 4000.
    let span = RenderSpan {
        start: 2_000,
        end: 4_000,
    };
    play.set_loop(Some(span)).unwrap();
    assert_eq!(play.status().loop_range, Some(span));

    // Read 3000 (1000..4000) + 2 full passes of 2000 (2000..4000 x2) = 7000 frames.
    got.extend(read_exactly(&play, 7_000, 250));
    let mut expected = Vec::new();
    expected.extend_from_slice(&fresh[0..4_000 * channels]);
    expected.extend_from_slice(&fresh[2_000 * channels..4_000 * channels]);
    expected.extend_from_slice(&fresh[2_000 * channels..4_000 * channels]);
    assert_eq!(bits(&expected), bits(&got), "after enabling loop");

    // Now disable looping and read until the end of the song.
    // Wait briefly so the ring has already queued the next pass starting at 2000,
    // exercising the discontinuity-trimming path too.
    wait_for("next pass buffered", || {
        play.status().buffered_frames >= 2_500
    });
    let resume_from = play.status().position;
    assert_eq!(resume_from, 2_000);
    play.set_loop(None).unwrap();
    assert_eq!(play.status().loop_range, None);

    let rest = read_to_end(&play, 700);
    let status = play.status();
    assert!(status.ended && status.last_error.is_none(), "{status:?}");
    assert_eq!(
        bits(&fresh[resume_from as usize * channels..total as usize * channels]),
        bits(&rest),
        "after disabling loop"
    );
}

#[test]
fn editing_while_looping_switches_cleanly_to_exact_new_plan() {
    let song = base_song(4);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let reference = maac::dsp::CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut rng = Rng(17);
    let edited = (0..300)
        .find_map(|index| {
            let candidate = compile(&edit(&song, &mut rng, index).0).ok()?;
            (candidate.output().total_frames == total
                && matches!(
                    reference.reuse_for(&candidate).ok()?,
                    maac::dsp::Reuse::From(bound) if bound < 12_000
                ))
            .then_some(candidate)
        })
        .expect("an edit early in the song");
    let fresh_edited = render_full(&edited);

    let span = RenderSpan {
        start: 10_000,
        end: 18_000,
    };
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    let play = bg
        .play(
            span.start,
            PlaybackConfig {
                preview_preroll_frames: 4_000,
                approximate_start: true,
                loop_range: Some(span),
                ..base_config()
            },
        )
        .unwrap();

    // Read one full pass of the original plan, then apply an edit behind the playhead.
    let _ = read_exactly(&play, 8_000, 400);
    bg.update(&edited);

    // Keep reading until the producer switches to the exact edited plan, then
    // drain whatever remained in the ring before the switch took effect.
    let mut buf = vec![0.0; 400 * channels];
    wait_for("exact edited plan on loop", || {
        let _ = play.read(&mut buf);
        let status = play.status();
        assert!(status.last_error.is_none(), "{status:?}");
        !status.stale
            && !status.approximate
            && status
                .switches
                .last()
                .is_some_and(|sw| sw.generation == 1 && sw.approximate_from.is_none())
    });
    let _ = read_exactly(&play, 5_000, 400);

    // Align to the next `span.start` and verify a full pass matches `fresh_edited` bit-for-bit.
    while play.status().position != span.start {
        let pos = play.status().position;
        let step = if pos < span.end {
            (span.end - pos).min(400) as usize
        } else {
            1
        };
        let _ = read_exactly(&play, step, step);
    }
    let pass = read_exactly(&play, (span.end - span.start) as usize, 500);
    assert_eq!(
        bits(&fresh_edited[span.start as usize * channels..span.end as usize * channels]),
        bits(&pass),
        "edited loop pass must be bit-identical"
    );
}

// ---- edit_debounce -------------------------------------------------------

#[test]
fn edit_debounce_coalesces_rapid_edits_and_keeps_ring_fed() {
    let song = base_song(2);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut rng = Rng(99);
    let mut edits = Vec::new();
    for index in 0..400 {
        if let Ok(candidate) = compile(&edit(&song, &mut rng, index).0) {
            if candidate.output().total_frames == total {
                edits.push(candidate);
            }
        }
        if edits.len() == 5 {
            break;
        }
    }
    assert_eq!(edits.len(), 5);

    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    let play = bg
        .play(
            0,
            PlaybackConfig {
                edit_debounce: Duration::from_millis(120),
                ..base_config()
            },
        )
        .unwrap();

    // Warm up the ring before starting the burst.
    wait_for("ring warm", || play.status().buffered_frames >= 2_000);
    let underruns_before = play.status().underruns;

    // Rapidly push 5 edits 15 ms apart (well inside the 120 ms debounce window)
    // while reading gently so the ring stays alive.
    let channels = play.channels();
    let mut scratch = vec![0.0; 128 * channels];
    for edited in &edits {
        bg.update(edited);
        let _ = play.read(&mut scratch);
        std::thread::sleep(Duration::from_millis(15));
    }
    // Right after the last update (before 120 ms elapsed), none of the edits
    // should have caused a switch yet.
    assert_eq!(
        play.status().switches.len(),
        1,
        "debounce should hold rapid edits until quiet: {:?}",
        play.status()
    );
    assert_eq!(
        play.status().underruns,
        underruns_before,
        "ring should stay fed during debounce"
    );

    // Once quiet past 120 ms, the playback adopts generation 5 directly (skipping 1..=4).
    wait_for("debounced switch to latest generation", || {
        let _ = play.read(&mut scratch);
        let status = play.status();
        assert!(status.last_error.is_none(), "{status:?}");
        status.switches.last().is_some_and(|sw| sw.generation == 5)
    });
    let status = play.status();
    let gens: Vec<u64> = status.switches.iter().map(|sw| sw.generation).collect();
    assert_eq!(
        gens,
        vec![0, 5],
        "intermediate edits should have been coalesced: {status:?}"
    );
}

#[test]
fn seek_bypasses_edit_debounce() {
    let song = base_song(2);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut rng = Rng(55);
    let edited = (0..200)
        .find_map(|index| {
            let candidate = compile(&edit(&song, &mut rng, index).0).ok()?;
            (candidate.output().total_frames == total).then_some(candidate)
        })
        .expect("one edit");
    let fresh_edited = render_full(&edited);
    let channels = usize::from(plan.output().channels);

    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    // Huge debounce (10 s) so any debounce wait would fail the timing check.
    let play = bg
        .play(
            0,
            PlaybackConfig {
                edit_debounce: Duration::from_secs(10),
                ..base_config()
            },
        )
        .unwrap();
    let _ = read_exactly(&play, 1_000, 500);

    let started = Instant::now();
    bg.update(&edited);
    play.seek(4_000);
    let got = read_exactly(&play, 2_000, 500);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "seek waited on edit_debounce"
    );
    let status = play.status();
    assert_eq!(status.switches.len(), 1, "{status:?}");
    assert_eq!(status.switches[0].generation, 1, "{status:?}");
    assert_eq!(
        bits(&fresh_edited[4_000 * channels..6_000 * channels]),
        bits(&got)
    );
}

#[test]
fn loop_end_beyond_plan_length_clamps_to_total() {
    let plan = compile(&base_song(1)).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    let loop_start = total - 3_500;
    let span = RenderSpan {
        start: loop_start,
        end: total + 50_000,
    };
    let play = bg
        .play(
            loop_start,
            PlaybackConfig {
                loop_range: Some(span),
                ..base_config()
            },
        )
        .unwrap();
    // Read 2.5 passes of [loop_start..total).
    let pass_len = (total - loop_start) as usize;
    let got = read_exactly(&play, pass_len * 2 + 1_000, 333);
    assert!(!play.status().ended);
    let mut expected = Vec::new();
    expected.extend_from_slice(&fresh[loop_start as usize * channels..total as usize * channels]);
    expected.extend_from_slice(&fresh[loop_start as usize * channels..total as usize * channels]);
    expected.extend_from_slice(
        &fresh[loop_start as usize * channels..(loop_start as usize + 1_000) * channels],
    );
    assert_eq!(bits(&expected), bits(&got));
}

#[test]
fn set_loop_shrinking_an_active_loop_stays_bit_identical() {
    let plan = compile(&base_song(7)).unwrap();
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    let wide = RenderSpan {
        start: 2_000,
        end: 7_000,
    };
    let play = bg
        .play(
            wide.start,
            PlaybackConfig {
                loop_range: Some(wide),
                ..base_config()
            },
        )
        .unwrap();
    // Read 1000 frames into the wide loop (position = 3000) while buffer is ahead past 5000.
    let _ = read_exactly(&play, 1_000, 500);
    wait_for("wide loop buffered past 5500", || {
        play.status().buffered_frames >= 2_600
    });
    assert_eq!(play.status().position, 3_000);

    // Shrink the loop to [2_500..5_000) while playhead is at 3_000.
    let tight = RenderSpan {
        start: 2_500,
        end: 5_000,
    };
    play.set_loop(Some(tight)).unwrap();

    // Next frames must be [3000..5000) followed by two full passes of [2500..5000).
    let got = read_exactly(&play, 2_000 + 2_500 * 2, 300);
    let mut expected = Vec::new();
    expected.extend_from_slice(&fresh[3_000 * channels..5_000 * channels]);
    expected.extend_from_slice(&fresh[2_500 * channels..5_000 * channels]);
    expected.extend_from_slice(&fresh[2_500 * channels..5_000 * channels]);
    assert_eq!(bits(&expected), bits(&got));
}

#[test]
fn approximate_start_inside_short_loop_converges_to_exact() {
    let plan = compile(&base_song(5)).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(SPACING), false).unwrap();
    let loop_start = total * 2 / 3;
    let span = RenderSpan {
        start: loop_start,
        end: loop_start + 6_000,
    };
    let play = bg
        .play(
            loop_start,
            PlaybackConfig {
                preview_preroll_frames: 8_000,
                approximate_start: true,
                loop_range: Some(span),
                ..base_config()
            },
        )
        .unwrap();
    // Pace reads until the approximate start appears and then the exact render
    // takes over cleanly, then drain any approximate frames still in the ring.
    let mut buf = vec![0.0; 480 * channels];
    wait_for("approximate then exact loop", || {
        let _ = play.read(&mut buf);
        let status = play.status();
        assert!(status.last_error.is_none(), "{status:?}");
        let saw_approx = status
            .switches
            .iter()
            .any(|sw| sw.approximate_from.is_some());
        saw_approx
            && !status.approximate
            && !status.stale
            && status
                .switches
                .last()
                .is_some_and(|sw| sw.approximate_from.is_none())
    });
    // Drain at least one lead so every remaining approximate chunk has left the ring.
    let _ = read_exactly(&play, 5_000, 480);
    // Align to `loop_start` and confirm a full exact pass.
    while play.status().position != loop_start {
        let pos = play.status().position;
        let step = if pos < span.end {
            (span.end - pos).min(480) as usize
        } else {
            1
        };
        let _ = read_exactly(&play, step, step);
    }
    let pass = read_exactly(&play, 6_000, 480);
    assert_eq!(
        bits(&fresh[loop_start as usize * channels..span.end as usize * channels]),
        bits(&pass)
    );
}
