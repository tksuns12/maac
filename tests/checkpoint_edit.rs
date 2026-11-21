//! Resuming a render after an edit. Songs are generated from a small model
//! (a bowed line with joins and slides, polyphonic sine notes, a reverb, a
//! delay loop and two automation lanes), edited at random, and every edit is
//! checked against a fresh render of the edited song: the frames after the
//! reported bound must equal it bit for bit, with the checkpoint taken exactly
//! at the bound and on a coarse grid. A bound that is too late must be caught.

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{CheckpointSet, DspEngine, RenderSpan, Reuse};
use maac::plan::PlanLimits;
use maac::{PlanArtifact, SourceBundle};

#[path = "common/song_edits.rs"]
mod song_edits;
use song_edits::*;

fn limits() -> PlanLimits {
    PlanLimits::song()
}

fn bits(frames: &[f64]) -> Vec<u64> {
    frames.iter().map(|sample| sample.to_bits()).collect()
}

fn render_full(plan: &PlanArtifact) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    DspEngine::new_artifact_with_limits(plan, &limits())
        .map_err(|e| format!("{e:?}"))?
        .render(|frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .map_err(|e| format!("{e:?}"))?;
    Ok(out)
}

/// The frames after a resume, from `set`, trusting `bound` when given.
fn resumed_tail(
    set: &CheckpointSet,
    plan: &PlanArtifact,
    bound: Option<u64>,
) -> Result<(u64, Vec<f64>), String> {
    let mut resumed = set.resume_with(plan, bound).map_err(|e| format!("{e:?}"))?;
    let total = plan.output().total_frames;
    let start = resumed.start_frame;
    let mut out = Vec::new();
    resumed
        .engine
        .render_span(RenderSpan { start, end: total }, |frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .map_err(|e| format!("{e:?}"))?;
    Ok((start, out))
}

#[derive(Default)]
struct Tally {
    accepted: usize,
    rejected: usize,
    unchanged: usize,
    full: usize,
    /// Partial results whose bound is frame 0: a full render.
    from_zero: usize,
    partial: usize,
    resumed_late: usize,
    caught_late: usize,
    note_edits_late: usize,
    note_edits_caught: usize,
    kinds: std::collections::BTreeMap<&'static str, usize>,
}

/// Every edit in `edits` of the song `seed`, checked three ways.
fn fuzz(seed: u64, edits: usize, tally: &mut Tally) {
    let base = base_song(seed);
    let base_plan = compile(&base).unwrap_or_else(|error| panic!("seed {seed} base: {error}"));
    let grid = CheckpointSet::render(&base_plan, &limits(), 4_801, |_| Ok(())).unwrap();
    let channels = usize::from(base_plan.output().channels);
    let mut rng = Rng(seed ^ 0xA5A5_5A5A_1234_5678);
    for index in 0..edits {
        let (song, kind) = edit(&base, &mut rng, seed + index as u64);
        let plan = match compile(&song) {
            Ok(plan) => plan,
            Err(_) => {
                tally.rejected += 1;
                continue;
            }
        };
        let Ok(fresh) = render_full(&plan) else {
            tally.rejected += 1;
            continue;
        };
        tally.accepted += 1;
        *tally.kinds.entry(kind).or_default() += 1;
        let reuse = grid.reuse_for(&plan).unwrap();
        let context = format!("seed {seed} edit {kind} reuse {reuse:?}");

        // On the coarse grid.
        let (start, tail) = resumed_tail(&grid, &plan, None)
            .unwrap_or_else(|error| panic!("{context}: grid resume failed: {error}"));
        assert_eq!(
            bits(&fresh[start as usize * channels..]),
            bits(&tail),
            "{context}: grid, resumed at {start}"
        );

        match reuse {
            Reuse::Unchanged => tally.unchanged += 1,
            Reuse::Full(_) => tally.full += 1,
            Reuse::From(bound) => {
                tally.partial += 1;
                tally.from_zero += usize::from(bound == 0);
                // With a checkpoint exactly at the bound, and just after it.
                let total = plan.output().total_frames;
                let late = [bound + 1, bound + 97].map(|frame| frame.min(total - 1));
                let frames = [bound, late[0], late[1]];
                let exact =
                    CheckpointSet::render_at(&base_plan, &limits(), &frames, |_| Ok(())).unwrap();
                let (start, tail) = resumed_tail(&exact, &plan, None).unwrap();
                assert_eq!(start, bound, "{context}: no checkpoint at the bound");
                assert_eq!(
                    bits(&fresh[start as usize * channels..]),
                    bits(&tail),
                    "{context}: exact checkpoint at {bound}"
                );
                // A bound that is too late must give wrong frames.
                for frame in late {
                    if frame <= bound {
                        continue;
                    }
                    // Wrong state may give wrong frames or an outright error
                    // (a note-off for a voice the old state never started).
                    let wrong = match resumed_tail(&exact, &plan, Some(frame)) {
                        Ok((start, tail)) => {
                            assert_eq!(start, frame);
                            bits(&fresh[start as usize * channels..]) != bits(&tail)
                        }
                        Err(_) => true,
                    };
                    tally.resumed_late += 1;
                    tally.caught_late += usize::from(wrong);
                    if matches!(
                        kind,
                        "move" | "pitch" | "duration" | "velocity" | "delete" | "add"
                    ) {
                        tally.note_edits_late += 1;
                        tally.note_edits_caught += usize::from(wrong);
                    }
                }
            }
        }
    }
}

#[test]
fn an_edited_song_resumes_exactly_from_the_reported_bound() {
    let mut tally = Tally::default();
    // Fifteen consecutive edits cover every kind once.
    for seed in [1, 20] {
        fuzz(seed, 15, &mut tally);
    }
    eprintln!(
        "accepted {} rejected {} unchanged {} full {} from-zero {} partial {}; late bounds caught {}/{} (note edits {}/{}); kinds {:?}",
        tally.accepted,
        tally.rejected,
        tally.unchanged,
        tally.full,
        tally.from_zero,
        tally.partial,
        tally.caught_late,
        tally.resumed_late,
        tally.note_edits_caught,
        tally.note_edits_late,
        tally.kinds
    );
    assert!(tally.accepted >= 18, "{}", tally.accepted);
    assert!(tally.partial >= 10, "{}", tally.partial);
    // A diff that always says "frame 0" would pass every exactness check.
    // An edit near the start of the song really is a full render, so some
    // zeros are right; a diff that always said frame 0 would fail this.
    assert!(
        tally.full * 4 <= tally.accepted && tally.from_zero * 2 <= tally.partial,
        "too many full renders: {} full, {} from frame 0 of {} partial, {} accepted",
        tally.full,
        tally.from_zero,
        tally.partial,
        tally.accepted
    );
    // The fuzz must be able to see a bound that is too late.
    assert!(tally.note_edits_late >= 8, "{}", tally.note_edits_late);
    assert!(
        tally.note_edits_caught * 10 >= tally.note_edits_late * 8,
        "only {} of {} late bounds on note edits were caught",
        tally.note_edits_caught,
        tally.note_edits_late
    );
    for kind in ["move", "pitch", "add", "knot-value"] {
        assert!(
            tally.kinds.get(kind).copied().unwrap_or(0) >= 1,
            "no {kind} edit ran"
        );
    }
}

/// A late note changed in a long song reports a bound near its onset, not 0.
#[test]
fn a_late_note_edit_reports_a_late_bound() {
    let base = base_song(7);
    let plan = compile(&base).unwrap();
    let grid = CheckpointSet::render(&plan, &limits(), 4_801, |_| Ok(())).unwrap();
    let mut edited = base.clone();
    let late = edited
        .notes
        .iter_mut()
        .filter(|n| !n.strings)
        .max_by_key(|n| n.at)
        .unwrap();
    late.pitch = (late.pitch + 1) % PITCHES.len();
    let on = late.at as u64 * EIGHTH;
    let new_plan = compile(&edited).unwrap();
    match grid.reuse_for(&new_plan).unwrap() {
        Reuse::From(frame) => assert_eq!(
            frame, on,
            "a changed winds note starts the bound at its onset"
        ),
        other => panic!("{other:?}"),
    }
}

/// Many more songs and edits; slow. `MAAC_FUZZ_SEEDS=a..b` picks the seeds.
#[test]
#[ignore]
fn an_edited_song_resumes_exactly_over_many_seeds() {
    let range = std::env::var("MAAC_FUZZ_SEEDS").unwrap_or_else(|_| "100..130".into());
    let (low, high) = range.split_once("..").expect("a..b");
    let mut tally = Tally::default();
    for seed in low.parse::<u64>().unwrap()..high.parse::<u64>().unwrap() {
        fuzz(seed, 15, &mut tally);
    }
    eprintln!(
        "accepted {} rejected {} unchanged {} full {} partial {}; late caught {}/{}; note edits {}/{}; kinds {:?}",
        tally.accepted, tally.rejected, tally.unchanged, tally.full, tally.partial,
        tally.caught_late, tally.resumed_late, tally.note_edits_caught, tally.note_edits_late, tally.kinds
    );
    assert!(tally.note_edits_caught * 10 >= tally.note_edits_late * 8);
}

/// On the checked-in examples, editing a note's velocity in the text must not
/// make the comparison give up: every plan carries provenance (source hashes,
/// execution identities) that changes with each edit and a render never reads.
#[test]
fn a_velocity_edit_in_a_real_example_keeps_checkpoints() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let (mut partial, mut full, mut unchanged, mut compiled) = (0, Vec::new(), 0, 0);
    for name in [
        "effects",
        "core-delay",
        "evening-window",
        "hello",
        "core-fader",
        "production",
        "core-noise",
        "lofi/lofi",
        "ferry/ferry",
        "dub/dub",
    ] {
        let path = root.join("examples").join(format!("{name}.maac"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(base) = compile_text(&text) else {
            continue;
        };
        compiled += 1;
        let set = CheckpointSet::without_checkpoints(&base, &limits()).unwrap();
        for (index, _) in text.rmatch_indices("velocity = ") {
            let rest = &text[index + 11..];
            let end = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '/'))
                .unwrap_or(0);
            if end == 0 {
                continue;
            }
            let edited = format!("{}velocity = 0.31{}", &text[..index], &rest[end..]);
            let Ok(plan) = compile_text(&edited) else {
                continue;
            };
            match set.reuse_for(&plan).unwrap() {
                Reuse::From(_) => partial += 1,
                Reuse::Unchanged => unchanged += 1,
                Reuse::Full(reason) => full.push((name, reason)),
            }
            break;
        }
    }
    assert!(compiled >= 8, "only {compiled} examples compiled");
    // An edit inside an instrument definition is a real full render; a note
    // is not.
    assert!(
        partial >= 3,
        "partial {partial}, unchanged {unchanged}, full {full:?}"
    );
    assert!(
        full.iter().all(|(_, reason)| *reason == "instruments"),
        "{full:?}"
    );
}

