use std::collections::BTreeMap;

use maac::generic_lock::{
    DependencyIdentity, ExpectedAsset, ExpectedEngine, ExpectedOutput, ExpectedProcessor,
    LockVerificationContext,
};
use maac::generic_lock_normalization::{
    generate_generic_lock, GenericLockBuildContext, OutputEvidence, ResolvedEngine, ResolvedOutput,
    ResolvedProcessor,
};
use maac::generic_render::{
    binary32_le, generic_render_engine_identity, render_generic_lock, GenericRenderContext,
};
use maac::plan::{Plan, PortRef, Processor};
use maac::{compile, parse};
use serde_json::{json, Value};

const MONO: &str = r#"maac 1;
project p { score=[0q,1/32q]; rate=48000Hz; tempo=&t; meter=&m; output=&sine:out; tail=0s; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
pattern notes { length=1/32q; note n { at=0q; dur=1/64q; pitch=A4; velocity=1; } }
track tr { target=&sine:events; }
place pl { pattern=&notes; track=&tr; at=0q; }
node sine { type="core.sine/1"; config={voices=1;}; params={attack=0s;release=0s;level=1;}; }
"#;

const STEREO: &str = r#"maac 1;
project p { score=[0q,1/32q]; rate=48000Hz; tempo=&t; meter=&m; output=&pan:out; tail=0s; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
pattern notes { length=1/32q; note n { at=0q; dur=1/64q; pitch=A4; velocity=1; } }
track tr { target=&sine:events; }
place pl { pattern=&notes; track=&tr; at=0q; }
node sine { type="core.sine/1"; config={voices=1;}; params={attack=0s;release=0s;level=1;}; }
node pan { type="core.pan/1"; params={pan=1/3;}; }
connect c { from=&sine:out; to=&pan:in; }
"#;

const DELAY: &str = r#"maac 1;
project p { score=[0q,1/32q]; rate=48000Hz; tempo=&t; meter=&m; output=&delay:out; tail=0s; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
pattern notes { length=1/32q; note n { at=0q; dur=1/64q; pitch=A4; velocity=1; } }
track tr { target=&sine:events; }
place pl { pattern=&notes; track=&tr; at=0q; }
node sine { type="core.sine/1"; config={voices=1;}; params={attack=0s;release=0s;level=1;}; }
node delay { type="core.delay/1"; config={channels=1;frames=16;}; }
connect c { from=&sine:out; to=&delay:in; }
"#;

fn plan(source: &str) -> Plan {
    compile(&parse(source).unwrap()).unwrap()
}

fn processor_type(processor: &Processor) -> &'static str {
    match processor {
        Processor::Sine { .. } => "core.sine/1",
        Processor::OnePole { .. } => "core.onepole/1",
        Processor::Gain { .. } => "core.gain/1",
        Processor::Fader { .. } => "core.fader/1",
        Processor::Noise { .. } => "core.noise/1",
        Processor::Delay { .. } => "core.delay/1",
        Processor::Matrix { .. } => "core.matrix/1",
        Processor::Eq { .. } => "fx.eq/1",
        Processor::Compressor { .. } => "fx.compressor/1",
        Processor::Reverb { .. } => "fx.reverb/1",
        Processor::Pan => "core.pan/1",
        Processor::Sum { .. } => "core.sum/1",
        Processor::Instrument { .. } => panic!("fixture uses only core processors"),
    }
}

fn typed_quantity(value: &maac::Rational, unit: &str) -> Value {
    json!({
        "t": "quantity",
        "n": value.numer().to_string(),
        "d": value.denom().to_string(),
        "u": unit,
    })
}

fn resolved_engine(sample_rate: u64) -> ResolvedEngine {
    let host = generic_render_engine_identity(sample_rate);
    ResolvedEngine {
        implementation_id: host.implementation_id,
        build_id: host.build_id,
        platform_id: host.platform_id,
        architecture_id: host.architecture_id,
        numerical_mode_id: host.numerical_mode_id,
        sample_rate: host.sample_rate,
        block_schedule: host.block_schedule,
        block_independent: host.block_independent,
    }
}

fn build_context(
    plan: &Plan,
    output: &PortRef,
    crop: (u64, u64),
    order: Vec<u64>,
    evidence: Option<OutputEvidence>,
) -> GenericLockBuildContext {
    let processors = plan
        .nodes
        .iter()
        .map(|node| {
            (
                vec![node.id.clone()],
                ResolvedProcessor {
                    processor_type: processor_type(&node.processor).into(),
                    implementation_hash: None,
                    descriptor_hash: None,
                    adapter_id: None,
                    state_hash: None,
                    normalized_config: json!({"t":"record","fields":{}}),
                    latency_frames: node.processor.technical_latency_frames(),
                    determinism: "declared_deterministic".into(),
                },
            )
        })
        .collect();
    GenericLockBuildContext {
        execution_preimage: json!({"fixture":"generic-render","version":1}),
        assets: BTreeMap::new(),
        dependencies: BTreeMap::from([(
            DependencyIdentity {
                owner: None,
                role: vec!["engine".into(), "implementation".into()],
            },
            b"maac generic-render host identity".to_vec(),
        )]),
        processors,
        engine: resolved_engine(u64::from(plan.output.sample_rate_hz)),
        output: ResolvedOutput {
            port: json!({"t":"ref","path":[output.node.clone()],"port":output.port.clone()}),
            score: [
                typed_quantity(&plan.output.score_start_q, "q"),
                typed_quantity(&plan.output.score_end_q, "q"),
            ],
            tail: typed_quantity(&plan.output.tail_seconds, "s"),
            render_frames: plan.output.total_frames,
            crop,
            channel_order: order,
        },
        evidence,
    }
}

fn verification(
    context: &GenericLockBuildContext,
    lock: &maac::generic_lock::GenericLock,
) -> LockVerificationContext {
    LockVerificationContext {
        execution_preimage: context.execution_preimage.clone(),
        assets: context
            .assets
            .iter()
            .map(|(path, asset)| {
                (
                    path.clone(),
                    ExpectedAsset {
                        kind: asset.kind.clone(),
                        bytes: asset.bytes.clone(),
                    },
                )
            })
            .collect(),
        dependencies: context.dependencies.clone(),
        processors: context
            .processors
            .iter()
            .map(|(node, processor)| {
                let config = lock.value()["processors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["node"] == json!(node))
                    .unwrap()["config"]
                    .clone();
                (
                    node.clone(),
                    ExpectedProcessor {
                        processor_type: processor.processor_type.clone(),
                        implementation_hash: processor.implementation_hash.clone(),
                        descriptor_hash: processor.descriptor_hash.clone(),
                        adapter_id: processor.adapter_id.clone(),
                        state_hash: processor.state_hash.clone(),
                        config,
                        latency_frames: processor.latency_frames,
                        determinism: processor.determinism.clone(),
                    },
                )
            })
            .collect(),
        engine: ExpectedEngine {
            implementation_id: context.engine.implementation_id.clone(),
            build_id: context.engine.build_id.clone(),
            platform_id: context.engine.platform_id.clone(),
            architecture_id: context.engine.architecture_id.clone(),
            numerical_mode_id: context.engine.numerical_mode_id.clone(),
            sample_rate: context.engine.sample_rate,
            block_schedule: context.engine.block_schedule.clone(),
            block_independent: context.engine.block_independent,
        },
        output: ExpectedOutput {
            port: context.output.port.clone(),
            score: context.output.score.clone(),
            tail: context.output.tail.clone(),
            render_frames: context.output.render_frames,
            crop: context.output.crop,
            channel_order: context.output.channel_order.clone(),
        },
        pcm: context.evidence.as_ref().map(|e| e.pcm.clone()),
        file: context.evidence.as_ref().and_then(|e| e.file.clone()),
    }
}

fn render(
    plan: &Plan,
    output: PortRef,
    crop: (u64, u64),
    order: Vec<u64>,
) -> maac::generic_render::GenericRenderResult {
    let build = build_context(plan, &output, crop, order, None);
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    render_generic_lock(&generated.lock, &GenericRenderContext::new(plan, &verify)).unwrap()
}

#[test]
fn core_lock_renders_repeatably_to_raw_f32le() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let a = render(&plan, output.clone(), (0, 64), vec![0]);
    let b = render(&plan, output, (0, 64), vec![0]);
    assert_eq!(a.pcm, b.pcm);
    assert_eq!(a.pcm.len(), 64 * 4);
    assert_eq!(a.evidence.pcm_bytes, 256);
    assert_eq!(a.verified.crop, (0, 64));
}

#[test]
fn nonzero_crop_preserves_reset_origin_prehistory() {
    let plan = plan(DELAY);
    let output = PortRef::new("delay", "out").unwrap();
    let full = render(&plan, output.clone(), (0, 160), vec![0]);
    let crop = render(&plan, output, (40, 120), vec![0]);
    assert_eq!(crop.pcm, full.pcm[40 * 4..120 * 4]);
    assert!(crop.pcm.iter().any(|byte| *byte != 0));
}

#[test]
fn stereo_channel_order_is_an_exact_permutation() {
    let plan = plan(STEREO);
    let output = PortRef::new("pan", "out").unwrap();
    let natural = render(&plan, output.clone(), (0, 80), vec![0, 1]);
    let reversed = render(&plan, output, (0, 80), vec![1, 0]);
    for (a, b) in natural
        .pcm
        .chunks_exact(8)
        .zip(reversed.pcm.chunks_exact(8))
    {
        assert_eq!(&a[..4], &b[4..]);
        assert_eq!(&a[4..], &b[..4]);
    }
}

#[test]
fn generated_pcm_evidence_round_trips_without_changing_render_key() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let initial = build_context(&plan, &output, (0, 48), vec![0], None);
    let initial_lock = generate_generic_lock(&initial).unwrap();
    let initial_verify = verification(&initial, &initial_lock.lock);
    let first = render_generic_lock(
        &initial_lock.lock,
        &GenericRenderContext::new(&plan, &initial_verify),
    )
    .unwrap();

    let with_evidence = build_context(
        &plan,
        &output,
        (0, 48),
        vec![0],
        Some(OutputEvidence {
            pcm: first.pcm.clone(),
            file: Some(first.pcm.clone()),
        }),
    );
    let evidenced_lock = generate_generic_lock(&with_evidence).unwrap();
    assert_eq!(
        initial_lock.lock.render_key(),
        evidenced_lock.lock.render_key()
    );
    let evidenced_verify = verification(&with_evidence, &evidenced_lock.lock);
    let second = render_generic_lock(
        &evidenced_lock.lock,
        &GenericRenderContext::new(&plan, &evidenced_verify),
    )
    .unwrap();
    assert_eq!(second.pcm, first.pcm);
    assert!(second.verified.pcm_verified);
    assert!(second.verified.file_verified);
}

#[test]
fn tampered_pre_render_context_is_rejected_before_execution() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let build = build_context(&plan, &output, (0, 16), vec![0], None);
    let generated = generate_generic_lock(&build).unwrap();
    let mut verify = verification(&build, &generated.lock);
    verify.execution_preimage = json!({"fixture":"tampered"});
    let failure = render_generic_lock(&generated.lock, &GenericRenderContext::new(&plan, &verify))
        .unwrap_err();
    assert_eq!(failure.code, "E_DIGEST");
}

