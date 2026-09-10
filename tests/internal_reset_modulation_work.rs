use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::diagnostic::DiagnosticCode;
use maac::dsp::{self, DspEngine, RenderError};
use maac::plan::{PlanLimits, PortRef};
use maac::PlanArtifact;

const OUTPUT_FRAMES: u64 = 2;
const SHARED_BASE_GRAPH: u64 = 3;
const SHARED_RESET_GRAPH: u64 = 4;
const SHARED_NODES: u64 = 2;
const SHARED_RESET_EDGE: u64 = 1;
const SHARED_RESET_PREVIEW: u64 = SHARED_RESET_GRAPH + SHARED_NODES + SHARED_RESET_EDGE;

fn source(internal: &str, instances: usize) -> SourceBundle {
    assert!(matches!(instances, 1 | 2));
    let second_instance = if instances == 2 {
        "node spare { instrument = &local; config = { voices = 4; }; }"
    } else {
        ""
    };
    let text = format!(
        r#"maac 1;
project reset_work {{ score = [0q, 1/24000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}

instrument local {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; }}
  node osc {{ type = "synth.sine/1"; }}
 }}
 shared fx {{
  channels = 1; output = &gain:out;
  node lfo {{ type = "synth.lfo/1"; params = {{ phase = 1/4; }}; }}
  node gain {{ type = "synth.gain/1"; config = {{ channels = 1; }}; }}
  connect input_gain {{ from = &input:out; to = &gain:in; }}
  {internal}
 }}
 control phase {{ target = &fx.lfo.params.phase; default = 1/4; }}
}}
node sound {{ instrument = &local; preset = &quarter; params = {{ phase = 3/4; }}; config = {{ voices = 4; }}; }}
{second_instance}
preset quarter {{ instrument = &local; params = {{ phase = 1/2; }}; }}
"#,
        internal = internal,
        second_instance = second_instance,
    );
    SourceBundle::new("reset_work.maac", text)
}

fn compile(internal: &str, instances: usize) -> PlanArtifact {
    compile_bundle_artifact_with_limits(&source(internal, instances), &PlanLimits::song())
        .expect("reset work fixture compiles")
}

fn limits(max_execution_work: u64) -> PlanLimits {
    PlanLimits {
        max_execution_work,
        ..PlanLimits::song()
    }
}

