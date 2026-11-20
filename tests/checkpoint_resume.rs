//! A checkpoint taken during a render lets a restored engine continue it:
//! the frames it produces equal, bit for bit, the frames the uninterrupted
//! render produced. Taking checkpoints must not change the render either.

use std::path::{Path, PathBuf};

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{Checkpoint, DspEngine, RenderSpan};
use maac::plan::PlanLimits;
use maac::PlanArtifact;

fn bits(frames: &[f64]) -> Vec<u64> {
    frames.iter().map(|sample| sample.to_bits()).collect()
}

fn plan_of(path: &Path) -> Option<PlanArtifact> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bundle = maac::bundle_fs::load_bundle(path, root)
        .unwrap_or_else(|error| panic!("{} loads: {error:?}", path.display()));
    let plan = compile_bundle_artifact_with_limits(&bundle, &PlanLimits::song())
        .unwrap_or_else(|error| panic!("{} compiles: {error:?}", path.display()));
    Some(plan)
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

fn render_checkpointed(plan: &PlanArtifact, every: u64) -> (Vec<f64>, Vec<Checkpoint>) {
    let mut out = Vec::new();
    let checkpoints = engine(plan)
        .render_with_checkpoints(every, |frame| {
            out.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
    (out, checkpoints)
}

/// A freshly built engine for `plan` that continues from `checkpoint`.
fn restored<'a>(plan: &'a PlanArtifact, checkpoint: &Checkpoint) -> DspEngine<'a> {
    let mut engine = engine(plan);
    engine.restore_from(checkpoint).unwrap();
    engine
}

fn resume(plan: &PlanArtifact, checkpoint: &Checkpoint, total: u64) -> Vec<f64> {
    let mut out = Vec::new();
    restored(plan, checkpoint)
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
    out
}

/// Render `plan` plainly and with checkpoints, then resume from the first,
/// a middle and the last checkpoint on an engine built from `second`, a separate
/// compile of the same source.
fn check(name: &str, plan: &PlanArtifact, second: &PlanArtifact, every: u64) {
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
        let tail = resume(second, checkpoint, total);
        let from = checkpoint.frame() as usize * channels;
        assert_eq!(
            bits(&plain[from..]),
            bits(&tail),
            "{name}: resuming at frame {} differs",
            checkpoint.frame()
        );
    }
}

/// The songs are long; they run in the ignored test below.
const SONGS: [&str; 5] = ["homeward", "afterlight", "lantern", "ferry", "lofi"];

/// Examples that cannot be rendered on their own: libraries of sounds, and a
/// showcase whose plan is larger than the song byte limit.
const NOT_RENDERABLE: [&str; 2] = ["showcase", "studio"];

fn collect(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "sounds" {
                collect(&path, into);
            }
        } else if path.extension().is_some_and(|ext| ext == "maac")
            && !SONGS.contains(&name.as_str())
            && !NOT_RENDERABLE.contains(&name.as_str())
        {
            into.push(path);
        }
    }
}

/// Every example that renders on its own, found in subdirectories too, except
/// the songs.
fn examples(dir: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect(&Path::new(env!("CARGO_MANIFEST_DIR")).join(dir), &mut paths);
    paths.sort();
    paths
}

/// Examples longer than this (in frames) run in the ignored test.
const SHORT_FRAMES: u64 = 20 * 48_000;

#[test]
fn resuming_a_checkpoint_equals_the_uninterrupted_render() {
    let mut checked = 0;
    for path in examples("examples") {
        let Some(plan) = plan_of(&path) else { continue };
        if plan.output().total_frames > SHORT_FRAMES {
            continue;
        }
        // Short checkpoints for short songs, and spacing off the block size.
        let every = (plan.output().total_frames / 5).max(1) | 1;
        check(
            &path.display().to_string(),
            &plan,
            &plan_of(&path).unwrap(),
            every,
        );
        checked += 1;
    }
    assert!(checked >= 30, "only {checked} examples were checked");
}

/// The examples longer than 20 s and the full songs; slow, run with `--ignored --nocapture`.
#[test]
#[ignore]
fn resuming_a_checkpoint_equals_the_uninterrupted_render_on_the_songs() {
    for path in examples("examples") {
        let plan = plan_of(&path).unwrap();
        if plan.output().total_frames > SHORT_FRAMES {
            check(
                &path.display().to_string(),
                &plan,
                &plan_of(&path).unwrap(),
                48_000 * 4 + 37,
            );
        }
    }
    for song in SONGS {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(song)
            .join(format!("{song}.maac"));
        let plan = plan_of(&path).unwrap_or_else(|| panic!("{song} compiles on its own"));
        check(song, &plan, &plan_of(&path).unwrap(), 48_000 * 4 + 37);
    }
}

