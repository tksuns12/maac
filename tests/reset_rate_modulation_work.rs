use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::diagnostic::DiagnosticCode;
use maac::dsp::{self, DspEngine, RenderError};
use maac::plan::{PlanLimits, PortRef};
use maac::PlanArtifact;

const OUTPUT_FRAMES: u64 = 2;
const SHARED_GRAPH_COST: u64 = 1;
const CONTROL_NODE_WEIGHT: u64 = 8;
const RESET_SOURCE_AUTOMATION_LOOKUP: u64 = 18;

fn source(reset_targets: usize) -> SourceBundle {
    assert!((0..=2).contains(&reset_targets));
    let spare = if reset_targets == 2 {
        "node spare { instrument = &local; config = { voices = 1; }; }"
    } else {
        ""
    };
    let reset_edges: String = match reset_targets {
        0 => "".into(),
        1 => "modulate reset_sound { from = &bridge:out; target = &sound.params.phase; amount = 0; }"
            .into(),
        2 => r#"modulate reset_sound { from = &bridge:out; target = &sound.params.phase; amount = 0; }
modulate reset_spare { from = &bridge:out; target = &spare.params.phase; amount = 0; }"#
            .into(),
        _ => unreachable!(),
    };
    let text = format!(
        r#"maac 1;
project reset_rate_work {{ score = [0q, 1/24000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}

instrument local {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; params = {{ attack = 0s; decay = 0s; sustain = 1; release = 0s; }}; }}
  node osc {{ type = "synth.sine/1"; params = {{ frequency = 0Hz; phase = 1/4; level = 1; }}; }}
 }}
 shared fx {{
  channels = 1; output = &lfo:out;
  node lfo {{ type = "synth.lfo/1"; params = {{ frequency = 100Hz; phase = 0; level = 1; }}; }}
 }}
 control phase {{ target = &fx.lfo.params.phase; default = 0; }}
 control level {{ target = &fx.lfo.params.level; default = 1; }}
}}
node sound {{ instrument = &local; params = {{ phase = 0; level = 1; }}; config = {{ voices = 1; }}; }}
node source {{ type = "core.constant/1"; params = {{ value = 1/4; }}; }}
node bridge {{ type = "core.constant/1"; params = {{ value = 0; }}; }}
node side {{ type = "core.constant/1"; params = {{ value = 1/2; }}; }}
{spare}
modulate source_bridge {{ from = &source:out; target = &bridge.params.value; amount = 1; }}
{reset_edges}
modulate sample_level {{ from = &side:out; target = &sound.params.level; amount = 1; }}
curve source_values {{ clock = score; points = [(0q, 1/4, step), (1q, 1/2, step)]; }}
automation source_lane {{ target = &source.params.value; curve = &source_values; at = 0q; }}
"#,
        spare = spare,
        reset_edges = reset_edges,
    );
    SourceBundle::new("reset-rate-work.maac", text)
}

fn compile(reset_targets: usize) -> PlanArtifact {
    compile_bundle_artifact_with_limits(&source(reset_targets), &PlanLimits::song())
        .expect("reset-rate resource fixture compiles")
}

fn limits(max_execution_work: u64) -> PlanLimits {
    PlanLimits {
        max_execution_work,
        ..PlanLimits::song()
    }
}

fn expected_work(reset_targets: usize) -> u64 {
    assert!((0..=2).contains(&reset_targets));
    let control_nodes = 3 * CONTROL_NODE_WEIGHT;
    let edge_count = 1u64
        + if reset_targets == 0 {
            1
        } else {
            reset_targets as u64 + 1
        };
    let per_frame = control_nodes + 8 * edge_count;
    let instances = u64::from(reset_targets == 2) + 1;
    let shared = instances * OUTPUT_FRAMES * SHARED_GRAPH_COST;
    let preparation = if reset_targets == 0 {
        0
    } else {
        // The reset closure is source -> bridge -> each Reset target. `side`
        // and its sample-rate target remain in the ordinary frame charge.
        2 * CONTROL_NODE_WEIGHT + 8 * (1 + reset_targets as u64) + RESET_SOURCE_AUTOMATION_LOOKUP
    };
    OUTPUT_FRAMES * per_frame + shared + preparation
}

fn assert_resource_limit(result: Result<(), maac::plan::PlanError>) {
    assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
}

fn assert_exact_budget(artifact: &PlanArtifact, budget: u64) {
    artifact
        .validate_with_limits(&limits(budget))
        .expect("exact execution budget validates");
    assert_resource_limit(artifact.validate_with_limits(&limits(budget - 1)));
}

#[test]
fn reset_preparation_charges_only_the_unique_control_closure() {
    let legacy = compile(0);
    let reset = compile(1);
    let reset_twice = compile(2);

    assert_eq!(legacy.output().total_frames, OUTPUT_FRAMES);
    assert_eq!(reset.output().total_frames, OUTPUT_FRAMES);
    assert_eq!(reset_twice.output().total_frames, OUTPUT_FRAMES);
    assert_eq!(expected_work(0), 82);
    assert_eq!(expected_work(1), 148);
    assert_eq!(expected_work(2), 174);
    assert_exact_budget(&legacy, expected_work(0));
    assert_exact_budget(&reset, expected_work(1));
    assert_exact_budget(&reset_twice, expected_work(2));

    // The zero-depth edge is still a Reset target. A second disconnected
    // target shares source and bridge, so only its own edge is added.
    assert_eq!(expected_work(1) - expected_work(0), 66);
    assert_eq!(expected_work(2) - expected_work(1), 26);
}

#[test]
fn reset_work_is_enforced_at_source_retained_prepare_render_and_capture_boundaries() {
    let artifact = compile(1);
    let budget = expected_work(1);
    artifact
        .validate_with_limits(&limits(budget))
        .expect("exact execution budget validates");
    let exact = limits(budget);
    let below = limits(budget - 1);

    compile_bundle_artifact_with_limits(&source(1), &exact)
        .expect("source compilation validates at exact reset work");
    let diagnostics = compile_bundle_artifact_with_limits(&source(1), &below)
        .expect_err("source compilation rejects one unit below reset work");
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
        Ok(_) => panic!("unexpected successful preparation below reset work"),
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
