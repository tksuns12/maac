//! Deterministic Generic Interchange v1 lock generation/normalization.
//!
//! Callers supply an already-resolved semantic context and exact closure bytes.
//! This module owns canonical envelope construction, ordering, digests, cross-pins,
//! and optional evidence construction. It performs no filesystem or network discovery.
//! Every failure uses the code that `GenericLock` verification reports for the
//! same condition, and each generated lock is independently verified before it
//! is returned.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::external::discover_external_processors;
use crate::generic_lock::{
    DependencyIdentity, ExpectedAsset, ExpectedEngine, ExpectedOutput, ExpectedProcessor,
    GenericLock, LockError, LockVerificationContext,
};
use crate::production_identity::canonical_json_bytes;

/// Exact asset bytes and kind, keyed by source path in the build context.
pub type ResolvedAsset = ExpectedAsset;
/// The understood host engine and its block-schedule contract.
pub type ResolvedEngine = ExpectedEngine;
/// The normalized output selection and raw output contract.
pub type ResolvedOutput = ExpectedOutput;

#[derive(Clone, Debug)]
pub struct ResolvedProcessor {
    pub processor_type: String,
    pub implementation_hash: Option<String>,
    pub descriptor_hash: Option<String>,
    pub adapter_id: Option<String>,
    pub state_hash: Option<String>,
    /// Normalized node.config record only, with understood defaults expanded.
    /// The generator constructs the surrounding maac.generic-config envelope.
    pub normalized_config: Value,
    pub latency_frames: u64,
    pub determinism: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputEvidence {
    pub pcm: Vec<u8>,
    pub file: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct GenericLockBuildContext {
    pub execution_preimage: Value,
    pub assets: BTreeMap<Vec<String>, ResolvedAsset>,
    pub dependencies: BTreeMap<DependencyIdentity, Vec<u8>>,
    pub processors: BTreeMap<Vec<String>, ResolvedProcessor>,
    pub engine: ResolvedEngine,
    pub output: ResolvedOutput,
    /// Optional rendered output. Evidence is recorded in the lock but never
    /// enters the render key.
    pub evidence: Option<OutputEvidence>,
}

#[derive(Clone, Debug)]
pub struct GeneratedGenericLock {
    /// Canonical Config bytes keyed by processor node.
    pub configs: BTreeMap<Vec<String>, Vec<u8>>,
    pub render_input: Value,
    pub render_input_bytes: Vec<u8>,
    pub lock: GenericLock,
    pub lock_bytes: Vec<u8>,
}

fn error(code: &'static str, message: impl Into<String>) -> LockError {
    LockError {
        code,
        message: message.into(),
    }
}

fn canon(value: &Value) -> Result<Vec<u8>, LockError> {
    canonical_json_bytes(value).map_err(|e| error("E_SCHEMA", e.to_string()))
}

fn sha_uri(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn u(value: u64) -> Value {
    Value::String(value.to_string())
}

fn opt(value: &Option<String>) -> Value {
    value.clone().map(Value::String).unwrap_or(Value::Null)
}

fn canonical_path_key(path: &[String]) -> Result<Vec<u8>, LockError> {
    canon(&serde_json::to_value(path).map_err(|e| error("E_SCHEMA", e.to_string()))?)
}

fn canonical_dependency_key(id: &DependencyIdentity) -> Result<Vec<u8>, LockError> {
    canon(&json!({"owner": id.owner, "role": id.role}))
}

fn processor_slot<'a>(
    dependencies: &'a BTreeMap<DependencyIdentity, Vec<u8>>,
    node: &[String],
    slot: &str,
) -> Option<&'a Vec<u8>> {
    dependencies.get(&DependencyIdentity {
        owner: Some(node.to_vec()),
        role: vec!["processor".into(), slot.into()],
    })
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
        Err(error(
            "E_SCHEMA",
            format!("{field} is not a canonical SHA-256 URI"),
        ))
    }
}

