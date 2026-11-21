//! A [`RenderSession`] with a worker thread that fills in its checkpoints.
//!
//! After an edit, or when a plan is first opened, the checkpoints of a session
//! reach only as far as the last render did. The worker renders the rest of
//! the plan, keeping nothing but checkpoints, so that the next edit anywhere
//! has one close by.
//!
//! * [`BackgroundSession::update`] never waits: it leaves the plan in a slot
//!   and wakes the worker.
//! * [`BackgroundSession::render`] runs on the caller's thread, as
//!   [`RenderSession::render`] does. While one runs the worker stops, and it
//!   carries on afterwards from the last checkpoint.
//! * What the worker produced is accepted only if the session is still on the
//!   plan the work was started for. That is decided under the state lock by
//!   the session's own count of updates, so a render that finishes late can
//!   never put a checkpoint of an older plan into the set.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::session::PrerenderJob;
use super::*;
use crate::plan_artifact::PlanArtifact;

/// How often the worker looks for a reason to stop, in frames rendered.
const CHECK_EVERY_FRAMES: u64 = 2_048;

/// What a [`BackgroundSession`] holds and is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionStatus {
    /// Checkpoints held.
    pub checkpoints: usize,
    /// The frame of the last checkpoint: how far the worker has got.
    pub last_checkpoint: u64,
    /// The worker is rendering, or has work waiting.
    pub busy: bool,
    /// Counts the calls of `update`.
    pub generation: u64,
    /// Why the worker last gave up, until something succeeds.
    pub last_error: Option<String>,
}

#[derive(Default)]
struct Signal {
    /// There may be work for the worker.
    dirty: bool,
    busy: bool,
    shutdown: bool,
}

struct State {
    session: RenderSession,
}

type Hook = Arc<dyn Fn(u64) + Send + Sync>;

struct Shared {
    state: Mutex<State>,
    /// The latest plan not yet handed to the session.
    slot: Mutex<Option<PlanArtifact>>,
    /// What the session holds, copied out whenever it changes so that
    /// [`BackgroundSession::status`] never waits for a render.
    checkpoint_count: AtomicUsize,
    last_checkpoint: AtomicU64,
    last_error: Mutex<Option<String>>,
    /// Raised before a plan is put in the slot; the worker reads it to stop
    /// early. Correctness does not depend on it (see the module notes).
    generation: AtomicU64,
    /// Foreground renders running or waiting for the state lock.
    foreground: AtomicUsize,
    auto: AtomicBool,
    signal: Mutex<Signal>,
    wake: Condvar,
    /// Called by the worker with a checkpoint's frame just before it is
    /// committed. Tests use it to hold the worker at a chosen moment.
    hook: Mutex<Option<Hook>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// See the module documentation.
pub struct BackgroundSession {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl BackgroundSession {
    /// A session for `plan`. Unless turned off with
    /// [`Self::set_auto_prerender`], the worker starts filling its checkpoints
    /// at once.
    pub fn new(plan: &PlanArtifact, limits: &PlanLimits, policy: SessionPolicy) -> Result<Self> {
        Self::new_with(plan, limits, policy, true)
    }

