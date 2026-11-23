//! Worker threads that render blocks of independent instruments side by side.
//!
//! An instrument that nothing modulates renders from its own state and its own
//! events alone, so the blocks that several of them need at the same frame can
//! be rendered at once without changing a sample: each runtime is moved to
//! whichever thread takes its job and moved back, and the engine reads the
//! results in its own order. Nothing is shared while a block renders.

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Condvar, Mutex, OnceLock};

use crate::voice::InstrumentRuntime;

struct Job {
    index: usize,
    runtime: InstrumentRuntime,
    frame: u64,
    horizon: u64,
    reply: Sender<(usize, InstrumentRuntime, bool)>,
}

struct Pool {
    jobs: Mutex<VecDeque<Job>>,
    wake: Condvar,
}

static POOL: OnceLock<&'static Pool> = OnceLock::new();

/// How many threads render beside the caller: the cores less one, at most 7.
pub(crate) fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .saturating_sub(1)
        .min(7)
}

fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        let pool: &'static Pool = Box::leak(Box::new(Pool {
            jobs: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        }));
        for number in 0..default_workers() {
            let spawned = std::thread::Builder::new()
                .name(format!("maac-render-{number}"))
                .spawn(move || loop {
                    let job = {
                        let mut jobs = pool.jobs.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some(job) = jobs.pop_front() {
                                break job;
                            }
                            jobs = pool.wake.wait(jobs).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    run(job);
                });
            // Without a thread the caller renders everything itself.
            if spawned.is_err() {
                break;
            }
        }
        pool
    })
}

fn run(mut job: Job) {
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        job.runtime.prefill(job.frame, job.horizon);
    }))
    .is_ok();
    // The caller may have gone: then there is nobody to tell.
    let _ = job.reply.send((job.index, job.runtime, done));
}

/// Render the block each runtime needs at `frame`, up to its horizon, on the
/// pool's threads and this one. Returns the runtimes by the index they came
/// with, or the indices of those whose rendering panicked.
pub(crate) fn prefill_all(
    work: Vec<(usize, InstrumentRuntime, u64)>,
    frame: u64,
) -> Vec<(usize, InstrumentRuntime, bool)> {
    let pool = pool();
    let count = work.len();
    let (reply, results) = channel();
    {
        let mut jobs = pool.jobs.lock().unwrap_or_else(|e| e.into_inner());
        for (index, runtime, horizon) in work {
            jobs.push_back(Job {
                index,
                runtime,
                frame,
                horizon,
                reply: reply.clone(),
            });
        }
    }
    drop(reply);
    pool.wake.notify_all();
    // Help with whatever is queued, this batch's or another's, rather than wait.
    loop {
        let job = pool
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front();
        match job {
            Some(job) => run(job),
            None => break,
        }
    }
    let mut done = Vec::with_capacity(count);
    while done.len() < count {
        match results.recv() {
            Ok(result) => done.push(result),
            Err(_) => break,
        }
    }
    done
}
