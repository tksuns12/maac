//! Deterministic construction of Generic Interchange v1 lock artifacts.
//!
//! Resolution stays with the caller: this module accepts normalized semantic
//! context plus exact asset/dependency bytes, constructs all v1 envelopes and
//! hashes, then independently validates and verifies the result.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::external::discover_external_processors;
use crate::generic_lock::{
    DependencyIdentity, ExpectedAsset, ExpectedEngine, ExpectedOutput, ExpectedProcessor,
    GenericLock, LockError, LockVerificationContext,
};
use crate::production_identity::canonical_json_bytes;

#[derive(Clone, Debug)]
pub struct ResolvedProcessor {
    pub processor_type: String,
    pub implementation_hash: Option<String>,
    pub descriptor_hash: Option<String>,
    pub adapter_id: Option<String>,
    pub state_hash: Option<String>,
    /// The normalized node.config typed record only. The generator constructs
    /// the surrounding maac.generic-config envelope.
    pub config: Value,
    pub latency_frames: u64,
    pub determinism: String,
}

#[derive(Clone, Debug)]
pub struct GenericLockGenerationContext {
    pub execution_preimage: Value,
    pub assets: BTreeMap<Vec<String>, ExpectedAsset>,
    pub dependencies: BTreeMap<DependencyIdentity, Vec<u8>>,
    pub processors: BTreeMap<Vec<String>, ResolvedProcessor>,
    pub engine: ExpectedEngine,
    pub output: ExpectedOutput,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputEvidence {
    pub pcm: Vec<u8>,
    pub file: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct GeneratedGenericLock {
    lock: GenericLock,
    configs: BTreeMap<Vec<String>, Value>,
    render_input: Value,
}

impl GeneratedGenericLock {
    pub fn lock(&self) -> &GenericLock {
        &self.lock
    }

    pub fn into_lock(self) -> GenericLock {
        self.lock
    }

    pub fn configs(&self) -> &BTreeMap<Vec<String>, Value> {
        &self.configs
    }

    pub fn config(&self, node: &[String]) -> Option<&Value> {
        self.configs.get(node)
    }

    pub fn render_input(&self) -> &Value {
        &self.render_input
    }

    pub fn config_bytes(&self, node: &[String]) -> Result<Option<Vec<u8>>, LockError> {
        self.config(node).map(canon).transpose()
    }

    pub fn render_input_bytes(&self) -> Result<Vec<u8>, LockError> {
        canon(&self.render_input)
    }

    pub fn lock_bytes(&self) -> Result<Vec<u8>, LockError> {
        self.lock.canonical_bytes()
    }
}

pub struct GenericLockBuilder<'a> {
    context: &'a GenericLockGenerationContext,
    evidence: Option<&'a OutputEvidence>,
}

impl<'a> GenericLockBuilder<'a> {
    pub fn new(context: &'a GenericLockGenerationContext) -> Self {
        Self {
            context,
            evidence: None,
        }
    }

    pub fn evidence(mut self, evidence: &'a OutputEvidence) -> Self {
        self.evidence = Some(evidence);
        self
    }

    pub fn build(self) -> Result<GeneratedGenericLock, LockError> {
        normalize_generic_lock(self.context, self.evidence)
    }
}

pub fn normalize_generic_lock(
    context: &GenericLockGenerationContext,
    evidence: Option<&OutputEvidence>,
) -> Result<GeneratedGenericLock, LockError> {
    let execution_hash = sha_uri(
        &canonical_json_bytes(&context.execution_preimage)
            .map_err(|error| lock_error("E_SCHEMA", format!("execution preimage: {error}")))?,
    );

    validate_processor_pins(context)?;

    let assets = build_assets(context)?;
    let dependencies = build_dependencies(context)?;
    let (processors, configs) = build_processors(context)?;
    let engine = build_engine(context)?;
    let output = build_output(context);

    let render_input = json!({
        "format": "maac.generic-render-input",
        "version": 1,
        "execution_hash": execution_hash,
        "assets": assets,
        "dependencies": dependencies,
        "processors": processors,
        "engine": engine,
        "output": output,
    });
    let render_key = sha_uri(&canon(&render_input)?);

    let evidence_value = match evidence {
        None => Value::Null,
        Some(evidence) => {
            let file = evidence.file.as_ref().map_or(Value::Null, |bytes| {
                json!({
                    "sha256": sha_uri(bytes),
                    "bytes": bytes.len().to_string(),
                })
            });
            json!({
                "pcm_sha256": sha_uri(&evidence.pcm),
                "pcm_bytes": evidence.pcm.len().to_string(),
                "file": file,
            })
        }
    };

    let mut lock_value = render_input
        .as_object()
        .expect("constructed render input is an object")
        .clone();
    lock_value.insert("format".into(), Value::String("maac.generic-lock".into()));
    lock_value.insert("render_key".into(), Value::String(render_key));
    lock_value.insert("evidence".into(), evidence_value);

    let lock = GenericLock::from_value(Value::Object(lock_value))?;

    let verification_context = verification_context(context, &configs, evidence);
    lock.verify(&verification_context)?;

    discover_external_processors(&lock, &context.dependencies).map_err(|error| {
        let code = if error.code == "E_SYNTAX" {
            "E_SCHEMA"
        } else {
            error.code
        };
        lock_error(code, error.message)
    })?;

    Ok(GeneratedGenericLock {
        lock,
        configs,
        render_input,
    })
}

fn build_assets(context: &GenericLockGenerationContext) -> Result<Vec<Value>, LockError> {
    let mut entries = context
        .assets
        .iter()
        .map(|(source, asset)| {
            let source_value = path_value(source);
            Ok((
                canon(&source_value)?,
                json!({
                    "source": source_value,
                    "kind": asset.kind,
                    "sha256": sha_uri(&asset.bytes),
                    "bytes": asset.bytes.len().to_string(),
                }),
            ))
        })
        .collect::<Result<Vec<_>, LockError>>()?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries.into_iter().map(|(_, value)| value).collect())
}

