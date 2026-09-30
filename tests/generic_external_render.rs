use std::collections::BTreeMap;

use maac::external::{DiscoveredExternalProcessor, ExternalError, ExternalPermissions};
use maac::external_host::{
    ExternalAbiAdapter, ExternalHost, ExternalParameterOverride, ExternalProcessRequest,
    ExternalProcessorInstance,
};
use maac::generic_external_render::{
    generic_external_render_engine_identity, render_external_generator_lock,
    GenericExternalRenderContext,
};
use maac::generic_lock::{
    DependencyIdentity, ExpectedEngine, ExpectedOutput, ExpectedProcessor, LockVerificationContext,
};
use maac::generic_lock_normalization::{
    generate_generic_lock, GenericLockBuildContext, OutputEvidence, ResolvedEngine, ResolvedOutput,
    ResolvedProcessor,
};
use maac::production_identity::canonical_json_bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ABI: &str = "test.generator.abi/1";
const ADAPTER: &str = "test.generator.adapter/1";

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn descriptor_bytes_with(parameter_rate: &str, latency_frames: u64) -> Vec<u8> {
    canonical_json_bytes(&json!({
        "format":"maac.external-processor-descriptor",
        "wire_version":1,
        "id":"test.generator/1",
        "version":"1.0.0",
        "abi":ABI,
        "ports":[{
            "name":"out","direction":"output","kind":"audio","channels":2,
            "cardinality":null,"zero_default":null,"empty_event_stream":null
        }],
        "events":{
            "native_notes":false,"expression_kinds":[],"hit_keys":[],"message_protocols":[]
        },
        "config":[],
        "parameters":[{
            "id":"scale","source_name":"scale","unit":"1",
            "default":1,"minimum":1,"maximum":8,"range_policy":"error"
        }],
        "parameter_rates":[{
            "parameter_id":"scale","rate":parameter_rate,"interpolation":["step"]
        }],
        "feedthrough":[],
        "latency_frames":latency_frames,
        "state":{
            "serialization_version":"test.generator.state/1",
            "reset_semantics":"reset_frame_zero",
            "save_restore":false
        },
        "determinism":{"mode":"declared_deterministic","dependencies":[]},
        "permissions":{
            "assets":false,"network":false,"devices":false,
            "process_execution":false,"shared_memory":false
        }
    }))
    .unwrap()
}

fn build_context_with(parameter_rate: &str, latency_frames: u64) -> GenericLockBuildContext {
    let descriptor = descriptor_bytes_with(parameter_rate, latency_frames);
    let implementation = b"test-generator-implementation/1".to_vec();
    let adapter = b"test-generator-adapter/1".to_vec();
    let node = vec!["gen".to_owned()];
    let engine = generic_external_render_engine_identity(48_000);
    let dependencies = BTreeMap::from([
        (
            DependencyIdentity {
                owner: None,
                role: vec!["engine".into(), "implementation".into()],
            },
            b"maac generic external renderer".to_vec(),
        ),
        (
            DependencyIdentity {
                owner: Some(node.clone()),
                role: vec!["processor".into(), "implementation".into()],
            },
            implementation.clone(),
        ),
        (
            DependencyIdentity {
                owner: Some(node.clone()),
                role: vec!["processor".into(), "descriptor".into()],
            },
            descriptor.clone(),
        ),
        (
            DependencyIdentity {
                owner: Some(node.clone()),
                role: vec!["processor".into(), "adapter".into()],
            },
            adapter,
        ),
    ]);
    GenericLockBuildContext {
        execution_preimage: json!({
            "fixture":"single-external-generator",
            "node":"gen",
            "output":"out"
        }),
        assets: BTreeMap::new(),
        dependencies,
        processors: BTreeMap::from([(
            node,
            ResolvedProcessor {
                processor_type: "test.generator/1".into(),
                implementation_hash: Some(sha(&implementation)),
                descriptor_hash: Some(sha(&descriptor)),
                adapter_id: Some(ADAPTER.into()),
                state_hash: None,
                normalized_config: json!({"t":"record","fields":{}}),
                latency_frames,
                determinism: "declared_deterministic".into(),
            },
        )]),
        engine: ResolvedEngine {
            implementation_id: engine.implementation_id,
            build_id: engine.build_id,
            platform_id: engine.platform_id,
            architecture_id: engine.architecture_id,
            numerical_mode_id: engine.numerical_mode_id,
            sample_rate: engine.sample_rate,
            block_schedule: None,
            block_independent: true,
        },
        output: ResolvedOutput {
            port: json!({"t":"ref","path":["gen"],"port":"out"}),
            score: [
                json!({"t":"quantity","n":"0","d":"1","u":"q"}),
                json!({"t":"quantity","n":"1","d":"1","u":"q"}),
            ],
            tail: json!({"t":"quantity","n":"0","d":"1","u":"s"}),
            render_frames: 8,
            crop: (2, 6),
            channel_order: vec![1, 0],
        },
        evidence: None,
    }
}

