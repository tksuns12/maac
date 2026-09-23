//! Bounded Generic Interchange rendering for one resolved external generator.
//!
//! This is deliberately not a graph resolver. The caller supplies the verified
//! lock context, exact dependency bytes, executable host, and resolved parameter
//! values for the already-understood single external node.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::external::{
    discover_external_processors, ExternalParameterRate, ExternalPortDirection, ExternalPortKind,
};
use crate::external_host::{
    ExternalHost, ExternalLoadRequest, ExternalParameterOverride, ExternalProcessRequest,
};
use crate::generic_lock::{
    DependencyIdentity, ExpectedEngine, GenericLock, LockError, LockVerificationContext,
};
use crate::generic_render::{binary32_le, GenericPcmEvidence, GenericRenderResult};

pub const GENERIC_EXTERNAL_RENDER_IMPLEMENTATION_ID: &str = "maac.dsp.generic-external-render/1";
pub const GENERIC_EXTERNAL_RENDER_NUMERICAL_MODE_ID: &str = "maac.rust-f64-to-binary32/1";
pub const DEFAULT_EXTERNAL_RENDER_CHUNK_FRAMES: usize = 1024;
pub const DEFAULT_MAX_EXTERNAL_PCM_BYTES: u64 = 256 * 1024 * 1024;

pub struct GenericExternalRenderContext<'a> {
    pub verification: &'a LockVerificationContext,
    pub dependencies: &'a BTreeMap<DependencyIdentity, Vec<u8>>,
    pub host: &'a ExternalHost,
    pub parameter_overrides: &'a [ExternalParameterOverride],
    pub chunk_frames: usize,
    pub max_output_bytes: u64,
}

impl<'a> GenericExternalRenderContext<'a> {
    pub fn new(
        verification: &'a LockVerificationContext,
        dependencies: &'a BTreeMap<DependencyIdentity, Vec<u8>>,
        host: &'a ExternalHost,
        parameter_overrides: &'a [ExternalParameterOverride],
    ) -> Self {
        Self {
            verification,
            dependencies,
            host,
            parameter_overrides,
            chunk_frames: DEFAULT_EXTERNAL_RENDER_CHUNK_FRAMES,
            max_output_bytes: DEFAULT_MAX_EXTERNAL_PCM_BYTES,
        }
    }
}

pub fn generic_external_render_engine_identity(sample_rate: u64) -> ExpectedEngine {
    ExpectedEngine {
        implementation_id: GENERIC_EXTERNAL_RENDER_IMPLEMENTATION_ID.into(),
        build_id: format!("maac/{}", env!("CARGO_PKG_VERSION")),
        platform_id: format!("rust-os/{}", std::env::consts::OS),
        architecture_id: format!("rust-arch/{}", std::env::consts::ARCH),
        numerical_mode_id: GENERIC_EXTERNAL_RENDER_NUMERICAL_MODE_ID.into(),
        sample_rate,
        block_schedule: None,
        block_independent: true,
    }
}