fn compile_text(text: &str) -> Result<PlanArtifact, String> {
    compile_bundle_artifact_with_limits(
        &SourceBundle::new("standalone.maac", text.to_owned()),
        &limits(),
    )
    .map_err(|error| format!("{error:?}"))
}

/// Editing the last knot of a lane (a fade-out) changes the value held after
/// the score ends, but a render can differ only from the knot before it.
#[test]
fn a_change_to_the_last_knot_reports_the_knot_before_it() {
    let base = base_song(7);
    let plan = compile(&base).unwrap();
    let grid = CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut edited = base.clone();
    // The base lane has knots at 0, 6 (a step) and 12 eighths.
    edited.fader[2].value -= 3;
    let new_plan = compile(&edited).unwrap();
    assert_eq!(
        grid.reuse_for(&new_plan).unwrap(),
        Reuse::From(6 * EIGHTH),
        "the segment before the last knot is a step, so the knot at 6 bounds it"
    );
}

/// Editing text before an instrument moves the instrument's source span; that
/// is not a change to the instrument.
#[test]
fn a_note_edit_that_changes_the_text_length_before_an_instrument_is_not_a_full_render() {
    let base = base_song(7);
    let plan = compile(&base).unwrap();
    let grid = CheckpointSet::without_checkpoints(&plan, &limits()).unwrap();
    let mut edited = base.clone();
    let note = edited.notes.iter_mut().find(|n| !n.strings).unwrap();
    // "A4" and "Bb4" differ in length.
    note.pitch = if PITCHES[note.pitch].len() == 2 { 6 } else { 0 };
    assert_ne!(source(&base).len(), source(&edited).len());
    let new_plan = compile(&edited).unwrap();
    assert!(matches!(grid.reuse_for(&new_plan).unwrap(), Reuse::From(_)));
}