/// Check the processor identity group and pin formats before any closure
/// comparison, so a malformed pin is a schema error rather than a digest one.
fn validate_processor_shape(processor: &ResolvedProcessor) -> Result<(), LockError> {
    let group = [
        processor.implementation_hash.is_some(),
        processor.descriptor_hash.is_some(),
        processor.adapter_id.is_some(),
    ];
    if group.iter().any(|present| *present) && !group.iter().all(|present| *present) {
        return Err(error(
            "E_SCHEMA",
            "processor implementation/descriptor/adapter identity must be all null or all nonnull",
        ));
    }
    for (value, field) in [
        (&processor.implementation_hash, "implementation_hash"),
        (&processor.descriptor_hash, "descriptor_hash"),
        (&processor.state_hash, "state_hash"),
    ] {
        if let Some(value) = value {
            validate_digest(value, field)?;
        }
    }
    Ok(())
}

/// Cross-pin one processor dependency slot: a pinned slot needs matching
/// bytes, and bytes without a pin are an unexpected closure member.
fn require_pin(
    supplied: Option<&String>,
    actual: Option<&Vec<u8>>,
    label: &str,
) -> Result<Value, LockError> {
    match (supplied, actual) {
        (None, None) => Ok(Value::Null),
        (Some(expected), Some(bytes)) => {
            let digest = sha_uri(bytes);
            if &digest != expected {
                return Err(error(
                    "E_DIGEST",
                    format!("{label} differs from supplied closure bytes"),
                ));
            }
            Ok(Value::String(digest))
        }
        (Some(_), None) => Err(error(
            "E_REFERENCE",
            format!("{label} is pinned but its dependency bytes are missing"),
        )),
        (None, Some(_)) => Err(error(
            "E_CLOSURE",
            format!("{label} dependency is present without a semantic pin"),
        )),
    }
}

