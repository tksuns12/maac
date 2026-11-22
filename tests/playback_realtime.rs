//! `Playback::read` may run on an audio thread, so it must not free memory:
//! the buffers of the chunks it has finished go back to the producer.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

use maac::dsp::{BackgroundSession, PlaybackConfig, SessionPolicy};
use maac::plan::PlanLimits;

#[path = "common/song_edits.rs"]
mod song_edits;
use song_edits::*;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static FREES: Cell<u64> = const { Cell::new(0) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = COUNTING.try_with(|on| {
            if on.get() {
                let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = COUNTING.try_with(|on| {
            if on.get() {
                let _ = FREES.try_with(|n| n.set(n.get() + 1));
            }
        });
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn reading_neither_frees_nor_allocates() {
    let limits = PlanLimits::song();
    let plan = compile(&base_song(7)).unwrap();
    let bg = BackgroundSession::new_with(&plan, &limits, SessionPolicy::new(4_801), false).unwrap();
    let play = bg
        .play(
            0,
            PlaybackConfig {
                lead_frames: 8_192,
                chunk_frames: 512,
                pause_prerender: false,
                preview_preroll_frames: 0,
                approximate_start: false,
            },
        )
        .unwrap();
    let channels = play.channels();
    let mut buffer = vec![0.0; 256 * channels];
    let started = Instant::now();
    // The first reads may set up locks inside the standard library; what
    // counts is the steady state.
    let mut warm = 0;
    while warm < 4 {
        assert!(started.elapsed() < Duration::from_secs(120), "stalled");
        if play.read(&mut buffer) > 0 {
            warm += 1;
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let (mut frees, mut allocs, mut frames) = (0, 0, 0usize);
    // Many chunks go by, so that many buffers are finished.
    while frames < 40_000 {
        assert!(started.elapsed() < Duration::from_secs(120), "stalled");
        COUNTING.with(|on| on.set(true));
        let (f0, a0) = (FREES.with(Cell::get), ALLOCS.with(Cell::get));
        let got = play.read(&mut buffer);
        let (f1, a1) = (FREES.with(Cell::get), ALLOCS.with(Cell::get));
        COUNTING.with(|on| on.set(false));
        frees += f1 - f0;
        allocs += a1 - a0;
        frames += got;
        if got == 0 {
            assert!(play.status().last_error.is_none());
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(frees, 0, "read freed memory");
    assert_eq!(allocs, 0, "read allocated memory");
}