fn build_context() -> GenericLockBuildContext {
    build_context_with("static", 0)
}

fn verification(
    build: &GenericLockBuildContext,
    lock: &maac::generic_lock::GenericLock,
) -> LockVerificationContext {
    LockVerificationContext {
        execution_preimage: build.execution_preimage.clone(),
        assets: BTreeMap::new(),
        dependencies: build.dependencies.clone(),
        processors: build
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
                        state_hash: None,
                        config,
                        latency_frames: processor.latency_frames,
                        determinism: "declared_deterministic".into(),
                    },
                )
            })
            .collect(),
        engine: ExpectedEngine {
            implementation_id: build.engine.implementation_id.clone(),
            build_id: build.engine.build_id.clone(),
            platform_id: build.engine.platform_id.clone(),
            architecture_id: build.engine.architecture_id.clone(),
            numerical_mode_id: build.engine.numerical_mode_id.clone(),
            sample_rate: build.engine.sample_rate,
            block_schedule: None,
            block_independent: true,
        },
        output: ExpectedOutput {
            port: build.output.port.clone(),
            score: build.output.score.clone(),
            tail: build.output.tail.clone(),
            render_frames: build.output.render_frames,
            crop: build.output.crop,
            channel_order: build.output.channel_order.clone(),
        },
        pcm: None,
        file: None,
    }
}

struct GeneratorAdapter;

impl ExternalAbiAdapter for GeneratorAdapter {
    fn abi_id(&self) -> &str {
        ABI
    }

    fn adapter_id(&self) -> &str {
        ADAPTER
    }

    fn block_independent(&self) -> bool {
        true
    }

    fn instantiate(
        &self,
        processor: &DiscoveredExternalProcessor,
        sample_rate: u64,
    ) -> Result<Box<dyn ExternalProcessorInstance>, ExternalError> {
        assert_eq!(sample_rate, 48_000);
        assert_eq!(
            processor.dependencies.implementation,
            b"test-generator-implementation/1"
        );
        Ok(Box::new(GeneratorInstance {
            frame: 0,
            scale: 1.0,
        }))
    }
}

struct GeneratorInstance {
    frame: u64,
    scale: f64,
}

impl ExternalProcessorInstance for GeneratorInstance {
    fn restore_state(&mut self, _: &[u8]) -> Result<(), ExternalError> {
        Err(ExternalError {
            code: "E_CAPABILITY",
            message: "fixture has no state".into(),
        })
    }

    fn apply_config(&mut self, config: &Value) -> Result<(), ExternalError> {
        assert_eq!(config, &json!({"t":"record","fields":{}}));
        Ok(())
    }

    fn set_parameter(&mut self, id: &str, value: &Value) -> Result<(), ExternalError> {
        assert_eq!(id, "scale");
        self.scale = value.as_u64().unwrap() as f64;
        Ok(())
    }

    fn process(&mut self, request: ExternalProcessRequest<'_>) -> Result<(), ExternalError> {
        assert!(request.audio_inputs.is_empty());
        assert_eq!(request.audio_outputs.len(), 2);
        for local in 0..request.frames {
            let value = (self.frame + local as u64) as f64 * self.scale;
            request.audio_outputs[0][local] = value;
            request.audio_outputs[1][local] = -value;
        }
        self.frame += request.frames as u64;
        Ok(())
    }
}

fn host() -> ExternalHost {
    let mut host = ExternalHost::new(ExternalPermissions::default(), false);
    host.register(Box::new(GeneratorAdapter)).unwrap();
    host
}

struct BlockDependentAdapter;

impl ExternalAbiAdapter for BlockDependentAdapter {
    fn abi_id(&self) -> &str {
        ABI
    }
    fn adapter_id(&self) -> &str {
        ADAPTER
    }
    fn instantiate(
        &self,
        _: &DiscoveredExternalProcessor,
        _: u64,
    ) -> Result<Box<dyn ExternalProcessorInstance>, ExternalError> {
        panic!("block-dependent adapter must be rejected before instantiation")
    }
}

struct PreflightAdapter;

impl ExternalAbiAdapter for PreflightAdapter {
    fn abi_id(&self) -> &str {
        ABI
    }
    fn adapter_id(&self) -> &str {
        ADAPTER
    }
    fn block_independent(&self) -> bool {
        true
    }
    fn instantiate(
        &self,
        _: &DiscoveredExternalProcessor,
        _: u64,
    ) -> Result<Box<dyn ExternalProcessorInstance>, ExternalError> {
        panic!("resource preflight must reject before instantiation")
    }
}

