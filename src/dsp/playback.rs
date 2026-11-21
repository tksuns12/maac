//! Playing a [`BackgroundSession`](super::BackgroundSession): audio that keeps
//! coming while the plan is edited.
//!
//! [`Playback`] runs one render thread that stays a little ahead of whoever
//! reads. The reader (an audio callback, say) calls [`Playback::read`], which
//! copies what is ready and never renders. Nothing the reader does waits on a
//! render; it waits only for the producer to put a chunk in or take one out,
//! which is a few microseconds under a lock.
//!
//! Every frame handed out is a frame of an exact render of one plan: the
//! frames before a switch are those of the old plan, the frames after it those
//! of the new one, each bit for bit what a fresh render of that plan gives.
//!
//! * An edit that leaves the frames already rendered alone takes effect at the
//!   next chunk: the engine's state at that point is a checkpoint of the new
//!   plan too, so the new plan carries on from it.
//! * An edit that changes frames already rendered needs the new plan's state
//!   at the playhead. The session's nearest checkpoint (or frame 0) is
//!   rendered forward silently, in the time the producer has to spare, while
//!   the old plan keeps playing. [`PlaybackStatus::stale`] says it is going
//!   on. When the new plan has caught up the sound switches to it.
//! * A seek throws away what was ready and starts again from the session's
//!   nearest checkpoint; nothing plays until that has caught up.
//!
//! The switches are listed in [`PlaybackStatus::switches`] with the frame
//! where each took effect.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::background::{locked, publish, Foreground, Shared};
use super::checkpoint::{CheckpointSet, Reuse, Schedule};
use super::*;
use crate::plan_artifact::PlanArtifact;

/// How long the producer sleeps when the reader has left it nothing to do.
const POLL: Duration = Duration::from_millis(5);
/// Frames the producer renders silently in one step while catching up.
const CATCH_UP_STEP: u64 = 2_048;
/// Frames between the snapshots the engine keeps of its own state behind the
/// playhead, for an edit that changes what was just heard.
const HISTORY_EVERY: u64 = 24_000;
/// How many such snapshots are kept.
const HISTORY_KEPT: usize = 8;
/// The most switches [`PlaybackStatus::switches`] remembers.
const MAX_SWITCHES: usize = 256;

/// How a [`Playback`] renders ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackConfig {
    /// Frames the producer keeps ready ahead of the reader. More makes
    /// dropouts less likely when a render is slow, and an edit is heard that
    /// much later when it changes frames already rendered.
    pub lead_frames: u64,
    /// Frames the producer renders at a time.
    pub chunk_frames: u64,
    /// Hold the session's background pre-render still for as long as the
    /// playback lives, to leave the processor to the producer.
    pub pause_prerender: bool,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            lead_frames: 65_536,
            chunk_frames: 2_048,
            pause_prerender: false,
        }
    }
}

/// Where the sound changed to another plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackSwitch {
    /// The first frame that is the new plan's.
    pub frame: u64,
    /// The plan: how many times the session was updated, as
    /// [`SessionStatus::generation`](super::SessionStatus) counts them, once
    /// the plan was in place.
    pub generation: u64,
}

/// What a [`Playback`] is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackStatus {
    /// The frame the next [`Playback::read`] starts at.
    pub position: u64,
    /// Frames ready to be read.
    pub buffered_frames: u64,
    /// Reads that came up short before the end of the plan.
    pub underruns: u64,
    /// Everything has been read.
    pub ended: bool,
    /// An edit changed frames already rendered and the producer is still
    /// rendering the new plan up to the playhead: the sound is the old plan's.
    pub stale: bool,
    /// The plan each stretch of sound belongs to: the first entry is the plan
    /// at the start (or at the last seek), and each later one a change.
    pub switches: Vec<PlaybackSwitch>,
    /// The frame the last catch-up rendered forward from.
    pub catch_up_from: Option<u64>,
    /// Why the producer stopped, if it did.
    pub last_error: Option<String>,
}

struct Chunk {
    samples: Vec<f64>,
    /// Samples already read.
    taken: usize,
}

struct Ring {
    chunks: VecDeque<Chunk>,
    /// The frame the next read starts at.
    read_frame: u64,
    buffered: u64,
    /// Counts the seeks. A chunk rendered before one is dropped.
    epoch: u64,
    seek_to: Option<u64>,
    /// The producer has put the last frame of the plan in.
    finished: bool,
    underruns: u64,
    stale: bool,
    catch_up_from: Option<u64>,
    switches: Vec<PlaybackSwitch>,
    error: Option<String>,
}