fn build_dependencies(context: &GenericLockGenerationContext) -> Result<Vec<Value>, LockError> {
    let mut entries = context
        .dependencies
        .iter()
        .map(|(identity, bytes)| {
            let owner = identity
                .owner
                .as_ref()
                .map_or(Value::Null, |owner| path_value(owner));
            let role = Value::Array(identity.role.iter().cloned().map(Value::String).collect());
            let identity_value = json!({"owner": owner, "role": role});
            Ok((
                canon(&identity_value)?,
                json!({
                    "owner": identity_value["owner"].clone(),
                    "role": identity_value["role"].clone(),
                    "sha256": sha_uri(bytes),
                    "bytes": bytes.len().to_string(),
                }),
            ))
        })
        .collect::<Result<Vec<_>, LockError>>()?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries.into_iter().map(|(_, value)| value).collect())
}

type BuiltProcessors = (Vec<Value>, BTreeMap<Vec<String>, Value>);

fn build_processors(context: &GenericLockGenerationContext) -> Result<BuiltProcessors, LockError> {
    let mut entries = Vec::with_capacity(context.processors.len());
    let mut configs = BTreeMap::new();

    for (node, processor) in &context.processors {
        let config = json!({
            "format": "maac.generic-config",
            "version": 1,
            "processor_type": processor.processor_type,
            "descriptor_hash": option_value(&processor.descriptor_hash),
            "config": processor.config,
        });
        let config_digest = sha_uri(&canon(&config)?);
        configs.insert(node.clone(), config.clone());
        let node_value = path_value(node);
        let value = json!({
            "node": node_value,
            "processor_type": processor.processor_type,
            "implementation_hash": option_value(&processor.implementation_hash),
            "descriptor_hash": option_value(&processor.descriptor_hash),
            "adapter_id": option_value(&processor.adapter_id),
            "state_hash": option_value(&processor.state_hash),
            "config": config,
            "config_digest": config_digest,
            "latency_frames": processor.latency_frames.to_string(),
            "determinism": processor.determinism,
        });
        entries.push((canon(&value["node"])?, value));
    }

    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok((
        entries.into_iter().map(|(_, value)| value).collect(),
        configs,
    ))
}

fn build_engine(context: &GenericLockGenerationContext) -> Result<Value, LockError> {
    if context.engine.block_schedule.is_none() && !context.engine.block_independent {
        return Err(lock_error(
            "E_CLOSURE",
            "null block schedule lacks a proven block-independent contract",
        ));
    }
    if let Some(schedule) = &context.engine.block_schedule {
        let total = schedule.iter().try_fold(0u64, |sum, block| {
            if *block == 0 {
                return Err(lock_error("E_TIMING", "block schedule contains zero"));
            }
            sum.checked_add(*block)
                .ok_or_else(|| lock_error("E_RESOURCE_LIMIT", "block schedule sum overflows u64"))
        })?;
        if total != context.output.render_frames {
            return Err(lock_error(
                "E_TIMING",
                "block schedule does not sum to render_frames",
            ));
        }
    }

    Ok(json!({
        "implementation_id": context.engine.implementation_id,
        "build_id": context.engine.build_id,
        "platform_id": context.engine.platform_id,
        "architecture_id": context.engine.architecture_id,
        "numerical_mode_id": context.engine.numerical_mode_id,
        "sample_rate": context.engine.sample_rate.to_string(),
        "block_schedule": context.engine.block_schedule.as_ref().map(|schedule| {
            schedule.iter().map(u64::to_string).collect::<Vec<_>>()
        }),
    }))
}

fn build_output(context: &GenericLockGenerationContext) -> Value {
    json!({
        "port": context.output.port,
        "score": context.output.score,
        "tail": context.output.tail,
        "render_frames": context.output.render_frames.to_string(),
        "crop": [
            context.output.crop.0.to_string(),
            context.output.crop.1.to_string(),
        ],
        "channel_order": context
            .output
            .channel_order
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>(),
        "encoding_id": "pcm_f32le_interleaved/1",
        "clipping_id": "none",
        "dither_id": "none",
    })
}

