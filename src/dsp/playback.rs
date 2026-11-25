//! Playing a [`BackgroundSession`](super::BackgroundSession): audio that keeps
//! coming while the plan is edited.
//!
//! [`Playback`] runs one render thread that stays a little ahead of whoever
//! reads. The reader (an audio callback, say) calls [`Playback::read`], which
//! copies what is ready and never renders or frees memory (the buffers of
//! chunks it has finished go back to the producer). Nothing the reader does waits on a
//! render; it waits only for the producer to put a chunk in or take one out,
//! which is a few microseconds under a lock.
//!
//! Every frame handed out is a frame of a render of one plan: the frames before
//! a switch are those of the old plan, the frames after it those of the new one.
//! Each is bit for bit what a fresh render of that plan gives, except in a
//! stretch that [`PlaybackSwitch::approximate_from`] marks: there the plan was
//! rendered cold from that frame (see [`PlaybackConfig::preview_preroll_frames`]
//! and [`PlaybackConfig::approximate_start`]), which is what a fresh preview
//! from that frame gives, until the exact render has caught up.
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
//!   nearest checkpoint. Close to it, nothing plays until the playhead is
//!   reached; farther than a few `preview_preroll_frames`, the song is played
//!   approximately from just before the playhead until the exact render has
//!   caught up (`PlaybackConfig::approximate_start`, also for a start).
//!
//! The switches are listed in [`PlaybackStatus::switches`] with the frame
//! where each took effect.

use std::collections::{BTreeMap, HashSet, VecDeque};
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
/// A start is approximate only when the playhead is more than this many
/// `preview_preroll_frames` past the checkpoint it starts from: closer, waiting
/// is about as short as the approximation.
const START_GATE_PREROLLS: u64 = 3;
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
    /// When an edit changes frames already rendered, the new plan is first
    /// rendered cold from this many frames before the playhead, to be heard
    /// at once but approximately (see [`PlaybackSwitch::approximate_from`]),
    /// until the exact render catches up. 0 turns the approximation off.
    pub preview_preroll_frames: u64,
    /// When the playhead is far (more than a few `preview_preroll_frames`)
    /// past the nearest checkpoint, start or seek by playing the song
    /// approximately from just before the playhead, while the exact render
    /// catches up, instead of waiting for it in silence. Needs
    /// `preview_preroll_frames` above 0.
    pub approximate_start: bool,
    /// Render threads for this playback's engines. `None` uses the engine
    /// default (`MAAC_RENDER_THREADS`, else the machine's cores up to four).
    /// `Some(0)` is clamped to 1.
    pub render_threads: Option<usize>,
    /// When `Some(span)`, play `[span.start, span.end)` repeatedly (clamped to
    /// the plan's length) without stopping at `span.end`. Every pass is
    /// bit-identical to an exact render of that range. Must be nonempty
    /// (`start < end`).
    pub loop_range: Option<RenderSpan>,
    /// How long edits must stay quiet before this playback adopts the latest
    /// plan. `Duration::ZERO` (the default) adopts every edit at once. While
    /// waiting, the current plan keeps filling the ring so playback does not
    /// underrun; [`Playback::seek`] always adopts the latest plan at once.
    pub edit_debounce: Duration,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            lead_frames: 65_536,
            chunk_frames: 2_048,
            pause_prerender: false,
            preview_preroll_frames: 24_000,
            approximate_start: true,
            render_threads: None,
            loop_range: None,
            edit_debounce: Duration::ZERO,
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
    /// `Some(frame)`: the sound from here on is an approximation of the plan,
    /// the frames of a render that started cold at `frame` (see
    /// [`DspEngine::render_span`]), until the next switch of the same
    /// generation without it, which is exact again.
    pub approximate_from: Option<u64>,
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
    /// The sound is an approximation of the newest plan while the exact render
    /// catches up.
    pub approximate: bool,
    /// The plan each stretch of sound belongs to: the first entry is the plan
    /// at the start (or at the last seek), and each later one a change.
    pub switches: Vec<PlaybackSwitch>,
    /// The frame the last catch-up rendered forward from.
    pub catch_up_from: Option<u64>,
    /// The active loop range, if any.
    pub loop_range: Option<RenderSpan>,
    /// Why the producer stopped, if it did.
    pub last_error: Option<String>,
}

/// How many spent buffers the ring keeps: twice the chunks the lead holds, and
/// some more. The ring holds about `lead / chunk` chunks and each finished one
/// goes to `spent`, so this is out of reach unless the chunks are short (the
/// last one of a song, say) or the producer is not taking buffers back for a
/// long time; only then does `read` free one.
fn spent_buffers(lead_frames: u64, chunk_frames: u64) -> usize {
    usize::try_from(lead_frames / chunk_frames.max(1))
        .unwrap_or(usize::MAX / 4)
        .saturating_mul(2)
        .saturating_add(16)
        .min(1 << 16)
}

fn validate_loop(range: Option<RenderSpan>) -> Result<Option<RenderSpan>> {
    if let Some(span) = range {
        if span.start >= span.end {
            return Err(RenderError::RenderState(format!(
                "loop range {}..{} must be nonempty",
                span.start, span.end
            )));
        }
    }
    Ok(range)
}

fn effective_loop(loop_range: Option<RenderSpan>, total: u64) -> Option<RenderSpan> {
    let span = loop_range?;
    if span.start >= total {
        return None;
    }
    let end = span.end.min(total);
    (span.start < end).then_some(RenderSpan {
        start: span.start,
        end,
    })
}

struct Chunk {
    /// The song frame of `samples[0]`.
    start_frame: u64,
    samples: Vec<f64>,
    /// Samples already read.
    taken: usize,
}

struct Ring {
    chunks: VecDeque<Chunk>,
    /// Buffers of chunks that were read to the end, kept for the producer to
    /// fill again, so that `read` never frees memory (it may run on an audio
    /// thread). Its capacity is fixed when the ring is made and it never grows.
    spent: Vec<Vec<f64>>,
    /// The frame the next read starts at.
    read_frame: u64,
    buffered: u64,
    /// Counts the seeks. A chunk rendered before one is dropped.
    epoch: u64,
    seek_to: Option<u64>,
    /// Counts `set_loop` calls so the producer picks up loop changes.
    loop_epoch: u64,
    loop_range: Option<RenderSpan>,
    /// Counts live mute / parameter override updates so the producer applies
    /// them on the next chunk without rebuilding the plan or checkpoints.
    live_epoch: u64,
    live_muted: HashSet<String>,
    live_params: BTreeMap<(String, String), f64>,
    /// When set, `live_params` are cleared automatically once playback reaches
    /// a plan generation at or above this value (for glitch-free handoff after
    /// committing a live slider tweak to the plan).
    clear_params_at_generation: Option<u64>,
    /// The producer has put the last frame of the plan in.
    finished: bool,
    underruns: u64,
    stale: bool,
    approximate: bool,
    catch_up_from: Option<u64>,
    switches: Vec<PlaybackSwitch>,
    error: Option<String>,
}

