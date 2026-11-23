//! Rendering instrument blocks on several threads changes no sample: whatever
//! the number of threads, a render is the same bit for bit.

use maac::dsp::{DspEngine, RenderSpan};
use maac::plan::PlanLimits;
use maac::PlanArtifact;

#[path = "common/song_edits.rs"]
mod song_edits;
use song_edits::*;

fn limits() -> PlanLimits {
    PlanLimits::song()
}

fn render(plan: &PlanArtifact, threads: usize) -> (Vec<u64>, u64) {
    let mut engine = DspEngine::new_artifact_with_limits(plan, &limits()).unwrap();
    engine.set_render_threads(threads);
    let mut out = Vec::new();
    engine
        .render(|frame| {
            out.extend(frame.iter().map(|sample| sample.to_bits()));
            Ok(())
        })
        .unwrap();
    (out, engine.parallel_batches())
}

#[test]
fn a_render_is_the_same_on_any_number_of_threads() {
    let mut batched = 0;
    for seed in 1..=4 {
        let plan = compile(&base_song(seed)).unwrap();
        let (serial, serial_batches) = render(&plan, 1);
        assert_eq!(serial_batches, 0);
        for threads in [2, 3, 8] {
            let (parallel, batches) = render(&plan, threads);
            assert!(serial == parallel, "seed {seed}, {threads} threads differ");
            batched += batches;
        }
    }
    assert!(batched > 0, "no block was ever rendered side by side");
}

#[test]
fn a_preview_and_pieces_are_the_same_on_any_number_of_threads() {
    let plan = compile(&base_song(2)).unwrap();
    let total = plan.output().total_frames;
    let start = total / 3;
    let mut outputs = Vec::new();
    for threads in [1, 4] {
        let mut engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
        engine.set_render_threads(threads);
        let mut out = Vec::new();
        let mut at = start;
        // A cold preview, in pieces that end between block boundaries.
        while at < total {
            let end = (at + 777).min(total);
            engine
                .render_preview_chunk(RenderSpan { start: at, end }, |frame| {
                    out.extend(frame.iter().map(|sample| sample.to_bits()));
                    Ok(())
                })
                .unwrap();
            at = end;
        }
        outputs.push(out);
    }
    assert!(outputs[0] == outputs[1], "the preview differs");
}

#[test]
fn checkpoints_and_resumes_are_the_same_on_any_number_of_threads() {
    let plan = compile(&base_song(1)).unwrap();
    let (plain, _) = render(&plan, 1);
    let total = plan.output().total_frames;
    let channels = usize::from(plan.output().channels);
    let every = (total / 5).max(512);

    // Taking checkpoints on one thread or several gives the same render and
    // the same checkpoints, and a resume on either continues it exactly.
    let mut taken = Vec::new();
    for threads in [1, 4] {
        let mut engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
        engine.set_render_threads(threads);
        let mut out = Vec::new();
        let checkpoints = engine
            .render_with_checkpoints(every, |frame| {
                out.extend(frame.iter().map(|sample| sample.to_bits()));
                Ok(())
            })
            .unwrap();
        assert!(
            out == plain,
            "{threads} threads: checkpoints changed the render"
        );
        assert!(!checkpoints.is_empty());
        taken.push(checkpoints);
    }
    assert_eq!(taken[0].len(), taken[1].len());
    for (a, b) in taken[0].iter().zip(&taken[1]) {
        assert_eq!(a.frame(), b.frame());
    }

    let pick = taken[0].len() / 2;
    for (checkpoints, threads) in [(&taken[0], 4), (&taken[1], 1)] {
        let checkpoint = &checkpoints[pick];
        let mut engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
        engine.set_render_threads(threads);
        engine.restore_from(checkpoint).unwrap();
        let mut out = Vec::new();
        engine
            .render_span(
                RenderSpan {
                    start: checkpoint.frame(),
                    end: total,
                },
                |frame| {
                    out.extend(frame.iter().map(|sample| sample.to_bits()));
                    Ok(())
                },
            )
            .unwrap();
        assert!(
            plain[checkpoint.frame() as usize * channels..] == out[..],
            "a resume on {threads} threads differs"
        );
    }
}
