//! Execution boundary for verified Generic Interchange v1 locks.
//!
//! This module never reconstructs an executable graph from lock JSON. Callers
//! provide both the lock-verification context and the already-resolved Plan.

use std::collections::BTreeMap;

use num_bigint::BigInt;
use num_rational::BigRational;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::dsp::{DspEngine, RenderError};
use crate::generic_lock::{
    ExpectedEngine, GenericLock, LockError, LockVerificationContext, VerifiedLock,
};
use crate::plan::{Plan, PlanLimits, PortRef, Processor};

pub const GENERIC_RENDER_IMPLEMENTATION_ID: &str = "maac.dsp.generic-render/1";
pub const GENERIC_RENDER_NUMERICAL_MODE_ID: &str = "maac.rust-f64-to-binary32/1";
pub const DEFAULT_MAX_GENERIC_PCM_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericPcmEvidence {
    pub pcm_sha256: String,
    pub pcm_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct GenericRenderResult {
    pub pcm: Vec<u8>,
    pub evidence: GenericPcmEvidence,
    pub verified: VerifiedLock,
}

pub struct GenericRenderContext<'a> {
    pub plan: &'a Plan,
    pub verification: &'a LockVerificationContext,
    pub plan_limits: PlanLimits,
    pub max_output_bytes: u64,
}

impl<'a> GenericRenderContext<'a> {
    pub fn new(plan: &'a Plan, verification: &'a LockVerificationContext) -> Self {
        Self {
            plan,
            verification,
            plan_limits: PlanLimits::default(),
            max_output_bytes: DEFAULT_MAX_GENERIC_PCM_BYTES,
        }
    }
}

pub fn generic_render_engine_identity(sample_rate: u64) -> ExpectedEngine {
    ExpectedEngine {
        implementation_id: GENERIC_RENDER_IMPLEMENTATION_ID.into(),
        build_id: format!("maac/{}", env!("CARGO_PKG_VERSION")),
        platform_id: format!("rust-os/{}", std::env::consts::OS),
        architecture_id: format!("rust-arch/{}", std::env::consts::ARCH),
        numerical_mode_id: GENERIC_RENDER_NUMERICAL_MODE_ID.into(),
        sample_rate,
        block_schedule: None,
        block_independent: true,
    }
}

fn error(code: &'static str, message: impl Into<String>) -> LockError {
    LockError {
        code,
        message: message.into(),
    }
}

fn sha_uri(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Convert one finite DSP sample to the Generic Interchange v1 binary32 profile.
pub fn binary32_le(sample: f64) -> Result<[u8; 4], LockError> {
    if !sample.is_finite() {
        return Err(error("E_EVIDENCE", "cannot encode nonfinite PCM sample"));
    }
    let value = sample as f32;
    if !value.is_finite() {
        return Err(error("E_EVIDENCE", "binary32 conversion overflow"));
    }
    Ok(value.to_le_bytes())
}

fn typed_rational(value: &Value, unit: &str, label: &str) -> Result<BigRational, LockError> {
    let object = value
        .as_object()
        .ok_or_else(|| error("E_SCHEMA", format!("{label}: expected typed quantity")))?;
    if object.get("t").and_then(Value::as_str) != Some("quantity")
        || object.get("u").and_then(Value::as_str) != Some(unit)
    {
        return Err(error(
            "E_SCHEMA",
            format!("{label}: expected quantity in {unit}"),
        ));
    }
    let numerator = object
        .get("n")
        .and_then(Value::as_str)
        .ok_or_else(|| error("E_SCHEMA", format!("{label}: missing numerator")))?
        .parse::<BigInt>()
        .map_err(|_| error("E_SCHEMA", format!("{label}: invalid numerator")))?;
    let denominator = object
        .get("d")
        .and_then(Value::as_str)
        .ok_or_else(|| error("E_SCHEMA", format!("{label}: missing denominator")))?
        .parse::<BigInt>()
        .map_err(|_| error("E_SCHEMA", format!("{label}: invalid denominator")))?;
    if denominator <= BigInt::from(0) {
        return Err(error(
            "E_SCHEMA",
            format!("{label}: denominator must be positive"),
        ));
    }
    Ok(BigRational::new(numerator, denominator))
}

fn locked_output_port(value: &Value) -> Result<PortRef, LockError> {
    let object = value
        .as_object()
        .ok_or_else(|| error("E_SCHEMA", "output port must be a typed reference"))?;
    if object.get("t").and_then(Value::as_str) != Some("ref") {
        return Err(error("E_SCHEMA", "output port must be a typed reference"));
    }
    let path = object
        .get("path")
        .and_then(Value::as_array)
        .ok_or_else(|| error("E_SCHEMA", "output port path must be an array"))?;
    if path.len() != 1 {
        return Err(error(
            "E_CAPABILITY",
            "generic renderer currently supports only single-segment Plan node paths",
        ));
    }
    let node = path[0]
        .as_str()
        .ok_or_else(|| error("E_SCHEMA", "output node path segment must be a string"))?;
    let port = object
        .get("port")
        .and_then(Value::as_str)
        .ok_or_else(|| error("E_SCHEMA", "output port name must be present"))?;
    PortRef::new(node, port).map_err(|e| error("E_REFERENCE", e.to_string()))
}

fn processor_type(processor: &Processor) -> Result<&'static str, LockError> {
    Ok(match processor {
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
        Processor::Instrument { .. } => {
            return Err(error(
                "E_CAPABILITY",
                "generic renderer does not yet execute instrument-program processors",
            ))
        }
    })
}