#[test]
fn host_engine_identity_is_not_caller_selectable() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let mut build = build_context(&plan, &output, (0, 16), vec![0], None);
    build.engine.implementation_id = "other.engine/1".into();
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let failure = render_generic_lock(&generated.lock, &GenericRenderContext::new(&plan, &verify))
        .unwrap_err();
    assert_eq!(failure.code, "E_CAPABILITY");
}

#[test]
fn nonnull_block_schedule_is_rejected_by_bounded_host() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let mut build = build_context(&plan, &output, (0, 16), vec![0], None);
    build.engine.block_schedule = Some(vec![plan.output.total_frames]);
    build.engine.block_independent = false;
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let failure = render_generic_lock(&generated.lock, &GenericRenderContext::new(&plan, &verify))
        .unwrap_err();
    assert_eq!(failure.code, "E_CAPABILITY");
}

#[test]
fn output_byte_limit_is_preflighted() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let build = build_context(&plan, &output, (0, 16), vec![0], None);
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let mut render = GenericRenderContext::new(&plan, &verify);
    render.max_output_bytes = 63;
    let failure = render_generic_lock(&generated.lock, &render).unwrap_err();
    assert_eq!(failure.code, "E_RESOURCE_LIMIT");
}

#[test]
fn binary32_profile_preserves_signed_zero_and_subnormals_and_rejects_bad_values() {
    assert_eq!(u32::from_le_bytes(binary32_le(0.0).unwrap()), 0x0000_0000);
    assert_eq!(u32::from_le_bytes(binary32_le(-0.0).unwrap()), 0x8000_0000);
    assert_eq!(
        u32::from_le_bytes(binary32_le(f32::from_bits(1) as f64).unwrap()),
        1
    );
    assert_eq!(binary32_le(f64::INFINITY).unwrap_err().code, "E_EVIDENCE");
    assert_eq!(binary32_le(f64::MAX).unwrap_err().code, "E_EVIDENCE");
}

