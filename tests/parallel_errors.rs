//! A render that fails fails the same way on any number of threads: the same
//! error at the same frame, with the same frames delivered before it. Three
//! instruments refill together (two of them fail, one is healthy), so the
//! blocks are rendered side by side and the errors are found ahead of the
//! frame they belong to.

use maac::dsp::DspEngine;

#[path = "support/frames.rs"]
mod frames;
use frames::FrameProject;

/// Two sweeps that push a filter cutoff below zero about 70 ms into a note
/// (frame 3360), and a healthy tone.
fn project(frames_per_note: u64) -> FrameProject {
    let declarations = r#"
instrument swept { channels = 1;
  voice v { channels = 1; amplitude = &amp; output = &tone:out;
    node amp { type = "synth.adsr/1"; params = { sustain = 1; release = 50ms; }; }
    node osc { type = "synth.saw/1"; }
    node sweep { type = "synth.lfo/1"; params = { frequency = 3Hz; }; }
    node tone { type = "synth.svf/1"; config = { channels = 1; mode = lowpass; }; params = { cutoff = 1000Hz; }; }
    connect osc_tone { from = &osc:out; to = &tone:in; }
    modulate swing { from = &sweep:out; to = &tone.params.cutoff; depth = -2000Hz; }
  }
}
instrument steady { channels = 1;
  voice v { channels = 1; amplitude = &amp; output = &osc:out;
    node amp { type = "synth.adsr/1"; params = { sustain = 1; release = 50ms; }; }
    node osc { type = "synth.sine/1"; }
  }
}
node a { instrument = &swept; }
node b { instrument = &swept; }
node c { instrument = &steady; }
node mix { type = "core.sum/1"; config = { channels = 1; }; }
connect a_mix { from = &a:out; to = &mix:in; }
connect b_mix { from = &b:out; to = &mix:in; }
connect c_mix { from = &c:out; to = &mix:in; }
track notes_b { target = &b:events; }
track notes_c { target = &c:events; }
place play_b { pattern = &phrase; track = &notes_b; at = 0q; }
place play_c { pattern = &phrase; track = &notes_c; at = 0q; }
"#;
    let mut project = FrameProject::new(frames_per_note * 2, 0, "a", declarations);
    project.output = "mix".to_owned();
    project.note("n1", 0, frames_per_note, "440Hz", "").note(
        "n2",
        frames_per_note,
        frames_per_note * 2 - 1,
        "330Hz",
        "",
    )
}

/// Frames delivered, the error and its text, from a render on `threads`.
fn run(project: &FrameProject, threads: usize) -> (Vec<u64>, Option<String>, u64) {
    let artifact = project.artifact();
    let mut engine = DspEngine::new_artifact(&artifact).unwrap();
    engine.set_render_threads(threads);
    let mut out = Vec::new();
    let result = engine.render(|frame| {
        out.extend(frame.iter().map(|sample| sample.to_bits()));
        Ok(())
    });
    (
        out,
        result.err().map(|e| format!("{e:?}")),
        engine.parallel_batches(),
    )
}

#[test]
fn an_error_comes_at_the_same_frame_on_any_number_of_threads() {
    // Notes of different lengths put the failure on and off block boundaries.
    for frames_per_note in [24_000, 24_960, 25_088, 26_000] {
        let project = project(frames_per_note);
        let (serial, error, batches) = run(&project, 1);
        assert_eq!(batches, 0);
        let error = error.unwrap_or_else(|| panic!("{frames_per_note}: the sweep did not fail"));
        assert!(
            !serial.is_empty(),
            "{frames_per_note}: nothing before the error"
        );
        let mut batched = 0;
        for threads in [2, 4, 8] {
            let (parallel, parallel_error, batches) = run(&project, threads);
            assert_eq!(
                parallel_error.as_deref(),
                Some(error.as_str()),
                "{frames_per_note}, {threads} threads: another error"
            );
            assert!(
                serial == parallel,
                "{frames_per_note}, {threads} threads: other frames before the error"
            );
            batched += batches;
        }
        assert!(
            batched > 0,
            "{frames_per_note}: nothing rendered side by side"
        );
    }
}
