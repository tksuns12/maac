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
    let total = {
        let engine = DspEngine::new_artifact_with_limits(&plan, &limits()).unwrap();
        drop(engine);
        let (all, _) = render(&plan, 1);
        (all.len() / 2) as u64
    };
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