    /// As [`Self::new`]; with `auto_prerender` false the worker waits for
    /// [`Self::prerender`].
    pub fn new_with(
        plan: &PlanArtifact,
        limits: &PlanLimits,
        policy: SessionPolicy,
        auto_prerender: bool,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                session: RenderSession::new(plan, limits, policy)?,
            }),
            slot: Mutex::new(None),
            checkpoint_count: AtomicUsize::new(0),
            last_checkpoint: AtomicU64::new(0),
            last_error: Mutex::new(None),
            generation: AtomicU64::new(0),
            foreground: AtomicUsize::new(0),
            auto: AtomicBool::new(auto_prerender),
            signal: Mutex::new(Signal {
                dirty: auto_prerender,
                ..Signal::default()
            }),
            wake: Condvar::new(),
            hook: Mutex::new(None),
        });
        let worker = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("maac-prerender".into())
                .spawn(move || worker(&shared))
                .map_err(|error| RenderError::RenderState(format!("no worker thread: {error}")))?
        };
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// Whether an edit (and a render) starts the worker. On by default.
    pub fn set_auto_prerender(&self, on: bool) {
        self.shared.auto.store(on, Ordering::Release);
    }

    /// Ask the worker to fill the checkpoints now, whatever the setting.
    pub fn prerender(&self) {
        self.wake(true);
    }

    /// Replace the plan. Returns at once; the next render uses it, and the
    /// worker stops what it was doing and carries on with it.
    pub fn update(&self, plan: &PlanArtifact) {
        // The count goes up before the plan is in the slot, so a worker that
        // took the count before this call and the plan after it still stops.
        // It only hurries the worker; what the worker may commit is decided by
        // the session's own update count.
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        *locked(&self.shared.slot) = Some(plan.clone());
        self.wake(false);
    }

    /// Render frames `start..end` of the latest plan on this thread, with the
    /// frames [`RenderSession::render`] gives. The worker stops while this
    /// runs, and two renders at once wait for each other. The report's `reuse`
    /// says `Unchanged` when the worker had already taken up the edit.
    ///
    /// Do not call [`Self::checkpoint_frames`] from `callback`: it waits for
    /// the lock this render holds.
    pub fn render<F>(&self, start: u64, end: u64, callback: F) -> Result<RenderReport>
    where
        F: FnMut(&[f64]) -> Result<()>,
    {
        // Counted before the lock is asked for, so a worker holding it stops
        // at its next check instead of finishing its stretch.
        let _foreground = Foreground::new(&self.shared);
        let mut state = locked(&self.shared.state);
        if let Some(plan) = locked(&self.shared.slot).take() {
            state.session.update(&plan);
        }
        let result = state.session.render(start, end, callback);
        publish(&self.shared, &state.session);
        result
    }

    pub fn status(&self) -> SessionStatus {
        let busy = {
            let signal = locked(&self.shared.signal);
            signal.busy || signal.dirty
        };
        SessionStatus {
            checkpoints: self.shared.checkpoint_count.load(Ordering::Acquire),
            last_checkpoint: self.shared.last_checkpoint.load(Ordering::Acquire),
            busy,
            generation: self.shared.generation.load(Ordering::Acquire),
            last_error: locked(&self.shared.last_error).clone(),
        }
    }

    /// Wait until the worker has nothing left to do; false if `timeout` ran
    /// out first.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut signal = locked(&self.shared.signal);
        while signal.busy || signal.dirty {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            signal = self
                .shared
                .wake
                .wait_timeout(signal, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }

    /// The frames of the checkpoints held, ascending. Waits while a render
    /// runs; [`Self::status`] does not.
    pub fn checkpoint_frames(&self) -> Vec<u64> {
        locked(&self.shared.state).session.checkpoint_frames()
    }

    /// Call `hook` with each checkpoint's frame just before the worker commits
    /// it. For tests that need the worker held at a known point.
    #[doc(hidden)]
    pub fn set_worker_hook(&self, hook: impl Fn(u64) + Send + Sync + 'static) {
        *locked(&self.shared.hook) = Some(Arc::new(hook));
    }

    fn wake(&self, force: bool) {
        let mut signal = locked(&self.shared.signal);
        if force || self.shared.auto.load(Ordering::Acquire) {
            signal.dirty = true;
        }
        self.shared.wake.notify_all();
    }
}

/// Counts a foreground render for as long as it lives, also when its callback
/// panics, and lets the worker know when it ends.
struct Foreground<'a>(&'a Arc<Shared>);

impl<'a> Foreground<'a> {
    fn new(shared: &'a Arc<Shared>) -> Self {
        shared.foreground.fetch_add(1, Ordering::AcqRel);
        Self(shared)
    }
}

impl Drop for Foreground<'_> {
    fn drop(&mut self) {
        let mut signal = locked(&self.0.signal);
        self.0.foreground.fetch_sub(1, Ordering::AcqRel);
        if self.0.auto.load(Ordering::Acquire) {
            signal.dirty = true;
        }
        self.0.wake.notify_all();
    }
}