#[test]
fn empty_crop_produces_empty_pcm_and_zero_length_evidence() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let result = render(&plan, output, (24, 24), vec![0]);
    assert!(result.pcm.is_empty());
    assert_eq!(result.evidence.pcm_bytes, 0);
    assert_eq!(
        result.evidence.pcm_sha256,
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn tampered_dependency_bytes_are_rejected_before_execution() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let build = build_context(&plan, &output, (0, 16), vec![0], None);
    let generated = generate_generic_lock(&build).unwrap();
    let mut verify = verification(&build, &generated.lock);
    verify.dependencies.values_mut().next().unwrap().push(b'!');
    let failure = render_generic_lock(&generated.lock, &GenericRenderContext::new(&plan, &verify))
        .unwrap_err();
    assert_eq!(failure.code, "E_CLOSURE");
}

#[test]
fn wrong_processor_context_is_rejected_before_execution() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let build = build_context(&plan, &output, (0, 16), vec![0], None);
    let generated = generate_generic_lock(&build).unwrap();
    let mut verify = verification(&build, &generated.lock);
    verify
        .processors
        .values_mut()
        .next()
        .unwrap()
        .processor_type = "core.gain/1".into();
    let failure = render_generic_lock(&generated.lock, &GenericRenderContext::new(&plan, &verify))
        .unwrap_err();
    assert_eq!(failure.code, "E_CLOSURE");
}

