//! Strict external-processor descriptor and dependency-discovery contracts.
//!
//! MaaC/1 §17 defines descriptor semantics but intentionally does not bless a
//! filename-driven native ABI. This module publishes one explicit descriptor
//! wire capability and resolves its exact bytes from an already-validated
//! Generic Lock. It never loads or executes module bytes.

use crate::generic_lock::{DependencyIdentity, GenericLock, LockError};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const DESCRIPTOR_FORMAT: &str = "maac.external-processor-descriptor";
pub const DESCRIPTOR_WIRE_VERSION: u32 = 1;
pub const MAX_DESCRIPTOR_BYTES: usize = 4 * 1024 * 1024;
const MAX_DESCRIPTOR_VALUES: usize = 65_536;
const MAX_ITEMS: usize = 4096;
const MAX_STRING_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalPortDirection {
    Input,
    Output,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalPortKind {
    Audio,
    Control,
    Events,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCardinality {
    Single,
    Summing,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalParameterRate {
    Sample,
    Event,
    Block,
    Static,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalRangePolicy {
    Error,
    Clamp,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalDeterminismMode {
    DeclaredDeterministic,
    Nondeterministic,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalPortDescriptor {
    pub name: String,
    pub direction: ExternalPortDirection,
    pub kind: ExternalPortKind,
    pub channels: u32,
    pub cardinality: Option<ExternalCardinality>,
    pub zero_default: Option<bool>,
    pub empty_event_stream: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalEventContract {
    pub native_notes: bool,
    pub expression_kinds: Vec<String>,
    pub hit_keys: Vec<String>,
    pub message_protocols: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalConfigField {
    pub name: String,
    pub value_type: String,
    pub default: Option<Value>,
    pub minimum: Option<Value>,
    pub maximum: Option<Value>,
    pub asset_kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalParameterDescriptor {
    pub id: String,
    pub source_name: String,
    pub unit: String,
    pub default: Value,
    pub minimum: Value,
    pub maximum: Value,
    pub range_policy: ExternalRangePolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalParameterRateDescriptor {
    pub parameter_id: String,
    pub rate: ExternalParameterRate,
    pub interpolation: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalFeedthrough {
    pub output_port: String,
    pub input_ports: Vec<String>,
    pub parameters: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalStateContract {
    pub serialization_version: String,
    pub reset_semantics: String,
    pub save_restore: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalDeterminism {
    pub mode: ExternalDeterminismMode,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalPermissions {
    pub assets: bool,
    pub network: bool,
    pub devices: bool,
    pub process_execution: bool,
    pub shared_memory: bool,
}

impl ExternalPermissions {
    fn allowed_by(self, allowed: Self) -> bool {
        (!self.assets || allowed.assets)
            && (!self.network || allowed.network)
            && (!self.devices || allowed.devices)
            && (!self.process_execution || allowed.process_execution)
            && (!self.shared_memory || allowed.shared_memory)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalProcessorDescriptor {
    pub format: String,
    pub wire_version: u32,
    pub id: String,
    pub version: String,
    pub abi: String,
    pub ports: Vec<ExternalPortDescriptor>,
    pub events: ExternalEventContract,
    pub config: Vec<ExternalConfigField>,
    pub parameters: Vec<ExternalParameterDescriptor>,
    pub parameter_rates: Vec<ExternalParameterRateDescriptor>,
    pub feedthrough: Vec<ExternalFeedthrough>,
    pub latency_frames: u64,
    pub state: ExternalStateContract,
    pub determinism: ExternalDeterminism,
    pub permissions: ExternalPermissions,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalError {
    pub code: &'static str,
    pub message: String,
}
impl ExternalError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl fmt::Display for ExternalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ExternalError {}

struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded unique-key integer JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Unique, E> {
                Ok(Unique(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Unique, E> {
                Err(E::custom("JSON floats are forbidden"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Unique, E> {
                Ok(Unique(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Unique, E> {
                Ok(Unique(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut x = Vec::new();
                while let Some(Unique(v)) = a.next_element()? {
                    if x.len() >= MAX_DESCRIPTOR_VALUES {
                        return Err(de::Error::custom("descriptor value limit"));
                    }
                    x.push(v);
                }
                Ok(Unique(Value::Array(x)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut m = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if m.contains_key(&k) {
                        return Err(de::Error::custom(format!("duplicate JSON key `{k}`")));
                    }
                    let Unique(v) = a.next_value()?;
                    m.insert(k, v);
                }
                Ok(Unique(Value::Object(m)))
            }
        }
        d.deserialize_any(V)
    }
}

fn bounded_string(s: &str, field: &str) -> Result<(), ExternalError> {
    if s.is_empty() {
        return Err(ExternalError::new(
            "E_SCHEMA",
            format!("{field} must be nonempty"),
        ));
    }
    if s.len() > MAX_STRING_BYTES {
        return Err(ExternalError::new(
            "E_RESOURCE_LIMIT",
            format!("{field} exceeds string bound"),
        ));
    }
    Ok(())
}
fn unique_nonempty(xs: &[String], field: &str) -> Result<(), ExternalError> {
    if xs.len() > MAX_ITEMS {
        return Err(ExternalError::new(
            "E_RESOURCE_LIMIT",
            format!("{field} exceeds item bound"),
        ));
    }
    let mut seen = BTreeSet::new();
    for x in xs {
        bounded_string(x, field)?;
        if !seen.insert(x) {
            return Err(ExternalError::new(
                "E_SCHEMA",
                format!("duplicate {field} `{x}`"),
            ));
        }
    }
    Ok(())
}
fn bounded_value(v: &Value, count: &mut usize, depth: usize) -> Result<(), ExternalError> {
    if depth > 64 {
        return Err(ExternalError::new(
            "E_RESOURCE_LIMIT",
            "descriptor nesting limit exceeded",
        ));
    }
    *count = count
        .checked_add(1)
        .ok_or_else(|| ExternalError::new("E_RESOURCE_LIMIT", "descriptor value count overflow"))?;
    if *count > MAX_DESCRIPTOR_VALUES {
        return Err(ExternalError::new(
            "E_RESOURCE_LIMIT",
            "descriptor value limit exceeded",
        ));
    }
    match v {
        Value::String(s) if s.len() > MAX_STRING_BYTES => {
            return Err(ExternalError::new(
                "E_RESOURCE_LIMIT",
                "descriptor string bound exceeded",
            ))
        }
        Value::Number(n) if n.as_i64().is_none() && n.as_u64().is_none() => {
            return Err(ExternalError::new(
                "E_SCHEMA",
                "descriptor floats are forbidden",
            ))
        }
        Value::Array(a) => {
            for x in a {
                bounded_value(x, count, depth + 1)?;
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                if k.len() > MAX_STRING_BYTES {
                    return Err(ExternalError::new(
                        "E_RESOURCE_LIMIT",
                        "descriptor key bound exceeded",
                    ));
                }
                bounded_value(x, count, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl ExternalProcessorDescriptor {
    pub fn from_json(bytes: &[u8]) -> Result<Self, ExternalError> {
        if bytes.len() > MAX_DESCRIPTOR_BYTES {
            return Err(ExternalError::new(
                "E_RESOURCE_LIMIT",
                "descriptor exceeds 4 MiB",
            ));
        }
        let mut d = serde_json::Deserializer::from_slice(bytes);
        let Unique(v) = Unique::deserialize(&mut d)
            .map_err(|e| ExternalError::new("E_SYNTAX", e.to_string()))?;
        d.end()
            .map_err(|e| ExternalError::new("E_SYNTAX", e.to_string()))?;
        let mut count = 0;
        bounded_value(&v, &mut count, 0)?;
        let out: Self =
            serde_json::from_value(v).map_err(|e| ExternalError::new("E_SCHEMA", e.to_string()))?;
        out.validate()?;
        Ok(out)
    }
    pub fn validate(&self) -> Result<(), ExternalError> {
        if self.format != DESCRIPTOR_FORMAT || self.wire_version != DESCRIPTOR_WIRE_VERSION {
            return Err(ExternalError::new(
                "E_CAPABILITY",
                "unsupported external descriptor format/version",
            ));
        }
        for (v, n) in [
            (&self.id, "id"),
            (&self.version, "version"),
            (&self.abi, "abi"),
            (
                &self.state.serialization_version,
                "state.serialization_version",
            ),
            (&self.state.reset_semantics, "state.reset_semantics"),
        ] {
            bounded_string(v, n)?;
        }
        if self.ports.len() > MAX_ITEMS
            || self.config.len() > MAX_ITEMS
            || self.parameters.len() > MAX_ITEMS
            || self.parameter_rates.len() > MAX_ITEMS
            || self.feedthrough.len() > MAX_ITEMS
        {
            return Err(ExternalError::new(
                "E_RESOURCE_LIMIT",
                "descriptor collection bound exceeded",
            ));
        }
        let mut ports = BTreeMap::new();
        for p in &self.ports {
            bounded_string(&p.name, "port.name")?;
            if ports.insert(p.name.as_str(), p).is_some() {
                return Err(ExternalError::new(
                    "E_SCHEMA",
                    format!("duplicate port `{}`", p.name),
                ));
            }
            match (p.direction, p.kind) {
                (
                    ExternalPortDirection::Input,
                    ExternalPortKind::Audio | ExternalPortKind::Control,
                ) => {
                    if p.channels == 0
                        || p.cardinality.is_none()
                        || p.zero_default.is_none()
                        || p.empty_event_stream.is_some()
                    {
                        return Err(ExternalError::new(
                            "E_SCHEMA",
                            format!(
                                "audio/control input `{}` has incomplete connection policy",
                                p.name
                            ),
                        ));
                    }
                }
                (ExternalPortDirection::Input, ExternalPortKind::Events) => {
                    if p.cardinality.is_some()
                        || p.zero_default.is_some()
                        || p.empty_event_stream.is_none()
                    {
                        return Err(ExternalError::new(
                            "E_SCHEMA",
                            format!("event input `{}` has invalid connection policy", p.name),
                        ));
                    }
                }
                (ExternalPortDirection::Output, _) => {
                    if p.channels == 0 && p.kind != ExternalPortKind::Events {
                        return Err(ExternalError::new(
                            "E_RANGE",
                            format!("output `{}` has zero channels", p.name),
                        ));
                    }
                    if p.cardinality.is_some()
                        || p.zero_default.is_some()
                        || p.empty_event_stream.is_some()
                    {
                        return Err(ExternalError::new(
                            "E_SCHEMA",
                            format!("output `{}` must not declare input policy", p.name),
                        ));
                    }
                }
            }
        }
        unique_nonempty(&self.events.expression_kinds, "events.expression_kinds")?;
        unique_nonempty(&self.events.hit_keys, "events.hit_keys")?;
        unique_nonempty(&self.events.message_protocols, "events.message_protocols")?;
        let mut config = BTreeSet::new();
        for f in &self.config {
            bounded_string(&f.name, "config.name")?;
            bounded_string(&f.value_type, "config.value_type")?;
            if !config.insert(f.name.as_str()) {
                return Err(ExternalError::new(
                    "E_SCHEMA",
                    format!("duplicate config field `{}`", f.name),
                ));
            }
            if let Some(k) = &f.asset_kind {
                if !matches!(k.as_str(), "audio" | "blob" | "descriptor" | "module") {
                    return Err(ExternalError::new(
                        "E_SCHEMA",
                        format!("bad asset kind `{k}`"),
                    ));
                }
            }
            for v in [&f.default, &f.minimum, &f.maximum].into_iter().flatten() {
                let mut n = 0;
                bounded_value(v, &mut n, 0)?;
            }
        }
        let mut params = BTreeSet::new();
        for p in &self.parameters {
            for (v, n) in [
                (&p.id, "parameter.id"),
                (&p.source_name, "parameter.source_name"),
                (&p.unit, "parameter.unit"),
            ] {
                bounded_string(v, n)?;
            }
            if !params.insert(p.id.as_str()) {
                return Err(ExternalError::new(
                    "E_SCHEMA",
                    format!("duplicate parameter id `{}`", p.id),
                ));
            }
            for v in [&p.default, &p.minimum, &p.maximum] {
                let mut n = 0;
                bounded_value(v, &mut n, 0)?;
            }
        }
        let mut rates = BTreeSet::new();
        for r in &self.parameter_rates {
            if !params.contains(r.parameter_id.as_str()) {
                return Err(ExternalError::new(
                    "E_REFERENCE",
                    format!("rate references unknown parameter `{}`", r.parameter_id),
                ));
            }
            if !rates.insert(r.parameter_id.as_str()) {
                return Err(ExternalError::new(
                    "E_SCHEMA",
                    format!("duplicate rate for `{}`", r.parameter_id),
                ));
            }
            unique_nonempty(&r.interpolation, "parameter_rates.interpolation")?;
        }
        if rates.len() != params.len() {
            return Err(ExternalError::new(
                "E_SCHEMA",
                "every parameter requires exactly one parameter_rates entry",
            ));
        }
        let mut feedthrough_outputs = BTreeSet::new();
        for f in &self.feedthrough {
            if !feedthrough_outputs.insert(f.output_port.as_str()) {
                return Err(ExternalError::new(
                    "E_SCHEMA",
                    format!("duplicate feedthrough output `{}`", f.output_port),
                ));
            }
            let out = ports.get(f.output_port.as_str()).ok_or_else(|| {
                ExternalError::new(
                    "E_REFERENCE",
                    format!("feedthrough output `{}` is unknown", f.output_port),
                )
            })?;
            if out.direction != ExternalPortDirection::Output {
                return Err(ExternalError::new(
                    "E_PORT_TYPE",
                    format!("feedthrough `{}` is not an output", f.output_port),
                ));
            }
            unique_nonempty(&f.input_ports, "feedthrough.input_ports")?;
            unique_nonempty(&f.parameters, "feedthrough.parameters")?;
            for i in &f.input_ports {
                let p = ports.get(i.as_str()).ok_or_else(|| {
                    ExternalError::new("E_REFERENCE", format!("feedthrough input `{i}` is unknown"))
                })?;
                if p.direction != ExternalPortDirection::Input {
                    return Err(ExternalError::new(
                        "E_PORT_TYPE",
                        format!("feedthrough `{i}` is not an input"),
                    ));
                }
            }
            for p in &f.parameters {
                if !params.contains(p.as_str()) {
                    return Err(ExternalError::new(
                        "E_REFERENCE",
                        format!("feedthrough parameter `{p}` is unknown"),
                    ));
                }
            }
        }
        unique_nonempty(&self.determinism.dependencies, "determinism.dependencies")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalDependencyBytes {
    pub implementation: Vec<u8>,
    pub descriptor: Vec<u8>,
    pub adapter: Vec<u8>,
    pub state: Option<Vec<u8>>,
    pub closure: BTreeMap<DependencyIdentity, Vec<u8>>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredExternalProcessor {
    pub node: Vec<String>,
    pub processor_type: String,
    pub adapter_id: String,
    pub descriptor_hash: String,
    pub descriptor: ExternalProcessorDescriptor,
    pub dependencies: ExternalDependencyBytes,
}

fn sha_uri(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn dep_key(owner: &[String], slot: &str) -> DependencyIdentity {
    DependencyIdentity {
        owner: Some(owner.to_vec()),
        role: vec!["processor".into(), slot.into()],
    }
}
fn resolve_owned_dependencies(
    lock: &Value,
    supplied: &BTreeMap<DependencyIdentity, Vec<u8>>,
    owner: &[String],
) -> Result<BTreeMap<DependencyIdentity, Vec<u8>>, ExternalError> {
    let dependencies = lock["dependencies"]
        .as_array()
        .ok_or_else(|| ExternalError::new("E_SCHEMA", "lock dependencies missing"))?;
    let mut out = BTreeMap::new();
    for dependency in dependencies {
        let Some(raw_owner) = dependency["owner"].as_array() else {
            continue;
        };
        let dependency_owner = raw_owner
            .iter()
            .map(|part| {
                part.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| ExternalError::new("E_SCHEMA", "dependency owner is not a path"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if dependency_owner != owner {
            continue;
        }
        let role = dependency["role"]
            .as_array()
            .ok_or_else(|| ExternalError::new("E_SCHEMA", "dependency role is not a path"))?
            .iter()
            .map(|part| {
                part.as_str().map(str::to_owned).ok_or_else(|| {
                    ExternalError::new("E_SCHEMA", "dependency role is not a string")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let identity = DependencyIdentity {
            owner: Some(dependency_owner),
            role,
        };
        let bytes = supplied.get(&identity).ok_or_else(|| {
            ExternalError::new(
                "E_REFERENCE",
                format!(
                    "locked dependency {:?} bytes were not supplied",
                    identity.role
                ),
            )
        })?;
        let expected = dependency["sha256"]
            .as_str()
            .ok_or_else(|| ExternalError::new("E_SCHEMA", "invalid locked dependency digest"))?;
        let expected_len = dependency["bytes"]
            .as_str()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| ExternalError::new("E_SCHEMA", "invalid locked dependency length"))?;
        if bytes.len() != expected_len || sha_uri(bytes) != expected {
            return Err(ExternalError::new(
                "E_DIGEST",
                format!(
                    "locked dependency {:?} bytes differ from lock",
                    identity.role
                ),
            ));
        }
        out.insert(identity, bytes.clone());
    }
    Ok(out)
}

fn closure_slot(
    closure: &BTreeMap<DependencyIdentity, Vec<u8>>,
    owner: &[String],
    slot: &str,
) -> Result<Vec<u8>, ExternalError> {
    closure.get(&dep_key(owner, slot)).cloned().ok_or_else(|| {
        ExternalError::new(
            "E_CLOSURE",
            format!("missing locked processor/{slot} dependency"),
        )
    })
}

pub fn discover_external_processors(
    lock: &GenericLock,
    supplied: &BTreeMap<DependencyIdentity, Vec<u8>>,
) -> Result<Vec<DiscoveredExternalProcessor>, ExternalError> {
    lock.validate()
        .map_err(|e| ExternalError::new(e.code, e.message))?;
    let root = lock.value();
    let mut out = Vec::new();
    for p in root["processors"]
        .as_array()
        .ok_or_else(|| ExternalError::new("E_SCHEMA", "lock processors missing"))?
    {
        if p["implementation_hash"].is_null() {
            continue;
        }
        let node = p["node"]
            .as_array()
            .ok_or_else(|| ExternalError::new("E_SCHEMA", "processor node is not a path"))?
            .iter()
            .map(|x| x.as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        let closure = resolve_owned_dependencies(root, supplied, &node)?;
        let implementation = closure_slot(&closure, &node, "implementation")?;
        let descriptor_bytes = closure_slot(&closure, &node, "descriptor")?;
        let adapter = closure_slot(&closure, &node, "adapter")?;
        let state = if p["state_hash"].is_null() {
            None
        } else {
            Some(closure_slot(&closure, &node, "state")?)
        };
        let descriptor = ExternalProcessorDescriptor::from_json(&descriptor_bytes)?;
        if state.is_some() && !descriptor.state.save_restore {
            return Err(ExternalError::new(
                "E_CAPABILITY",
                "locked processor state requires descriptor save/restore support",
            ));
        }
        let processor_type = p["processor_type"].as_str().unwrap_or_default().to_owned();
        if descriptor.id != processor_type {
            return Err(ExternalError::new(
                "E_CAPABILITY",
                format!(
                    "descriptor id `{}` does not match processor type `{processor_type}`",
                    descriptor.id
                ),
            ));
        }
        let latency = p["latency_frames"]
            .as_str()
            .and_then(|x| x.parse::<u64>().ok())
            .ok_or_else(|| ExternalError::new("E_SCHEMA", "invalid processor latency"))?;
        if descriptor.latency_frames != latency {
            return Err(ExternalError::new(
                "E_CAPABILITY",
                "descriptor latency differs from locked processor latency",
            ));
        }
        let det = p["determinism"].as_str().unwrap_or_default();
        let descriptor_det = match descriptor.determinism.mode {
            ExternalDeterminismMode::DeclaredDeterministic => "declared_deterministic",
            ExternalDeterminismMode::Nondeterministic => "nondeterministic",
        };
        if det != descriptor_det {
            return Err(ExternalError::new(
                "E_CAPABILITY",
                "descriptor determinism differs from locked processor determinism",
            ));
        }
        out.push(DiscoveredExternalProcessor {
            node,
            processor_type,
            adapter_id: p["adapter_id"].as_str().unwrap_or_default().to_owned(),
            descriptor_hash: p["descriptor_hash"].as_str().unwrap_or_default().to_owned(),
            descriptor,
            dependencies: ExternalDependencyBytes {
                implementation,
                descriptor: descriptor_bytes,
                adapter,
                state,
                closure,
            },
        });
    }
    out.sort_by(|a, b| a.node.cmp(&b.node));
    Ok(out)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExternalHostCapabilities {
    pub abi_ids: BTreeSet<String>,
    pub adapter_ids: BTreeSet<String>,
    pub permissions: ExternalPermissions,
    pub allow_nondeterministic: bool,
}
impl ExternalHostCapabilities {
    pub fn authorize(&self, p: &DiscoveredExternalProcessor) -> Result<(), ExternalError> {
        if !self.abi_ids.contains(&p.descriptor.abi) {
            return Err(ExternalError::new(
                "E_CAPABILITY",
                format!("ABI `{}` is not advertised by the host", p.descriptor.abi),
            ));
        }
        if !self.adapter_ids.contains(&p.adapter_id) {
            return Err(ExternalError::new(
                "E_CAPABILITY",
                format!("adapter `{}` is not advertised by the host", p.adapter_id),
            ));
        }
        if !p.descriptor.permissions.allowed_by(self.permissions) {
            return Err(ExternalError::new(
                "E_PERMISSION",
                "processor requests permissions not granted by the host",
            ));
        }
        if p.descriptor.determinism.mode == ExternalDeterminismMode::Nondeterministic
            && !self.allow_nondeterministic
        {
            return Err(ExternalError::new(
                "E_CAPABILITY",
                "host policy rejects nondeterministic external processors",
            ));
        }
        Ok(())
    }
}

impl From<LockError> for ExternalError {
    fn from(e: LockError) -> Self {
        Self::new(e.code, e.message)
    }
}
