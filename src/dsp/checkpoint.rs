//! Resuming a render after an edit.
//!
//! A [`CheckpointSet`] keeps the checkpoints of one exact render together with
//! what that render's plan scheduled. After an edit it answers one question:
//! *from which frame is the old state untrustworthy?* Only differences in the
//! notes, hits and automation lanes are allowed to keep a checkpoint; any other
//! difference in the plan renders from frame 0 and says which.
//!
//! Frames before the answer render identically on both plans, so the latest
//! checkpoint at or before it holds exactly the state a fresh render of the
//! edited plan would have there.

use super::*;
use crate::plan_artifact::PlanArtifact;

/// What an edit lets a render keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reuse {
    /// Nothing the render reads has changed.
    Unchanged,
    /// Frames before this one render the same on both plans.
    From(u64),
    /// The whole render is redone; the text says what changed.
    Full(&'static str),
}

/// What a render scheduled: every note and hit, and every automated parameter.
struct Schedule {
    events: Vec<EventRuntime>,
    lanes: BTreeMap<(String, String), AutomationLanes>,
    /// Whether each node's instrument reads the whole connected line a note
    /// belongs to when the note starts (a bowed section does).
    reads_lines: Vec<bool>,
    /// From this frame on, a lane holds its tail value.
    score_end: u64,
}

impl Schedule {
    fn of(engine: &DspEngine<'_>) -> Self {
        let mut lanes = BTreeMap::new();
        for node in &engine.nodes {
            for (parameter, binding) in &node.automations {
                lanes.insert((node.id.clone(), parameter.clone()), binding.lane.clone());
            }
        }
        Self {
            events: engine.events.clone(),
            lanes,
            reads_lines: engine
                .nodes
                .iter()
                .map(|node| {
                    node.instrument
                        .as_ref()
                        .is_some_and(|instrument| instrument.plays_lines())
                })
                .collect(),
            score_end: engine.tempo.score_end_frame,
        }
    }
}

/// The checkpoints of one exact render of a plan, and the plan.
pub struct CheckpointSet {
    plan: PlanArtifact,
    limits: PlanLimits,
    schedule: Schedule,
    checkpoints: Vec<Checkpoint>,
}

/// An engine for an edited plan, ready to render from `start_frame`.
pub struct Resumed<'a> {
    pub engine: DspEngine<'a>,
    /// Render `start_frame .. total_frames` with [`DspEngine::render_span`].
    pub start_frame: u64,
    pub reuse: Reuse,
}

impl CheckpointSet {
    /// A set with no checkpoints, to ask what an edit would affect without
    /// rendering the plan first.
    pub fn without_checkpoints(plan: &PlanArtifact, limits: &PlanLimits) -> Result<Self> {
        let engine = DspEngine::new_artifact_with_limits(plan, limits)?;
        Ok(Self {
            plan: plan.clone(),
            limits: *limits,
            schedule: Schedule::of(&engine),
            checkpoints: Vec::new(),
        })
    }

    /// Render `plan` exactly, keeping a checkpoint every `every` frames.
    pub fn render<F>(
        plan: &PlanArtifact,
        limits: &PlanLimits,
        every: u64,
        callback: F,
    ) -> Result<Self>
    where
        F: FnMut(&[f64]) -> Result<()>,
    {
        let mut engine = DspEngine::new_artifact_with_limits(plan, limits)?;
        let checkpoints = engine.render_with_checkpoints(every, callback)?;
        Ok(Self {
            plan: plan.clone(),
            limits: *limits,
            schedule: Schedule::of(&engine),
            checkpoints,
        })
    }

    /// As [`Self::render`], with a checkpoint just before each of `frames`
    /// instead of at a fixed spacing.
    pub fn render_at<F>(
        plan: &PlanArtifact,
        limits: &PlanLimits,
        frames: &[u64],
        callback: F,
    ) -> Result<Self>
    where
        F: FnMut(&[f64]) -> Result<()>,
    {
        let mut engine = DspEngine::new_artifact_with_limits(plan, limits)?;
        let checkpoints = engine.render_with_checkpoints_at(frames, callback)?;
        Ok(Self {
            plan: plan.clone(),
            limits: *limits,
            schedule: Schedule::of(&engine),
            checkpoints,
        })
    }

    /// The checkpoints, ascending by frame.
    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// What an edit to `new_plan` lets this render keep.
    pub fn reuse_for(&self, new_plan: &PlanArtifact) -> Result<Reuse> {
        let engine = DspEngine::new_artifact_with_limits(new_plan, &self.limits)?;
        Ok(self.reuse(&engine, new_plan))
    }