struct Play {
    ring: Mutex<Ring>,
    /// The reader took something out, or something else changed.
    room: Condvar,
    shutdown: AtomicBool,
    channels: usize,
}

/// See the module documentation.
pub struct Playback {
    play: Arc<Play>,
    thread: Option<JoinHandle<()>>,
}

impl Playback {
    pub(super) fn start(shared: &Arc<Shared>, start: u64, config: PlaybackConfig) -> Result<Self> {
        if config.chunk_frames < 64 || config.lead_frames < config.chunk_frames {
            return Err(RenderError::RenderState(
                "a playback needs chunks of at least 64 frames and a lead of at least one chunk"
                    .into(),
            ));
        }
        let synced = sync(shared, start)?;
        let output = synced.plan.output();
        let total = output.total_frames;
        if start >= total {
            return Err(RenderError::RenderState(format!(
                "playback cannot start at frame {start} of {total}"
            )));
        }
        let play = Arc::new(Play {
            ring: Mutex::new(Ring {
                chunks: VecDeque::new(),
                read_frame: start,
                buffered: 0,
                epoch: 0,
                seek_to: None,
                finished: false,
                underruns: 0,
                stale: false,
                catch_up_from: None,
                switches: vec![PlaybackSwitch {
                    frame: start,
                    generation: synced.generation,
                }],
                error: None,
            }),
            room: Condvar::new(),
            shutdown: AtomicBool::new(false),
            channels: usize::from(output.channels),
        });
        let first = Setup::new(synced, start);
        let mut ctx = Ctx {
            shared: Arc::clone(shared),
            play: Arc::clone(&play),
            lead: config.lead_frames,
            chunk: config.chunk_frames,
            epoch: 0,
        };
        let pause = config.pause_prerender;
        let thread = std::thread::Builder::new()
            .name("maac-playback".into())
            .spawn(move || {
                let shared = Arc::clone(&ctx.shared);
                let _hold = pause.then(|| Foreground::new(&shared));
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut ctx, first)));
                let message = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(format!("{error:?}")),
                    Err(panic) => Some(format!(
                        "the playback thread panicked: {}",
                        panic
                            .downcast_ref::<&str>()
                            .map(|text| (*text).to_string())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown".into())
                    )),
                };
                if let Some(message) = message {
                    locked(&ctx.play.ring).error = Some(message);
                }
            })
            .map_err(|error| RenderError::RenderState(format!("no playback thread: {error}")))?;
        Ok(Self {
            play,
            thread: Some(thread),
        })
    }

    /// Interleaved samples per frame.
    pub fn channels(&self) -> usize {
        self.play.channels
    }

    /// Copy the next frames into `out` (a whole number of frames; a trailing
    /// part of one is left alone) and return how many were ready. The rest of
    /// `out` is set to silence. Never renders.
    pub fn read(&self, out: &mut [f64]) -> usize {
        let channels = self.play.channels;
        let want = out.len() / channels;
        let mut ring = locked(&self.play.ring);
        let mut got = 0usize;
        while got < want {
            let (done, take) = {
                let Some(chunk) = ring.chunks.front_mut() else {
                    break;
                };
                let available = (chunk.samples.len() - chunk.taken) / channels;
                let take = available.min(want - got);
                let from = chunk.taken;
                out[got * channels..(got + take) * channels]
                    .copy_from_slice(&chunk.samples[from..from + take * channels]);
                chunk.taken += take * channels;
                (chunk.taken == chunk.samples.len(), take)
            };
            if done {
                ring.chunks.pop_front();
            }
            got += take;
        }
        ring.read_frame += got as u64;
        ring.buffered -= got as u64;
        if got < want && !ring.finished && ring.error.is_none() {
            ring.underruns += 1;
        }
        drop(ring);
        out[got * channels..].fill(0.0);
        self.play.room.notify_all();
        got
    }

    /// Move the playhead. What was ready is dropped, and reads return nothing
    /// until the producer has rendered from the new place. A frame at or past
    /// the end of the plan ends the playback.
    pub fn seek(&self, frame: u64) {
        let mut ring = locked(&self.play.ring);
        ring.chunks.clear();
        ring.read_frame = frame;
        ring.buffered = 0;
        ring.epoch += 1;
        ring.seek_to = Some(frame);
        ring.finished = false;
        ring.stale = false;
        ring.error = None;
        drop(ring);
        self.play.room.notify_all();
    }

    pub fn status(&self) -> PlaybackStatus {
        let ring = locked(&self.play.ring);
        PlaybackStatus {
            position: ring.read_frame,
            buffered_frames: ring.buffered,
            underruns: ring.underruns,
            ended: ring.finished && ring.buffered == 0,
            stale: ring.stale,
            switches: ring.switches.clone(),
            catch_up_from: ring.catch_up_from,
            last_error: ring.error.clone(),
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.play.shutdown.store(true, Ordering::Release);
        self.play.room.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---- the producer --------------------------------------------------------

struct Ctx {
    shared: Arc<Shared>,
    play: Arc<Play>,
    lead: u64,
    chunk: u64,
    /// The seek count the producer has taken up.
    epoch: u64,
}

/// A plan, and what the producer starts it from.
struct Synced {
    plan: PlanArtifact,
    limits: PlanLimits,
    /// The latest checkpoint at or before the frame asked for that is the
    /// plan's own, or `None` to start at frame 0.
    checkpoint: Option<Checkpoint>,
    generation: u64,
}

struct Setup {
    plan: PlanArtifact,
    limits: PlanLimits,
    restore: Option<Checkpoint>,
    pos: u64,
    generation: u64,
}

impl Setup {
    fn new(synced: Synced, pos: u64) -> Self {
        Self {
            plan: synced.plan,
            limits: synced.limits,
            restore: synced.checkpoint,
            pos,
            generation: synced.generation,
        }
    }
}

enum Next {
    Exit,
    Again(Box<Setup>),
}

enum Wake {
    /// One wait ran out with nothing to do (only when asked for).
    Idle,
    Room,
    Exit,
    Seek,
    Edit,
}

/// The latest plan, taken up into the session, and the checkpoint to start it
/// from for `frame`.
fn sync(shared: &Arc<Shared>, frame: u64) -> Result<Synced> {
    // Read before the slot is: an update counted here has its plan in the slot
    // or already in the session.
    let generation = shared.stored.load(Ordering::Acquire);
    let _foreground = Foreground::new(shared);
    let mut state = locked(&shared.state);
    if let Some(plan) = locked(&shared.slot).take() {
        state.session.update(&plan);
    }
    let (plan, limits, checkpoint) = state.session.checkpoint_before(frame)?;
    publish(shared, &state.session);
    Ok(Synced {
        plan,
        limits,
        checkpoint,
        generation,
    })
}

fn run(ctx: &mut Ctx, first: Setup) -> Result<()> {
    let mut setup = first;
    loop {
        let total = setup.plan.output().total_frames;
        if setup.pos >= total {
            // An edit made the plan shorter than the playhead: nothing is left.
            locked(&ctx.play.ring).finished = true;
            match idle_until_seek(ctx) {
                Next::Exit => return Ok(()),
                Next::Again(next) => {
                    setup = *next;
                    continue;
                }
            }
        }
        let mut engine = DspEngine::new_artifact_with_limits(&setup.plan, &setup.limits)?;
        let from = match setup.restore.take() {
            Some(checkpoint) => {
                engine.restore_from(&checkpoint)?;
                checkpoint.frame()
            }
            None => 0,
        };
        match play_engine(ctx, &setup, &mut engine, from)? {
            Next::Exit => return Ok(()),
            Next::Again(next) => setup = *next,
        }
    }
}

fn idle_until_seek(ctx: &mut Ctx) -> Next {
    loop {
        match wait_room(ctx, 0, false, false) {
            Wake::Exit => return Next::Exit,
            Wake::Seek => match take_seek(ctx) {
                Some(next) => return next,
                None => continue,
            },
            Wake::Room | Wake::Edit | Wake::Idle => {
                // Nothing to render: wait for the reader or a seek.
                let ring = locked(&ctx.play.ring);
                let _ = ctx
                    .play
                    .room
                    .wait_timeout(ring, POLL)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }
}

/// Wait until there is room for a chunk, or something else needs the
/// producer's attention.
fn wait_room(ctx: &Ctx, generation: u64, watch_edits: bool, once: bool) -> Wake {
    let mut ring = locked(&ctx.play.ring);
    loop {
        if ctx.play.shutdown.load(Ordering::Acquire) {
            return Wake::Exit;
        }
        if ring.epoch != ctx.epoch {
            return Wake::Seek;
        }
        if watch_edits && ctx.shared.stored.load(Ordering::Acquire) != generation {
            return Wake::Edit;
        }
        if !ring.finished && ring.buffered < ctx.lead {
            return Wake::Room;
        }
        let (next, timeout) = ctx
            .play
            .room
            .wait_timeout(ring, POLL)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ring = next;
        if once && timeout.timed_out() {
            return Wake::Idle;
        }
    }
}

/// Take up a seek: the plan as it is now, from the checkpoint nearest the
/// frame. `None` if the seek was undone by the time it was looked at.
fn take_seek(ctx: &mut Ctx) -> Option<Next> {
    let frame = {
        let mut ring = locked(&ctx.play.ring);
        ctx.epoch = ring.epoch;
        ring.seek_to.take()?
    };
    let synced = match sync(&ctx.shared, frame) {
        Ok(synced) => synced,
        Err(error) => {
            locked(&ctx.play.ring).error = Some(format!("{error:?}"));
            return Some(Next::Exit);
        }
    };
    {
        let mut ring = locked(&ctx.play.ring);
        // Only the newest seek counts.
        if ring.epoch != ctx.epoch {
            return None;
        }
        ring.switches.clear();
        ring.switches.push(PlaybackSwitch {
            frame,
            generation: synced.generation,
        });
        ring.stale = false;
    }
    Some(Next::Again(Box::new(Setup::new(synced, frame))))
}

/// Put a rendered chunk in the ring. False if a seek came in meanwhile and
/// the chunk was dropped.
fn push(ctx: &Ctx, samples: Vec<f64>, end: u64, total: u64) -> bool {
    let mut ring = locked(&ctx.play.ring);
    if ring.epoch != ctx.epoch {
        return false;
    }
    ring.buffered += (samples.len() / ctx.play.channels) as u64;
    ring.chunks.push_back(Chunk { samples, taken: 0 });
    if end >= total {
        ring.finished = true;
    }
    true
}

fn record_switch(ctx: &Ctx, frame: u64, generation: u64) {
    let mut ring = locked(&ctx.play.ring);
    if ring.switches.len() >= MAX_SWITCHES {
        ring.switches.remove(0);
    }
    ring.switches.push(PlaybackSwitch { frame, generation });
    ring.stale = false;
}

/// Render one chunk of `engine` from `pos` into the ring. Returns the new
/// position, or `None` if a seek came in and the chunk was dropped.
fn produce(ctx: &Ctx, engine: &mut DspEngine<'_>, pos: u64, total: u64) -> Result<Option<u64>> {
    let end = (pos + ctx.chunk).min(total);
    let mut samples = Vec::with_capacity((end - pos) as usize * ctx.play.channels);
    engine.render_chunk(RenderSpan { start: pos, end }, |frame| {
        samples.extend_from_slice(frame);
        Ok(())
    })?;
    Ok(push(ctx, samples, end, total).then_some(end))
}

/// Play `engine` (built for `setup.plan`, at frame `from`) from `setup.pos`
/// until something needs another engine.
fn play_engine(
    ctx: &mut Ctx,
    setup: &Setup,
    engine: &mut DspEngine<'_>,
    from: u64,
) -> Result<Next> {
    let total = setup.plan.output().total_frames;
    let mut pos = setup.pos;
    let mut generation = setup.generation;

    // Up to the playhead silently, when it was not a checkpoint's frame.
    let mut at = from;
    while at < pos {
        if ctx.play.shutdown.load(Ordering::Acquire) {
            return Ok(Next::Exit);
        }
        if locked(&ctx.play.ring).epoch != ctx.epoch {
            if let Some(next) = take_seek(ctx) {
                return Ok(next);
            }
            continue;
        }
        if ctx.shared.stored.load(Ordering::Acquire) != generation {
            // Start again from the newest plan.
            let synced = sync(&ctx.shared, pos)?;
            return Ok(Next::Again(Box::new(Setup::new(synced, pos))));
        }
        let end = (at + CATCH_UP_STEP).min(pos);
        engine.render_chunk(RenderSpan { start: at, end }, |_| Ok(()))?;
        at = end;
    }

    let mut history: VecDeque<Checkpoint> = VecDeque::new();
    let mut next_snapshot = (pos / HISTORY_EVERY + 1) * HISTORY_EVERY;
    let own = CheckpointSet {
        plan: setup.plan.clone(),
        limits: setup.limits,
        schedule: Schedule::of(engine),
        checkpoints: Vec::new(),
    };
    loop {
        if pos >= total {
            // Everything is rendered; wait for the reader, or a seek.
            return Ok(idle_until_seek(ctx));
        }
        match wait_room(ctx, generation, true, false) {
            Wake::Exit => return Ok(Next::Exit),
            Wake::Seek => {
                if let Some(next) = take_seek(ctx) {
                    return Ok(next);
                }
            }
            Wake::Room | Wake::Idle => match produce(ctx, engine, pos, total)? {
                Some(end) => {
                    pos = end;
                    if pos >= next_snapshot && pos < total {
                        history.push_back(engine.chunk_checkpoint()?);
                        if history.len() > HISTORY_KEPT {
                            history.pop_front();
                        }
                        next_snapshot = (pos / HISTORY_EVERY + 1) * HISTORY_EVERY;
                    }
                }
                None => continue,
            },
            Wake::Edit => {
                let synced = sync(&ctx.shared, pos)?;
                let candidate = DspEngine::new_artifact_with_limits(&synced.plan, &synced.limits)?;
                if synced.plan.output().total_frames <= pos {
                    // The plan is now shorter than the playhead.
                    return Ok(Next::Again(Box::new(Setup {
                        plan: synced.plan,
                        limits: synced.limits,
                        restore: None,
                        pos,
                        generation: synced.generation,
                    })));
                }
                let reuse = own.reuse_with(&Schedule::of(&candidate), &synced.plan);
                match reuse {
                    Reuse::Unchanged => generation = synced.generation,
                    Reuse::From(frame) if frame >= pos => {
                        // The frames rendered so far are the new plan's too.
                        let restore = (pos > 0).then(|| engine.chunk_checkpoint()).transpose()?;
                        record_switch(ctx, pos, synced.generation);
                        return Ok(Next::Again(Box::new(Setup {
                            plan: synced.plan,
                            limits: synced.limits,
                            restore,
                            pos,
                            generation: synced.generation,
                        })));
                    }
                    _ => {
                        // Start from the nearest state that is the new plan's
                        // too: the session's checkpoint, or a snapshot this
                        // playback took of its own engine, whichever is later.
                        let bound = match reuse {
                            Reuse::From(frame) => Some(frame),
                            _ => None,
                        };
                        let own_snapshot = bound.and_then(|bound| {
                            history.iter().rev().find(|c| c.frame() <= bound).cloned()
                        });
                        let start = match (synced.checkpoint.clone(), own_snapshot) {
                            (Some(a), Some(b)) => Some(if b.frame() > a.frame() { b } else { a }),
                            (a, b) => a.or(b),
                        };
                        {
                            let mut ring = locked(&ctx.play.ring);
                            ring.stale = true;
                            ring.catch_up_from = Some(start.as_ref().map_or(0, Checkpoint::frame));
                        }
                        let new_total = synced.plan.output().total_frames;
                        let target_plan = NewPlan {
                            engine: candidate,
                            total: new_total,
                            start,
                            generation: synced.generation,
                        };
                        match catch_up(ctx, engine, &mut pos, total, target_plan)? {
                            Caught::Swap(restore) => {
                                record_switch(ctx, pos, synced.generation);
                                return Ok(Next::Again(Box::new(Setup {
                                    plan: synced.plan,
                                    limits: synced.limits,
                                    restore,
                                    pos,
                                    generation: synced.generation,
                                })));
                            }
                            Caught::Exit => return Ok(Next::Exit),
                            Caught::Seek(next) => return Ok(next),
                            // Another edit: looked at again from the top.
                            Caught::Superseded => locked(&ctx.play.ring).stale = false,
                            Caught::Finished => {
                                locked(&ctx.play.ring).stale = false;
                                generation = synced.generation;
                            }
                        }
                    }
                }
            }
        }
    }
}

enum Caught {
    /// The new plan is at the playhead; this is its state there.
    Swap(Option<Checkpoint>),
    Exit,
    Seek(Next),
    Superseded,
    /// The old plan reached the end of the song first.
    Finished,
}

/// The edited plan a catch-up renders toward the playhead.
struct NewPlan<'a> {
    engine: DspEngine<'a>,
    total: u64,
    /// The state to start from; `None` for frame 0.
    start: Option<Checkpoint>,
    generation: u64,
}

enum Chase {
    Ready(Option<Checkpoint>),
    Aborted,
}

/// On its own thread: render the new plan forward toward `target` (the
/// producer's position, which moves) until told to hold, and hand over its
/// state at the target.
fn chase(
    mut engine: DspEngine<'_>,
    mut at: u64,
    target: &AtomicU64,
    reached: &AtomicU64,
    hold: &AtomicBool,
    abort: &AtomicBool,
) -> Result<Chase> {
    loop {
        if abort.load(Ordering::Acquire) {
            return Ok(Chase::Aborted);
        }
        let to = target.load(Ordering::Acquire);
        if at < to {
            let end = (at + CATCH_UP_STEP).min(to);
            engine.render_chunk(RenderSpan { start: at, end }, |_| Ok(()))?;
            at = end;
            reached.store(at, Ordering::Release);
        } else if hold.load(Ordering::Acquire) {
            // The producer sets `hold` after its last move of `target`.
            if target.load(Ordering::Acquire) == at {
                let state = (at > 0).then(|| engine.chunk_checkpoint()).transpose()?;
                return Ok(Chase::Ready(state));
            }
        } else {
            std::thread::sleep(Duration::from_micros(300));
        }
    }
}

/// Render the new plan up to the playhead on a second thread while this one
/// goes on filling the ring with the old plan. When the new plan is within a
/// few chunks of the playhead and the ring can spare the time, this thread
/// stops producing, the other finishes the last of the distance, and the
/// caller switches.
fn catch_up(
    ctx: &mut Ctx,
    old: &mut DspEngine<'_>,
    pos: &mut u64,
    total: u64,
    target_plan: NewPlan<'_>,
) -> Result<Caught> {
    let NewPlan {
        engine: mut candidate,
        total: new_total,
        start,
        generation,
    } = target_plan;
    let at = match &start {
        Some(checkpoint) => {
            candidate.restore_from(checkpoint)?;
            checkpoint.frame()
        }
        None => 0,
    };
    let target = AtomicU64::new(*pos);
    let reached = AtomicU64::new(at);
    let hold = AtomicBool::new(false);
    let abort = AtomicBool::new(false);
    let close = 4 * ctx.chunk;

    enum Done {
        Caught(Caught),
        Hold,
    }
    let (done, joined) = std::thread::scope(|scope| {
        let handle = scope.spawn(|| chase(candidate, at, &target, &reached, &hold, &abort));
        let done = (|| -> Result<Done> {
            loop {
                if *pos >= total {
                    return Ok(Done::Caught(Caught::Finished));
                }
                if *pos >= new_total {
                    // The new plan ends before the playhead: it takes over there.
                    return Ok(Done::Caught(Caught::Swap(None)));
                }
                let wake = wait_room(ctx, generation, true, true);
                match wake {
                    Wake::Exit => return Ok(Done::Caught(Caught::Exit)),
                    Wake::Seek => {
                        if let Some(next) = take_seek(ctx) {
                            return Ok(Done::Caught(Caught::Seek(next)));
                        }
                        continue;
                    }
                    Wake::Edit => return Ok(Done::Caught(Caught::Superseded)),
                    Wake::Room | Wake::Idle => {}
                }
                let behind = *pos - reached.load(Ordering::Acquire);
                let spare = locked(&ctx.play.ring).buffered >= 2 * ctx.chunk;
                if behind <= close && spare {
                    hold.store(true, Ordering::Release);
                    return Ok(Done::Hold);
                }
                if matches!(wake, Wake::Room) {
                    if let Some(end) = produce(ctx, old, *pos, total)? {
                        *pos = end;
                        target.store(end, Ordering::Release);
                    }
                }
            }
        })();
        if !matches!(done, Ok(Done::Hold)) {
            abort.store(true, Ordering::Release);
        }
        (done, handle.join())
    });
    let chased =
        joined.map_err(|_| RenderError::RenderState("the catch-up thread panicked".into()))?;
    match done? {
        Done::Caught(caught) => {
            chased?;
            Ok(caught)
        }
        Done::Hold => match chased? {
            Chase::Ready(state) => Ok(Caught::Swap(state)),
            Chase::Aborted => Ok(Caught::Superseded),
        },
    }
}