fn verify_host_engine(context: &LockVerificationContext, plan: &Plan) -> Result<(), LockError> {
    let expected = generic_render_engine_identity(u64::from(plan.output.sample_rate_hz));
    let actual = &context.engine;
    if actual.implementation_id != expected.implementation_id
        || actual.build_id != expected.build_id
        || actual.platform_id != expected.platform_id
        || actual.architecture_id != expected.architecture_id
        || actual.numerical_mode_id != expected.numerical_mode_id
        || actual.sample_rate != expected.sample_rate
    {
        return Err(error(
            "E_CAPABILITY",
            "locked engine identity is not this generic-render host",
        ));
    }
    if actual.block_schedule.is_some() || !actual.block_independent {
        return Err(error(
            "E_CAPABILITY",
            "generic renderer currently supports only the proven block-independent null-schedule contract",
        ));
    }
    Ok(())
}

fn verify_plan_binding(
    context: &LockVerificationContext,
    plan: &Plan,
) -> Result<PortRef, LockError> {
    plan.validate_with_limits(&PlanLimits::song())
        .map_err(|e| error("E_CAPABILITY", e.to_string()))?;

    if context.output.render_frames != plan.output.total_frames {
        return Err(error(
            "E_TIMING",
            "locked render_frames differs from the resolved Plan",
        ));
    }
    let score_start = typed_rational(&context.output.score[0], "q", "score start")?;
    let score_end = typed_rational(&context.output.score[1], "q", "score end")?;
    let tail = typed_rational(&context.output.tail, "s", "tail")?;
    if score_start != plan.output.score_start_q
        || score_end != plan.output.score_end_q
        || tail != plan.output.tail_seconds
    {
        return Err(error(
            "E_TIMING",
            "locked score/tail interval differs from the resolved Plan",
        ));
    }

    let mut plan_processors = BTreeMap::new();
    for node in &plan.nodes {
        plan_processors.insert(node.id.as_str(), &node.processor);
    }
    if context.processors.len() != plan_processors.len() {
        return Err(error(
            "E_CLOSURE",
            "locked processor set differs from the resolved Plan",
        ));
    }
    for (path, locked) in &context.processors {
        if path.len() != 1 {
            return Err(error(
                "E_CAPABILITY",
                "generic renderer currently supports only single-segment Plan processor paths",
            ));
        }
        if locked.implementation_hash.is_some()
            || locked.descriptor_hash.is_some()
            || locked.adapter_id.is_some()
            || locked.state_hash.is_some()
        {
            return Err(error(
                "E_CAPABILITY",
                "external or state-pinned processor execution is not supported",
            ));
        }
        let plan_processor = plan_processors
            .get(path[0].as_str())
            .ok_or_else(|| error("E_CLOSURE", "locked processor node is absent from the Plan"))?;
        if locked.processor_type != processor_type(plan_processor)?
            || locked.latency_frames != plan_processor.technical_latency_frames()
        {
            return Err(error(
                "E_CLOSURE",
                "locked processor identity differs from the resolved Plan",
            ));
        }
    }

    let port = locked_output_port(&context.output.port)?;
    let channels = plan
        .audio_output_channels(&port)
        .map_err(|e| error("E_REFERENCE", e.to_string()))?;
    if usize::from(channels) != context.output.channel_order.len() {
        return Err(error(
            "E_REFERENCE",
            "locked channel_order width differs from the selected Plan port",
        ));
    }
    Ok(port)
}

