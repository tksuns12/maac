//! How big a checkpoint is and how long it takes to take. Not a pass/fail
//! test: run with `--ignored --nocapture --release` and read the numbers.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::DspEngine;
use maac::plan::PlanLimits;
use maac::SourceBundle;

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            LIVE.fetch_add(new_size - layout.size(), Ordering::Relaxed);
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

#[test]
#[ignore]
fn checkpoint_bytes_and_clone_time() {
    for song in ["homeward", "afterlight", "lantern", "ferry", "lofi"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(song)
            .join(format!("{song}.maac"));
        let source = std::fs::read_to_string(path).unwrap();
        let plan = compile_bundle_artifact_with_limits(
            &SourceBundle::new("standalone.maac", source),
            &PlanLimits::song(),
        )
        .unwrap();

        let before = live();
        let fresh = DspEngine::new_artifact_with_limits(&plan, &PlanLimits::song()).unwrap();
        let fresh_bytes = live() - before;
        drop(fresh);

        let every = 48_000 * 4;
        let mut engine = DspEngine::new_artifact_with_limits(&plan, &PlanLimits::song()).unwrap();
        let started = Instant::now();
        let checkpoints = engine.render_with_checkpoints(every, |_| Ok(())).unwrap();
        let render_secs = started.elapsed().as_secs_f64();

        let mut sizes = Vec::new();
        let mut clone_micros = Vec::new();
        for checkpoint in &checkpoints {
            let before = live();
            let started = Instant::now();
            let copy = DspEngine::restore(checkpoint);
            clone_micros.push(started.elapsed().as_secs_f64() * 1e6);
            sizes.push(live() - before);
            drop(copy);
        }
        let max = sizes.iter().max().copied().unwrap_or(0);
        let mean = sizes.iter().sum::<usize>() / sizes.len().max(1);
        let slowest = clone_micros.iter().cloned().fold(0.0, f64::max);
        println!(
            "COST {song}: {} checkpoints @4s, render {render_secs:.1}s; fresh engine {fresh_bytes} B; checkpoint mean {mean} B max {max} B (total {} MB); clone slowest {slowest:.0} us",
            checkpoints.len(),
            sizes.iter().sum::<usize>() / 1_000_000,
        );
    }
}
