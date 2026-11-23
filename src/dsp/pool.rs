//! Worker threads that render pieces of an instrument side by side.
//!
//! An instrument that nothing modulates renders from its own state and its own
//! events alone, and the voices of one instrument render from their own state
//! within a block. So the blocks several instruments need at the same frame, or
//! the voices of one block, can be rendered at once without changing a sample:
//! what a piece needs is moved to whichever thread takes its task and moved
//! back, and the caller reads the results in its own order. Nothing is shared
//! while a piece renders.

use std::collections::VecDeque;
use std::sync::mpsc::channel;
use std::sync::{Condvar, Mutex, OnceLock};

type Task = Box<dyn FnOnce() + Send + 'static>;

struct Pool {
    tasks: Mutex<VecDeque<Task>>,
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
            tasks: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        }));
        for number in 0..default_workers() {
            let spawned = std::thread::Builder::new()
                .name(format!("maac-render-{number}"))
                .spawn(move || loop {
                    let task = {
                        let mut tasks = pool.tasks.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some(task) = tasks.pop_front() {
                                break task;
                            }
                            tasks = pool.wake.wait(tasks).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    task();
                });
            // Without a thread the caller does everything itself.
            if spawned.is_err() {
                break;
            }
        }
        pool
    })
}

/// Run every job, on the pool's threads and this one, and return what each
/// returned, in the order given. A job that panicked gives `None`.
///
/// The caller helps with whatever is queued (its own jobs or another's) while
/// it waits, so a job may itself call this.
pub(crate) fn map<T: Send + 'static>(
    jobs: Vec<Box<dyn FnOnce() -> T + Send + 'static>>,
) -> Vec<Option<T>> {
    let pool = pool();
    let count = jobs.len();
    let (reply, results) = channel();
    {
        let mut tasks = pool.tasks.lock().unwrap_or_else(|e| e.into_inner());
        for (index, job) in jobs.into_iter().enumerate() {
            let reply = reply.clone();
            tasks.push_back(Box::new(move || {
                let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).ok();
                // The caller may have gone: then there is nobody to tell.
                let _ = reply.send((index, value));
            }));
        }
    }
    drop(reply);
    // One wake per job, and none for the job this thread starts with: waking
    // every worker for a few jobs costs more than the jobs.
    for _ in 1..count.min(default_workers() + 1) {
        pool.wake.notify_one();
    }
    loop {
        let task = pool
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front();
        match task {
            Some(task) => task(),
            None => break,
        }
    }
    let mut out: Vec<Option<T>> = (0..count).map(|_| None).collect();
    for _ in 0..count {
        match results.recv() {
            Ok((index, value)) => out[index] = value,
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    type Job<T> = Box<dyn FnOnce() -> T + Send + 'static>;

    #[test]
    fn results_come_back_in_the_order_given() {
        let jobs: Vec<Job<usize>> = (0..64usize)
            .map(|n| {
                Box::new(move || {
                    // Later jobs finish first.
                    std::thread::sleep(std::time::Duration::from_micros((64 - n as u64) * 20));
                    n
                }) as Job<usize>
            })
            .collect();
        let done = map(jobs);
        assert_eq!(done, (0..64usize).map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn a_job_that_panics_gives_none_and_the_others_still_run() {
        let jobs: Vec<Job<u32>> = vec![
            Box::new(|| 1),
            Box::new(|| panic!("a job failed on purpose")),
            Box::new(|| 3),
        ];
        assert_eq!(map(jobs), vec![Some(1), None, Some(3)]);
        // The pool is still alive after a panic.
        assert_eq!(map::<u8>(vec![Box::new(|| 7)]), vec![Some(7)]);
    }

    #[test]
    fn a_job_may_use_the_pool_itself() {
        let jobs: Vec<Job<u32>> = (0..16u32)
            .map(|n| {
                Box::new(move || {
                    let inner: Vec<Job<u32>> = (0..8u32)
                        .map(|m| Box::new(move || n * 10 + m) as Job<u32>)
                        .collect();
                    map(inner).into_iter().map(|v| v.unwrap()).sum::<u32>()
                }) as Job<u32>
            })
            .collect();
        let want: Vec<Option<u32>> = (0..16u32)
            .map(|n| Some((0..8u32).map(|m| n * 10 + m).sum()))
            .collect();
        assert_eq!(map(jobs), want);
    }

    #[test]
    fn no_jobs_give_no_results() {
        assert!(map::<u8>(Vec::new()).is_empty());
    }
}
