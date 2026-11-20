//! A checkpoint taken during a render lets a restored engine continue it:
//! the frames it produces equal, bit for bit, the frames the uninterrupted
//! render produced. Taking checkpoints must not change the render either.

use std::path::{Path, PathBuf};

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{Checkpoint, DspEngine, RenderSpan};
use maac::plan::PlanLimits;
use maac::{PlanArtifact, SourceBundle};

fn bits(frames: &[f64]) -> Vec<u64> {
    frames.iter().map(|sample| sample.to_bits()).collect()
}

fn plan_of(path: &Path) -> Option<PlanArtifact> {
    let source = std::fs::read_to_string(path).unwrap();
    let bundle = SourceBundle::new("standalone.maac", source);
    // A song that needs files beside it is not a self-contained example.
    match compile_bundle_artifact_with_limits(&bundle, &PlanLimits::song()) {
        Ok(plan) => Some(plan),
        Err(error) => {
            eprintln!("{}: {error:?}", path.display());
            None
        }
    }
}

fn engine(plan: &PlanArtifact) -> DspEngine<'_> {
    DspEngine::new_artifact_with_limits(plan, &PlanLimits::song()).unwrap()
}

fn render_plain(plan: &PlanArtifact) -> Vec<f64> {
    let mut out = Vec::new();
    engine(plan)
        .render(|frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    out
}

fn render_checkpointed(plan: &PlanArtifact, every: u64) -> (Vec<f64>, Vec<Checkpoint<'_>>) {
    let mut out = Vec::new();
    let checkpoints = engine(plan)
        .render_with_checkpoints(every, |frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    (out, checkpoints)
}

fn resume(plan: &PlanArtifact, checkpoint: &Checkpoint<'_>, total: u64) -> Vec<f64> {
    let mut out = Vec::new();
    DspEngine::restore(checkpoint)
        .render_span(
            RenderSpan {
                start: checkpoint.frame(),
                end: total,
            },
            |frame| {
                out.extend_from_slice(frame);
                Ok(())
            },
        )
        .unwrap();
    let _ = plan;
    out
}

/// Render `plan` plainly and with checkpoints, then resume from the first,
/// a middle and the last checkpoint.
fn check(name: &str, plan: &PlanArtifact, every: u64) {
    let channels = usize::from(plan.output().channels);
    let total = plan.output().total_frames;
    let plain = render_plain(plan);
    let (with, checkpoints) = render_checkpointed(plan, every);
    assert_eq!(
        bits(&plain),
        bits(&with),
        "{name}: taking checkpoints changed the render"
    );
    assert!(!checkpoints.is_empty(), "{name}: no checkpoint taken");
    let picks = [0, checkpoints.len() / 2, checkpoints.len() - 1];
    for &pick in &picks {
        let checkpoint = &checkpoints[pick];
        let tail = resume(plan, checkpoint, total);
        let from = checkpoint.frame() as usize * channels;
        assert_eq!(
            bits(&plain[from..]),
            bits(&tail),
            "{name}: resuming at frame {} differs",
            checkpoint.frame()
        );
    }
}

fn examples(dir: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> =
        std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(dir))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "maac"))
            .collect();
    paths.sort();
    paths
}

#[test]
fn resuming_a_checkpoint_equals_the_uninterrupted_render() {
    let mut checked = 0;
    for path in examples("examples") {
        let Some(plan) = plan_of(&path) else { continue };
        // Short checkpoints for short songs, and spacing off the block size.
        let every = (plan.output().total_frames / 5).max(1) | 1;
        check(&path.display().to_string(), &plan, every);
        checked += 1;
    }
    assert!(
        checked >= 10,
        "only {checked} examples compiled on their own"
    );
}

/// The full songs; slow, run with `--ignored --nocapture`.
#[test]
#[ignore]
fn resuming_a_checkpoint_equals_the_uninterrupted_render_on_the_songs() {
    for song in ["homeward", "afterlight", "lantern", "ferry", "lofi"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(song)
            .join(format!("{song}.maac"));
        let plan = plan_of(&path).unwrap_or_else(|| panic!("{song} compiles on its own"));
        check(song, &plan, 48_000 * 4 + 37);
    }
}

/// The check is only worth something if a wrong restore is caught: an engine
/// that starts cold at a checkpoint frame renders something else.
#[test]
fn a_cold_engine_does_not_pass_for_a_restored_one() {
    let mut differing = 0;
    for path in examples("examples") {
        let Some(plan) = plan_of(&path) else { continue };
        let total = plan.output().total_frames;
        let every = (total / 5).max(1) | 1;
        let (plain, checkpoints) = render_checkpointed(&plan, every);
        let Some(checkpoint) = checkpoints.get(checkpoints.len() / 2) else {
            continue;
        };
        let mut cold = Vec::new();
        engine(&plan)
            .render_span(
                RenderSpan {
                    start: checkpoint.frame(),
                    end: total,
                },
                |frame| {
                    cold.extend_from_slice(frame);
                    Ok(())
                },
            )
            .unwrap();
        let from = checkpoint.frame() as usize * usize::from(plan.output().channels);
        if bits(&plain[from..]) != bits(&cold) {
            differing += 1;
        }
    }
    assert!(
        differing >= 5,
        "only {differing} examples tell cold from restored"
    );
}