    /// Build the engine for `new_plan` and, when the edit allows, restore the
    /// latest checkpoint before it.
    pub fn resume<'a>(&self, new_plan: &'a PlanArtifact) -> Result<Resumed<'a>> {
        self.resume_with(new_plan, None)
    }

    /// As [`Self::resume`], but trusting `affected_from` instead of working it
    /// out. A wrong value renders wrong frames; this exists so tests can show
    /// that a bound that is too late is caught.
    #[doc(hidden)]
    pub fn resume_with<'a>(
        &self,
        new_plan: &'a PlanArtifact,
        affected_from: Option<u64>,
    ) -> Result<Resumed<'a>> {
        let mut engine = DspEngine::new_artifact_with_limits(new_plan, &self.limits)?;
        let reuse = self.reuse(&engine, new_plan);
        let total = engine.plan.output.total_frames;
        let from = affected_from.or(match reuse {
            Reuse::From(frame) => Some(frame),
            Reuse::Unchanged => Some(total),
            Reuse::Full(_) => None,
        });
        let checkpoint = from.and_then(|frame| {
            self.checkpoints
                .iter()
                .rev()
                .find(|checkpoint| checkpoint.frame <= frame)
        });
        let start_frame = match checkpoint {
            Some(checkpoint) => {
                engine.restore_from(checkpoint)?;
                checkpoint.frame
            }
            None => 0,
        };
        Ok(Resumed {
            engine,
            start_frame,
            reuse,
        })
    }

    fn reuse(&self, new: &DspEngine<'_>, new_plan: &PlanArtifact) -> Reuse {
        if let Some(part) = other_difference(&self.plan.view(), &new_plan.view()) {
            return Reuse::Full(part);
        }
        let earliest = std::cell::Cell::new(None::<u64>);
        let mut note =
            |frame: u64| earliest.set(Some(earliest.get().map_or(frame, |e| e.min(frame))));
        let reads_lines = &self.schedule.reads_lines;
        events_differ(&self.schedule.events, &new.events, reads_lines, &mut note);
        lanes_differ(
            &self.schedule.lanes,
            &new_lanes(new),
            self.schedule.score_end,
            &mut note,
        );
        earliest.get().map_or(Reuse::Unchanged, Reuse::From)
    }
}

fn new_lanes(engine: &DspEngine<'_>) -> BTreeMap<(String, String), AutomationLanes> {
    Schedule::of(engine).lanes
}

/// The first part of the plan, other than its notes, hits and automation, that
/// differs. Source positions and regions are not read by a render.
fn other_difference(old: &PlanView<'_>, new: &PlanView<'_>) -> Option<&'static str> {
    if old.version != new.version {
        Some("plan version")
    } else if old.output != new.output {
        Some("output settings")
    } else if old.tempo != new.tempo {
        Some("tempo map")
    } else if !old.nodes.same_as(&new.nodes) {
        Some("nodes")
    } else if old.audio_assets != new.audio_assets {
        Some("audio assets")
    } else if old.connections != new.connections {
        Some("connections")
    } else if old.modulations != new.modulations {
        Some("modulations")
    } else if !same_instruments(old.instruments, new.instruments) {
        Some("instruments")
    } else if !same_production(old.production, new.production) {
        Some("production settings")
    } else {
        None
    }
}

/// Whether two lists of programs are equal once `strip` has removed what
/// says where in the source each came from: a byte span moves with every edit
/// of the text before it.
fn same_ignoring_spans<T: Clone + PartialEq>(old: &[T], new: &[T], strip: impl Fn(&mut T)) -> bool {
    old.len() == new.len()
        && old.iter().zip(new).all(|(a, b)| {
            let (mut a, mut b) = (a.clone(), b.clone());
            strip(&mut a);
            strip(&mut b);
            a == b
        })
}

/// Whether two instrument resources render alike. Which source files they were
/// compiled from, and their hashes, are provenance: they change with every edit
/// of the text and are not read by a render.
fn same_instruments(
    old: Option<&crate::instrument_plan::InstrumentResources>,
    new: Option<&crate::instrument_plan::InstrumentResources>,
) -> bool {
    match (old, new) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            same_ignoring_spans(&a.programs, &b.programs, |program| {
                program.source.span = None;
            }) && a.wavetables == b.wavetables
                && a.samples == b.samples
                && same_ignoring_spans(&a.kits, &b.kits, |kit| kit.source.span = None)
                && same_ignoring_spans(&a.effects, &b.effects, |effect| {
                    effect.source.span = None;
                })
        }
        _ => false,
    }
}

/// Whether two production settings are alike apart from the execution
/// identity, which hashes the source text and so changes with every edit.
fn same_production(
    old: Option<&crate::production_data::ProductionSettings>,
    new: Option<&crate::production_data::ProductionSettings>,
) -> bool {
    match (old, new) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.schema_hash == b.schema_hash
                && a.extension_id == b.extension_id
                && a.deliveries == b.deliveries
        }
        _ => false,
    }
}

/// Each note's line head: the note that starts the connected line it is in,
/// or itself.
fn line_heads(events: &[EventRuntime]) -> Vec<usize> {
    let by_address: HashMap<(usize, &str), usize> = events
        .iter()
        .enumerate()
        .map(|(index, event)| ((event.node, event.address.as_str()), index))
        .collect();
    events
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let mut head = index;
            // A line is at most as long as the event list.
            for _ in 0..events.len() {
                let Some(join) = &events[head].join else {
                    break;
                };
                match by_address.get(&(events[head].node, join.from.as_str())) {
                    Some(&before) => head = before,
                    None => break,
                }
            }
            head
        })
        .collect()
}