fn minimum_execution_budget(artifact: &PlanArtifact) -> u64 {
    let mut low = 0;
    let mut high = PlanLimits::song().max_execution_work;
    while low < high {
        let middle = low + (high - low) / 2;
        if artifact.validate_with_limits(&limits(middle)).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    artifact
        .validate_with_limits(&limits(low))
        .expect("minimum execution budget validates");
    low
}

fn expected_work(instances: u64, has_reset_preview: bool) -> u64 {
    let graph_cost = if has_reset_preview {
        SHARED_RESET_GRAPH
    } else {
        SHARED_BASE_GRAPH
    };
    let per_instance = OUTPUT_FRAMES * graph_cost;
    let preview = if has_reset_preview {
        SHARED_RESET_PREVIEW
    } else {
        0
    };
    instances * (per_instance + preview)
}

fn assert_resource_limit(result: Result<(), maac::plan::PlanError>) {
    assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn shared_reset_preview_is_once_per_instrument_instance_without_voice_work() {
    let legacy = compile("", 1);
    let reset = compile(
        "modulate reset_phase { from = &input:out; to = &lfo.params.phase; depth = 0; }",
        1,
    );
    let reset_twice = compile(
        "modulate reset_phase { from = &input:out; to = &lfo.params.phase; depth = 0; }",
        2,
    );

    assert_eq!(legacy.output().total_frames, OUTPUT_FRAMES);
    assert_eq!(reset.output().total_frames, OUTPUT_FRAMES);
    assert_eq!(minimum_execution_budget(&legacy), expected_work(1, false));
    assert_eq!(minimum_execution_budget(&reset), expected_work(1, true));
    assert_eq!(
        minimum_execution_budget(&reset_twice),
        expected_work(2, true)
    );

    // The reset edge is zero-depth and targets an LFO disconnected from the
    // shared output. It is still charged once, independently of notes.
    assert_eq!(
        minimum_execution_budget(&reset) - minimum_execution_budget(&legacy),
        9
    );
    // A second instrument node adds one shared frame graph and one preview;
    // voice capacity does not multiply either charge.
    assert_eq!(
        minimum_execution_budget(&reset_twice) - minimum_execution_budget(&reset),
        SHARED_RESET_GRAPH * OUTPUT_FRAMES + SHARED_RESET_PREVIEW
    );
}

#[test]
fn shared_reset_work_is_enforced_at_source_retained_prepare_render_and_capture_boundaries() {
    let internal = "modulate reset_phase { from = &input:out; to = &lfo.params.phase; depth = 0; }";
    let artifact = compile(internal, 1);
    let budget = minimum_execution_budget(&artifact);
    assert_eq!(budget, expected_work(1, true));
    let exact = limits(budget);
    let below = limits(budget - 1);

    compile_bundle_artifact_with_limits(&source(internal, 1), &exact)
        .expect("source compilation validates at the exact execution budget");
    let diagnostics = compile_bundle_artifact_with_limits(&source(internal, 1), &below)
        .expect_err("source compilation rejects one unit below the execution budget");
    assert_eq!(
        diagnostics.first().expect("resource diagnostic").code,
        DiagnosticCode::ResourceLimit
    );

    artifact.validate_with_limits(&exact).unwrap();
    assert_resource_limit(artifact.validate_with_limits(&below));

    let bytes = artifact.to_json_with_limits(&exact).unwrap();
    let retained = PlanArtifact::from_json_with_limits(&bytes, &exact).unwrap();
    assert_eq!(retained.to_json_with_limits(&exact).unwrap(), bytes);
    assert_resource_limit(artifact.to_json_with_limits(&below).map(|_| ()));
    assert_resource_limit(PlanArtifact::from_json_with_limits(&bytes, &below).map(|_| ()));

    DspEngine::new_artifact_with_limits(&retained, &exact).unwrap();
    match DspEngine::new_artifact_with_limits(&retained, &below) {
        Err(RenderError::Plan(error)) => assert_eq!(error.code, "E_RESOURCE_LIMIT"),
        Ok(_) => panic!("unexpected successful preparation below the work budget"),
        Err(error) => panic!("unexpected preparation error: {error:?}"),
    }

    let mut callbacks = 0;
    dsp::render_artifact_with_limits(&retained, &exact, |_| {
        callbacks += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(callbacks, OUTPUT_FRAMES as usize);

    callbacks = 0;
    match dsp::render_artifact_with_limits(&retained, &below, |_| {
        callbacks += 1;
        Ok(())
    }) {
        Err(RenderError::Plan(error)) => assert_eq!(error.code, "E_RESOURCE_LIMIT"),
        result => panic!("unexpected render result: {result:?}"),
    }
    assert_eq!(callbacks, 0);

    let capture_exact = limits(budget + OUTPUT_FRAMES);
    let mut captured = 0;
    dsp::render_ports_artifact_with_limits(
        &retained,
        &capture_exact,
        &[PortRef::new("sound", "out").unwrap()],
        |_| {
            captured += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(captured, OUTPUT_FRAMES as usize);

    let mut captured = 0;
    match dsp::render_ports_artifact_with_limits(
        &retained,
        &limits(budget + OUTPUT_FRAMES - 1),
        &[PortRef::new("sound", "out").unwrap()],
        |_| {
            captured += 1;
            Ok(())
        },
    ) {
        Err(RenderError::Plan(error)) => assert_eq!(error.code, "E_RESOURCE_LIMIT"),
        result => panic!("unexpected capture result: {result:?}"),
    }
    assert_eq!(captured, 0);
}
