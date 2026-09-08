use maac::bundle::SourceBundle;
use maac::compiler::compile_bundle;
use maac::graph::{GraphProcessor, InstrumentProgram};
use maac::voice::{CompiledInstrument, InstrumentRuntime};
use std::collections::BTreeMap;
use std::sync::Arc;

fn source(config: &str, params: &str) -> String {
    format!(
        r#"maac 1;
project song {{ score = [0q, 1/50q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; tail = 1ms; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument string {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &pluck:out;
  node amp {{ type = "synth.adsr/1"; params = {{ release = 1ms; }}; }}
  node pluck {{ type = "synth.pluck/1"; {config} {params} }}
 }}
 control ratio {{ target = &v.pluck.params.ratio; default = 1; }}
 control damping {{ target = &v.pluck.params.damping; default = 0; }}
 control decay {{ target = &v.pluck.params.decay; default = 3s; }}
 control level {{ target = &v.pluck.params.level; default = 1; }}
}}
node sound {{ instrument = &string; config = {{ voices = 4; }}; }}
pattern phrase {{ length = 1/50q; note a {{ at = 0q; dur = 1/100q; pitch = A4; velocity = 0.6; }} }}
track t {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &t; at = 0q; }}
"#
    )
}

fn plan() -> maac::Plan {
    compile_bundle(&SourceBundle::new(
        "pluck.maac",
        source("config = { seed = 1; };", ""),
    ))
    .unwrap()
}

fn program() -> InstrumentProgram {
    plan().instruments.unwrap().programs.remove(0)
}

fn runtime(program: &InstrumentProgram, voices: u32) -> maac::dsp::Result<InstrumentRuntime> {
    InstrumentRuntime::new(
        Arc::new(CompiledInstrument::compile(program, &BTreeMap::new())?),
        voices,
        48000.0,
        &BTreeMap::new(),
    )
}

fn ring(seed: u32) -> Vec<f64> {
    let mut state = seed;
    (0..2402)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f64 / 2147483648.0 - 1.0
        })
        .collect()
}

#[test]
fn source_defaults_and_strict_resolved_plan_seed() {
    for config in ["", "config = {};", "config = { seed = 1831565813; };"] {
        let plan = compile_bundle(&SourceBundle::new("pluck.maac", source(config, ""))).unwrap();
        let processor = &plan.instruments.as_ref().unwrap().programs[0]
            .voice
            .nodes
            .iter()
            .find(|n| n.id == "pluck")
            .unwrap()
            .processor;
        assert_eq!(
            serde_json::to_value(processor).unwrap(),
            serde_json::json!({"kind":"synth.pluck/1","seed":1831565813u32})
        );
    }
    for seed in [
        serde_json::Value::Null,
        0.into(),
        (-1).into(),
        (u32::MAX as u64 + 1).into(),
        1.5.into(),
        true.into(),
    ] {
        let mut value = serde_json::json!({"kind":"synth.pluck/1","seed":seed});
        if seed.is_null() {
            value.as_object_mut().unwrap().remove("seed");
        }
        assert!(serde_json::from_value::<GraphProcessor>(value).is_err());
    }
    for text in [
        r#"{"kind":"synth.pluck/1","seed":1,"channels":1}"#,
        r#"{"kind":"synth.pluck/2","seed":1}"#,
    ] {
        assert!(serde_json::from_str::<GraphProcessor>(text).is_err());
    }
}

#[test]
fn ratio_controls_change_delay_without_reinitializing_string_history() {
    let mut runtime = runtime(&program(), 1).unwrap();
    let expected = ring(1);
    runtime.note_on("note", 480.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap()[0], expected[2302]);
    runtime
        .update_controls(&[("ratio".into(), 0.5)].into_iter().collect())
        .unwrap();
    assert_eq!(runtime.render(1).unwrap()[0], expected[2203]);
    runtime
        .update_controls(&[("ratio".into(), 2.0)].into_iter().collect())
        .unwrap();
    assert_eq!(runtime.render(2).unwrap()[0], expected[2354]);
    runtime.reset(&BTreeMap::new()).unwrap();
    runtime.note_on("note", 480.0, 1.0, 0).unwrap();
    assert_eq!(runtime.render(0).unwrap()[0], expected[2302]);
}

