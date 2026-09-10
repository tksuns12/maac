use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::diagnostic::DiagnosticCode;
use maac::dsp::{self, DspEngine, RenderError};
use maac::plan::{PlanLimits, PortRef};
use maac::PlanArtifact;

const OUTPUT_FRAMES: u64 = 12;
const NOTE_OFF: u64 = 4;
const BASE_GRAPH_COST: u64 = 6;
const GRAPH_NODES: u64 = 5;
const EXPRESSION_POINTS: usize = 2;

fn source(internal: &str) -> SourceBundle {
    let text = r#"maac 1;
project work { score = [0q, 1/4000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }
tempo clock { points = [(0q, 60bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }

instrument local {
 channels = 1;
 voice v {
  channels = 1; amplitude = &amp; output = &gain:out;
  node amp { type = "synth.adsr/1"; params = { release = 0s; }; }
  node carrier { type = "synth.sine/1"; }
  node gain { type = "synth.gain/1"; config = { channels = 1; }; }
  node pressure { type = "synth.pressure/1"; }
  node timbre { type = "synth.timbre/1"; }
  connect carrier_gain { from = &carrier:out; to = &gain:in; }
  INTERNAL_MODULATIONS
 }
}
node sound { instrument = &local; config = { voices = 1; }; }

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
    SourceBundle::new("work.maac", text.replace("INTERNAL_MODULATIONS", internal))
}

fn compile(internal: &str) -> PlanArtifact {
    compile_bundle_artifact_with_limits(&source(internal), &PlanLimits::song())
        .expect("phase work fixture compiles")
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

fn expected_work(edge_count: u64, note_on_preview: bool) -> u64 {
    let graph_cost = BASE_GRAPH_COST + edge_count;
    let active_frames = NOTE_OFF;
    let preview = if note_on_preview {
        graph_cost + GRAPH_NODES + edge_count + 3 * lookup_cost()
    } else {
        0
    };
    active_frames * graph_cost + active_frames * 4 * lookup_cost() + preview
}

fn assert_resource_limit(result: Result<(), maac::plan::PlanError>) {
    assert_eq!(result.unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn phase_preview_uses_one_full_graph_charge_and_does_not_extend_release() {
    let no_edge = compile("");
    let phase_only = compile(
        "modulate phase_zero { from = &pressure:out; to = &carrier.params.phase; depth = 0; }",
    );
    let phase_and_attack = compile(
        "modulate phase_zero { from = &pressure:out; to = &carrier.params.phase; depth = 0; }\n  modulate attack_zero { from = &pressure:out; to = &amp.params.attack; depth = 0s; }",
    );

    assert_eq!(no_edge.version(), 2);
    assert_eq!(no_edge.output().total_frames, OUTPUT_FRAMES);
    assert_eq!(minimum_execution_budget(&no_edge), expected_work(0, false));
    assert_eq!(
        minimum_execution_budget(&phase_only),
        expected_work(1, true)
    );
    assert_eq!(
        minimum_execution_budget(&phase_and_attack),
        expected_work(2, true)
    );
    // Adding another NoteOn target changes G and the graph sweep size, but it
    // does not schedule a second preview or a release-tail charge.
    assert_eq!(
        minimum_execution_budget(&phase_and_attack) - minimum_execution_budget(&phase_only),
        6
    );
}

#[test]
fn phase_work_is_enforced_at_source_retained_prepare_render_and_capture_boundaries() {
    let internal =
        "modulate phase_zero { from = &pressure:out; to = &carrier.params.phase; depth = 0; }";
    let artifact = compile(internal);
    let budget = minimum_execution_budget(&artifact);
    let exact = limits(budget);
    let below = limits(budget - 1);

    compile_bundle_artifact_with_limits(&source(internal), &exact)
        .expect("source compilation validates at the exact execution budget");
    let diagnostics = compile_bundle_artifact_with_limits(&source(internal), &below)
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
