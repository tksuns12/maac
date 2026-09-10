use maac::bundle::SourceBundle;
use maac::compiler::{compile_bundle_artifact_with_limits, compile_bundle_with_limits};
use maac::dsp::{self, RenderError};
use maac::plan::{PlanLimits, PortRef};
use maac::{Plan, PlanArtifact};

const CALLBACK_ERROR: &str = "stop after preparation";

fn source(control: &str, modulation: &str) -> SourceBundle {
    SourceBundle::new(
        "score.maac",
        format!(
            r#"maac 1;
project p {{ score = [0q, 1q]; tail = 1799s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument string {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &pluck:out;
  node amp {{ type = "synth.adsr/1"; params = {{ release = 0s; }}; }}
  node pluck {{ type = "synth.pluck/1"; config = {{ seed = 1; }}; }}
 }}
 {control}
}}
node sound {{ instrument = &string; config = {{ voices = 1; }}; }}
node source {{ type = "core.constant/1"; }}
curve dynamics {{ clock = seconds; points = [(0s, 1, step)]; }}
pattern phrase {{ length = 1q; note n {{ at = 0q; dur = 1/2q; pitch = A4; velocity = 1; expression g {{ kind = gain; curve = &dynamics; }} }} }}
track notes {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &notes; at = 0q; }}
{modulation}
"#,
            control = control,
            modulation = modulation,
        ),
    )
}

fn matching_release() -> SourceBundle {
    source(
        "control release { target = &v.amp.params.release; default = 0s; }",
        "modulate release_tail { from = &source:out; target = &sound.params.release; amount = 0s; }",
    )
}

fn attack_only() -> SourceBundle {
    source(
        "control attack { target = &v.amp.params.attack; default = 0s; }",
        "modulate attack_only { from = &source:out; target = &sound.params.attack; amount = 0s; }",
    )
}

fn unrelated_release() -> SourceBundle {
    source(
        "control release { target = &v.pluck.params.decay; default = 3s; }",
        "modulate unrelated_release { from = &source:out; target = &sound.params.release; amount = 0s; }",
    )
}

fn no_modulation() -> SourceBundle {
    source("", "")
}

fn legacy_no_modulation() -> SourceBundle {
    let mut bundle = no_modulation();
    let text = bundle.sources.get_mut("score.maac").unwrap();
    *text = text.replace("node source { type = \"core.constant/1\"; }", "");
    bundle
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
        .expect("minimum execution budget must validate");
    low
}

fn minimum_legacy_execution_budget(plan: &Plan) -> u64 {
    let mut low = 0;
    let mut high = PlanLimits::default().max_execution_work;
    while low < high {
        let middle = low + (high - low) / 2;
        if plan.validate_with_limits(&limits(middle)).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    plan.validate_with_limits(&limits(low))
        .expect("minimum legacy execution budget must validate");
    low
}

fn assert_resource_limit(result: Result<(), maac::plan::PlanError>) {
    assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn release_modulation_is_charged_through_source_retained_render_and_capture_boundaries() {
    let bundle = matching_release();
    let artifact = compile_bundle_artifact_with_limits(&bundle, &PlanLimits::song()).unwrap();
    let budget = minimum_execution_budget(&artifact);
    assert!(budget > 0);

    compile_bundle_artifact_with_limits(&bundle, &limits(budget)).unwrap();
    assert_eq!(
        compile_bundle_artifact_with_limits(&bundle, &limits(budget - 1))
            .unwrap_err()
            .first()
            .unwrap()
            .code,
        maac::diagnostic::DiagnosticCode::ResourceLimit
    );

    let bytes = artifact.to_json_with_limits(&limits(budget)).unwrap();
    let retained = PlanArtifact::from_json_with_limits(&bytes, &limits(budget)).unwrap();
    assert_eq!(
        retained.to_json_with_limits(&limits(budget)).unwrap(),
        bytes
    );
    assert_resource_limit(
        artifact
            .to_json_with_limits(&limits(budget - 1))
            .map(|_| ()),
    );
    assert_resource_limit(
        PlanArtifact::from_json_with_limits(&bytes, &limits(budget - 1)).map(|_| ()),
    );

    let callback_error = RenderError::Callback(CALLBACK_ERROR.into());
    assert_eq!(
        dsp::render_artifact_with_limits(&retained, &limits(budget), |_| {
            Err(callback_error.clone())
        })
        .unwrap_err(),
        callback_error
    );
    match dsp::render_artifact_with_limits(&retained, &limits(budget - 1), |_| {
        panic!("render callback must not run below the execution budget")
    }) {
        Err(RenderError::Plan(error)) => assert_eq!(error.code, "E_RESOURCE_LIMIT"),
        result => panic!("unexpected render result: {result:?}"),
    }

    let capture_budget = budget + retained.output().total_frames;
    let capture_limits = limits(capture_budget);
    assert_eq!(
        dsp::render_ports_artifact_with_limits(
            &retained,
            &capture_limits,
            &[PortRef::new("sound", "out").unwrap()],
            |_| Err(callback_error.clone()),
        )
        .unwrap_err(),
        callback_error
    );
    match dsp::render_ports_artifact_with_limits(
        &retained,
        &limits(capture_budget - 1),
        &[PortRef::new("sound", "out").unwrap()],
        |_| panic!("capture callback must not run below the capture budget"),
    ) {
        Err(RenderError::Plan(error)) => assert_eq!(error.code, "E_RESOURCE_LIMIT"),
        result => panic!("unexpected capture result: {result:?}"),
    }
}

#[test]
fn only_a_matching_amplitude_release_modulation_expands_the_voice_window() {
    let song = PlanLimits::song();
    let matching = compile_bundle_artifact_with_limits(&matching_release(), &song).unwrap();
    let attack = compile_bundle_artifact_with_limits(&attack_only(), &song).unwrap();
    let unrelated = compile_bundle_artifact_with_limits(&unrelated_release(), &song).unwrap();
    let no_edge = compile_bundle_artifact_with_limits(&no_modulation(), &song).unwrap();

    let matching_budget = minimum_execution_budget(&matching);
    let attack_budget = minimum_execution_budget(&attack);
    let unrelated_budget = minimum_execution_budget(&unrelated);
    let no_edge_budget = minimum_execution_budget(&no_edge);
    let total_frames = matching.output().total_frames;
    assert_eq!(total_frames, 86_400_000);
    let base_active_frames = 24_000;
    let per_active_frame_cost = 1 + maac::graph::PLUCK_SAMPLE_WORK + 17;
    let expected_release_delta = (total_frames - base_active_frames) * per_active_frame_cost;

    assert_eq!(matching_budget, attack_budget + expected_release_delta);
    assert!(matching_budget > unrelated_budget);
    assert_eq!(attack_budget, unrelated_budget);
    assert_eq!(attack_budget, no_edge_budget + total_frames * 8);
}

#[test]
fn legacy_no_edge_keeps_the_base_release_window() {
    let plan = compile_bundle_with_limits(&legacy_no_modulation(), &PlanLimits::default()).unwrap();
    let budget = minimum_legacy_execution_budget(&plan);
    let expected = 24_000 * (1 + maac::graph::PLUCK_SAMPLE_WORK + 17) + 2_402;

    assert_eq!(budget, expected);
    assert_resource_limit(plan.validate_with_limits(&limits(budget - 1)).map(|_| ()));
}
