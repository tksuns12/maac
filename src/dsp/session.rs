//! A long-lived exact render of one plan that follows its edits.
//!
//! [`RenderSession`] owns the checkpoints of the plan it last rendered. An
//! edit ([`RenderSession::update`]) only records the new plan. The next
//! [`RenderSession::render`] builds one engine for it, asks how far the old
//! checkpoints can be trusted ([`Reuse`]), resumes from the last one that can,
//! renders the range asked for, and lays down fresh checkpoints on the way, so
//! the render after the following edit starts close by again. Several edits
//! before a render cost no more than one: the last plan is compared with the
//! plan the checkpoints belong to.
//!
//! The checkpoints in memory are kept to a budget. Dropping any of them never
//! changes a result, only how far a later render starts from.

use super::checkpoint::{CheckpointSet, Reuse, Schedule};
use super::*;
use crate::plan_artifact::PlanArtifact;

/// An upper bound on the heap one checkpoint holds, in bytes. The five example
/// songs measured 1.1 to 5.0 MB (`tests/checkpoint_cost.rs`); the state is
/// spread over every node, so no one part could be measured on its own, and a
/// session counts checkpoints with this figure instead of measuring them.
pub const ESTIMATED_CHECKPOINT_BYTES: u64 = 8 << 20;

/// How a session spaces and bounds its checkpoints.
#[derive(Clone, Copy, Debug)]
pub struct SessionPolicy {
    /// Frames between checkpoints; at least `MIN_CHECKPOINT_SPACING`.
    pub spacing_frames: u64,
    /// The most memory the checkpoints may take, counted with
    /// [`ESTIMATED_CHECKPOINT_BYTES`] each. The default is 1 GiB; a caller
    /// that knows the machine's memory can set a share of it.
    pub budget_bytes: u64,
}

impl SessionPolicy {
    pub const DEFAULT_BUDGET_BYTES: u64 = 1 << 30;

    pub fn new(spacing_frames: u64) -> Self {
        Self {
            spacing_frames,
            budget_bytes: Self::DEFAULT_BUDGET_BYTES,
        }
    }

    pub fn with_budget_bytes(mut self, budget_bytes: u64) -> Self {
        self.budget_bytes = budget_bytes;
        self
    }

    /// At least one, so a tiny budget still lets a render resume.
    fn max_checkpoints(&self) -> usize {
        usize::try_from((self.budget_bytes / ESTIMATED_CHECKPOINT_BYTES).max(1))
            .unwrap_or(usize::MAX)
    }
}

/// What one [`RenderSession::render`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderReport {
    /// What the edits since the last render let the checkpoints keep.
    pub reuse: Reuse,
    /// The frame the engine started from: 0, or a checkpoint's.
    pub started_at: u64,
    /// Checkpoints held after the render.
    pub checkpoints: usize,
}

/// See the module documentation.
pub struct RenderSession {
    /// The plan the checkpoints belong to, and those checkpoints.
    set: CheckpointSet,
    /// The plan to render next, once edited.
    pending: Option<PlanArtifact>,
    policy: SessionPolicy,
}

impl RenderSession {
    /// A session for `plan` with nothing rendered yet.
    pub fn new(plan: &PlanArtifact, limits: &PlanLimits, policy: SessionPolicy) -> Result<Self> {
        if policy.spacing_frames < MIN_CHECKPOINT_SPACING {
            return Err(RenderError::RenderState(format!(
                "checkpoint spacing must be at least {MIN_CHECKPOINT_SPACING} frames"
            )));
        }
        Ok(Self {
            set: CheckpointSet::without_checkpoints(plan, limits)?,
            pending: None,
            policy,
        })
    }

    /// Replace the plan to render. Nothing is built or compared until the next
    /// [`Self::render`].
    pub fn update(&mut self, plan: &PlanArtifact) {
        self.pending = Some(plan.clone());
    }

    /// The plan the next render uses.
    pub fn plan(&self) -> &PlanArtifact {
        self.pending.as_ref().unwrap_or(&self.set.plan)
    }

    /// The frames of the checkpoints held, ascending.
    pub fn checkpoint_frames(&self) -> Vec<u64> {
        self.set.checkpoints.iter().map(Checkpoint::frame).collect()
    }

