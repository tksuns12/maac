//! With the gate at its default (`MAAC_PARALLEL_MIN_NANOS` unset), blocks go
//! to other threads only when the work is worth it, and the render is the same
//! bit for bit on any number of threads: some batches parallel, some rendered
//! one by one, whatever the timing of this run makes of them. This file never
//! sets the variable, so its process reads the default. How many batches are
//! parallel depends on timing and is not asserted here.

use std::path::Path;

use maac::compiler::compile_bundle_artifact_with_limits;
use maac::dsp::{DspEngine, RenderSpan};
use maac::plan::PlanLimits;
use maac::PlanArtifact;

fn example(name: &str) -> PlanArtifact {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bundle =
        maac::bundle_fs::load_bundle(&root.join(format!("examples/{name}/{name}.maac")), root)
            .unwrap();
    compile_bundle_artifact_with_limits(&bundle, &PlanLimits::song()).unwrap()
}

fn excerpt(plan: &PlanArtifact, threads: usize, seconds: u64) -> Vec<u64> {
    assert!(
        std::env::var_os("MAAC_PARALLEL_MIN_NANOS").is_none(),
        "this test needs the default gate"
    );
    let mut engine = DspEngine::new_artifact_with_limits(plan, &PlanLimits::song()).unwrap();
    engine.set_render_threads(threads);
    let end = (u64::from(plan.output().sample_rate_hz) * seconds).min(plan.output().total_frames);
    let mut out = Vec::new();
    engine
        .render_span(RenderSpan { start: 0, end }, |frame| {
            out.extend(frame.iter().map(|sample| sample.to_bits()));
            Ok(())
        })
        .unwrap();
    out
}

#[test]
fn songs_are_the_same_on_any_number_of_threads_at_the_default_gate() {
    for name in ["lantern", "afterlight"] {
        let plan = example(name);
        let serial = excerpt(&plan, 1, 10);
        for threads in [2, 4, 8] {
            assert!(
                serial == excerpt(&plan, threads, 10),
                "{name}: {threads} threads differ"
            );
        }
    }
}
