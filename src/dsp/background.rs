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
    last_error: Option<String>,
}

type Hook = Arc<dyn Fn(u64) + Send + Sync>;

struct Shared {
    state: Mutex<State>,
    /// The latest plan not yet handed to the session.
    slot: Mutex<Option<PlanArtifact>>,
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
                last_error: None,
            }),
            slot: Mutex::new(None),
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
        // The count goes up first: anything the worker commits after this is
        // refused, and the plan is not applied before it is in the slot.
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        *locked(&self.shared.slot) = Some(plan.clone());
        self.wake(false);
    }

    /// Render frames `start..end` of the latest plan on this thread, exactly as
    /// [`RenderSession::render`] does. The worker stops while this runs. Two
    /// renders at once wait for each other.
    pub fn render<F>(&self, start: u64, end: u64, callback: F) -> Result<RenderReport>
    where
        F: FnMut(&[f64]) -> Result<()>,
    {
        // Counted before the lock is asked for, so a worker holding it stops
        // at its next check instead of finishing its stretch.
        self.shared.foreground.fetch_add(1, Ordering::AcqRel);
        let result = {
            let mut state = locked(&self.shared.state);
            if let Some(plan) = locked(&self.shared.slot).take() {
                state.session.update(&plan);
            }
            state.session.render(start, end, callback)
        };
        let _signal = locked(&self.shared.signal);
        self.shared.foreground.fetch_sub(1, Ordering::AcqRel);
        drop(_signal);
        self.wake(false);
        result
    }

    pub fn status(&self) -> SessionStatus {
        let busy = {
            let signal = locked(&self.shared.signal);
            signal.busy || signal.dirty
        };
        let state = locked(&self.shared.state);
        SessionStatus {
            checkpoints: state.session.checkpoint_count(),
            last_checkpoint: state.session.last_checkpoint_frame(),
            busy,
            generation: self.shared.generation.load(Ordering::Acquire),
            last_error: state.last_error.clone(),
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

    /// The frames of the checkpoints held, ascending.
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
        let retry = pass(shared);
        let mut signal = locked(&shared.signal);
        signal.busy = false;
        signal.dirty |= retry;
        shared.wake.notify_all();
    }
}

/// One stretch of work: take up the latest plan and render the rest of it for
/// checkpoints. True if it was cut short and should be tried again.
fn pass(shared: &Arc<Shared>) -> bool {
    let job = {
        let mut state = locked(&shared.state);
        if let Some(plan) = locked(&shared.slot).take() {
            state.session.update(&plan);
        }
        match state.session.adopt_pending() {
            Ok(_) => state.last_error = None,
            Err(error) => {
                state.last_error = Some(format!("{error:?}"));
                return false;
            }
        }
        match state.session.prerender_job() {
            Some(job) => job,
            None => return false,
        }
    };
    let generation = shared.generation.load(Ordering::Acquire);
    let stop = |shared: &Shared| {
        shared.generation.load(Ordering::Acquire) != generation
            || shared.foreground.load(Ordering::Acquire) > 0
            || locked(&shared.signal).shutdown
    };
    match fill(shared, job, &stop) {
        Ok(()) => false,
        Err(RenderError::Callback(_)) => true,
        Err(error) => {
            locked(&shared.state).last_error = Some(format!("{error:?}"));
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
            Ok(())
        },
    )
}

fn cancelled() -> RenderError {
    RenderError::Callback("background render stopped".into())
}