#[test]
fn tampered_preexisting_evidence_is_rejected_after_rendering() {
    let plan = plan(MONO);
    let output = PortRef::new("sine", "out").unwrap();
    let initial = build_context(&plan, &output, (0, 24), vec![0], None);
    let initial_lock = generate_generic_lock(&initial).unwrap();
    let initial_verify = verification(&initial, &initial_lock.lock);
    let first = render_generic_lock(
        &initial_lock.lock,
        &GenericRenderContext::new(&plan, &initial_verify),
    )
    .unwrap();

    let mut wrong_pcm = first.pcm.clone();
    wrong_pcm[0] ^= 1;
    let with_evidence = build_context(
        &plan,
        &output,
        (0, 24),
        vec![0],
        Some(OutputEvidence {
            pcm: wrong_pcm,
            file: None,
        }),
    );
    let evidenced_lock = generate_generic_lock(&with_evidence).unwrap();
    let evidenced_verify = verification(&with_evidence, &evidenced_lock.lock);
    let failure = render_generic_lock(
        &evidenced_lock.lock,
        &GenericRenderContext::new(&plan, &evidenced_verify),
    )
    .unwrap_err();
    assert_eq!(failure.code, "E_EVIDENCE");
}

#[test]
fn selected_port_channel_width_must_match_locked_order() {
    let plan = plan(STEREO);
    let output = PortRef::new("sine", "out").unwrap();
    let build = build_context(&plan, &output, (0, 16), vec![0, 1], None);
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let failure = render_generic_lock(&generated.lock, &GenericRenderContext::new(&plan, &verify))
        .unwrap_err();
    assert_eq!(failure.code, "E_REFERENCE");
}