impl Ring {
    /// The song frame immediately after the last buffered frame, or
    /// `read_frame` when the ring is empty.
    fn tail_frame(&self, channels: usize) -> u64 {
        match self.chunks.back() {
            Some(last) => last.start_frame + (last.samples.len() / channels) as u64,
            None => self.read_frame,
        }
    }
}

struct Play {
    ring: Mutex<Ring>,
    /// The reader took something out, or something else changed.
    room: Condvar,
    shutdown: AtomicBool,
    channels: usize,
    rate: u32,
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
        let loop_range = validate_loop(config.loop_range)?;
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
                spent: Vec::with_capacity(spent_buffers(config.lead_frames, config.chunk_frames)),
                read_frame: start,
                buffered: 0,
                epoch: 0,
                seek_to: None,
                loop_epoch: 0,
                loop_range,
                live_epoch: 0,
                live_muted: HashSet::new(),
                live_params: BTreeMap::new(),
                clear_params_at_generation: None,
                finished: false,
                underruns: 0,
                stale: false,
                approximate: false,
                catch_up_from: None,
                switches: vec![PlaybackSwitch {
                    frame: start,
                    generation: synced.generation,
                    approximate_from: None,
                }],
                error: None,
            }),
            room: Condvar::new(),
            shutdown: AtomicBool::new(false),
            channels: usize::from(output.channels),
            rate: output.sample_rate_hz,
        });
        // The standard library sets a lock up on its first use (an allocation on
        // some systems): do it here, not in the first `read`.
        drop(locked(&play.ring));
        play.room.notify_all();
        let first = Setup::new(synced, start);
        let mut ctx = Ctx {
            shared: Arc::clone(shared),
            play: Arc::clone(&play),
            lead: config.lead_frames,
            chunk: config.chunk_frames,
            preroll: config.preview_preroll_frames,
            approximate_start: config.approximate_start,
            render_threads: config.render_threads,
            edit_debounce: config.edit_debounce,
            loop_range,
            loop_epoch: 0,
            loop_cache: None,
            live_epoch: 0,
            live_muted: HashSet::new(),
            live_params: BTreeMap::new(),
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
    /// part of one is set to silence) and return how many were ready. The rest of
    /// `out` is set to silence. Never renders.
    pub fn read(&self, out: &mut [f64]) -> usize {
        let channels = self.play.channels;
        let want = out.len() / channels;
        let mut ring = locked(&self.play.ring);
        let mut got = 0usize;
        while got < want {
            let (done, take, next_frame) = {
                let Some(chunk) = ring.chunks.front_mut() else {
                    break;
                };
                let available = (chunk.samples.len() - chunk.taken) / channels;
                let take = available.min(want - got);
                let from = chunk.taken;
                out[got * channels..(got + take) * channels]
                    .copy_from_slice(&chunk.samples[from..from + take * channels]);
                chunk.taken += take * channels;
                let next_frame = chunk.start_frame + (chunk.taken / channels) as u64;
                (chunk.taken == chunk.samples.len(), take, next_frame)
            };
            ring.read_frame = next_frame;
            if done {
                if let Some(chunk) = ring.chunks.pop_front() {
                    // Keep the buffer unless there is no room: then it is freed,
                    // which only happens if the producer is not taking them.
                    if ring.spent.len() < ring.spent.capacity() {
                        ring.spent.push(chunk.samples);
                    }
                }
            }
            got += take;
        }
        if let Some(front) = ring.chunks.front() {
            ring.read_frame = front.start_frame + (front.taken / channels) as u64;
        }
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
        ring.approximate = false;
        ring.error = None;
        drop(ring);
        self.play.room.notify_all();
    }

    /// Set or clear the loop range while playing. A nonempty `RenderSpan`
    /// repeats `[start, end)` seamlessly (clamped to the plan's length); `None`
    /// turns looping off. Any already-buffered frames that cross a boundary
    /// inconsistent with `range` are trimmed so the change takes effect without
    /// waiting for the full lead to drain.
    pub fn set_loop(&self, range: Option<RenderSpan>) -> Result<()> {
        let range = validate_loop(range)?;
        let channels = self.play.channels;
        let mut ring = locked(&self.play.ring);
        if ring.loop_range == range {
            return Ok(());
        }
        ring.loop_range = range;
        ring.loop_epoch += 1;
        // Do not wake an already-ended playback.
        if !(ring.finished && ring.buffered == 0) {
            let active_end = range
                .filter(|span| ring.read_frame < span.end)
                .map(|span| span.end);
            let mut expected = ring.read_frame;
            let mut keep = 0usize;
            let mut trimmed = false;
            let mut new_buffered = 0u64;
            while keep < ring.chunks.len() {
                let chunk = &mut ring.chunks[keep];
                let chunk_start = chunk.start_frame + (chunk.taken / channels) as u64;
                let chunk_frames = (chunk.samples.len() - chunk.taken) / channels;
                let chunk_end = chunk_start + chunk_frames as u64;
                if chunk_start != expected {
                    let same_wrap =
                        range.is_some_and(|span| expected == span.end && chunk_start == span.start);
                    if !same_wrap {
                        trimmed = true;
                        break;
                    }
                }
                if let Some(limit) = active_end {
                    if chunk_end > limit {
                        if chunk_start < limit {
                            let keep_frames = (limit - chunk_start) as usize;
                            chunk.samples.truncate(chunk.taken + keep_frames * channels);
                            new_buffered += keep_frames as u64;
                            keep += 1;
                        }
                        trimmed = true;
                        break;
                    }
                }
                new_buffered += chunk_frames as u64;
                expected = chunk_end;
                keep += 1;
            }
            while ring.chunks.len() > keep {
                if let Some(chunk) = ring.chunks.pop_back() {
                    if ring.spent.len() < ring.spent.capacity() {
                        ring.spent.push(chunk.samples);
                    }
                }
                trimmed = true;
            }
            if trimmed {
                ring.buffered = new_buffered;
                ring.finished = false;
            } else if ring.finished && ring.buffered > 0 && active_end.is_some() {
                ring.finished = false;
            }
        }
        drop(ring);
        self.play.room.notify_all();
        Ok(())
    }

    /// Mute or unmute graph nodes in real time during live playback.
    ///
    /// Unlike replacing the plan, this updates the active DSP engine in place
    /// with a smooth ~5 ms fade per node and takes effect on the next rendered
    /// chunk without invalidating checkpoints or triggering catch-up.
    pub fn set_muted_nodes(&self, muted: HashSet<String>) {
        let mut ring = locked(&self.play.ring);
        if ring.live_muted == muted {
            return;
        }
        ring.live_muted = muted;
        ring.live_epoch = ring.live_epoch.wrapping_add(1);
        drop(ring);
        self.play.room.notify_all();
    }

    /// Override a node parameter in real time during live playback (for
    /// example, while dragging a mixer or effect slider). Takes effect on the
    /// next rendered chunk without recompiling or invalidating checkpoints.
    pub fn set_live_param(&self, node: impl Into<String>, param: impl Into<String>, value: f64) {
        if !value.is_finite() {
            return;
        }
        let mut ring = locked(&self.play.ring);
        let key = (node.into(), param.into());
        if ring.live_params.get(&key).copied() == Some(value)
            && ring.clear_params_at_generation.is_none()
        {
            return;
        }
        ring.live_params.insert(key, value);
        ring.clear_params_at_generation = None;
        ring.live_epoch = ring.live_epoch.wrapping_add(1);
        drop(ring);
        self.play.room.notify_all();
    }

    /// Clear all real-time parameter overrides immediately while preserving
    /// any active node mutes.
    pub fn clear_live_params(&self) {
        let mut ring = locked(&self.play.ring);
        if ring.live_params.is_empty() && ring.clear_params_at_generation.is_none() {
            return;
        }
        ring.live_params.clear();
        ring.clear_params_at_generation = None;
        ring.live_epoch = ring.live_epoch.wrapping_add(1);
        drop(ring);
        self.play.room.notify_all();
    }

    /// Schedule active live parameter overrides to clear automatically once
    /// playback adopts a plan whose generation is at least `generation`.
    ///
    /// This keeps a dragged mixer/FX slider audible at its new value while the
    /// committed `.maac` edit compiles and switches over, preventing any brief
    /// dip back to the previous plan's parameter value.
    pub fn clear_live_params_from_generation(&self, generation: u64) {
        let mut ring = locked(&self.play.ring);
        if ring.live_params.is_empty() {
            ring.clear_params_at_generation = None;
            return;
        }
        ring.clear_params_at_generation = Some(generation);
        ring.live_epoch = ring.live_epoch.wrapping_add(1);
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
            approximate: ring.approximate,
            switches: ring.switches.clone(),
            catch_up_from: ring.catch_up_from,
            loop_range: ring.loop_range,
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
    preroll: u64,
    approximate_start: bool,
    render_threads: Option<usize>,
    edit_debounce: Duration,
    loop_range: Option<RenderSpan>,
    /// The `set_loop` count the producer has taken up.
    loop_epoch: u64,
    /// Exact checkpoint cached at the active loop's `start` frame:
    /// `(generation, frame, checkpoint)`.
    loop_cache: Option<(u64, u64, Checkpoint)>,
    /// The live-control update count the producer has applied.
    live_epoch: u64,
    live_muted: HashSet<String>,
    live_params: BTreeMap<(String, String), f64>,
    /// The seek count the producer has taken up.
    epoch: u64,
}

impl Ctx {
    fn new_engine<'a>(&self, plan: &'a PlanArtifact, limits: &PlanLimits) -> Result<DspEngine<'a>> {
        let mut engine = DspEngine::new_artifact_with_limits(plan, limits)?;
        if let Some(threads) = self.render_threads {
            engine.set_render_threads(threads);
        }
        Ok(engine)
    }

    fn cached_loop_checkpoint(&self, generation: u64, frame: u64) -> Option<Checkpoint> {
        self.loop_cache
            .as_ref()
            .filter(|(gen, at, _)| *gen == generation && *at == frame)
            .map(|(_, _, checkpoint)| checkpoint.clone())
    }

    /// Snapshot the latest live mute & parameter overrides from the ring and
    /// apply them immediately (no mute fade-out burst) onto a freshly prepared
    /// engine right before it starts producing audible chunks.
    fn apply_live_state_to_new_engine(&mut self, engine: &mut DspEngine<'_>, generation: u64) {
        {
            let mut ring = locked(&self.play.ring);
            if ring
                .clear_params_at_generation
                .is_some_and(|target| generation >= target)
            {
                ring.live_params.clear();
                ring.clear_params_at_generation = None;
                ring.live_epoch = ring.live_epoch.wrapping_add(1);
            }
            self.live_epoch = ring.live_epoch;
            self.live_muted.clone_from(&ring.live_muted);
            self.live_params.clone_from(&ring.live_params);
        }
        if !self.live_muted.is_empty() || engine.has_live_mutes() {
            engine.set_muted_nodes_immediate(&self.live_muted);
        }
        if engine.has_live_params() {
            engine.clear_live_params();
        }
        for ((node, param), &value) in &self.live_params {
            engine.set_live_param(node, param, value);
        }
    }

    /// Sync any new live mute / parameter updates onto an already-playing
    /// engine (with smooth ~5 ms mute fades). Fast-paths in ~20 ns when
    /// nothing changed.
    fn sync_live_controls(&mut self, engine: &mut DspEngine<'_>, generation: u64) {
        let (changed, params_changed) = {
            let mut ring = locked(&self.play.ring);
            if ring
                .clear_params_at_generation
                .is_some_and(|target| generation >= target)
            {
                ring.live_params.clear();
                ring.clear_params_at_generation = None;
                ring.live_epoch = ring.live_epoch.wrapping_add(1);
            }
            if ring.live_epoch == self.live_epoch {
                return;
            }
            self.live_epoch = ring.live_epoch;
            let params_changed = self.live_params != ring.live_params;
            self.live_muted.clone_from(&ring.live_muted);
            if params_changed {
                self.live_params.clone_from(&ring.live_params);
            }
            (true, params_changed)
        };
        if changed {
            engine.set_muted_nodes(&self.live_muted);
            if params_changed {
                engine.clear_live_params();
                for ((node, param), &value) in &self.live_params {
                    engine.set_live_param(node, param, value);
                }
            }
        }
    }
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
    Loop,
    Edit,
}