fn map_render_error(error_value: RenderError) -> LockError {
    let code = match error_value.code() {
        "E_RESOURCE_LIMIT" => "E_RESOURCE_LIMIT",
        "E_REFERENCE" | "E_PORT_TYPE" => "E_REFERENCE",
        "E_CAPABILITY" => "E_CAPABILITY",
        "E_SCHEDULE" | "E_TEMPO" | "E_INTERVAL" | "E_TIME_PRECISION" => "E_TIMING",
        "E_NONFINITE" => "E_EVIDENCE",
        _ => "E_CAPABILITY",
    };
    error(code, error_value.to_string())
}

pub fn render_generic_lock(
    lock: &GenericLock,
    context: &GenericRenderContext<'_>,
) -> Result<GenericRenderResult, LockError> {
    let mut verified = lock.verify_inputs(context.verification)?;
    verify_host_engine(context.verification, context.plan)?;
    let port = verify_plan_binding(context.verification, context.plan)?;

    let (crop_start, crop_end) = verified.crop;
    let frames = crop_end
        .checked_sub(crop_start)
        .ok_or_else(|| error("E_TIMING", "crop is reversed"))?;
    let channels = u64::try_from(verified.channels)
        .map_err(|_| error("E_RESOURCE_LIMIT", "channel count overflows u64"))?;
    let output_bytes = frames
        .checked_mul(channels)
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| error("E_RESOURCE_LIMIT", "PCM byte length overflows"))?;
    if output_bytes > context.max_output_bytes {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "locked PCM output exceeds the generic-render byte limit",
        ));
    }
    let capacity = usize::try_from(output_bytes).map_err(|_| {
        error(
            "E_RESOURCE_LIMIT",
            "PCM allocation exceeds addressable memory",
        )
    })?;
    let mut pcm = Vec::new();
    pcm.try_reserve_exact(capacity)
        .map_err(|_| error("E_RESOURCE_LIMIT", "PCM allocation failed"))?;

    let mut frame_index = 0u64;
    let order = context.verification.output.channel_order.clone();
    let mut engine =
        DspEngine::new_with_limits(context.plan, &context.plan_limits).map_err(map_render_error)?;
    engine
        .render_ports(&[port], |outputs| {
            if frame_index >= crop_start && frame_index < crop_end {
                let frame = &outputs[0];
                for &channel in &order {
                    let sample = frame[usize::try_from(channel).map_err(|_| {
                        RenderError::Callback("channel index overflows usize".into())
                    })?];
                    let bytes = binary32_le(sample).map_err(|failure| {
                        RenderError::Nonfinite(format!(
                            "{} at frame {frame_index}",
                            failure.message
                        ))
                    })?;
                    pcm.extend_from_slice(&bytes);
                }
            }
            frame_index = frame_index
                .checked_add(1)
                .ok_or_else(|| RenderError::Callback("frame counter overflow".into()))?;
            Ok(())
        })
        .map_err(map_render_error)?;

    if frame_index != context.verification.output.render_frames {
        return Err(error(
            "E_TIMING",
            "DSP execution length differs from locked render_frames",
        ));
    }
    if pcm.len() != capacity {
        return Err(error(
            "E_EVIDENCE",
            "rendered PCM length differs from preflighted output length",
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