/// Report, through `note`, the on frame of every note or hit that differs
/// between the two schedules. On an instrument that plays lines (a bowed
/// section) also the on frame of the head of its connected line, whose first
/// note reads the whole line when it starts. On any other instrument a joined
/// note's effect on the note before it (which gives up its release) happens at
/// the joined note's own on frame.
fn events_differ(
    old: &[EventRuntime],
    new: &[EventRuntime],
    reads_lines: &[bool],
    note: &mut impl FnMut(u64),
) {
    let old_heads = line_heads(old);
    let new_heads = line_heads(new);
    let key = |event: &EventRuntime| (event.node, event.address.clone());
    let old_by: HashMap<_, _> = old
        .iter()
        .enumerate()
        .map(|(index, event)| (key(event), index))
        .collect();
    let new_by: HashMap<_, _> = new
        .iter()
        .enumerate()
        .map(|(index, event)| (key(event), index))
        .collect();
    for (index, event) in old.iter().enumerate() {
        let counterpart = new_by.get(&key(event)).copied();
        if counterpart.is_some_and(|other| new[other] == *event) {
            continue;
        }
        note(event.on_frame);
        if reads_lines[event.node] {
            note(old[old_heads[index]].on_frame);
        }
        if let Some(other) = counterpart {
            note(new[other].on_frame);
            if reads_lines[new[other].node] {
                note(new[new_heads[other]].on_frame);
            }
        }
    }
    for (index, event) in new.iter().enumerate() {
        if !old_by.contains_key(&key(event)) {
            note(event.on_frame);
            if reads_lines[event.node] {
                note(new[new_heads[index]].on_frame);
            }
        }
    }
}

/// Report the first frame at which the two sets of automation lanes may give
/// different values, for every parameter where they differ.
fn lanes_differ(
    old: &BTreeMap<(String, String), AutomationLanes>,
    new: &BTreeMap<(String, String), AutomationLanes>,
    score_end: u64,
    note: &mut impl FnMut(u64),
) {
    for key in old.keys().chain(new.keys()) {
        let (old_lanes, new_lanes) = (old.get(key), new.get(key));
        match (old_lanes, new_lanes) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => lane_lists_differ(&a.lanes, &b.lanes, score_end, note),
            (Some(only), None) | (None, Some(only)) => note(start_of(&only.lanes[0])),
            (None, None) => {}
        }
    }
}

fn start_of(lane: &AutomationRuntime) -> u64 {
    lane.knot_frames.first().copied().unwrap_or(0).max(0) as u64
}

fn lane_lists_differ(
    old: &[AutomationRuntime],
    new: &[AutomationRuntime],
    score_end: u64,
    note: &mut impl FnMut(u64),
) {
    let shared = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if old.len() != new.len() {
        // A lane added or removed: the value changes where it starts.
        for lane in old[shared..].iter().chain(&new[shared..]) {
            note(start_of(lane));
        }
        return;
    }
    for (a, b) in old[shared..].iter().zip(&new[shared..]) {
        if a != b {
            note(lane_differs_from(a, b, score_end));
        }
    }
}

/// Where two lanes in the same slot first may differ. When only knots differ,
/// the segment before the first differing knot interpolates toward it, so the
/// value may differ from the knot before.
fn lane_differs_from(a: &AutomationRuntime, b: &AutomationRuntime, score_end: u64) -> u64 {
    let same_curve =
        a.clock == b.clock && a.at == b.at && a.at_exact == b.at_exact && a.base == b.base;
    let start = start_of(a).min(start_of(b));
    if !same_curve {
        return start;
    }
    // The value held from the score's end on is the lane's tail; it can differ
    // only from there.
    let tail_differs = a.tail_value != b.tail_value || a.starts_by_end != b.starts_by_end;
    let same_knot = |index: usize| {
        a.points.get(index) == b.points.get(index)
            && a.knot_frames.get(index) == b.knot_frames.get(index)
            && a.point_positions_exact.get(index) == b.point_positions_exact.get(index)
    };
    let longest = a
        .points
        .len()
        .max(b.points.len())
        .max(a.knot_frames.len())
        .max(b.knot_frames.len());
    let knots = match (0..longest).find(|&index| !same_knot(index)) {
        Some(0) => Some(start),
        Some(first) => Some(
            a.knot_frames
                .get(first - 1)
                .map_or(start, |frame| (*frame).max(0) as u64),
        ),
        None => None,
    };
    match (knots, tail_differs) {
        (Some(frame), true) => frame.min(score_end),
        (Some(frame), false) => frame,
        (None, true) => score_end,
        // Unequal lanes that no knot or tail explains: be safe.
        (None, false) => start,
    }
}