#[test]
fn source_plan_roundtrip_repeats_exact_samples_and_releases_to_silence() {
    let plan = plan();
    let loaded = maac::load_plan(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let collect = |plan: &maac::Plan| {
        let mut out = Vec::new();
        maac::render(plan, |frame| {
            out.push(frame[0]);
            Ok(())
        })
        .unwrap();
        out
    };
    let first = collect(&plan);
    assert!(first.iter().all(|x| x.is_finite()));
    assert!(first.iter().any(|x| *x != 0.0));
    assert!(first[288..].iter().all(|x| *x == 0.0));
    assert_eq!(first, collect(&plan));
    assert_eq!(first, collect(&loaded));
}

#[test]
fn declared_delay_capacity_is_checked_for_plans_and_direct_runtimes() {
    use maac::plan::{PlanLimits, Processor};
    let mut plan = plan();
    let set_capacity = |plan: &mut maac::Plan, capacity| {
        let Processor::Instrument { voices, .. } = &mut plan.nodes[0].processor else {
            panic!("instrument")
        };
        *voices = capacity;
    };
    set_capacity(&mut plan, 3492);
    plan.validate().unwrap();
    runtime(&program(), 3492).unwrap();
    set_capacity(&mut plan, 3493);
    assert_eq!(plan.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
    assert_eq!(
        runtime(&program(), 3493).unwrap_err().code(),
        "E_RESOURCE_LIMIT"
    );
    assert_eq!(
        runtime(&program(), u32::MAX).unwrap_err().code(),
        "E_RESOURCE_LIMIT"
    );
    let oversized = source("", "").replace("voices = 4", "voices = 3493");
    assert_eq!(
        compile_bundle(&SourceBundle::new("pluck.maac", oversized))
            .unwrap_err()
            .first()
            .unwrap()
            .code
            .as_str(),
        "E_RESOURCE_LIMIT"
    );
    let plan = self::plan();
    let exact = PlanLimits {
        max_pluck_delay_cells: 4 * 2402,
        ..PlanLimits::default()
    };
    plan.validate_with_limits(&exact).unwrap();
    let short = PlanLimits {
        max_pluck_delay_cells: 4 * 2402 - 1,
        ..exact
    };
    assert_eq!(
        plan.validate_with_limits(&short).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );
    let relaxed = PlanLimits {
        max_pluck_delay_cells: usize::MAX,
        ..PlanLimits::default()
    };
    let mut oversized = plan.clone();
    set_capacity(&mut oversized, 3493);
    assert_eq!(
        oversized.validate_with_limits(&relaxed).unwrap_err().code,
        "E_RESOURCE_LIMIT"
    );

    // The direct Rust API previously accepted large lazy non-pluck capacities.
    let mut old = program();
    old.controls.clear();
    old.voice
        .nodes
        .iter_mut()
        .find(|n| n.id == "pluck")
        .unwrap()
        .processor = GraphProcessor::Noise { seed: 1 };
    runtime(&old, u32::MAX).unwrap();
    let mut old_plan = plan.clone();
    old_plan.instruments.as_mut().unwrap().programs[0] = old;
    old_plan.nodes[0].params.clear();
    old_plan
        .validate_with_limits(&PlanLimits {
            max_pluck_delay_cells: 0,
            ..PlanLimits::default()
        })
        .unwrap();
}

#[test]
fn unused_pluck_nodes_and_multiple_instances_count_toward_memory() {
    use maac::plan::{PlanLimits, Processor};
    let base = source("config = { seed = 1; };", "");
    let two = base.replace(
        "node amp {",
        "node unused { type = \"synth.pluck/1\"; } node amp {",
    );
    let plan = compile_bundle(&SourceBundle::new("pluck.maac", two)).unwrap();
    let exact = PlanLimits {
        max_pluck_delay_cells: 4 * 2 * 2402,
        ..PlanLimits::default()
    };
    plan.validate_with_limits(&exact).unwrap();
    assert_eq!(
        plan.validate_with_limits(&PlanLimits {
            max_pluck_delay_cells: 4 * 2402,
            ..exact
        })
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );
    let mut multiple = self::plan();
    let mut extra = multiple.nodes[0].clone();
    extra.id = "extra".into();
    let Processor::Instrument { voices, .. } = &mut extra.processor else {
        panic!("instrument")
    };
    *voices = 3490;
    multiple.nodes.push(extra);
    assert_eq!(multiple.validate().unwrap_err().code, "E_RESOURCE_LIMIT");
}

#[test]
fn work_counts_weighted_visits_and_initialization_even_for_muted_notes() {
    use maac::plan::PlanLimits;
    let source = source("", "").replace("velocity = 0.6", "velocity = 0");
    let plan = compile_bundle(&SourceBundle::new("pluck.maac", source)).unwrap();
    // One 240-frame gate + 48-frame release; ADSR1 + pluck16; initialize2402.
    let cost = 288 * 17 + 2402;
    plan.validate_with_limits(&PlanLimits {
        max_execution_work: cost,
        ..PlanLimits::default()
    })
    .unwrap();
    assert_eq!(
        plan.validate_with_limits(&PlanLimits {
            max_execution_work: cost - 1,
            ..PlanLimits::default()
        })
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );
    assert_eq!(
        plan.validate_with_limits(&PlanLimits {
            max_execution_work: 2401,
            ..PlanLimits::default()
        })
        .unwrap_err()
        .code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn strict_parameter_descriptors_and_source_wire_typed_boundaries_agree() {
    use maac::graph::{parameter_descriptor, GraphUnit, ParameterRate};
    let processor = GraphProcessor::Pluck { seed: 1 };
    for (name, unit, default, min, max) in [
        ("ratio", GraphUnit::Dimensionless, "1", "1/8", "8"),
        ("decay", GraphUnit::Seconds, "3", "1/20", "30"),
        ("damping", GraphUnit::Dimensionless, "1/2", "0", "1"),
        ("level", GraphUnit::Dimensionless, "1", "0", "16"),
    ] {
        let spec = parameter_descriptor(&processor, name).unwrap();
        assert_eq!(spec.unit, unit);
        assert_eq!(spec.rate, ParameterRate::Sample);
        assert_eq!(spec.default, maac::parse_rational(default).unwrap());
        assert_eq!(spec.min, maac::parse_rational(min).unwrap());
        assert_eq!(spec.max, maac::parse_rational(max).unwrap());
        assert!(!spec.min_open && !spec.max_open);
    }
    for params in [
        "ratio = 0;",
        "ratio = -1;",
        "ratio = 9;",
        "ratio = 0.124;",
        "decay = 0s;",
        "decay = 31s;",
        "decay = 3;",
        "damping = 1.01;",
        "damping = -0.1;",
        "level = 17;",
        "phase = 0;",
        "frequency = 0Hz;",
    ] {
        assert!(
            compile_bundle(&SourceBundle::new(
                "pluck.maac",
                source("", &format!("params = {{ {params} }};"))
            ))
            .is_err(),
            "{params}"
        );
    }
    for seed in ["0", "-1", "4294967296", "1/2", "1Hz", "true"] {
        assert!(
            compile_bundle(&SourceBundle::new(
                "pluck.maac",
                source(&format!("config = {{ seed = {seed}; }};"), "")
            ))
            .is_err(),
            "{seed}"
        );
    }
    assert!(compile_bundle(&SourceBundle::new(
        "pluck.maac",
        source("config = { seed = 1; extra = 0; };", "")
    ))
    .is_err());
    let mut typed = plan();
    typed.instruments.as_mut().unwrap().programs[0]
        .voice
        .nodes
        .iter_mut()
        .find(|n| n.id == "pluck")
        .unwrap()
        .processor = GraphProcessor::Pluck { seed: 0 };
    assert_eq!(typed.validate().unwrap_err().code, "E_RANGE");
    let mut value = serde_json::to_value(plan()).unwrap();
    let node = value["instruments"]["programs"][0]["voice"]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["id"] == "pluck")
        .unwrap();
    node["processor"].as_object_mut().unwrap().remove("seed");
    assert!(maac::load_plan(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn pluck_remains_voice_only_inputless_and_subject_to_dag_validation() {
    let base = source("", "");
    for extra in [
        "connect input { from = &amp:out; to = &pluck:in; }",
        "modulate loop { from = &pluck:out; to = &pluck.params.ratio; depth = 1; }",
        "node amp_gain { type = \"synth.gain/1\"; config = { channels = 1; }; } connect forward { from = &pluck:out; to = &amp_gain:in; } modulate backward { from = &amp_gain:out; to = &pluck.params.ratio; depth = 1; }",
    ] {
        let invalid = base.replace("node amp {", &format!("{extra} node amp {{"));
        assert!(compile_bundle(&SourceBundle::new("pluck.maac", invalid)).is_err(), "{extra}");
    }
    let mut program = program();
    let mut shared = program.voice.clone();
    shared.amplitude = None;
    shared.nodes.retain(|node| node.id == "pluck");
    program.shared = Some(shared);
    assert_eq!(program.validate().unwrap_err().code, "E_CAPABILITY");
}

#[test]
fn invalid_evaluated_frequency_does_not_advance_the_affected_node() {
    let program = program();
    for pitch in [19.0, 4001.0, -1.0, 0.0] {
        let mut runtime = runtime(&program, 1).unwrap();
        runtime.note_on("note", pitch, 0.0, 0).unwrap();
        assert_eq!(runtime.render(0).unwrap_err().code(), "E_NONFINITE");
    }
    let expected = ring(1);
    let mut failing = self::runtime(&program, 1).unwrap();
    failing.note_on("note", 600.0, 1.0, 0).unwrap();
    failing
        .update_controls(&[("ratio".into(), 8.0)].into_iter().collect())
        .unwrap();
    assert_eq!(failing.render(0).unwrap_err().code(), "E_NONFINITE");
    failing.update_controls(&BTreeMap::new()).unwrap();
    assert_eq!(failing.render(0).unwrap()[0], expected[2402 - 80]);
}

#[test]
fn output_mute_velocity_and_overlapping_voices_preserve_independent_history() {
    let program = program();
    let mut audible = runtime(&program, 2).unwrap();
    let mut muted = runtime(&program, 2).unwrap();
    for r in [&mut audible, &mut muted] {
        r.note_on("a", 480.0, 0.5, 0).unwrap();
    }
    muted
        .update_controls(&[("level".into(), 0.0)].into_iter().collect())
        .unwrap();
    for frame in 0..110 {
        audible.render(frame).unwrap();
        assert_eq!(muted.render(frame).unwrap()[0], 0.0);
    }
    muted.update_controls(&BTreeMap::new()).unwrap();
    for frame in 110..120 {
        assert_eq!(audible.render(frame).unwrap(), muted.render(frame).unwrap());
    }
    let mut solo = runtime(&program, 1).unwrap();
    solo.note_on("new", 240.0, 0.25, 120).unwrap();
    muted.note_on("b", 240.0, 0.25, 120).unwrap();
    for frame in 120..130 {
        let old = audible.render(frame).unwrap()[0];
        let new = solo.render(frame).unwrap()[0];
        assert_eq!(muted.render(frame).unwrap()[0], old + new);
    }
    muted.note_off("a", 130).unwrap();
    muted.prune_finished(177).unwrap();
    assert_eq!(muted.active_voice_count(), 2);
    muted.prune_finished(178).unwrap();
    assert_eq!(muted.active_voice_count(), 1);
}

#[test]
fn source_ratio_automation_and_modulation_reach_existing_active_voices() {
    let base = source("config = { seed = 1; };", "");
    let automation = r#"
curve bend { clock = seconds; points = [(0s, 1, step), (1ms, 2, step)]; }
automation bend_string { target = &sound.params.ratio; curve = &bend; at = 0s; }
"#;
    let automated =
        compile_bundle(&SourceBundle::new("pluck.maac", base.clone() + automation)).unwrap();
    let mut direct = runtime(&program(), 1).unwrap();
    direct.note_on("note", 440.0, 0.6, 0).unwrap();
    let mut frame = 0;
    maac::render(&automated, |sample| {
        if frame == 48 {
            direct.update_controls(&[("ratio".into(), 2.0)].into_iter().collect())?;
        }
        if frame == 240 {
            direct.note_off("note", frame)?;
        }
        direct.prune_finished(frame)?;
        assert_eq!(sample, direct.render(frame)?);
        frame += 1;
        Ok(())
    })
    .unwrap();

    let modulated_source = base.replace(
        "node amp {",
        r#"node bend { type = "synth.lfo/1"; params = { frequency = 0Hz; phase = 0.25; }; }
modulate bend_ratio { from = &bend:out; to = &pluck.params.ratio; depth = 1; }
node amp {"#,
    );
    let modulated = compile_bundle(&SourceBundle::new("pluck.maac", modulated_source)).unwrap();
    let mut direct = runtime(&program(), 1).unwrap();
    direct
        .update_controls(&[("ratio".into(), 2.0)].into_iter().collect())
        .unwrap();
    direct.note_on("note", 440.0, 0.6, 0).unwrap();
    let mut frame = 0;
    maac::render(&modulated, |sample| {
        if frame == 240 {
            direct.note_off("note", frame)?;
        }
        direct.prune_finished(frame)?;
        assert_eq!(sample, direct.render(frame)?);
        frame += 1;
        Ok(())
    })
    .unwrap();
}