/// Whether a newer plan is ready to be adopted (after `edit_debounce`), and if
/// not, how long until the debounce window expires.
fn edit_ready(ctx: &Ctx, generation: u64) -> (bool, Option<Duration>) {
    if ctx.shared.stored.load(Ordering::Acquire) == generation {
        return (false, None);
    }
    if ctx.edit_debounce.is_zero() {
        return (true, None);
    }
    let updated_at = locked(&ctx.shared.slot).updated_at;
    match updated_at {
        Some(at) => {
            let elapsed = at.elapsed();
            if elapsed >= ctx.edit_debounce {
                (true, None)
            } else {
                (false, Some(ctx.edit_debounce - elapsed))
            }
        }
        None => (true, None),
    }
}

/// The latest plan, taken up into the session, and the checkpoint to start it
/// from for `frame`.
fn sync(shared: &Arc<Shared>, frame: u64) -> Result<Synced> {
    let _foreground = Foreground::new(shared);
    let mut state = locked(&shared.state);
    // `update` stores its plan and counts it under the slot lock, so the count
    // read here is exactly the number of plans that are in the slot or in the
    // session: the plan taken below is the one the count names.
    let generation = {
        let mut slot = locked(&shared.slot);
        if let Some(plan) = slot.plan.take() {
            state.session.update(&plan);
        }
        shared.stored.load(Ordering::Acquire)
    };
    let (plan, limits, checkpoint) = state.session.checkpoint_before(frame)?;
    publish(shared, &state.session);
    Ok(Synced {
        plan,
        limits,
        checkpoint,
        generation,
    })
}