pub fn render_external_generator_lock(
    lock: &GenericLock,
    context: &GenericExternalRenderContext<'_>,
) -> Result<GenericRenderResult, LockError> {
    if context.chunk_frames == 0 {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "external render chunk size is zero",
        ));
    }

    let mut verified = lock.verify_inputs(context.verification)?;
    verify_engine(&context.verification.engine)?;

    let discovered = discover_external_processors(lock, context.dependencies)
        .map_err(|e| error(e.code, e.message))?;
    if discovered.len() != 1 || context.verification.processors.len() != 1 {
        return Err(error(
            "E_CAPABILITY",
            "bounded external renderer requires exactly one external processor",
        ));
    }
    let processor = &discovered[0];
    if !context.host.is_block_independent(processor) {
        return Err(error(
            "E_CAPABILITY",
            "null block schedule requires an explicitly block-independent external ABI adapter",
        ));
    }
    let expected = context
        .verification
        .processors
        .get(&processor.node)
        .ok_or_else(|| {
            error(
                "E_CLOSURE",
                "external processor is absent from resolved context",
            )
        })?;

    let output_port = locked_output_port(&context.verification.output.port)?;
    if output_port.0 != processor.node {
        return Err(error(
            "E_REFERENCE",
            "locked output node is not the resolved external processor",
        ));
    }

    let mut audio_outputs = processor
        .descriptor
        .ports
        .iter()
        .filter(|port| {
            port.direction == ExternalPortDirection::Output && port.kind == ExternalPortKind::Audio
        })
        .collect::<Vec<_>>();
    audio_outputs.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    if audio_outputs.len() != 1 || audio_outputs[0].name != output_port.1 {
        return Err(error(
            "E_CAPABILITY",
            "bounded external renderer requires exactly one selected audio output port",
        ));
    }
    if processor.descriptor.latency_frames != 0 {
        return Err(error(
            "E_CAPABILITY",
            "bounded external renderer currently requires zero technical latency",
        ));
    }
    if processor
        .descriptor
        .parameter_rates
        .iter()
        .any(|rate| rate.rate != ExternalParameterRate::Static)
    {
        return Err(error(
            "E_CAPABILITY",
            "bounded external renderer currently requires static external parameters",
        ));
    }
    if processor
        .descriptor
        .ports
        .iter()
        .any(|port| port.direction == ExternalPortDirection::Input)
    {
        return Err(error(
            "E_CAPABILITY",
            "bounded external renderer supports output-only generator processors",
        ));
    }

    let channels = usize::try_from(audio_outputs[0].channels).map_err(|_| {
        error(
            "E_RESOURCE_LIMIT",
            "external output channel count overflows usize",
        )
    })?;
    if channels == 0 || channels != verified.channels {
        return Err(error(
            "E_REFERENCE",
            "external output width differs from locked channel order",
        ));
    }

    let declared = processor
        .descriptor
        .parameters
        .iter()
        .map(|parameter| parameter.id.as_str())
        .collect::<BTreeSet<_>>();
    let supplied = context
        .parameter_overrides
        .iter()
        .map(|parameter| parameter.id.as_str())
        .collect::<BTreeSet<_>>();
    if supplied.len() != context.parameter_overrides.len() || supplied != declared {
        return Err(error(
            "E_CAPABILITY",
            "bounded external renderer requires exactly one resolved override for every parameter",
        ));
    }

    let config = expected.config.get("config").ok_or_else(|| {
        error(
            "E_SCHEMA",
            "resolved external config envelope is missing config",
        )
    })?;
    let (crop_start, crop_end) = verified.crop;
    let crop_frames = crop_end
        .checked_sub(crop_start)
        .ok_or_else(|| error("E_TIMING", "crop is reversed"))?;
    let channel_count = u64::try_from(channels)
        .map_err(|_| error("E_RESOURCE_LIMIT", "channel count overflows u64"))?;
    let output_bytes = crop_frames
        .checked_mul(channel_count)
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| error("E_RESOURCE_LIMIT", "PCM byte length overflows"))?;
    if output_bytes > context.max_output_bytes {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "locked PCM output exceeds external-render byte limit",
        ));
    }
    let capacity = usize::try_from(output_bytes).map_err(|_| {
        error(
            "E_RESOURCE_LIMIT",
            "PCM allocation exceeds addressable memory",
        )
    })?;
    let chunk_limit = u64::try_from(context.chunk_frames).map_err(|_| {
        error(
            "E_RESOURCE_LIMIT",
            "external render chunk size overflows u64",
        )
    })?;
    let max_chunk = usize::try_from(context.verification.output.render_frames.min(chunk_limit))
        .map_err(|_| error("E_RESOURCE_LIMIT", "working chunk size overflows usize"))?;
    let working_samples = channels.checked_mul(max_chunk).ok_or_else(|| {
        error(
            "E_RESOURCE_LIMIT",
            "external working sample count overflows",
        )
    })?;
    let working_bytes = u64::try_from(working_samples)
        .ok()
        .and_then(|samples| samples.checked_mul(8))
        .ok_or_else(|| error("E_RESOURCE_LIMIT", "external working byte count overflows"))?;
    if working_bytes > context.max_output_bytes {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "external working buffers exceed external-render byte limit",
        ));
    }

    let mut pcm = Vec::new();
    pcm.try_reserve_exact(capacity)
        .map_err(|_| error("E_RESOURCE_LIMIT", "PCM allocation failed"))?;

    let mut instance = context
        .host
        .load(ExternalLoadRequest {
            processor,
            sample_rate: context.verification.engine.sample_rate,
            config,
            parameter_overrides: context.parameter_overrides,
        })
        .map_err(|e| error(e.code, e.message))?;

    let order = &context.verification.output.channel_order;
    let render_frames = context.verification.output.render_frames;
    let mut frame = 0u64;
    while frame < render_frames {
        let remaining = render_frames - frame;
        let chunk = usize::try_from(remaining.min(chunk_limit))
            .map_err(|_| error("E_RESOURCE_LIMIT", "chunk frame count overflows usize"))?;
        let sample_count = channels
            .checked_mul(chunk)
            .ok_or_else(|| error("E_RESOURCE_LIMIT", "external chunk sample count overflows"))?;
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(sample_count)
            .map_err(|_| error("E_RESOURCE_LIMIT", "external working allocation failed"))?;
        storage.resize(sample_count, 0.0f64);
        let mut outputs = storage.chunks_mut(chunk).collect::<Vec<_>>();
        instance
            .process(ExternalProcessRequest {
                frames: chunk,
                audio_inputs: &[],
                audio_outputs: &mut outputs,
            })
            .map_err(|e| error(e.code, e.message))?;

        for local in 0..chunk {
            let absolute = frame
                .checked_add(local as u64)
                .ok_or_else(|| error("E_RESOURCE_LIMIT", "frame index overflow"))?;
            if absolute < crop_start || absolute >= crop_end {
                continue;
            }
            for &channel in order {
                let channel = usize::try_from(channel)
                    .map_err(|_| error("E_RESOURCE_LIMIT", "channel index overflows usize"))?;
                let sample = storage[channel * chunk + local];
                pcm.extend_from_slice(&binary32_le(sample)?);
            }
        }
        frame = frame
            .checked_add(chunk as u64)
            .ok_or_else(|| error("E_RESOURCE_LIMIT", "frame counter overflow"))?;
    }

    if pcm.len() != capacity {
        return Err(error(
            "E_EVIDENCE",
            "rendered external PCM length differs from preflighted length",
        ));
    }
    let (pcm_verified, file_verified) = lock.verify_evidence(Some(&pcm), Some(&pcm))?;
    verified.pcm_verified = pcm_verified;
    verified.file_verified = file_verified;
    let pcm_bytes = u64::try_from(pcm.len())
        .map_err(|_| error("E_RESOURCE_LIMIT", "PCM length exceeds u64"))?;
    let evidence = GenericPcmEvidence {
        pcm_sha256: sha_uri(&pcm),
        pcm_bytes,
    };
    Ok(GenericRenderResult {
        pcm,
        evidence,
        verified,
    })
}