#[test]
fn verified_external_generator_renders_from_reset_then_crops_and_reorders() {
    let build = build_context();
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let overrides = [ExternalParameterOverride {
        id: "scale".into(),
        value: json!(2),
    }];
    let host = host();
    let mut context =
        GenericExternalRenderContext::new(&verify, &build.dependencies, &host, &overrides);
    context.chunk_frames = 2;

    let result = render_external_generator_lock(&generated.lock, &context).unwrap();
    let expected = [-4.0f32, 4.0, -6.0, 6.0, -8.0, 8.0, -10.0, 10.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    assert_eq!(result.pcm, expected);
    assert_eq!(result.evidence.pcm_bytes, expected.len() as u64);
    assert_eq!(result.verified.crop, (2, 6));
}

#[test]
fn engine_identity_and_output_budget_fail_before_external_processing() {
    let build = build_context();
    let generated = generate_generic_lock(&build).unwrap();
    let overrides = [ExternalParameterOverride {
        id: "scale".into(),
        value: json!(2),
    }];
    let host = host();

    let mut wrong_build = build_context();
    wrong_build.engine.implementation_id = "other.engine/1".into();
    let wrong_generated = generate_generic_lock(&wrong_build).unwrap();
    let wrong_verify = verification(&wrong_build, &wrong_generated.lock);
    let context = GenericExternalRenderContext::new(
        &wrong_verify,
        &wrong_build.dependencies,
        &host,
        &overrides,
    );
    assert_eq!(
        render_external_generator_lock(&wrong_generated.lock, &context)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );

    let verify = verification(&build, &generated.lock);
    let mut context =
        GenericExternalRenderContext::new(&verify, &build.dependencies, &host, &overrides);
    context.max_output_bytes = 31;
    assert_eq!(
        render_external_generator_lock(&generated.lock, &context)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn all_descriptor_parameters_must_be_resolved_before_instantiation() {
    let build = build_context();
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let host = host();
    let context = GenericExternalRenderContext::new(&verify, &build.dependencies, &host, &[]);
    assert_eq!(
        render_external_generator_lock(&generated.lock, &context)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
}

#[test]
fn latency_and_nonstatic_parameter_rates_are_rejected() {
    for build in [
        build_context_with("static", 1),
        build_context_with("sample", 0),
    ] {
        let generated = generate_generic_lock(&build).unwrap();
        let verify = verification(&build, &generated.lock);
        let overrides = [ExternalParameterOverride {
            id: "scale".into(),
            value: json!(2),
        }];
        let host = host();
        let context =
            GenericExternalRenderContext::new(&verify, &build.dependencies, &host, &overrides);
        assert_eq!(
            render_external_generator_lock(&generated.lock, &context)
                .unwrap_err()
                .code,
            "E_CAPABILITY"
        );
    }
}

#[test]
fn working_buffer_budget_fails_before_instantiation() {
    let build = build_context();
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let overrides = [ExternalParameterOverride {
        id: "scale".into(),
        value: json!(2),
    }];
    let mut host = ExternalHost::new(ExternalPermissions::default(), false);
    host.register(Box::new(PreflightAdapter)).unwrap();
    let mut context =
        GenericExternalRenderContext::new(&verify, &build.dependencies, &host, &overrides);
    context.max_output_bytes = 64;
    assert_eq!(
        render_external_generator_lock(&generated.lock, &context)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );
}

#[test]
fn block_dependent_adapter_is_rejected_before_instantiation() {
    let build = build_context();
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let overrides = [ExternalParameterOverride {
        id: "scale".into(),
        value: json!(2),
    }];
    let mut host = ExternalHost::new(ExternalPermissions::default(), false);
    host.register(Box::new(BlockDependentAdapter)).unwrap();
    let context =
        GenericExternalRenderContext::new(&verify, &build.dependencies, &host, &overrides);
    assert_eq!(
        render_external_generator_lock(&generated.lock, &context)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
}

#[test]
fn tampered_preexisting_evidence_is_rejected() {
    let mut build = build_context();
    build.evidence = Some(OutputEvidence {
        pcm: vec![0; 32],
        file: None,
    });
    let generated = generate_generic_lock(&build).unwrap();
    let verify = verification(&build, &generated.lock);
    let overrides = [ExternalParameterOverride {
        id: "scale".into(),
        value: json!(2),
    }];
    let host = host();
    let context =
        GenericExternalRenderContext::new(&verify, &build.dependencies, &host, &overrides);
    assert_eq!(
        render_external_generator_lock(&generated.lock, &context)
            .unwrap_err()
            .code,
        "E_EVIDENCE"
    );
}