/// The reader's idea of a frame (its size and rate) is fixed when the playback
/// starts; an edit that changes it cannot be played on.
fn check_format(ctx: &Ctx, plan: &PlanArtifact) -> Result<()> {
    let output = plan.output();
    if usize::from(output.channels) != ctx.play.channels || output.sample_rate_hz != ctx.play.rate {
        return Err(RenderError::RenderState(
            "the edit changes the number of channels or the sample rate; \
             start a new playback"
                .into(),
        ));
    }
    Ok(())
}

/// Note that the sound from `frame` on is the plan of `generation`. Where
/// nothing has been put in the ring for that frame yet the entry already there
/// is corrected instead of a new one added.
fn note_plan(ctx: &Ctx, frame: u64, generation: u64) {
    let mut ring = locked(&ctx.play.ring);
    let empty = ring.buffered == 0;
    let corrects = empty && ring.switches.last().is_some_and(|last| last.frame == frame);
    if corrects {
        if let Some(last) = ring.switches.last_mut() {
            last.generation = generation;
            // Nothing of an approximate start is in the ring: the plan the
            // sound starts with is exact again until a preview is up.
            last.approximate_from = None;
        }
    } else {
        if ring.switches.len() >= MAX_SWITCHES {
            ring.switches.remove(0);
        }
        ring.switches.push(PlaybackSwitch {
            frame,
            generation,
            approximate_from: None,
        });
    }
    // Either way the entry is exact: what follows is rendered from `frame` on.
    ring.approximate = false;
}

/// An error ends what the producer was doing, not the playback: it records
/// the error and waits for a seek, or for the plan to change, and starts again.
fn run(ctx: &mut Ctx, first: Setup) -> Result<()> {
    let mut setup = first;
    loop {
        match run_plan(ctx, setup) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let generation = ctx.shared.stored.load(Ordering::Acquire);
                locked(&ctx.play.ring).error = Some(format!("{error:?}"));
                match idle_until_seek(ctx, Some(generation)) {
                    Next::Exit => return Ok(()),
                    Next::Again(next) => setup = *next,
                }
            }
        }
    }
}