fn verify_engine(engine: &ExpectedEngine) -> Result<(), LockError> {
    let expected = generic_external_render_engine_identity(engine.sample_rate);
    if engine.implementation_id != expected.implementation_id
        || engine.build_id != expected.build_id
        || engine.platform_id != expected.platform_id
        || engine.architecture_id != expected.architecture_id
        || engine.numerical_mode_id != expected.numerical_mode_id
        || engine.block_schedule.is_some()
        || !engine.block_independent
    {
        return Err(error(
            "E_CAPABILITY",
            "locked engine identity is not the block-independent generic external renderer",
        ));
    }
    Ok(())
}

fn locked_output_port(value: &Value) -> Result<(Vec<String>, String), LockError> {
    let object = value
        .as_object()
        .ok_or_else(|| error("E_SCHEMA", "output port must be a typed reference"))?;
    if object.get("t").and_then(Value::as_str) != Some("ref") {
        return Err(error("E_SCHEMA", "output port must be a typed reference"));
    }
    let path = object
        .get("path")
        .and_then(Value::as_array)
        .ok_or_else(|| error("E_SCHEMA", "output port path must be an array"))?
        .iter()
        .map(|part| {
            part.as_str()
                .map(str::to_owned)
                .ok_or_else(|| error("E_SCHEMA", "output path component must be a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let port = object
        .get("port")
        .and_then(Value::as_str)
        .ok_or_else(|| error("E_SCHEMA", "output port name is missing"))?
        .to_owned();
    Ok((path, port))
}

fn sha_uri(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn error(code: &'static str, message: impl Into<String>) -> LockError {
    LockError {
        code,
        message: message.into(),
    }
}