    /// Render frames `start..end` of the current plan, exactly as a render from
    /// frame 0 would, and give each to `callback`. If it fails, the session is
    /// as it was before the call, edits included.
    pub fn render<F>(&mut self, start: u64, end: u64, mut callback: F) -> Result<RenderReport>
    where
        F: FnMut(&[f64]) -> Result<()>,
    {
        let plan = self.pending.as_ref().unwrap_or(&self.set.plan);
        let total = plan.output().total_frames;
        if start >= end || end > total {
            return Err(RenderError::RenderState(format!(
                "render range {start}..{end} must be nonempty and end within {total} frames"
            )));
        }
        let mut engine = DspEngine::new_artifact_with_limits(plan, &self.set.limits)?;
        let schedule = Schedule::of(&engine);
        let reuse = self.set.reuse_with(&schedule, plan);
        // Checkpoints at or before this frame render the same on both plans.
        let trusted = match reuse {
            Reuse::Unchanged => Some(total),
            Reuse::From(frame) => Some(frame),
            Reuse::Full(_) => None,
        };
        let resume_from = trusted.map(|bound| bound.min(start));
        let kept_through = trusted.unwrap_or(0);
        let checkpoint = resume_from.and_then(|frame| {
            self.set
                .checkpoints
                .iter()
                .rev()
                .find(|checkpoint| checkpoint.frame() <= frame)
        });
        let started_at = match checkpoint {
            Some(checkpoint) => {
                engine.restore_from(checkpoint)?;
                checkpoint.frame()
            }
            None => 0,
        };

        let channels = usize::from(plan.output().channels);
        let mut skip = (start - started_at) as usize * channels;
        let fresh = engine.render_span_with_checkpoints(
            RenderSpan {
                start: started_at,
                end,
            },
            self.policy.spacing_frames,
            |frame| {
                if skip >= frame.len() {
                    skip -= frame.len();
                    return Ok(());
                }
                callback(frame)
            },
        )?;

        // Success: the plan and what it kept become the session's. Old
        // checkpoints stay if they are trusted and the render did not redo
        // their frames; the rest are replaced by the fresh ones.
        let mut checkpoints: Vec<Checkpoint> = self
            .set
            .checkpoints
            .iter()
            .filter(|checkpoint| {
                let frame = checkpoint.frame();
                frame <= kept_through && !(started_at < frame && frame < end)
            })
            .cloned()
            .collect();
        checkpoints.extend(fresh);
        checkpoints.sort_by_key(Checkpoint::frame);
        thin(&mut checkpoints, self.policy.max_checkpoints(), start);

        if let Some(plan) = self.pending.take() {
            self.set.plan = plan;
        }
        self.set.schedule = schedule;
        self.set.checkpoints = checkpoints;
        Ok(RenderReport {
            reuse,
            started_at,
            checkpoints: self.set.checkpoints.len(),
        })
    }
}

/// Drop checkpoints until at most `max` remain. Each drop removes the one whose
/// loss widens the gap around it the least, weighted toward the ones farthest
/// from `focus` (the playhead), so the ends of the song thin out first and the
/// spacing near the playhead stays tight.
fn thin(checkpoints: &mut Vec<Checkpoint>, max: usize, focus: u64) {
    while checkpoints.len() > max {
        let mut best: Option<(f64, usize)> = None;
        for index in 0..checkpoints.len() {
            let before = index.checked_sub(1).map_or(0, |i| checkpoints[i].frame());
            // The last checkpoint's gap runs to the song's end, which is not
            // known here, so it is judged by its distance behind it instead.
            let after = checkpoints.get(index + 1).map_or(
                checkpoints[index].frame() + (checkpoints[index].frame() - before),
                |c| c.frame(),
            );
            let gap = (after - before) as f64;
            let distance = checkpoints[index].frame().abs_diff(focus) as f64;
            let score = gap / (1.0 + distance / (after - before).max(1) as f64);
            if best.is_none_or(|(lowest, _)| score < lowest) {
                best = Some((score, index));
            }
        }
        let Some((_, index)) = best else { return };
        checkpoints.remove(index);
    }
}