fn run_plan(ctx: &mut Ctx, first: Setup) -> Result<()> {
    let mut setup = first;
    loop {
        let total = setup.plan.output().total_frames;
        if setup.pos >= total {
            // An edit made the plan shorter than the playhead: nothing is left.
            {
                let mut ring = locked(&ctx.play.ring);
                // Not if a seek came in meanwhile: it cleared this.
                if ring.epoch == ctx.epoch {
                    ring.finished = true;
                }
            }
            match idle_until_seek(ctx, None) {
                Next::Exit => return Ok(()),
                Next::Again(next) => {
                    setup = *next;
                    continue;
                }
            }
        }
        if will_start_approximately(ctx, setup.pos, &setup.restore) {
            match start_approximately(ctx, &setup)? {
                Next::Exit => return Ok(()),
                Next::Again(next) => {
                    setup = *next;
                    continue;
                }
            }
        }
        let mut engine = ctx.new_engine(&setup.plan, &setup.limits)?;
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

/// Start far from a checkpoint: nothing is in the ring to play meanwhile, so
/// the catch-up has no old plan, and the sound starts with the approximate one.
fn start_approximately(ctx: &mut Ctx, setup: &Setup) -> Result<Next> {
    let exact = ctx.new_engine(&setup.plan, &setup.limits)?;
    let preview = ctx.new_engine(&setup.plan, &setup.limits)?;
    let mut pos = setup.pos;
    let total = setup.plan.output().total_frames;
    let target = NewPlan {
        engine: exact,
        preview: Some(preview),
        total,
        start: setup.restore.clone(),
        generation: setup.generation,
    };
    match catch_up(ctx, None, None, &mut pos, 0, target)? {
        Caught::Swap(restore) => {
            record_switch(ctx, pos, setup.generation, None);
            Ok(Next::Again(Box::new(Setup {
                plan: setup.plan.clone(),
                limits: setup.limits,
                restore,
                pos,
                generation: setup.generation,
            })))
        }
        Caught::Exit => Ok(Next::Exit),
        Caught::Seek(next) => Ok(next),
        // Another edit: the newest plan, from where the sound ends.
        Caught::Superseded | Caught::SupersededLive => {
            let synced = sync(&ctx.shared, pos)?;
            check_format(ctx, &synced.plan)?;
            note_plan(ctx, pos, synced.generation);
            Ok(Next::Again(Box::new(Setup::new(synced, pos))))
        }
        Caught::LoopWrap {
            loop_start,
            loop_checkpoint,
            progress,
        } => {
            if let Some(cp) = loop_checkpoint {
                ctx.loop_cache = Some((setup.generation, loop_start, cp));
            }
            let synced = sync(&ctx.shared, loop_start)?;
            check_format(ctx, &synced.plan)?;
            let best_restore = if synced.generation == setup.generation {
                pick_latest(
                    pick_latest(
                        synced.checkpoint,
                        ctx.cached_loop_checkpoint(setup.generation, loop_start),
                    ),
                    progress.filter(|cp| cp.frame() <= loop_start),
                )
            } else {
                synced.checkpoint
            };
            if !will_start_approximately(ctx, loop_start, &best_restore) {
                record_switch(ctx, loop_start, synced.generation, None);
            }
            Ok(Next::Again(Box::new(Setup {
                plan: synced.plan,
                limits: synced.limits,
                restore: best_restore,
                pos: loop_start,
                generation: synced.generation,
            })))
        }
        Caught::LoopChanged => take_loop_resync(ctx, setup.generation),
        Caught::Finished => Err(RenderError::RenderState(
            "a start without an old plan cannot run into its end".into(),
        )),
    }
}

fn will_start_approximately(ctx: &Ctx, pos: u64, restore: &Option<Checkpoint>) -> bool {
    let nearest = restore.as_ref().map_or(0, Checkpoint::frame);
    ctx.approximate_start
        && ctx.preroll > 0
        && pos > nearest.saturating_add(START_GATE_PREROLLS.saturating_mul(ctx.preroll))
}

fn pick_latest(a: Option<Checkpoint>, b: Option<Checkpoint>) -> Option<Checkpoint> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.frame() > a.frame() { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Adopt a `set_loop` change and resume from the current tail of the ring (or
/// wrap straight to `loop.start` if the ring was trimmed at `loop.end`).
fn take_loop_resync(ctx: &mut Ctx, current_generation: u64) -> Result<Next> {
    let tail = {
        let ring = locked(&ctx.play.ring);
        ctx.loop_epoch = ring.loop_epoch;
        ctx.loop_range = ring.loop_range;
        ring.tail_frame(ctx.play.channels)
    };
    let synced = sync(&ctx.shared, tail)?;
    check_format(ctx, &synced.plan)?;
    let total = synced.plan.output().total_frames;
    let target_pos = match effective_loop(ctx.loop_range, total) {
        Some(span) if tail == span.end => span.start,
        _ => tail,
    };
    let synced = if target_pos == tail {
        synced
    } else {
        sync(&ctx.shared, target_pos)?
    };
    check_format(ctx, &synced.plan)?;
    let restore = pick_latest(
        synced.checkpoint,
        ctx.cached_loop_checkpoint(synced.generation, target_pos),
    );
    if synced.generation != current_generation {
        record_switch(ctx, target_pos, synced.generation, None);
    }
    Ok(Next::Again(Box::new(Setup {
        plan: synced.plan,
        limits: synced.limits,
        restore,
        pos: target_pos,
        generation: synced.generation,
    })))
}

/// Wait for a seek (or the end of the playback). With `retry` set, a change of
/// plan after that generation also starts the producer again, from the end of
/// what is in the ring.
fn idle_until_seek(ctx: &mut Ctx, mut retry: Option<u64>) -> Next {
    loop {
        match wait_room(ctx, 0, false, false) {
            Wake::Exit => return Next::Exit,
            Wake::Seek => match take_seek(ctx) {
                Ok(Some(next)) => return next,
                Ok(None) => continue,
                Err(error) => locked(&ctx.play.ring).error = Some(format!("{error:?}")),
            },
            Wake::Loop => {
                let (woke, tail) = {
                    let ring = locked(&ctx.play.ring);
                    ctx.loop_epoch = ring.loop_epoch;
                    ctx.loop_range = ring.loop_range;
                    (!ring.finished, ring.tail_frame(ctx.play.channels))
                };
                if woke {
                    let generation = ctx.shared.stored.load(Ordering::Acquire);
                    match take_loop_resync(ctx, generation) {
                        Ok(next) => return next,
                        Err(error) => {
                            let _ = tail;
                            locked(&ctx.play.ring).error = Some(format!("{error:?}"));
                        }
                    }
                }
            }
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
        if let Some(seen) = retry {
            let (ready, _) = edit_ready(ctx, seen);
            if ready {
                let now = ctx.shared.stored.load(Ordering::Acquire);
                match resync_after_error(ctx) {
                    Ok(setup) => return Next::Again(Box::new(setup)),
                    Err(error) => {
                        locked(&ctx.play.ring).error = Some(format!("{error:?}"));
                        retry = Some(now);
                    }
                }
            }
        }
    }
}

/// Start again after an error from the newest plan, where the ring ends.
fn resync_after_error(ctx: &Ctx) -> Result<Setup> {
    let pos = {
        let ring = locked(&ctx.play.ring);
        ring.tail_frame(ctx.play.channels)
    };
    let synced = sync(&ctx.shared, pos)?;
    check_format(ctx, &synced.plan)?;
    let mut ring = locked(&ctx.play.ring);
    if ring.epoch != ctx.epoch {
        // A seek came in: it is taken up by the caller's next look.
        return Err(RenderError::RenderState("a seek came in".into()));
    }
    ring.error = None;
    drop(ring);
    note_plan(ctx, pos, synced.generation);
    Ok(Setup::new(synced, pos))
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
        if ring.loop_epoch != ctx.loop_epoch {
            return Wake::Loop;
        }
        let debounce_wait = if watch_edits {
            let (ready, wait_left) = edit_ready(ctx, generation);
            if ready {
                return Wake::Edit;
            }
            wait_left
        } else {
            None
        };
        if !ring.finished && ring.buffered < ctx.lead {
            return Wake::Room;
        }
        let wait = debounce_wait.map_or(POLL, |left| POLL.min(left.max(Duration::from_millis(1))));
        let (next, timeout) = ctx
            .play
            .room
            .wait_timeout(ring, wait)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ring = next;
        if once && timeout.timed_out() {
            return Wake::Idle;
        }
    }
}

/// Take up a seek: the plan as it is now, from the checkpoint nearest the
/// frame. `None` if the seek was undone by the time it was looked at.
fn take_seek(ctx: &mut Ctx) -> Result<Option<Next>> {
    let frame = {
        let mut ring = locked(&ctx.play.ring);
        ctx.epoch = ring.epoch;
        match ring.seek_to.take() {
            Some(frame) => frame,
            None => return Ok(None),
        }
    };
    let synced = sync(&ctx.shared, frame)?;
    check_format(ctx, &synced.plan)?;
    {
        let mut ring = locked(&ctx.play.ring);
        // Only the newest seek counts.
        if ring.epoch != ctx.epoch {
            return Ok(None);
        }
        ring.switches.clear();
        ring.switches.push(PlaybackSwitch {
            frame,
            generation: synced.generation,
            approximate_from: None,
        });
        ring.stale = false;
        ring.approximate = false;
    }
    Ok(Some(Next::Again(Box::new(Setup::new(synced, frame)))))
}

enum Push {
    Ok,
    Seeked,
    LoopChanged,
}

/// Put a rendered chunk in the ring.
fn push(ctx: &Ctx, start_frame: u64, samples: Vec<f64>, last: bool) -> Push {
    let mut ring = locked(&ctx.play.ring);
    if ring.epoch != ctx.epoch {
        if ring.spent.len() < ring.spent.capacity() {
            ring.spent.push(samples);
        }
        return Push::Seeked;
    }
    if ring.loop_epoch != ctx.loop_epoch {
        if ring.spent.len() < ring.spent.capacity() {
            ring.spent.push(samples);
        }
        return Push::LoopChanged;
    }
    if ring.chunks.is_empty() {
        ring.read_frame = start_frame;
    }
    ring.buffered += (samples.len() / ctx.play.channels) as u64;
    ring.chunks.push_back(Chunk {
        start_frame,
        samples,
        taken: 0,
    });
    if last {
        ring.finished = true;
    }
    Push::Ok
}

fn record_switch(ctx: &Ctx, frame: u64, generation: u64, approximate_from: Option<u64>) {
    let mut ring = locked(&ctx.play.ring);
    let empty = ring.buffered == 0;
    match ring.switches.last_mut() {
        // Nothing of the earlier sound was put in the ring: it is corrected.
        Some(last) if last.frame == frame && empty => {
            last.generation = generation;
            last.approximate_from = approximate_from;
        }
        _ => {
            if ring.switches.len() >= MAX_SWITCHES {
                ring.switches.remove(0);
            }
            ring.switches.push(PlaybackSwitch {
                frame,
                generation,
                approximate_from,
            });
        }
    }
    ring.stale = false;
    ring.approximate = approximate_from.is_some();
}

enum Produce {
    Pushed(u64),
    Seeked,
    LoopChanged,
}

/// Render one chunk of `engine` from `pos` into the ring.
///
/// `finish` says whether reaching `limit` ends the stream.
fn produce(
    ctx: &Ctx,
    engine: &mut DspEngine<'_>,
    pos: u64,
    limit: u64,
    finish: bool,
    approximate: bool,
) -> Result<Produce> {
    let end = (pos + ctx.chunk).min(limit);
    let wanted = (end - pos) as usize * ctx.play.channels;
    let recycled = locked(&ctx.play.ring).spent.pop();
    let mut samples = recycled.unwrap_or_default();
    samples.clear();
    samples.reserve(wanted);
    let sink = |frame: &[f64]| {
        samples.extend_from_slice(frame);
        Ok(())
    };
    if approximate {
        engine.render_preview_chunk(RenderSpan { start: pos, end }, sink)?;
    } else {
        engine.render_chunk(RenderSpan { start: pos, end }, sink)?;
    }
    Ok(match push(ctx, pos, samples, finish && end >= limit) {
        Push::Ok => Produce::Pushed(end),
        Push::Seeked => Produce::Seeked,
        Push::LoopChanged => Produce::LoopChanged,
    })
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
            if let Some(next) = take_seek(ctx)? {
                return Ok(next);
            }
            continue;
        }
        if edit_ready(ctx, generation).0 {
            // Start again from the newest plan; nothing of this one has been
            // heard, so the plan the sound starts with is corrected.
            let synced = sync(&ctx.shared, pos)?;
            check_format(ctx, &synced.plan)?;
            note_plan(ctx, pos, synced.generation);
            return Ok(Next::Again(Box::new(Setup::new(synced, pos))));
        }
        let end = (at + CATCH_UP_STEP).min(pos);
        engine.render_chunk(RenderSpan { start: at, end }, |_| Ok(()))?;
        at = end;
    }

    // If starting right at the active loop's start frame, cache its exact state
    // so subsequent passes wrap without re-rendering from an earlier checkpoint.
    if let Some(span) = effective_loop(ctx.loop_range, total) {
        if pos == span.start && pos > 0 && ctx.cached_loop_checkpoint(generation, pos).is_none() {
            ctx.loop_cache = Some((generation, pos, engine.chunk_checkpoint()?));
        }
    }
    ctx.apply_live_state_to_new_engine(engine, generation);

    let mut history: VecDeque<Checkpoint> = VecDeque::new();
    let mut next_snapshot = (pos / HISTORY_EVERY + 1) * HISTORY_EVERY;
    let own = CheckpointSet {
        plan: setup.plan.clone(),
        limits: setup.limits,
        schedule: Schedule::of(engine),
        checkpoints: Vec::new(),
    };
    loop {
        if let Some(span) = effective_loop(ctx.loop_range, total) {
            if pos == span.end && pos > span.start {
                // Seamless exact wrap back to `span.start`.
                if span.start == 0 {
                    engine.reset();
                    ctx.apply_live_state_to_new_engine(engine, generation);
                    pos = 0;
                    next_snapshot = HISTORY_EVERY;
                    continue;
                }
                if let Some(cp) = ctx.cached_loop_checkpoint(generation, span.start) {
                    engine.restore_from(&cp)?;
                    ctx.apply_live_state_to_new_engine(engine, generation);
                    pos = span.start;
                    next_snapshot = (pos / HISTORY_EVERY + 1) * HISTORY_EVERY;
                    continue;
                }
                // No cached checkpoint at `span.start` yet (e.g., started
                // inside the loop or loop start moved): sync and advance to
                // `span.start`, which will then cache the checkpoint.
                let synced = sync(&ctx.shared, span.start)?;
                check_format(ctx, &synced.plan)?;
                let own_snap = (synced.generation == generation)
                    .then(|| {
                        history
                            .iter()
                            .rev()
                            .find(|c| c.frame() <= span.start)
                            .cloned()
                    })
                    .flatten();
                let restore = pick_latest(synced.checkpoint, own_snap);
                if synced.generation != generation {
                    record_switch(ctx, span.start, synced.generation, None);
                }
                return Ok(Next::Again(Box::new(Setup {
                    plan: synced.plan,
                    limits: synced.limits,
                    restore,
                    pos: span.start,
                    generation: synced.generation,
                })));
            }
        }
        if pos >= total {
            // Everything is rendered; wait for the reader, a seek, or a loop.
            return Ok(idle_until_seek(ctx, None));
        }
        match wait_room(ctx, generation, true, false) {
            Wake::Exit => return Ok(Next::Exit),
            Wake::Seek => {
                if let Some(next) = take_seek(ctx)? {
                    return Ok(next);
                }
            }
            Wake::Loop => {
                let tail = {
                    let ring = locked(&ctx.play.ring);
                    ctx.loop_epoch = ring.loop_epoch;
                    ctx.loop_range = ring.loop_range;
                    ring.tail_frame(ctx.play.channels)
                };
                if tail == pos {
                    if !engine.has_live_mutes() && !engine.has_live_params() {
                        if let Some(span) = effective_loop(ctx.loop_range, total) {
                            if pos == span.start
                                && pos > 0
                                && ctx.cached_loop_checkpoint(generation, pos).is_none()
                            {
                                ctx.loop_cache =
                                    Some((generation, pos, engine.chunk_checkpoint()?));
                            }
                        }
                    }
                    continue;
                }
                // Buffered chunks were trimmed: wrap immediately if trimmed to
                // the new loop's end, otherwise resume at `tail`.
                if let Some(span) = effective_loop(ctx.loop_range, total) {
                    if tail == span.end {
                        pos = span.end;
                        continue;
                    }
                }
                let synced = sync(&ctx.shared, tail)?;
                check_format(ctx, &synced.plan)?;
                let own_snap = (synced.generation == generation)
                    .then(|| history.iter().rev().find(|c| c.frame() <= tail).cloned())
                    .flatten();
                let restore = pick_latest(
                    pick_latest(synced.checkpoint, own_snap),
                    ctx.cached_loop_checkpoint(synced.generation, tail),
                );
                if synced.generation != generation {
                    record_switch(ctx, tail, synced.generation, None);
                }
                return Ok(Next::Again(Box::new(Setup {
                    plan: synced.plan,
                    limits: synced.limits,
                    restore,
                    pos: tail,
                    generation: synced.generation,
                })));
            }
            Wake::Room | Wake::Idle => {
                ctx.sync_live_controls(engine, generation);
                let (limit, finish) = match effective_loop(ctx.loop_range, total) {
                    Some(span) if pos < span.start => (span.start, false),
                    Some(span) if pos < span.end => (span.end, false),
                    _ => (total, true),
                };
                match produce(ctx, engine, pos, limit, finish, false)? {
                    Produce::Pushed(end) => {
                        pos = end;
                        let pure_state = !engine.has_live_mutes() && !engine.has_live_params();
                        if pure_state {
                            if let Some(span) = effective_loop(ctx.loop_range, total) {
                                if pos == span.start
                                    && pos > 0
                                    && ctx.cached_loop_checkpoint(generation, pos).is_none()
                                {
                                    ctx.loop_cache =
                                        Some((generation, pos, engine.chunk_checkpoint()?));
                                }
                            }
                            if pos >= next_snapshot && pos < total {
                                history.push_back(engine.chunk_checkpoint()?);
                                if history.len() > HISTORY_KEPT {
                                    history.pop_front();
                                }
                                next_snapshot = (pos / HISTORY_EVERY + 1) * HISTORY_EVERY;
                            }
                        }
                    }
                    Produce::Seeked => continue,
                    Produce::LoopChanged => {
                        return take_loop_resync(ctx, generation);
                    }
                }
            }
            Wake::Edit => {
                let synced = sync(&ctx.shared, pos)?;
                check_format(ctx, &synced.plan)?;
                let candidate = ctx.new_engine(&synced.plan, &synced.limits)?;
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
                // Keep the cached loop-start checkpoint warm if the edit leaves
                // everything up to it unchanged.
                if let Some((cached_gen, cached_at, _)) = ctx.loop_cache.as_mut() {
                    if *cached_gen == generation
                        && match reuse {
                            Reuse::Unchanged => true,
                            Reuse::From(frame) => *cached_at <= frame,
                            Reuse::Full(_) => false,
                        }
                    {
                        *cached_gen = synced.generation;
                    }
                }
                match reuse {
                    Reuse::Unchanged => generation = synced.generation,
                    Reuse::From(frame) if frame >= pos && !engine.has_live_params() => {
                        // The frames rendered so far are the new plan's too.
                        let restore = (pos > 0).then(|| engine.chunk_checkpoint()).transpose()?;
                        record_switch(ctx, pos, synced.generation, None);
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
                        let start = pick_latest(synced.checkpoint.clone(), own_snapshot);
                        {
                            let mut ring = locked(&ctx.play.ring);
                            ring.stale = true;
                            ring.catch_up_from = Some(start.as_ref().map_or(0, Checkpoint::frame));
                        }
                        let new_total = synced.plan.output().total_frames;
                        let preview = if ctx.preroll > 0 {
                            Some(ctx.new_engine(&synced.plan, &synced.limits)?)
                        } else {
                            None
                        };
                        let target_plan = NewPlan {
                            engine: candidate,
                            preview,
                            total: new_total,
                            start,
                            generation: synced.generation,
                        };
                        match catch_up(
                            ctx,
                            Some(engine),
                            Some(generation),
                            &mut pos,
                            total,
                            target_plan,
                        )? {
                            Caught::Swap(restore) => {
                                record_switch(ctx, pos, synced.generation, None);
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
                            Caught::Superseded => {
                                let mut ring = locked(&ctx.play.ring);
                                ring.stale = false;
                            }
                            Caught::SupersededLive => {
                                // The engine of the old plan stopped where the
                                // approximate sound took over: start again from
                                // the newest plan at the end of the ring.
                                let synced = sync(&ctx.shared, pos)?;
                                check_format(ctx, &synced.plan)?;
                                note_plan(ctx, pos, synced.generation);
                                return Ok(Next::Again(Box::new(Setup::new(synced, pos))));
                            }
                            Caught::LoopWrap {
                                loop_start,
                                loop_checkpoint,
                                progress,
                            } => {
                                locked(&ctx.play.ring).stale = false;
                                if let Some(cp) = loop_checkpoint {
                                    ctx.loop_cache = Some((synced.generation, loop_start, cp));
                                }
                                let latest = sync(&ctx.shared, loop_start)?;
                                check_format(ctx, &latest.plan)?;
                                let best_restore = if latest.generation == synced.generation {
                                    pick_latest(
                                        pick_latest(
                                            latest.checkpoint,
                                            ctx.cached_loop_checkpoint(
                                                synced.generation,
                                                loop_start,
                                            ),
                                        ),
                                        progress.filter(|cp| cp.frame() <= loop_start),
                                    )
                                } else {
                                    latest.checkpoint
                                };
                                if !will_start_approximately(ctx, loop_start, &best_restore) {
                                    record_switch(ctx, loop_start, latest.generation, None);
                                }
                                return Ok(Next::Again(Box::new(Setup {
                                    plan: latest.plan,
                                    limits: latest.limits,
                                    restore: best_restore,
                                    pos: loop_start,
                                    generation: latest.generation,
                                })));
                            }
                            Caught::LoopChanged => {
                                locked(&ctx.play.ring).stale = false;
                                return take_loop_resync(ctx, generation);
                            }
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
    /// As `Superseded`, after the approximate sound had taken over.
    SupersededLive,
    /// The loop end was reached while catching up: wrap to `loop_start` on the
    /// latest plan, keeping any checkpoint the chase already reached.
    LoopWrap {
        loop_start: u64,
        loop_checkpoint: Option<Checkpoint>,
        progress: Option<Checkpoint>,
    },
    /// `set_loop` was called while catching up.
    LoopChanged,
    /// The old plan reached the end of the song first.
    Finished,
}

/// The edited plan a catch-up renders toward the playhead.
struct NewPlan<'a> {
    engine: DspEngine<'a>,
    /// A second engine for the new plan, for the approximate sound.
    preview: Option<DspEngine<'a>>,
    total: u64,
    /// The state to start from; `None` for frame 0.
    start: Option<Checkpoint>,
    generation: u64,
}

struct AbortGuard<'a> {
    flag: &'a AtomicBool,
    armed: bool,
}

impl Drop for AbortGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::Release);
        }
    }
}

enum Chase {
    Ready(Option<Checkpoint>),
    Aborted {
        loop_checkpoint: Option<Checkpoint>,
        progress: Option<Checkpoint>,
    },
}

/// On its own thread: render the new plan forward toward `target` (the
/// producer's position, which moves) until told to hold, and hand over its
/// state at the target.
fn chase(
    mut engine: DspEngine<'_>,
    mut at: u64,
    loop_start: Option<u64>,
    target: &AtomicU64,
    reached: &AtomicU64,
    hold: &AtomicBool,
    abort: &AtomicBool,
) -> Result<Chase> {
    let mut loop_checkpoint: Option<Checkpoint> = None;
    loop {
        if abort.load(Ordering::Acquire) {
            let progress = (at > 0 && loop_start.is_some_and(|l| at <= l))
                .then(|| engine.chunk_checkpoint())
                .transpose()?;
            return Ok(Chase::Aborted {
                loop_checkpoint,
                progress,
            });
        }
        let to = target.load(Ordering::Acquire);
        if at < to {
            let mut end = (at + CATCH_UP_STEP).min(to);
            if let Some(l) = loop_start {
                if at < l && end > l {
                    end = l;
                }
            }
            engine.render_chunk(RenderSpan { start: at, end }, |_| Ok(()))?;
            at = end;
            reached.store(at, Ordering::Release);
            if loop_checkpoint.is_none() && loop_start.is_some_and(|l| l > 0 && at == l) {
                loop_checkpoint = Some(engine.chunk_checkpoint()?);
            }
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
/// goes on filling the ring with the old plan (with no old plan, as at a start,
/// the ring is filled by the approximate sound alone). When the new plan is within a
/// few chunks of the playhead and the ring can spare the time, this thread
/// stops producing, the other finishes the last of the distance, and the
/// caller switches.
fn catch_up(
    ctx: &mut Ctx,
    mut old: Option<&mut DspEngine<'_>>,
    old_generation: Option<u64>,
    pos: &mut u64,
    total: u64,
    target_plan: NewPlan<'_>,
) -> Result<Caught> {
    let NewPlan {
        engine: mut candidate,
        preview,
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
    let chase_loop_start = effective_loop(ctx.loop_range, new_total).map(|span| span.start);
    // The approximate sound: the new plan rendered cold from `cold` and kept
    // in step with the playhead, then played in place of the old plan's.
    struct Preview<'a> {
        engine: DspEngine<'a>,
        cold: u64,
        at: u64,
        live: bool,
    }
    let cold = pos.saturating_sub(ctx.preroll);
    let mut preview = preview
        .filter(|_| cold > 0 && cold < *pos)
        .map(|engine| Preview {
            engine,
            cold,
            at: cold,
            live: false,
        });

    enum Done {
        Caught(Caught),
        LoopWrap(u64),
        Hold,
    }
    let (done, joined) = std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            chase(
                candidate,
                at,
                chase_loop_start,
                &target,
                &reached,
                &hold,
                &abort,
            )
        });
        // Whatever happens below, even a panic, the chase is told to stop
        // unless it is being waited for.
        let mut guard = AbortGuard {
            flag: &abort,
            armed: true,
        };
        let done = (|| -> Result<Done> {
            loop {
                let live = preview.as_ref().is_some_and(|preview| preview.live);
                let active_new_loop =
                    effective_loop(ctx.loop_range, new_total).filter(|span| *pos <= span.end);
                let active_old_loop =
                    effective_loop(ctx.loop_range, total).filter(|span| *pos <= span.end);
                if live {
                    if let Some(span) = active_new_loop {
                        if *pos >= span.end {
                            return Ok(Done::LoopWrap(span.start));
                        }
                    }
                } else if old.is_some() {
                    if let Some(span) = active_old_loop {
                        if *pos >= span.end {
                            return Ok(Done::LoopWrap(span.start));
                        }
                    }
                }
                if *pos >= new_total {
                    // The new plan ends before the playhead: it takes over there.
                    return Ok(Done::Caught(Caught::Swap(None)));
                }
                if old.is_some() && !live && *pos >= total {
                    if new_total > total {
                        // The old plan has no more to give but the new one is
                        // longer: nothing can be heard until it has caught up.
                        hold.store(true, Ordering::Release);
                        return Ok(Done::Hold);
                    }
                    return Ok(Done::Caught(Caught::Finished));
                }
                let wake = wait_room(ctx, generation, true, true);
                match wake {
                    Wake::Exit => return Ok(Done::Caught(Caught::Exit)),
                    Wake::Seek => {
                        if let Some(next) = take_seek(ctx)? {
                            return Ok(Done::Caught(Caught::Seek(next)));
                        }
                        continue;
                    }
                    Wake::Loop => return Ok(Done::Caught(Caught::LoopChanged)),
                    Wake::Edit => {
                        return Ok(Done::Caught(if live {
                            Caught::SupersededLive
                        } else {
                            Caught::Superseded
                        }))
                    }
                    Wake::Room | Wake::Idle => {}
                }
                let behind = *pos - reached.load(Ordering::Acquire);
                // With no old plan and nothing heard yet, the exact render
                // arriving first needs no spare sound: it just starts.
                let spare = (old.is_none() && !live)
                    || locked(&ctx.play.ring).buffered >= (ctx.lead / 4).max(ctx.chunk);
                if behind <= close && spare {
                    hold.store(true, Ordering::Release);
                    return Ok(Done::Hold);
                }
                // The old plan's sound is worth less than getting the new one
                // heard, so the preview comes first until the ring is nearly
                // empty.
                let starving = {
                    let ring = locked(&ctx.play.ring);
                    !ring.finished && ring.buffered < 2 * ctx.chunk
                };
                match preview.as_mut() {
                    // Taken over: the sound is the new plan's, approximately.
                    Some(preview) if preview.live => {
                        if matches!(wake, Wake::Room) {
                            ctx.sync_live_controls(&mut preview.engine, generation);
                            let (limit, finish) = match active_new_loop {
                                Some(span) if *pos < span.end => (span.end, false),
                                _ => (new_total, true),
                            };
                            match produce(ctx, &mut preview.engine, *pos, limit, finish, true)? {
                                Produce::Pushed(end) => {
                                    *pos = end;
                                    target.store(end, Ordering::Release);
                                }
                                Produce::Seeked => {}
                                Produce::LoopChanged => {
                                    return Ok(Done::Caught(Caught::LoopChanged));
                                }
                            }
                        }
                    }
                    // Not yet: the old plan fills the ring while it is hungry,
                    // and the preview is brought up to the playhead otherwise.
                    Some(preview) if old.is_none() || !(starving && matches!(wake, Wake::Room)) => {
                        let end = (preview.at + CATCH_UP_STEP).min(*pos);
                        preview.engine.render_preview_chunk(
                            RenderSpan {
                                start: preview.at,
                                end,
                            },
                            |_| Ok(()),
                        )?;
                        preview.at = end;
                        if preview.at == *pos {
                            ctx.apply_live_state_to_new_engine(&mut preview.engine, generation);
                            preview.live = true;
                            record_switch(ctx, *pos, generation, Some(preview.cold));
                        }
                    }
                    _ => {
                        if let (Some(old), true) = (old.as_deref_mut(), matches!(wake, Wake::Room))
                        {
                            ctx.sync_live_controls(old, old_generation.unwrap_or(0));
                            let (limit, finish) = match active_old_loop {
                                Some(span) if *pos < span.end => (span.end, false),
                                _ => (total, new_total <= total),
                            };
                            match produce(ctx, old, *pos, limit, finish, false)? {
                                Produce::Pushed(end) => {
                                    *pos = end;
                                    target.store(end, Ordering::Release);
                                }
                                Produce::Seeked => {}
                                Produce::LoopChanged => {
                                    return Ok(Done::Caught(Caught::LoopChanged));
                                }
                            }
                        }
                    }
                }
            }
        })();
        if matches!(done, Ok(Done::Hold)) {
            guard.armed = false;
        }
        drop(guard);
        (done, handle.join())
    });
    let chased =
        joined.map_err(|_| RenderError::RenderState("the catch-up thread panicked".into()))?;
    match done? {
        Done::Caught(caught) => {
            chased?;
            Ok(caught)
        }
        Done::LoopWrap(loop_start) => match chased? {
            Chase::Aborted {
                loop_checkpoint,
                progress,
            } => Ok(Caught::LoopWrap {
                loop_start,
                loop_checkpoint,
                progress,
            }),
            Chase::Ready(_) => Ok(Caught::LoopWrap {
                loop_start,
                loop_checkpoint: None,
                progress: None,
            }),
        },
        Done::Hold => match chased? {
            Chase::Ready(state) => Ok(Caught::Swap(state)),
            Chase::Aborted { .. } => Ok(Caught::Superseded),
        },
    }
}
