//! Playing a background session: whatever the reader does and whenever the plan
//! is edited, every frame it gets is a frame of an exact render of one plan,
//! bit for bit, and the plan changes only where the playback says it did.

use std::time::{Duration, Instant};

use maac::dsp::{BackgroundSession, DspEngine, Playback, PlaybackConfig, SessionPolicy};
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

fn session(plan: &PlanArtifact, auto: bool) -> BackgroundSession {
    BackgroundSession::new_with(plan, &limits(), SessionPolicy::new(SPACING), auto).unwrap()
}

fn config() -> PlaybackConfig {
    PlaybackConfig {
        lead_frames: 4_096,
        chunk_frames: 1_000,
        pause_prerender: false,
        preview_preroll_frames: 0,
        approximate_start: false,
        ..PlaybackConfig::default()
    }
}

/// Read `frames` frames in reads of `size`, waiting for the producer.
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

/// Read until the playback has ended, at about `size` frames per 5 ms: twice
/// as fast as the sound plays at 48 kHz when `size` is 480.
fn read_paced(play: &Playback, size: usize) -> Vec<f64> {
    let channels = play.channels();
    let mut out = Vec::new();
    let mut buffer = vec![0.0; size * channels];
    let started = Instant::now();
    loop {
        let got = play.read(&mut buffer);
        out.extend_from_slice(&buffer[..got * channels]);
        let status = play.status();
        assert!(status.last_error.is_none(), "{status:?}");
        if got == 0 && status.ended {
            return out;
        }
        assert!(started.elapsed() < WAIT, "the playback stalled: {status:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Read until the playback has ended, as fast as it will go.
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

#[test]
fn reads_of_any_size_are_a_fresh_render() {
    let plan = compile(&base_song(1)).unwrap();
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg = session(&plan, false);
    for (start, size, chunk) in [
        (0u64, 1usize, 64u64),
        (0, 17, 1_000),
        (0, 4_096, 4_801),
        (12_345, 100_000, 2_048),
        (12_345, 333, 64),
    ] {
        let play = bg
            .play(
                start,
                PlaybackConfig {
                    lead_frames: chunk.max(4_096),
                    chunk_frames: chunk,
                    pause_prerender: false,
                    preview_preroll_frames: 0,
                    approximate_start: false,
                    ..PlaybackConfig::default()
                },
            )
            .unwrap();
        let got = read_to_end(&play, size);
        let status = play.status();
        assert!(status.ended && !status.stale, "{status:?}");
        assert_eq!(status.switches.len(), 1, "{status:?}");
        assert_eq!(
            bits(&fresh[start as usize * channels..]),
            bits(&got),
            "start {start}, reads of {size}, chunks of {chunk}"
        );
    }
}

#[test]
fn a_read_gives_what_is_ready_and_silences_the_rest() {
    let plan = compile(&base_song(1)).unwrap();
    let bg = session(&plan, false);
    let play = bg.play(0, config()).unwrap();
    let channels = play.channels();
    // More than the producer ever holds ahead: it cannot all be there.
    let mut out = vec![1.0; 20_000 * channels];
    let got = play.read(&mut out);
    assert!(got < 20_000, "{got}");
    assert!(out[got * channels..].iter().all(|&sample| sample == 0.0));
    let status = play.status();
    assert!(status.underruns >= 1, "{status:?}");
    assert_eq!(status.position, got as u64);
    // The frames it did give are the plan's first.
    let fresh = render_full(&plan);
    assert_eq!(bits(&out[..got * channels]), bits(&fresh[..got * channels]));
}

/// The frames a playback gave, from `start`, against fresh renders of the
/// plans it says it played: each frame belongs to the last switch at or before
/// it.
fn assert_played(
    got: &[f64],
    start: u64,
    switches: &[maac::dsp::PlaybackSwitch],
    plans: &[&PlanArtifact],
    why: &str,
) {
    let channels = usize::from(plans[0].output().channels);
    let renders: Vec<Vec<f64>> = plans.iter().map(|plan| render_full(plan)).collect();
    let frames = got.len() / channels;
    let mut from = 0usize;
    for (index, switch) in switches.iter().enumerate() {
        let until = switches
            .get(index + 1)
            .map_or(frames, |next| (next.frame - start) as usize)
            .min(frames);
        assert!(until >= from, "{why}: switches out of order: {switches:?}");
        // The frames of the plan, from frame `base` of the song: an exact
        // render from 0, or a render that started cold at `cold`.
        let plan = plans[switch.generation as usize];
        let (base, render) = match switch.approximate_from {
            None => (0usize, renders[switch.generation as usize].clone()),
            Some(cold) => {
                let mut out = Vec::new();
                DspEngine::new_artifact_with_limits(plan, &limits())
                    .unwrap()
                    .render_span(
                        RenderSpan {
                            start: cold,
                            end: plan.output().total_frames,
                        },
                        |frame| {
                            out.extend_from_slice(frame);
                            Ok(())
                        },
                    )
                    .unwrap();
                (cold as usize, out)
            }
        };
        let first = start as usize + from;
        let last = start as usize + until;
        let want = &render[(first - base) * channels..((last - base) * channels).min(render.len())];
        assert_eq!(
            bits(want),
            bits(&got[from * channels..from * channels + want.len()]),
            "{why}: frames {first}..{last} should be plan {} ({}) ({switches:?})",
            switch.generation,
            if switch.approximate_from.is_some() {
                "approximate"
            } else {
                "exact"
            }
        );
        from = until;
    }
}

/// Edit while playing: read `read_before` frames from `start`, update to
/// `edited`, read on to the end.
fn edit_while_playing(
    plan: &PlanArtifact,
    edited: &PlanArtifact,
    start: u64,
    read_before: usize,
    auto: bool,
    why: &str,
) {
    let bg = session(plan, auto);
    if auto {
        assert!(bg.wait_idle(WAIT));
    }
    let play = bg.play(start, config()).unwrap();
    let mut got = read_exactly(&play, read_before, 500);
    let played = start + read_before as u64;
    bg.update(edited);
    got.extend(read_paced(&play, 480));
    let status = play.status();
    assert!(
        status.last_error.is_none() && !status.stale,
        "{why}: {status:?}"
    );
    let changed = render_full(plan) != render_full(edited);
    if changed {
        assert_eq!(status.switches.len(), 2, "{why}: {status:?}");
        assert!(
            status.switches[1].frame >= played,
            "{why}: the switch is behind the reader: {status:?}"
        );
        assert_eq!(status.switches[1].generation, 1, "{why}");
    }
    assert_played(&got, start, &status.switches, &[plan, edited], why);
    if changed {
        // The end of the song is the new plan's, not the old one's.
        let edited_total = edited.output().total_frames;
        assert_eq!(
            got.len() / usize::from(plan.output().channels),
            (edited_total - start) as usize,
            "{why}"
        );
    }
}

#[test]
fn an_edit_during_playback_is_heard_exactly() {
    let song = base_song(2);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut rng = Rng(5);
    let mut done = 0;
    for choice in 0..30u64 {
        let (next, label) = edit(&song, &mut rng, choice);
        let Ok(edited) = compile(&next) else { continue };
        if edited.output().total_frames != total {
            continue;
        }
        // Early, in the middle, and while the worker is filling checkpoints.
        let (start, read_before, auto) = match done % 3 {
            0 => (0, 1_500, false),
            1 => (total / 2, 3_000, true),
            _ => (total / 4, 2_000, false),
        };
        edit_while_playing(
            &plan,
            &edited,
            start,
            read_before,
            auto,
            &format!("edit {choice} ({label}) from {start}"),
        );
        done += 1;
        if done == 9 {
            break;
        }
    }
    assert!(done >= 6, "only {done} edits were tried");
}

#[test]
fn an_edit_before_the_playhead_is_caught_up_to() {
    // Playing late in a song nothing has been rendered for, an edit that
    // changes the start: the new plan must be rendered from frame 0 up to the
    // playhead before it can take over.
    let song = base_song(4);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let reference = maac::dsp::CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut rng = Rng(9);
    let early = (0..300)
        .find_map(|index| {
            let edited = compile(&edit(&song, &mut rng, 8 + index).0).ok()?;
            (edited.output().total_frames == total
                && matches!(
                    reference.reuse_for(&edited).unwrap(),
                    maac::dsp::Reuse::From(bound) if bound < total / 20
                ))
            .then_some(edited)
        })
        .expect("an early edit");
    edit_while_playing(
        &plan,
        &early,
        total * 2 / 3,
        2_000,
        false,
        "edit before the playhead",
    );
}

#[test]
fn an_edit_that_changes_the_length() {
    let song = base_song(6);
    let text = source(&song);
    let plan = compile(&song).unwrap();
    let shorter = {
        use maac::compiler::compile_bundle_artifact_with_limits;
        use maac::SourceBundle;
        compile_bundle_artifact_with_limits(
            &SourceBundle::new("standalone.maac", text.replace("120bpm", "240bpm")),
            &limits(),
        )
        .unwrap()
    };
    assert!(shorter.output().total_frames < plan.output().total_frames);
    // Early on: the new plan takes over, and the song ends where it ends.
    edit_while_playing(&plan, &shorter, 0, 1_500, false, "shorter, early");
    // Past the new end: nothing of the new plan is left to play.
    let bg = session(&plan, false);
    let play = bg.play(0, config()).unwrap();
    let read = shorter.output().total_frames as usize + 5_000;
    let mut got = read_exactly(&play, read, 4_000);
    bg.update(&shorter);
    got.extend(read_paced(&play, 480));
    let status = play.status();
    assert!(status.ended && status.last_error.is_none(), "{status:?}");
    let fresh = render_full(&plan);
    assert_eq!(
        bits(&got),
        bits(&fresh[..got.len()]),
        "what was played before the end of the new plan is the old plan's"
    );
}

#[test]
fn a_seek_lands_on_the_fresh_render() {
    let plan = compile(&base_song(3)).unwrap();
    let channels = usize::from(plan.output().channels);
    let total = plan.output().total_frames;
    let fresh = render_full(&plan);
    for auto in [false, true] {
        let bg = session(&plan, auto);
        if auto {
            assert!(bg.wait_idle(WAIT));
        }
        let play = bg.play(0, config()).unwrap();
        let _ = read_exactly(&play, 3_000, 700);
        for target in [total - 5_000, 17, total / 2 + 3, total / 2, 1 + SPACING] {
            play.seek(target);
            assert_eq!(play.status().position, target);
            let frames = 6_000.min((total - target) as usize);
            let got = read_exactly(&play, frames, 777);
            assert_eq!(
                bits(&fresh[target as usize * channels..(target as usize + frames) * channels]),
                bits(&got),
                "seek to {target}, worker {auto}"
            );
            let status = play.status();
            assert_eq!(status.switches.len(), 1);
            assert_eq!(status.switches[0].frame, target);
        }
        // Seek to the end: nothing more, and not an error.
        play.seek(total + 10);
        let mut buffer = vec![0.0; 100 * channels];
        let started = Instant::now();
        while !play.status().ended {
            assert_eq!(play.read(&mut buffer), 0);
            assert!(started.elapsed() < WAIT, "{:?}", play.status());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(play.status().last_error.is_none());
        // And back.
        play.seek(100);
        let got = read_exactly(&play, 2_000, 512);
        assert_eq!(
            bits(&fresh[100 * channels..2_100 * channels]),
            bits(&got),
            "back from the end"
        );
    }
}

#[test]
fn an_edit_then_a_seek() {
    let song = base_song(5);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut rng = Rng(3);
    let edited = (0..100)
        .find_map(|index| {
            let edited = compile(&edit(&song, &mut rng, 3 + index).0).ok()?;
            (edited.output().total_frames == total && render_full(&edited) != render_full(&plan))
                .then_some(edited)
        })
        .unwrap();
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&edited);
    let bg = session(&plan, true);
    let play = bg.play(0, config()).unwrap();
    let _ = read_exactly(&play, 2_000, 500);
    bg.update(&edited);
    // A seek straight after the edit lands on the edited plan, from whichever
    // checkpoint the session holds for it.
    play.seek(total / 2);
    let got = read_exactly(&play, 8_000, 900);
    let from = (total / 2) as usize * channels;
    assert_eq!(bits(&fresh[from..from + 8_000 * channels]), bits(&got));
    assert_eq!(play.status().switches.len(), 1);
}

#[test]
fn dropping_a_playback_stops_it_promptly() {
    let plan = compile(&base_song(1)).unwrap();
    let bg = session(&plan, true);
    let play = bg.play(0, config()).unwrap();
    let _ = read_exactly(&play, 100, 50);
    let started = Instant::now();
    drop(play);
    assert!(started.elapsed() < Duration::from_secs(5));
    // The session still works.
    let play = bg.play(1_000, config()).unwrap();
    let _ = read_exactly(&play, 100, 50);
}

#[test]
fn a_playback_can_hold_the_worker_still() {
    let plan = compile(&base_song(1)).unwrap();
    let bg = session(&plan, false);
    let play = bg
        .play(
            0,
            PlaybackConfig {
                pause_prerender: true,
                ..config()
            },
        )
        .unwrap();
    bg.prerender();
    for _ in 0..5 {
        let _ = read_exactly(&play, 500, 100);
        std::thread::sleep(Duration::from_millis(60));
    }
    assert_eq!(
        bg.status().checkpoints,
        0,
        "the worker ran while the playback held it: {:?}",
        bg.status()
    );
    drop(play);
    assert!(bg.wait_idle(WAIT));
    assert!(bg.status().checkpoints > 0);
}

#[test]
fn a_bad_start_or_config_is_refused() {
    let plan = compile(&base_song(1)).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, false);
    assert!(bg.play(total, config()).is_err());
    assert!(bg
        .play(
            0,
            PlaybackConfig {
                chunk_frames: 10,
                ..config()
            }
        )
        .is_err());
    assert!(bg
        .play(
            0,
            PlaybackConfig {
                lead_frames: 100,
                chunk_frames: 1_000,
                pause_prerender: false,
                preview_preroll_frames: 0,
                approximate_start: false,
                ..PlaybackConfig::default()
            }
        )
        .is_err());
}

// ---- the engine's chunks -------------------------------------------------

use maac::dsp::RenderSpan;

fn render_chunks(engine: &mut DspEngine<'_>, from: u64, to: u64, size: u64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let end = (at + size).min(to);
        engine
            .render_chunk(RenderSpan { start: at, end }, |frame| {
                out.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        at = end;
    }
    out
}

#[test]
fn chunks_of_any_size_are_one_render() {
    let plan = compile(&base_song(7)).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    for size in [1u64, 2, 17, 999, 4_801, 100_000] {
        // 1-frame chunks over the whole song would be slow: cap the range.
        let to = if size < 17 { 6_000 } else { total };
        let mut engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
        let got = render_chunks(&mut engine, 0, to, size);
        assert_eq!(
            bits(&fresh[..to as usize * channels]),
            bits(&got),
            "size {size}"
        );
    }
}

#[test]
fn a_chunk_state_is_a_checkpoint() {
    // Stop after any chunk, take the state, give it to a second engine: the
    // second goes on as the first would have.
    let plan = compile(&base_song(8)).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let mut first = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
    let mut at = 0;
    for step in [1_000u64, 4_801, 33, 20_000, 7] {
        render_chunks(&mut first, at, at + step, 500);
        at += step;
        let checkpoint = first.chunk_checkpoint().unwrap();
        assert_eq!(checkpoint.frame(), at);
        let mut second = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
        second.restore_from(&checkpoint).unwrap();
        let got = render_chunks(&mut second, at, total, 3_000);
        assert_eq!(
            bits(&fresh[at as usize * channels..]),
            bits(&got),
            "from {at}"
        );
    }
}

#[test]
fn a_misplaced_chunk_is_refused() {
    let plan = compile(&base_song(7)).unwrap();
    let mut engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
    let span = |start, end| RenderSpan { start, end };
    // A fresh engine starts at 0, not mid-song (that would be a preview).
    assert!(engine.render_chunk(span(500, 900), |_| Ok(())).is_err());
    engine.render_chunk(span(0, 500), |_| Ok(())).unwrap();
    // A gap, and going back.
    assert!(engine.render_chunk(span(600, 900), |_| Ok(())).is_err());
    assert!(engine.render_chunk(span(400, 900), |_| Ok(())).is_err());
    // Refused before anything ran: the engine is still at 500.
    assert_eq!(engine.chunk_checkpoint().unwrap().frame(), 500);
    // A chunk that the callback stops leaves the engine refused.
    let mut engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
    engine.render_chunk(span(0, 500), |_| Ok(())).unwrap();
    let stopped = engine.render_chunk(span(500, 900), |_| {
        Err(maac::dsp::RenderError::Callback("stop".into()))
    });
    assert!(stopped.is_err());
    assert!(engine.render_chunk(span(500, 900), |_| Ok(())).is_err());
    assert!(engine.chunk_checkpoint().is_err());
}

#[test]
fn an_edit_just_behind_the_playhead_starts_from_the_players_own_snapshot() {
    // Nothing is rendered for the session, so the only state near the edit is
    // what the playback kept of its own engine: one every 24 000 frames.
    let song = base_song(2);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let reference = maac::dsp::CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut rng = Rng(21);
    let (edited, bound) = (0..400)
        .find_map(|index| {
            let edited = compile(&edit(&song, &mut rng, index).0).ok()?;
            match reference.reuse_for(&edited).ok()? {
                maac::dsp::Reuse::From(bound)
                    if edited.output().total_frames == total
                        && (73_000..95_000).contains(&bound) =>
                {
                    Some((edited, bound))
                }
                _ => None,
            }
        })
        .expect("an edit just behind 100 000");
    let bg = session(&plan, false);
    let play = bg.play(0, config()).unwrap();
    let mut got = read_exactly(&play, 100_000, 500);
    bg.update(&edited);
    got.extend(read_paced(&play, 480));
    let status = play.status();
    assert_eq!(
        status.catch_up_from,
        Some(72_000),
        "bound {bound}: {status:?}"
    );
    assert_eq!(status.switches.len(), 2, "{status:?}");
    assert_played(
        &got,
        0,
        &status.switches,
        &[&plan, &edited],
        "from the snapshot",
    );
}

// ---- found by review -----------------------------------------------------

fn compile_text(text: String) -> PlanArtifact {
    use maac::compiler::compile_bundle_artifact_with_limits;
    use maac::SourceBundle;
    compile_bundle_artifact_with_limits(&SourceBundle::new("standalone.maac", text), &limits())
        .unwrap()
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A different number of channels cannot be played on: the stream says so and
/// keeps what it had; putting the plan back carries on where the ring ended.
#[test]
fn an_edit_that_changes_the_channels_stops_the_stream_cleanly() {
    let song = base_song(6);
    let plan = compile(&song).unwrap();
    let stereo = compile_text(
        source(&song)
            .replace("output = &fader:out;", "output = &wide:out;")
            .replace(
                "node fader {",
                "node wide { type = \"core.matrix/1\"; config = { inputs = 1; outputs = 2; \
                 coefficients = [[3/4], [1/2]]; }; }\n\
                 connect fader_wide { from = &fader:out; to = &wide:in; }\nnode fader {",
            ),
    );
    assert_eq!(stereo.output().channels, 2);
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg = session(&plan, false);
    let play = bg.play(0, config()).unwrap();
    let mut got = read_exactly(&play, 2_000, 500);
    bg.update(&stereo);
    wait_for("the format error", || play.status().last_error.is_some());
    assert!(play.status().last_error.unwrap().contains("channels"));
    // What was in the ring is still the old plan's, in whole mono frames.
    let mut buffer = vec![0.0; 1_000 * channels];
    loop {
        let n = play.read(&mut buffer);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buffer[..n * channels]);
    }
    assert_eq!(play.status().position as usize, got.len() / channels);
    assert_eq!(bits(&fresh[..got.len()]), bits(&got));
    // The old plan back: the stream goes on from the end of what it played.
    bg.update(&plan);
    wait_for("the error to clear", || play.status().last_error.is_none());
    got.extend(read_exactly(&play, 5_000, 700));
    assert_eq!(bits(&fresh[..got.len()]), bits(&got));
}

/// A render that fails ends the producer's run, not the playback: a seek or a
/// fixed plan starts it again.
#[test]
fn a_failed_render_is_recovered_from_by_a_seek_and_an_update() {
    let song = base_song(1);
    let plan = compile(&song).unwrap();
    let bad = compile_text(source(&song).replace("gain = 2/5", "gain = 100000"));
    let channels = usize::from(plan.output().channels);
    let fresh = render_full(&plan);
    let bg = session(&plan, false);
    let play = bg.play(0, config()).unwrap();
    let _ = read_exactly(&play, 500, 100);
    bg.update(&bad);
    // The failure comes when the producer reaches the frames that blow up, so
    // the reader goes on.
    let mut sink = vec![0.0; 480 * channels];
    let mut read_until_error = |play: &Playback| {
        wait_for("the render error", || {
            play.read(&mut sink);
            play.status().last_error.is_some()
        });
    };
    read_until_error(&play);
    // A seek with the bad plan in place fails again, but it is noticed.
    play.seek(100);
    assert!(play.status().last_error.is_none());
    read_until_error(&play);
    bg.update(&plan);
    wait_for("the error to clear", || play.status().last_error.is_none());
    // What the ring still held is the bad plan's; the sound from the switch on
    // is the fixed plan's.
    let from = play.status().switches.last().unwrap().frame;
    while play.status().position < from {
        let want = (from - play.status().position).min(400) as usize;
        let mut skip = vec![0.0; want * channels];
        let _ = play.read(&mut skip);
    }
    let got = read_exactly(&play, 2_000, 400);
    assert_eq!(
        bits(&fresh[from as usize * channels..(from as usize + 2_000) * channels]),
        bits(&got),
        "after the fix"
    );
}

/// An edit that comes in before the first frame is heard is the plan the sound
/// starts with: the list must say so.
#[test]
fn an_edit_during_the_lead_in_is_the_plan_the_sound_starts_with() {
    let song = base_song(4);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let before = render_full(&plan);
    let channels = usize::from(plan.output().channels);
    let start = total * 2 / 3;
    let mut rng = Rng(13);
    let edited = (0..200)
        .find_map(|index| {
            let edited = compile(&edit(&song, &mut rng, index).0).ok()?;
            let after = render_full(&edited);
            let from = start as usize * channels;
            (edited.output().total_frames == total && after[from..] != before[from..])
                .then_some(edited)
        })
        .expect("an edit that changes the sound after the start");
    for _ in 0..3 {
        let bg = session(&plan, false);
        let play = bg.play(start, config()).unwrap();
        bg.update(&edited);
        let got = read_paced(&play, 480);
        let status = play.status();
        assert_played(&got, start, &status.switches, &[&plan, &edited], "lead-in");
    }
}

/// With a lead that is one chunk the catch-up still gets to hand over.
#[test]
fn a_lead_of_one_chunk_still_switches() {
    let song = base_song(2);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let reference = maac::dsp::CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut rng = Rng(8);
    let edited = (0..300)
        .find_map(|index| {
            let edited = compile(&edit(&song, &mut rng, index).0).ok()?;
            (edited.output().total_frames == total
                && matches!(
                    reference.reuse_for(&edited).ok()?,
                    maac::dsp::Reuse::From(bound) if bound < 5_000
                ))
            .then_some(edited)
        })
        .expect("an edit near the start");
    let bg = session(&plan, false);
    let play = bg
        .play(
            0,
            PlaybackConfig {
                lead_frames: 1_000,
                chunk_frames: 1_000,
                pause_prerender: false,
                preview_preroll_frames: 0,
                approximate_start: false,
                ..PlaybackConfig::default()
            },
        )
        .unwrap();
    let mut got = read_exactly(&play, 20_000, 500);
    bg.update(&edited);
    got.extend(read_paced(&play, 480));
    let status = play.status();
    assert_eq!(status.switches.len(), 2, "{status:?}");
    assert_played(
        &got,
        0,
        &status.switches,
        &[&plan, &edited],
        "lead of one chunk",
    );
}

/// The new plan is longer and the old one runs out first: the song goes on as
/// the new plan instead of ending at the old end.
#[test]
fn a_longer_plan_is_played_to_its_end() {
    let song = base_song(6);
    let plan = compile(&song).unwrap();
    let longer = compile_text(source(&song).replace("120bpm", "100bpm"));
    let old_total = plan.output().total_frames;
    let new_total = longer.output().total_frames;
    assert!(new_total > old_total);
    let start = old_total - 9_000;
    let bg = session(&plan, false);
    let play = bg.play(start, config()).unwrap();
    let mut got = read_exactly(&play, 1_000, 500);
    bg.update(&longer);
    got.extend(read_paced(&play, 480));
    let status = play.status();
    assert!(status.last_error.is_none(), "{status:?}");
    assert_eq!(
        got.len() / usize::from(plan.output().channels),
        (new_total - start) as usize,
        "{status:?}"
    );
    assert_eq!(status.switches.len(), 2, "{status:?}");
    assert_played(
        &got,
        start,
        &status.switches,
        &[&plan, &longer],
        "longer plan",
    );
}

/// Two updates in quick succession: whichever plan a stretch of sound is, the
/// list names that one.
#[test]
fn quick_updates_are_named_for_the_plan_that_played() {
    let song = base_song(2);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let mut rng = Rng(77);
    let mut edits = Vec::new();
    for index in 0..200 {
        if let Ok(edited) = compile(&edit(&song, &mut rng, index).0) {
            if edited.output().total_frames == total {
                edits.push(edited);
            }
        }
        if edits.len() == 2 {
            break;
        }
    }
    for round in 0..6 {
        let bg = session(&plan, false);
        let play = bg.play(0, config()).unwrap();
        let mut got = read_exactly(&play, 2_000, 500);
        bg.update(&edits[0]);
        if round % 2 == 0 {
            std::thread::sleep(Duration::from_millis(round as u64));
        }
        bg.update(&edits[1]);
        got.extend(read_paced(&play, 480));
        let status = play.status();
        assert_played(
            &got,
            0,
            &status.switches,
            &[&plan, &edits[0], &edits[1]],
            &format!("round {round}"),
        );
        assert_eq!(status.switches.last().unwrap().generation, 2, "{status:?}");
    }
}

// ---- the approximate sound -----------------------------------------------

/// A preview cut into chunks is the preview of one render, whatever the cut.
#[test]
fn chunks_of_a_preview_are_one_preview() {
    let plan = compile(&base_song(7)).unwrap();
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    for cold in [1u64, 30_000, 100_000] {
        let mut whole = Vec::new();
        DspEngine::new_artifact_with_limits(&plan, &limits())
            .unwrap()
            .render_span(
                RenderSpan {
                    start: cold,
                    end: total,
                },
                |frame| {
                    whole.extend_from_slice(frame);
                    Ok(())
                },
            )
            .unwrap();
        for size in [1_000u64, 4_801, 70_000] {
            let mut engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
            let mut got = Vec::new();
            let mut at = cold;
            while at < total {
                let end = (at + size).min(total);
                engine
                    .render_preview_chunk(RenderSpan { start: at, end }, |frame| {
                        got.extend_from_slice(frame);
                        Ok(())
                    })
                    .unwrap();
                at = end;
            }
            assert_eq!(
                bits(&whole),
                bits(&got),
                "cold {cold}, chunks of {size} ({} frames)",
                got.len() / channels
            );
        }
    }
}

#[test]
fn a_misplaced_preview_chunk_is_refused() {
    let plan = compile(&base_song(7)).unwrap();
    let span = |start, end| RenderSpan { start, end };
    let mut engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
    // A preview does not start at frame 0 (that is the exact render).
    assert!(engine
        .render_preview_chunk(span(0, 100), |_| Ok(()))
        .is_err());
    engine
        .render_preview_chunk(span(500, 900), |_| Ok(()))
        .unwrap();
    assert!(engine
        .render_preview_chunk(span(1_000, 1_100), |_| Ok(()))
        .is_err());
    engine
        .render_preview_chunk(span(900, 1_100), |_| Ok(()))
        .unwrap();
    // An exact engine is not made to continue a preview, nor the other way.
    let mut exact = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
    exact.render_chunk(span(0, 500), |_| Ok(())).unwrap();
    assert!(exact
        .render_preview_chunk(span(500, 900), |_| Ok(()))
        .is_err());
}

/// An edit behind the playhead, far into a song nothing is rendered for: the
/// new plan is heard approximately long before the exact render catches up,
/// then exactly. Every stretch is what it says it is.
#[test]
fn the_new_plan_is_heard_approximately_then_exactly() {
    let song = base_song(4);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let reference = maac::dsp::CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut rng = Rng(9);
    let early = (0..300)
        .find_map(|index| {
            let edited = compile(&edit(&song, &mut rng, 8 + index).0).ok()?;
            (edited.output().total_frames == total
                && matches!(
                    reference.reuse_for(&edited).unwrap(),
                    maac::dsp::Reuse::From(bound) if bound < total / 20
                ))
            .then_some(edited)
        })
        .expect("an early edit");
    let start = total * 2 / 3;
    let bg = session(&plan, false);
    let play = bg
        .play(
            start,
            PlaybackConfig {
                preview_preroll_frames: 12_000,
                ..config()
            },
        )
        .unwrap();
    let mut got = read_exactly(&play, 2_000, 500);
    bg.update(&early);
    got.extend(read_paced(&play, 480));
    let status = play.status();
    assert!(!status.stale && !status.approximate, "{status:?}");
    let kinds: Vec<_> = status
        .switches
        .iter()
        .map(|switch| (switch.generation, switch.approximate_from.is_some()))
        .collect();
    assert_eq!(kinds, [(0, false), (1, true), (1, false)], "{status:?}");
    assert_exact_took_over(&status, total);
    assert_played(
        &got,
        start,
        &status.switches,
        &[&plan, &early],
        "approximate",
    );
}

/// The exact render took the sound over before the song ended: the last
/// switch is exact and starts at a frame that exists.
fn assert_exact_took_over(status: &maac::dsp::PlaybackStatus, total: u64) {
    let last = status.switches.last().unwrap();
    assert!(
        last.approximate_from.is_none() && last.frame < total,
        "the exact render never took over: {status:?}"
    );
}

fn approximate_start_config() -> PlaybackConfig {
    PlaybackConfig {
        preview_preroll_frames: 12_000,
        approximate_start: true,
        ..config()
    }
}

#[test]
fn a_start_far_from_a_checkpoint_is_approximate_then_exact() {
    let plan = compile(&base_song(5)).unwrap();
    let total = plan.output().total_frames;
    let start = total * 2 / 3;
    // No worker: the only checkpoint is frame 0.
    let bg = session(&plan, false);
    let play = bg.play(start, approximate_start_config()).unwrap();
    let got = read_paced(&play, 480);
    let status = play.status();
    assert!(!status.approximate && !status.stale, "{status:?}");
    let kinds: Vec<_> = status
        .switches
        .iter()
        .map(|switch| (switch.frame >= start, switch.approximate_from))
        .collect();
    assert_eq!(
        kinds,
        [(true, Some(start - 12_000)), (true, None)],
        "{status:?}"
    );
    assert_eq!(status.switches[0].frame, start);
    assert_exact_took_over(&status, total);
    assert_played(&got, start, &status.switches, &[&plan], "approximate start");
}

#[test]
fn a_start_near_a_checkpoint_is_exact() {
    let plan = compile(&base_song(5)).unwrap();
    let channels = usize::from(plan.output().channels);
    let total = plan.output().total_frames;
    let fresh = render_full(&plan);
    let bg = session(&plan, true);
    assert!(bg.wait_idle(WAIT));
    // Three prerolls is 36 000 frames; the checkpoints are 4 801 apart.
    let start = total / 2 + 1_234;
    let play = bg.play(start, approximate_start_config()).unwrap();
    let got = read_exactly(&play, 8_000, 500);
    assert_eq!(
        bits(&fresh[start as usize * channels..(start as usize + 8_000) * channels]),
        bits(&got)
    );
    let status = play.status();
    assert_eq!(status.switches.len(), 1, "{status:?}");
    assert_eq!(status.switches[0].approximate_from, None);
    assert!(!status.approximate);
}

#[test]
fn a_seek_far_from_a_checkpoint_is_approximate_then_exact() {
    let plan = compile(&base_song(5)).unwrap();
    let total = plan.output().total_frames;
    let bg = session(&plan, false);
    let play = bg.play(0, approximate_start_config()).unwrap();
    let _ = read_exactly(&play, 3_000, 700);
    let target = total / 2 + 77;
    play.seek(target);
    let got = read_paced(&play, 480);
    let status = play.status();
    let kinds: Vec<_> = status
        .switches
        .iter()
        .map(|switch| switch.approximate_from)
        .collect();
    assert_eq!(kinds, [Some(target - 12_000), None], "{status:?}");
    assert_exact_took_over(&status, total);
    assert_played(&got, target, &status.switches, &[&plan], "approximate seek");
}

#[test]
fn an_edit_while_the_start_is_approximate_is_followed() {
    let song = base_song(4);
    let plan = compile(&song).unwrap();
    let total = plan.output().total_frames;
    let reference = maac::dsp::CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut rng = Rng(11);
    let early = (0..300)
        .find_map(|index| {
            let edited = compile(&edit(&song, &mut rng, 20 + index).0).ok()?;
            (edited.output().total_frames == total
                && matches!(
                    reference.reuse_for(&edited).unwrap(),
                    maac::dsp::Reuse::From(bound) if bound < total / 20
                ))
            .then_some(edited)
        })
        .expect("an early edit");
    let start = total * 2 / 3;
    let bg = session(&plan, false);
    let play = bg.play(start, approximate_start_config()).unwrap();
    // Once the approximate sound is playing, so that the edit arrives while it
    // does (an edit that comes earlier is another path).
    // Read off the switch, which stays: the flag goes out again at the hand-over.
    wait_for("the approximate sound", || {
        play.status()
            .switches
            .first()
            .is_some_and(|switch| switch.approximate_from.is_some())
    });
    bg.update(&early);
    let got = read_paced(&play, 480);
    let status = play.status();
    assert!(!status.approximate && !status.stale, "{status:?}");
    let last = status.switches.last().unwrap();
    assert_eq!(
        (last.generation, last.approximate_from),
        (1, None),
        "{status:?}"
    );
    assert!(last.frame < total, "{status:?}");
    assert_played(
        &got,
        start,
        &status.switches,
        &[&plan, &early],
        "edit during an approximate start",
    );
}

#[test]
fn an_approximate_start_can_be_turned_off() {
    let plan = compile(&base_song(5)).unwrap();
    let channels = usize::from(plan.output().channels);
    let total = plan.output().total_frames;
    let fresh = render_full(&plan);
    let start = total * 2 / 3;
    for config in [
        PlaybackConfig {
            approximate_start: false,
            ..approximate_start_config()
        },
        PlaybackConfig {
            preview_preroll_frames: 0,
            ..approximate_start_config()
        },
    ] {
        let bg = session(&plan, false);
        let play = bg.play(start, config).unwrap();
        let got = read_exactly(&play, 8_000, 500);
        assert_eq!(
            bits(&fresh[start as usize * channels..(start as usize + 8_000) * channels]),
            bits(&got)
        );
        let status = play.status();
        assert_eq!(status.switches.len(), 1, "{status:?}");
        assert_eq!(status.switches[0].approximate_from, None);
    }
}

#[test]
fn an_approximate_start_catches_up_from_a_real_checkpoint() {
    let plan = compile(&base_song(5)).unwrap();
    let total = plan.output().total_frames;
    // Far apart, so that the exact render has much further to go than the
    // approximate one, whatever the machine is doing.
    let bg =
        BackgroundSession::new_with(&plan, &limits(), SessionPolicy::new(40_000), true).unwrap();
    assert!(bg.wait_idle(WAIT));
    // The gate is three prerolls, 3 000 frames: 30 000 past a checkpoint is far.
    let start = (total / 2 / 40_000) * 40_000 + 30_000;
    let config = PlaybackConfig {
        preview_preroll_frames: 1_000,
        ..approximate_start_config()
    };
    let play = bg.play(start, config).unwrap();
    let got = read_paced(&play, 480);
    let status = play.status();
    let kinds: Vec<_> = status
        .switches
        .iter()
        .map(|switch| switch.approximate_from)
        .collect();
    assert_eq!(kinds, [Some(start - 1_000), None], "{status:?}");
    assert_exact_took_over(&status, total);
    assert_played(&got, start, &status.switches, &[&plan], "from a checkpoint");
}

#[test]
fn an_enormous_preroll_does_not_overflow_the_start_gate() {
    let plan = compile(&base_song(5)).unwrap();
    let channels = usize::from(plan.output().channels);
    let total = plan.output().total_frames;
    let fresh = render_full(&plan);
    let start = total * 2 / 3;
    let bg = session(&plan, false);
    let play = bg
        .play(
            start,
            PlaybackConfig {
                preview_preroll_frames: u64::MAX,
                ..approximate_start_config()
            },
        )
        .unwrap();
    let got = read_exactly(&play, 8_000, 500);
    assert_eq!(
        bits(&fresh[start as usize * channels..(start as usize + 8_000) * channels]),
        bits(&got)
    );
    assert!(play.status().last_error.is_none(), "{:?}", play.status());
}