fn validate_processor_pins(context: &GenericLockGenerationContext) -> Result<(), LockError> {
    for (node, processor) in &context.processors {
        let external_group = (
            &processor.implementation_hash,
            &processor.descriptor_hash,
            &processor.adapter_id,
        );
        let external = match external_group {
            (Some(implementation), Some(descriptor), Some(_)) => {
                validate_digest(implementation, "implementation_hash")?;
                validate_digest(descriptor, "descriptor_hash")?;
                true
            }
            (None, None, None) => false,
            _ => {
                return Err(lock_error(
                    "E_SCHEMA",
                    "processor implementation/descriptor/adapter identity must be all null or all nonnull",
                ))
            }
        };
        if let Some(state) = &processor.state_hash {
            validate_digest(state, "state_hash")?;
        }

        if external {
            require_pin(
                context,
                node,
                "implementation",
                processor.implementation_hash.as_ref().unwrap(),
            )?;
            require_pin(
                context,
                node,
                "descriptor",
                processor.descriptor_hash.as_ref().unwrap(),
            )?;
            require_slot(context, node, "adapter")?;
        } else {
            reject_slot(context, node, "implementation")?;
            reject_slot(context, node, "descriptor")?;
            reject_slot(context, node, "adapter")?;
        }

        match &processor.state_hash {
            Some(hash) => require_pin(context, node, "state", hash)?,
            None => reject_slot(context, node, "state")?,
        }
    }
    Ok(())
}

fn processor_dependency(node: &[String], slot: &str) -> DependencyIdentity {
    DependencyIdentity {
        owner: Some(node.to_vec()),
        role: vec!["processor".into(), slot.into()],
    }
}

fn require_slot<'a>(
    context: &'a GenericLockGenerationContext,
    node: &[String],
    slot: &str,
) -> Result<&'a [u8], LockError> {
    context
        .dependencies
        .get(&processor_dependency(node, slot))
        .map(Vec::as_slice)
        .ok_or_else(|| {
            lock_error(
                "E_REFERENCE",
                format!("processor {:?} is missing dependency slot {slot}", node),
            )
        })
}

fn require_pin(
    context: &GenericLockGenerationContext,
    node: &[String],
    slot: &str,
    expected: &str,
) -> Result<(), LockError> {
    let bytes = require_slot(context, node, slot)?;
    if sha_uri(bytes) != expected {
        return Err(lock_error(
            "E_DIGEST",
            format!("processor {:?} {slot} bytes differ from resolved pin", node),
        ));
    }
    Ok(())
}

fn reject_slot(
    context: &GenericLockGenerationContext,
    node: &[String],
    slot: &str,
) -> Result<(), LockError> {
    if context
        .dependencies
        .contains_key(&processor_dependency(node, slot))
    {
        return Err(lock_error(
            "E_CLOSURE",
            format!("processor {:?} has unexpected dependency slot {slot}", node),
        ));
    }
    Ok(())
}

fn verification_context(
    context: &GenericLockGenerationContext,
    configs: &BTreeMap<Vec<String>, Value>,
    evidence: Option<&OutputEvidence>,
) -> LockVerificationContext {
    let processors = context
        .processors
        .iter()
        .map(|(node, processor)| {
            (
                node.clone(),
                ExpectedProcessor {
                    processor_type: processor.processor_type.clone(),
                    implementation_hash: processor.implementation_hash.clone(),
                    descriptor_hash: processor.descriptor_hash.clone(),
                    adapter_id: processor.adapter_id.clone(),
                    state_hash: processor.state_hash.clone(),
                    config: configs[node].clone(),
                    latency_frames: processor.latency_frames,
                    determinism: processor.determinism.clone(),
                },
            )
        })
        .collect();
    LockVerificationContext {
        execution_preimage: context.execution_preimage.clone(),
        assets: context.assets.clone(),
        dependencies: context.dependencies.clone(),
        processors,
        engine: context.engine.clone(),
        output: context.output.clone(),
        pcm: evidence.map(|value| value.pcm.clone()),
        file: evidence.and_then(|value| value.file.clone()),
    }
}

fn option_value(value: &Option<String>) -> Value {
    value.clone().map(Value::String).unwrap_or(Value::Null)
}

fn path_value(path: &[String]) -> Value {
    Value::Array(path.iter().cloned().map(Value::String).collect())
}

fn validate_digest(value: &str, field: &str) -> Result<(), LockError> {
    let valid = value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if valid {
        Ok(())
    } else {
        Err(lock_error(
            "E_SCHEMA",
            format!("{field} is not a canonical SHA-256 URI"),
        ))
    }
}

fn canon(value: &Value) -> Result<Vec<u8>, LockError> {
    canonical_json_bytes(value).map_err(|error| lock_error("E_SCHEMA", error.to_string()))
}

fn sha_uri(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn lock_error(code: &'static str, message: impl Into<String>) -> LockError {
    LockError {
        code,
        message: message.into(),
    }
}
