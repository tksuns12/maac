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

/// Read a whole song in reads of 256 frames, counting what `read` frees and
/// allocates on this thread, and seek once on the way (the seek itself is not
/// counted: it is the controlling thread's).
fn count_reads(lead_frames: u64, chunk_frames: u64) -> (u64, u64, usize) {
    let limits = PlanLimits::song();
    let plan = compile(&base_song(7)).unwrap();
    let bg = BackgroundSession::new_with(&plan, &limits, SessionPolicy::new(4_801), false).unwrap();
    let play = bg
        .play(
            0,
            PlaybackConfig {
                lead_frames,
                chunk_frames,
                pause_prerender: false,
                preview_preroll_frames: 0,
                approximate_start: false,
                ..PlaybackConfig::default()
            },
        )
        .unwrap();
    let channels = play.channels();
    let total = plan.output().total_frames;
    let mut buffer = vec![0.0; 256 * channels];
    let started = Instant::now();
    let (mut frees, mut allocs, mut frames, mut seeked, mut lagged) = (0, 0, 0usize, false, false);
    loop {
        assert!(started.elapsed() < Duration::from_secs(120), "stalled");
        if frames >= 20_000 && !seeked {
            seeked = true;
            play.seek(total - lead_frames * 3 / 4);
        }
        // Once, let the producer run to the end of the song before reading on,
        // so that the reader then finishes a whole lead of chunks while the
        // producer has nothing left to produce.
        if seeked && !lagged {
            let status = play.status();
            if status.position + status.buffered_frames >= total {
                lagged = true;
            } else {
                assert!(started.elapsed() < Duration::from_secs(120), "stalled");
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
        }
        COUNTING.with(|on| on.set(true));
        let (f0, a0) = (FREES.with(Cell::get), ALLOCS.with(Cell::get));
        let got = play.read(&mut buffer);
        let (f1, a1) = (FREES.with(Cell::get), ALLOCS.with(Cell::get));
        COUNTING.with(|on| on.set(false));
        frees += f1 - f0;
        allocs += a1 - a0;
        frames += got;
        if got == 0 {
            let status = play.status();
            assert!(status.last_error.is_none(), "{status:?}");
            if status.ended {
                return (frees, allocs, frames);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[test]
fn reading_neither_frees_nor_allocates() {
    let (frees, allocs, frames) = count_reads(8_192, 512);
    assert!(frames > 6_000, "the song was not read to its end");
    assert_eq!(frees, 0, "read freed memory");
    assert_eq!(allocs, 0, "read allocated memory");
}

/// More chunks of lead than any fixed number of kept buffers would cover.
#[test]
fn reading_a_long_lead_of_short_chunks_neither_frees_nor_allocates() {
    let (frees, allocs, frames) = count_reads(65_536, 256);
    assert!(frames > 6_000, "the song was not read to its end");
    assert_eq!(frees, 0, "read freed memory");
    assert_eq!(allocs, 0, "read allocated memory");
}

/// Seamless looping across multiple passes keeps `read` free of allocations and frees.
#[test]
fn reading_across_loop_wraps_neither_frees_nor_allocates() {
    let limits = PlanLimits::song();
    let plan = compile(&base_song(7)).unwrap();
    let bg = BackgroundSession::new_with(&plan, &limits, SessionPolicy::new(4_801), false).unwrap();
    let span = maac::dsp::RenderSpan {
        start: 3_000,
        end: 7_500,
    };
    let play = bg
        .play(
            span.start,
            PlaybackConfig {
                lead_frames: 8_192,
                chunk_frames: 512,
                pause_prerender: false,
                preview_preroll_frames: 0,
                approximate_start: false,
                loop_range: Some(span),
                ..PlaybackConfig::default()
            },
        )
        .unwrap();
    let channels = play.channels();
    let mut buffer = vec![0.0; 256 * channels];
    let started = Instant::now();
    let (mut frees, mut allocs, mut frames) = (0u64, 0u64, 0usize);
    while frames < 18_000 {
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
            let status = play.status();
            assert!(status.last_error.is_none(), "{status:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(frees, 0, "loop read freed memory");
    assert_eq!(allocs, 0, "loop read allocated memory");
}
