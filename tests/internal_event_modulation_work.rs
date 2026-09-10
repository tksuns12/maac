use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::diagnostic::DiagnosticCode;
use maac::dsp::{self, DspEngine, RenderError};
use maac::plan::{PlanLimits, PortRef};
use maac::PlanArtifact;

const OUTPUT_FRAMES: u64 = 12;
const NOTE_OFF: u64 = 4;
const BASE_GRAPH_COST: u64 = 22;
const GRAPH_NODES: u64 = 6;
const PLUCK_INITIALIZATION: u64 = 2_402;
const EXPRESSION_POINTS: usize = 2;

fn source(internal: &str, public_release: bool) -> SourceBundle {
    let public_control = if public_release {
        "control release { target = &v.amp.params.release; default = 0s; }"
    } else {
        ""
    };
    let public_modulation = if public_release {
        "node public_source { type = \"core.constant/1\"; params = { value = 0; }; }\nmodulate public_release { from = &public_source:out; target = &sound.params.release; amount = 0s; }"
    } else {
        ""
    };
    let text = r#"maac 1;
project work { score = [0q, 1/4000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }
tempo clock { points = [(0q, 60bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }

instrument local {
 channels = 1;
 voice v {
  channels = 1; amplitude = &amp; output = &gain:out;
  node amp { type = "synth.adsr/1"; params = { release = 0s; }; }
  node pluck { type = "synth.pluck/1"; config = { seed = 1; }; }
  node gain { type = "synth.gain/1"; config = { channels = 1; }; }
  node pressure { type = "synth.pressure/1"; }
  node timbre { type = "synth.timbre/1"; }
  node other { type = "synth.adsr/1"; params = { release = 0s; }; }
  connect pluck_gain { from = &pluck:out; to = &gain:in; }
  INTERNAL_MODULATIONS
 }
 PUBLIC_CONTROL
}
node sound { instrument = &local; config = { voices = 1; }; }
PUBLIC_MODULATION

curve pitch_shape { clock = normalized; points = [(0, 0ct, linear), (1, 100ct, step)]; }
curve gain_shape { clock = normalized; points = [(0, 1/2, linear), (1, 1/2, step)]; }
curve timbre_shape { clock = normalized; points = [(0, 1/4, linear), (1, 3/4, step)]; }
curve pressure_shape { clock = normalized; points = [(0, 1/4, linear), (1, 3/4, step)]; }
pattern phrase {
 length = 1/4000q;
 note n {
  at = 0q; dur = 1/12000q; pitch = C4; velocity = 1;
  expression bend { kind = pitch; curve = &pitch_shape; }
  expression level { kind = gain; curve = &gain_shape; }
  expression color { kind = timbre; curve = &timbre_shape; }
  expression touch { kind = pressure; curve = &pressure_shape; }
 }
}
track notes { target = &sound:events; }
place play { pattern = &phrase; track = &notes; at = 0q; }
"#;
    SourceBundle::new(
        "work.maac",
        text.replace("INTERNAL_MODULATIONS", internal)
            .replace("PUBLIC_CONTROL", public_control)
            .replace("PUBLIC_MODULATION", public_modulation),
    )
}

fn compile(internal: &str, public_release: bool) -> PlanArtifact {
    compile_bundle_artifact_with_limits(&source(internal, public_release), &PlanLimits::song())
        .expect("work fixture compiles")
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

fn lookup_cost() -> u64 {
    17 + u64::from(usize::BITS - (EXPRESSION_POINTS - 1).leading_zeros())
}

fn expected_work(edge_count: u64, note_on: bool, note_off: bool, release_bound: bool) -> u64 {
    let graph_cost = BASE_GRAPH_COST + edge_count;
    let active_frames = if release_bound {
        OUTPUT_FRAMES
    } else {
        NOTE_OFF
    };
    let previews = u64::from(note_on) + u64::from(note_off);
    let preview_expression_cost = 3 * lookup_cost();
    active_frames * graph_cost
        + PLUCK_INITIALIZATION
        + active_frames * 4 * lookup_cost()
        + previews * (graph_cost + GRAPH_NODES + edge_count + preview_expression_cost)
}

fn assert_resource_limit(result: Result<(), maac::plan::PlanError>) {
    assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn internal_event_previews_and_expression_lookup_work_are_exact() {
    let no_edge = compile("", false);
    let attack_and_release = compile(
        "modulate attack_zero { from = &pressure:out; to = &amp.params.attack; depth = 0s; }\n  modulate release_zero { from = &pressure:out; to = &amp.params.release; depth = 0s; }",
        false,
    );

    assert_eq!(no_edge.version(), 2);
    assert_eq!(no_edge.output().total_frames, OUTPUT_FRAMES);
    assert_eq!(
        minimum_execution_budget(&no_edge),
        expected_work(0, false, false, false)
    );
    assert_eq!(
        minimum_execution_budget(&attack_and_release),
        expected_work(2, true, true, true)
    );
    assert_eq!(
        minimum_execution_budget(&attack_and_release) - minimum_execution_budget(&no_edge),
        948
    );
}

#[test]
fn internal_release_scope_handles_private_zero_unrelated_and_public_edges() {
    let attack_only = compile(
        "modulate attack_zero { from = &pressure:out; to = &amp.params.attack; depth = 0s; }",
        false,
    );
    let unrelated_release = compile(
        "modulate unrelated_release { from = &pressure:out; to = &other.params.release; depth = 0s; }",
        false,
    );
    let private_release = compile(
        "modulate private_release { from = &pressure:out; to = &amp.params.release; depth = 1ms; }",
        false,
    );
    let private_zero_release = compile(
        "modulate private_release { from = &pressure:out; to = &amp.params.release; depth = 0s; }",
        false,
    );
    let public_and_private = compile(
        "modulate private_release { from = &pressure:out; to = &amp.params.release; depth = 0s; }",
        true,
    );

    let attack_budget = minimum_execution_budget(&attack_only);
    let unrelated_budget = minimum_execution_budget(&unrelated_release);
    let private_budget = minimum_execution_budget(&private_release);
    assert_eq!(attack_budget, expected_work(1, true, false, false));
    assert_eq!(unrelated_budget, expected_work(1, false, true, false));
    assert_eq!(private_budget, expected_work(1, false, true, true));
    assert_eq!(
        minimum_execution_budget(&private_zero_release),
        private_budget
    );
    assert_eq!(attack_budget, unrelated_budget);
    assert_eq!(
        minimum_execution_budget(&public_and_private),
        private_budget + OUTPUT_FRAMES * 16
    );
}

#[test]
fn internal_event_work_is_enforced_at_prepare_render_and_capture_boundaries() {
    let internal =
        "modulate release_zero { from = &pressure:out; to = &amp.params.release; depth = 0s; }";
    let artifact = compile(internal, false);
    let budget = minimum_execution_budget(&artifact);
    let exact = limits(budget);
    let below = limits(budget - 1);

    compile_bundle_artifact_with_limits(&source(internal, false), &exact)
        .expect("source compilation validates at the exact execution budget");
    let diagnostics = compile_bundle_artifact_with_limits(&source(internal, false), &below)
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