/// The check is only worth something if a wrong restore is caught: an engine
/// that starts cold at a checkpoint frame renders something else.
#[test]
fn a_cold_engine_does_not_pass_for_a_restored_one() {
    let mut differing = 0;
    for path in examples("examples") {
        let Some(plan) = plan_of(&path) else { continue };
        if plan.output().total_frames > SHORT_FRAMES {
            continue;
        }
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

fn example(name: &str) -> PlanArtifact {
    plan_of(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("examples/{name}.maac"))).unwrap()
}

/// Checkpoints that fall at every offset inside a render block, resuming from
/// each one.
#[test]
fn every_checkpoint_of_a_densely_spaced_render_resumes_exactly() {
    for name in ["hello", "core-delay"] {
        let plan = example(name);
        let channels = usize::from(plan.output().channels);
        let total = plan.output().total_frames;
        let plain = render_plain(&plan);
        for every in [1009, 4801] {
            let (_, checkpoints) = render_checkpointed(&plan, every);
            assert!(checkpoints.len() >= 3, "{name}: {every}");
            for checkpoint in checkpoints.iter().step_by(5) {
                let tail = resume(&plan, checkpoint, total);
                let from = checkpoint.frame() as usize * channels;
                assert_eq!(
                    bits(&plain[from..]),
                    bits(&tail),
                    "{name}: spacing {every}, resuming at frame {}",
                    checkpoint.frame()
                );
            }
        }
    }
}

fn tail_of(engine: &mut DspEngine<'_>, start: u64, end: u64) -> maac::dsp::Result<Vec<f64>> {
    let mut out = Vec::new();
    engine.render_span(RenderSpan { start, end }, |frame| {
        out.extend_from_slice(frame);
        Ok(())
    })?;
    Ok(out)
}

#[test]
fn a_restored_engine_is_an_ordinary_engine_after_its_one_render() {
    let plan = example("effects");
    let total = plan.output().total_frames;
    let (plain, checkpoints) = render_checkpointed(&plan, (total / 5) | 1);
    let checkpoint = &checkpoints[1];
    let from = checkpoint.frame() as usize * usize::from(plan.output().channels);
    let mut restored = restored(&plan, checkpoint);
    let tail = tail_of(&mut restored, checkpoint.frame(), total).unwrap();
    assert_eq!(bits(&plain[from..]), bits(&tail));
    // After its one render it is an ordinary engine: a full render is exact.
    let full = tail_of(&mut restored, 0, total).unwrap();
    assert_eq!(bits(&plain), bits(&full));
}

#[test]
fn a_restored_engine_refuses_a_span_that_misses_its_checkpoint_and_keeps_its_state() {
    let plan = example("effects");
    let total = plan.output().total_frames;
    let (plain, checkpoints) = render_checkpointed(&plan, (total / 5) | 1);
    let checkpoint = &checkpoints[1];
    let from = checkpoint.frame() as usize * usize::from(plan.output().channels);
    let mut restored = restored(&plan, checkpoint);
    assert!(tail_of(&mut restored, checkpoint.frame() + 1, total).is_err());
    assert!(tail_of(&mut restored, checkpoint.frame(), total + 1).is_err());
    let tail = tail_of(&mut restored, checkpoint.frame(), total).unwrap();
    assert_eq!(
        bits(&plain[from..]),
        bits(&tail),
        "a rejected span used up the checkpoint"
    );
}

#[test]
fn a_restored_render_that_stops_early_or_a_reset_never_passes_for_a_checkpoint() {
    let plan = example("effects");
    let total = plan.output().total_frames;
    let (_, checkpoints) = render_checkpointed(&plan, (total / 5) | 1);
    let checkpoint = &checkpoints[1];

    let mut stopped = restored(&plan, checkpoint);
    let result = stopped.render_span(
        RenderSpan {
            start: checkpoint.frame(),
            end: total,
        },
        |_| Err(maac::dsp::RenderError::RenderState("stop".into())),
    );
    assert!(result.is_err());
    assert!(
        tail_of(&mut stopped, checkpoint.frame(), total).is_err(),
        "a half-rendered engine rendered a preview without saying so"
    );

    let mut reset = restored(&plan, checkpoint);
    reset.reset();
    // Reset engines render like any other: from frame 0 this is the full render.
    let full = tail_of(&mut reset, 0, total).unwrap();
    assert_eq!(bits(&full), bits(&render_plain(&plan)));
}

#[test]
fn checkpoints_must_be_spaced_apart() {
    let plan = example("hello");
    assert!(engine(&plan)
        .render_with_checkpoints(0, |_| Ok(()))
        .is_err());
    assert!(engine(&plan)
        .render_with_checkpoints(1, |_| Ok(()))
        .is_err());
}