fn publish(shared: &Shared, session: &RenderSession) {
    shared
        .checkpoint_count
        .store(session.checkpoint_count(), Ordering::Release);
    shared
        .last_checkpoint
        .store(session.last_checkpoint_frame(), Ordering::Release);
}

impl Drop for BackgroundSession {
    fn drop(&mut self) {
        locked(&self.shared.signal).shutdown = true;
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker(shared: &Arc<Shared>) {
    loop {
        {
            let mut signal = locked(&shared.signal);
            while !signal.shutdown
                && !(signal.dirty && shared.foreground.load(Ordering::Acquire) == 0)
            {
                signal = shared
                    .wake
                    .wait(signal)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if signal.shutdown {
                return;
            }
            signal.dirty = false;
            signal.busy = true;
        }
        let retry = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pass(shared)))
            .unwrap_or_else(|panic| {
                let what = panic
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown".into());
                *locked(&shared.last_error) = Some(format!("the worker panicked: {what}"));
                false
            });
        let mut signal = locked(&shared.signal);
        signal.busy = false;
        signal.dirty |= retry;
        shared.wake.notify_all();
    }
}

/// One stretch of work: take up the latest plan and render the rest of it for
/// checkpoints. True if it was cut short and should be tried again.
fn pass(shared: &Arc<Shared>) -> bool {
    // Read before the plan is taken: an update that lands while the plan is
    // being taken up then still stops this stretch.
    let generation = shared.generation.load(Ordering::Acquire);
    let job = {
        let mut state = locked(&shared.state);
        if let Some(plan) = locked(&shared.slot).take() {
            state.session.update(&plan);
        }
        match state.session.adopt_pending() {
            Ok(_) => *locked(&shared.last_error) = None,
            Err(error) => {
                *locked(&shared.last_error) = Some(format!("{error:?}"));
                return false;
            }
        }
        publish(shared, &state.session);
        match state.session.prerender_job() {
            Some(job) => job,
            None => return false,
        }
    };
    let stop = |shared: &Shared| {
        shared.generation.load(Ordering::Acquire) != generation
            || shared.foreground.load(Ordering::Acquire) > 0
            || locked(&shared.signal).shutdown
    };
    match fill(shared, job, &stop) {
        Ok(()) => false,
        Err(RenderError::Callback(_)) => true,
        Err(error) => {
            *locked(&shared.last_error) = Some(format!("{error:?}"));
            false
        }
    }
}

fn fill(shared: &Arc<Shared>, job: PrerenderJob, stop: &dyn Fn(&Shared) -> bool) -> Result<()> {
    let mut engine = DspEngine::new_artifact_with_limits(&job.plan, &job.limits)?;
    let start = match &job.frontier {
        Some(checkpoint) => {
            engine.restore_from(checkpoint)?;
            checkpoint.frame()
        }
        None => 0,
    };
    let epoch = job.epoch;
    let sink_shared = Arc::clone(shared);
    let mut since_check = 0u64;
    engine.render_span_with_checkpoint_sink(
        RenderSpan {
            start,
            end: job.total,
        },
        job.spacing,
        |_| {
            since_check += 1;
            if since_check >= CHECK_EVERY_FRAMES {
                since_check = 0;
                if stop(shared) {
                    return Err(cancelled());
                }
            }
            Ok(())
        },
        move |checkpoint| {
            let hook = locked(&sink_shared.hook).clone();
            if let Some(hook) = hook {
                hook(checkpoint.frame());
            }
            let mut state = locked(&sink_shared.state);
            // The plan may have changed while this stretch was rendering.
            if state.session.epoch() != epoch || state.session.has_pending() {
                return Err(cancelled());
            }
            state.session.insert_checkpoint(checkpoint);
            publish(&sink_shared, &state.session);
            Ok(())
        },
    )
}

fn cancelled() -> RenderError {
    RenderError::Callback("background render stopped".into())
}