pub fn generate_generic_lock(
    context: &GenericLockBuildContext,
) -> Result<GeneratedGenericLock, LockError> {
    let execution_hash = sha_uri(&canon(&context.execution_preimage)?);

    let mut assets = context
        .assets
        .iter()
        .map(|(source, asset)| {
            Ok((
                canonical_path_key(source)?,
                json!({
                    "source": source,
                    "kind": asset.kind,
                    "sha256": sha_uri(&asset.bytes),
                    "bytes": asset.bytes.len().to_string(),
                }),
            ))
        })
        .collect::<Result<Vec<_>, LockError>>()?;
    assets.sort_by(|a, b| a.0.cmp(&b.0));

    let mut dependencies = context
        .dependencies
        .iter()
        .map(|(identity, bytes)| {
            Ok((
                canonical_dependency_key(identity)?,
                json!({
                    "owner": identity.owner,
                    "role": identity.role,
                    "sha256": sha_uri(bytes),
                    "bytes": bytes.len().to_string(),
                }),
            ))
        })
        .collect::<Result<Vec<_>, LockError>>()?;
    dependencies.sort_by(|a, b| a.0.cmp(&b.0));

    let mut configs = BTreeMap::new();
    let mut config_values = BTreeMap::new();
    let mut processors = Vec::new();
    for (node, processor) in &context.processors {
        validate_processor_shape(processor)?;
        let slot = |name: &str| processor_slot(&context.dependencies, node, name);
        let implementation_hash = require_pin(
            processor.implementation_hash.as_ref(),
            slot("implementation"),
            "processor implementation",
        )?;
        let descriptor_hash = require_pin(
            processor.descriptor_hash.as_ref(),
            slot("descriptor"),
            "processor descriptor",
        )?;
        // The adapter is identified by ID rather than digest, but its bytes
        // are still a required (external) or forbidden (core) closure slot.
        match (&processor.adapter_id, slot("adapter")) {
            (Some(_), None) => {
                return Err(error(
                    "E_REFERENCE",
                    "processor adapter is identified but its dependency bytes are missing",
                ))
            }
            (None, Some(_)) => {
                return Err(error(
                    "E_CLOSURE",
                    "processor adapter dependency is present for a core processor",
                ))
            }
            _ => {}
        }
        let state_hash = require_pin(
            processor.state_hash.as_ref(),
            slot("state"),
            "processor state",
        )?;

        let config = json!({
            "format": "maac.generic-config",
            "version": 1,
            "processor_type": processor.processor_type,
            "descriptor_hash": descriptor_hash,
            "config": processor.normalized_config,
        });
        let config_bytes = canon(&config)?;
        let config_digest = sha_uri(&config_bytes);
        configs.insert(node.clone(), config_bytes);
        config_values.insert(node.clone(), config.clone());

        processors.push((
            canonical_path_key(node)?,
            json!({
                "node": node,
                "processor_type": processor.processor_type,
                "implementation_hash": implementation_hash,
                "descriptor_hash": config["descriptor_hash"],
                "adapter_id": opt(&processor.adapter_id),
                "state_hash": state_hash,
                "config": config,
                "config_digest": config_digest,
                "latency_frames": processor.latency_frames.to_string(),
                "determinism": processor.determinism,
            }),
        ));
    }
    processors.sort_by(|a, b| a.0.cmp(&b.0));

    // Block-schedule positivity, sum, and block-independence are enforced by
    // lock parsing and verification below, with the verifier's own codes.
    let block_schedule = context
        .engine
        .block_schedule
        .as_ref()
        .map(|blocks| Value::Array(blocks.iter().copied().map(u).collect()))
        .unwrap_or(Value::Null);
    let engine = json!({
        "implementation_id": context.engine.implementation_id,
        "build_id": context.engine.build_id,
        "platform_id": context.engine.platform_id,
        "architecture_id": context.engine.architecture_id,
        "numerical_mode_id": context.engine.numerical_mode_id,
        "sample_rate": context.engine.sample_rate.to_string(),
        "block_schedule": block_schedule,
    });
    let output = json!({
        "port": context.output.port,
        "score": context.output.score,
        "tail": context.output.tail,
        "render_frames": context.output.render_frames.to_string(),
        "crop": [context.output.crop.0.to_string(), context.output.crop.1.to_string()],
        "channel_order": context.output.channel_order.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "encoding_id": "pcm_f32le_interleaved/1",
        "clipping_id": "none",
        "dither_id": "none",
    });

    let render_input = json!({
        "format": "maac.generic-render-input",
        "version": 1,
        "execution_hash": execution_hash,
        "assets": assets.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "dependencies": dependencies.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "processors": processors.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "engine": engine,
        "output": output,
    });
    let render_input_bytes = canon(&render_input)?;
    let render_key = sha_uri(&render_input_bytes);

    let evidence = match &context.evidence {
        None => Value::Null,
        Some(evidence) => {
            let file = evidence.file.as_ref().map_or(
                Value::Null,
                |bytes| json!({"sha256": sha_uri(bytes), "bytes": bytes.len().to_string()}),
            );
            json!({
                "pcm_sha256": sha_uri(&evidence.pcm),
                "pcm_bytes": evidence.pcm.len().to_string(),
                "file": file,
            })
        }
    };

    let mut lock = render_input
        .as_object()
        .expect("render input is constructed as an object")
        .clone();
    lock.insert("format".into(), Value::String("maac.generic-lock".into()));
    lock.insert("render_key".into(), Value::String(render_key));
    lock.insert("evidence".into(), evidence);
    let lock = GenericLock::from_value(Value::Object(lock))?;

    let verification = LockVerificationContext {
        execution_preimage: context.execution_preimage.clone(),
        assets: context.assets.clone(),
        dependencies: context.dependencies.clone(),
        processors: context
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
                        config: config_values[node].clone(),
                        latency_frames: processor.latency_frames,
                        determinism: processor.determinism.clone(),
                    },
                )
            })
            .collect(),
        engine: context.engine.clone(),
        output: context.output.clone(),
        pcm: context.evidence.as_ref().map(|e| e.pcm.clone()),
        file: context.evidence.as_ref().and_then(|e| e.file.clone()),
    };
    lock.verify(&verification)?;

    // Descriptor bytes are interchange dependencies, not MaaC source, so a
    // malformed descriptor is a schema failure of the lock closure.
    discover_external_processors(&lock, &context.dependencies).map_err(|e| {
        let code = if e.code == "E_SYNTAX" {
            "E_SCHEMA"
        } else {
            e.code
        };
        error(code, e.message)
    })?;

    Ok(GeneratedGenericLock {
        configs,
        render_input,
        render_input_bytes,
        lock_bytes: lock.canonical_bytes()?,
        lock,
    })
}
