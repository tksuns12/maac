//! Worker threads that render pieces of an instrument side by side.
//!
//! An instrument that nothing modulates renders from its own state and its own
//! events alone. So the blocks several instruments need at the same frame can
//! be rendered at once without changing a sample: an instrument is moved to
//! whichever thread takes its task and moved back, and the caller reads the
//! results in its own order. Nothing is shared while a piece renders.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Condvar, Mutex, OnceLock};

type Run = Box<dyn FnOnce() + Send + 'static>;

/// A piece of work, tagged with the call that queued it so that the caller
/// helps with its own pieces only.
struct Task {
    call: u64,
    run: Run,
}

#[derive(Default)]
struct Queue(VecDeque<Task>);

impl Queue {
    /// The first queued piece of one call.
    fn pop_of(&mut self, call: u64) -> Option<Task> {
        let at = self.0.iter().position(|task| task.call == call)?;
        self.0.remove(at)
    }
}

struct Pool {
    tasks: Mutex<Queue>,
    wake: Condvar,
}

static NEXT_CALL: AtomicU64 = AtomicU64::new(0);

static POOL: OnceLock<&'static Pool> = OnceLock::new();

/// How many threads render beside the caller: the cores less one, at most 7.
pub(crate) fn default_workers() -> usize {
    static WORKERS: OnceLock<usize> = OnceLock::new();
    *WORKERS.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .saturating_sub(1)
            .min(7)
    })
}

fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        let pool: &'static Pool = Box::leak(Box::new(Pool {
            tasks: Mutex::new(Queue::default()),
            wake: Condvar::new(),
        }));
        for number in 0..default_workers() {
            let spawned = std::thread::Builder::new()
                .name(format!("maac-render-{number}"))
                .spawn(move || loop {
                    let task = {
                        let mut tasks = pool.tasks.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some(task) = tasks.0.pop_front() {
                                break task;
                            }
                            tasks = pool.wake.wait(tasks).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    (task.run)();
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
/// The caller helps with its own queued jobs while it waits, so a job may
/// itself call this, and the caller is never held by another caller's work.
pub(crate) fn map<T: Send + 'static>(
    jobs: Vec<Box<dyn FnOnce() -> T + Send + 'static>>,
) -> Vec<Option<T>> {
    let pool = pool();
    let count = jobs.len();
    let (reply, results) = channel();
    let call = NEXT_CALL.fetch_add(1, Ordering::Relaxed);
    {
        let mut tasks = pool.tasks.lock().unwrap_or_else(|e| e.into_inner());
        for (index, job) in jobs.into_iter().enumerate() {
            let reply = reply.clone();
            let run: Run = Box::new(move || {
                let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).ok();
                // The caller may have gone: then there is nobody to tell.
                let _ = reply.send((index, value));
            });
            tasks.0.push_back(Task { call, run });
        }
    }
    drop(reply);
    // One wake per job, and none for the job this thread starts with: waking
    // every worker for a few jobs costs more than the jobs.
    for _ in 1..count.min(default_workers() + 1) {
        pool.wake.notify_one();
    }
    // Help with this call's own pieces while some result is missing, and stop
    // when the last one is in.
    let mut out: Vec<Option<T>> = (0..count).map(|_| None).collect();
    let mut received = 0;
    while received < count {
        while let Ok((index, value)) = results.try_recv() {
            out[index] = value;
            received += 1;
        }
        if received == count {
            break;
        }
        let task = pool
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_of(call);
        match task {
            Some(task) => (task.run)(),
            // Nothing queued: what is missing is running on other threads.
            None => match results.recv() {
                Ok((index, value)) => {
                    out[index] = value;
                    received += 1;
                }
                Err(_) => break,
            },
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
    fn a_caller_runs_only_its_own_pieces() {
        // Another call's pieces sit in the queue while this call waits.
        let other = NEXT_CALL.fetch_add(1, Ordering::Relaxed);
        let ran_here = std::sync::Arc::new(Mutex::new(Vec::new()));
        {
            let mut tasks = pool().tasks.lock().unwrap();
            let ran = std::sync::Arc::clone(&ran_here);
            let me = std::thread::current().id();
            tasks.0.push_front(Task {
                call: other,
                run: Box::new(move || ran.lock().unwrap().push(std::thread::current().id() == me)),
            });
        }
        let jobs: Vec<Job<u8>> = vec![Box::new(|| 1), Box::new(|| 2)];
        assert_eq!(map(jobs), vec![Some(1), Some(2)]);
        // Take the other call's piece back out if no worker has run it yet:
        // either way it did not run on this thread.
        let _ = pool().tasks.lock().unwrap().pop_of(other);
        assert!(!ran_here.lock().unwrap().contains(&true));
    }

    #[test]
    fn no_jobs_give_no_results() {
        assert!(map::<u8>(Vec::new()).is_empty());
    }
}
