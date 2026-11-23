//! Splitting the voices of an instrument across threads changes no sample.
//! This file sets the split limit to zero, so every block after the first is
//! split whatever the machine's speed; it must stay in a file of its own
//! because the limit is read once per process.

use maac::dsp::{DspEngine, RenderSpan};
use maac::plan::PlanLimits;
use maac::PlanArtifact;

#[path = "common/song_edits.rs"]
mod song_edits;
use song_edits::*;

fn render(plan: &PlanArtifact, threads: usize) -> Vec<u64> {
    render_counting(plan, threads).0
}

/// The render's bits and how many blocks had their voices split.
fn render_counting(plan: &PlanArtifact, threads: usize) -> (Vec<u64>, u64) {
    let mut engine = DspEngine::new_artifact_with_limits(plan, &PlanLimits::song()).unwrap();
    engine.set_render_threads(threads);
    let mut out = Vec::new();
    engine
        .render(|frame| {
            out.extend(frame.iter().map(|sample| sample.to_bits()));
            Ok(())
        })
        .unwrap();
    (out, engine.voice_splits())
}

#[test]
fn split_voices_render_the_same_samples() {
    std::env::set_var("MAAC_VOICE_SPLIT_NANOS", "0");
    for seed in 1..=4 {
        let plan = compile(&base_song(seed)).unwrap();
        let serial = render(&plan, 1);
        for threads in [2, 3, 8] {
            let (parallel, splits) = render_counting(&plan, threads);
            assert!(serial == parallel, "seed {seed}, {threads} threads differ");
            assert!(
                splits > 0,
                "seed {seed}, {threads} threads: no block was split"
            );
        }
        assert_eq!(render_counting(&plan, 1).1, 0);
    }
}

#[test]
fn split_voices_survive_a_seek_and_pieces() {
    std::env::set_var("MAAC_VOICE_SPLIT_NANOS", "0");
    let plan = compile(&base_song(3)).unwrap();
    let total = plan.output().total_frames;
    let mut outputs = Vec::new();
    for threads in [1, 4] {
        let mut engine = DspEngine::new_artifact_with_limits(&plan, &PlanLimits::song()).unwrap();
        engine.set_render_threads(threads);
        let mut out = Vec::new();
        let mut at = total / 4;
        while at < total {
            let end = (at + 1_001).min(total);
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
