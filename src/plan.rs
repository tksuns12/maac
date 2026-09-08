//! Versioned, standalone performance plans.
//!
//! A performance plan is the boundary between source resolution and execution.
//! It contains only resolved timing, processor topology, automation, and stable
//! source addresses.  The wire representation is intentionally stricter than a
//! general JSON value: every rational is a canonical string and every object
//! rejects unknown fields.  Imported plans and plans assembled in memory use
//! the same [`Plan::validate`] implementation.

use crate::diagnostic::{Diagnostic, DiagnosticCode, Span};
pub use crate::exact::Rational;
use crate::exact::{parse_rational_with_limit, rational_parts, RationalError, MAX_RATIONAL_BITS};
use crate::graph::{InstrumentProgram, ParameterRate, ParameterSpec};
use crate::instrument_plan::InstrumentResources;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

pub const LEGACY_PLAN_VERSION: u32 = 1;
pub const PLAN_VERSION: u32 = 2;

const MAX_RATIONAL_DECIMAL_DIGITS: usize = 1_235;

/// Published hard limits for imported and in-memory plans.
///
/// The limits are deliberately part of the public API.  Hosts can report the
/// same limit that rejected a plan instead of silently dropping events or
/// reducing precision.  The default profile is the foundation's 48 kHz,
/// mono/stereo profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanLimits {
    pub max_json_bytes: usize,
    pub max_events: usize,
    pub max_nodes: usize,
    pub max_connections: usize,
    pub max_automation_points: usize,
    pub max_tempo_points: usize,
    pub max_regions: usize,
    pub max_source_mappings: usize,
    pub max_objects: usize,
    pub max_id_bytes: usize,
    pub max_string_bytes: usize,
    pub max_total_string_bytes: usize,
    pub max_rational_bits: u64,
    pub max_duration_seconds: u32,
    pub max_rate_hz: u32,
    pub max_channels: u8,
    pub max_voices: u32,
    pub max_voice_work: u64,
    pub max_work: u64,
    pub max_instrument_programs: usize,
    pub max_graph_nodes: usize,
    pub max_graph_edges: usize,
    pub max_instrument_controls: usize,
    pub max_wavetables: usize,
    pub max_embedded_samples: usize,
    pub max_instrument_voices: u32,
    pub max_voice_graph_states: usize,
    pub max_execution_work: u64,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_json_bytes: Self::MAX_JSON_BYTES,
            max_events: Self::MAX_EVENTS,
            max_nodes: Self::MAX_NODES,
            max_connections: Self::MAX_CONNECTIONS,
            max_automation_points: Self::MAX_AUTOMATION_POINTS,
            max_tempo_points: Self::MAX_TEMPO_POINTS,
            max_regions: Self::MAX_REGIONS,
            max_source_mappings: Self::MAX_SOURCE_MAPPINGS,
            max_objects: Self::MAX_OBJECTS,
            max_id_bytes: Self::MAX_ID_BYTES,
            max_string_bytes: Self::MAX_STRING_BYTES,
            max_total_string_bytes: Self::MAX_TOTAL_STRING_BYTES,
            max_rational_bits: MAX_RATIONAL_BITS,
            max_duration_seconds: Self::MAX_DURATION_SECONDS,
            max_rate_hz: Self::MAX_RATE_HZ,
            max_channels: Self::MAX_CHANNELS,
            max_voices: Self::MAX_VOICES,
            max_voice_work: Self::MAX_VOICE_WORK,
            max_work: Self::MAX_WORK,
            max_instrument_programs: crate::graph::MAX_GRAPH_PROGRAMS,
            max_graph_nodes: crate::graph::MAX_TOTAL_GRAPH_NODES,
            max_graph_edges: crate::graph::MAX_TOTAL_GRAPH_EDGES,
            max_instrument_controls: crate::graph::MAX_INSTRUMENT_CONTROLS,
            max_wavetables: 64,
            max_embedded_samples: crate::graph::MAX_EMBEDDED_SAMPLES,
            max_instrument_voices: 4_096,
            max_voice_graph_states: crate::graph::MAX_ALLOCATED_VOICE_GRAPH_STATES,
            max_execution_work: crate::graph::MAX_EXECUTION_WORK,
        }
    }
}

impl PlanLimits {
    pub const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
    pub const MAX_EVENTS: usize = 100_000;
    pub const MAX_NODES: usize = 256;
    pub const MAX_CONNECTIONS: usize = 4_096;
    pub const MAX_AUTOMATION_POINTS: usize = 65_536;
    pub const MAX_TEMPO_POINTS: usize = 4_096;
    pub const MAX_REGIONS: usize = 16_384;
    pub const MAX_SOURCE_MAPPINGS: usize = 100_000;
    pub const MAX_OBJECTS: usize = 200_000;
    pub const MAX_ID_BYTES: usize = 128;
    pub const MAX_STRING_BYTES: usize = 4_096;
    pub const MAX_TOTAL_STRING_BYTES: usize = 4 * 1024 * 1024;
    pub const MAX_DURATION_SECONDS: u32 = 30 * 60;
    pub const MAX_RATE_HZ: u32 = 48_000;
    pub const MAX_CHANNELS: u8 = 2;
    pub const MAX_VOICES: u32 = 1_000_000;
    pub const MAX_VOICE_WORK: u64 = 5_000_000;
    pub const MAX_WORK: u64 = 20_000_000;
    pub const MAX_INSTRUMENT_PROGRAMS: usize = crate::graph::MAX_GRAPH_PROGRAMS;
    pub const MAX_GRAPH_NODES: usize = crate::graph::MAX_TOTAL_GRAPH_NODES;
    pub const MAX_GRAPH_EDGES: usize = crate::graph::MAX_TOTAL_GRAPH_EDGES;
    pub const MAX_INSTRUMENT_CONTROLS: usize = crate::graph::MAX_INSTRUMENT_CONTROLS;
    pub const MAX_WAVETABLES: usize = 64;
    pub const MAX_EMBEDDED_SAMPLES: usize = crate::graph::MAX_EMBEDDED_SAMPLES;
    pub const MAX_INSTRUMENT_VOICES: u32 = 4_096;
    pub const MAX_VOICE_GRAPH_STATES: usize = crate::graph::MAX_ALLOCATED_VOICE_GRAPH_STATES;
    pub const MAX_EXECUTION_WORK: u64 = crate::graph::MAX_EXECUTION_WORK;

    /// Apply the foundation ceiling to a caller-supplied profile.  Hosts may
    /// tighten a limit for a smaller sandbox, but cannot use a larger profile
    /// to bypass the published safety bounds of MaaC/1.
    fn bounded(self) -> Self {
        Self {
            max_json_bytes: self.max_json_bytes.min(Self::MAX_JSON_BYTES),
            max_events: self.max_events.min(Self::MAX_EVENTS),
            max_nodes: self.max_nodes.min(Self::MAX_NODES),
            max_connections: self.max_connections.min(Self::MAX_CONNECTIONS),
            max_automation_points: self.max_automation_points.min(Self::MAX_AUTOMATION_POINTS),
            max_tempo_points: self.max_tempo_points.min(Self::MAX_TEMPO_POINTS),
            max_regions: self.max_regions.min(Self::MAX_REGIONS),
            max_source_mappings: self.max_source_mappings.min(Self::MAX_SOURCE_MAPPINGS),
            max_objects: self.max_objects.min(Self::MAX_OBJECTS),
            max_id_bytes: self.max_id_bytes.min(Self::MAX_ID_BYTES),
            max_string_bytes: self.max_string_bytes.min(Self::MAX_STRING_BYTES),
            max_total_string_bytes: self
                .max_total_string_bytes
                .min(Self::MAX_TOTAL_STRING_BYTES),
            max_rational_bits: self.max_rational_bits.min(MAX_RATIONAL_BITS),
            max_duration_seconds: self.max_duration_seconds.min(Self::MAX_DURATION_SECONDS),
            max_rate_hz: self.max_rate_hz.min(Self::MAX_RATE_HZ),
            max_channels: self.max_channels.min(Self::MAX_CHANNELS),
            max_voices: self.max_voices.min(Self::MAX_VOICES),
            max_voice_work: self.max_voice_work.min(Self::MAX_VOICE_WORK),
            max_work: self.max_work.min(Self::MAX_WORK),
            max_instrument_programs: self
                .max_instrument_programs
                .min(Self::MAX_INSTRUMENT_PROGRAMS),
            max_graph_nodes: self.max_graph_nodes.min(Self::MAX_GRAPH_NODES),
            max_graph_edges: self.max_graph_edges.min(Self::MAX_GRAPH_EDGES),
            max_instrument_controls: self
                .max_instrument_controls
                .min(Self::MAX_INSTRUMENT_CONTROLS),
            max_wavetables: self.max_wavetables.min(Self::MAX_WAVETABLES),
            max_embedded_samples: self.max_embedded_samples.min(Self::MAX_EMBEDDED_SAMPLES),
            max_instrument_voices: self.max_instrument_voices.min(Self::MAX_INSTRUMENT_VOICES),
            max_voice_graph_states: self
                .max_voice_graph_states
                .min(Self::MAX_VOICE_GRAPH_STATES),
            max_execution_work: self.max_execution_work.min(Self::MAX_EXECUTION_WORK),
        }
    }
}

/// A stable validation failure.  `code` is an API string so new plan-local
/// checks can be introduced without changing the shared diagnostic enum.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanError {
    pub code: String,
    pub path: String,
    pub message: String,
    pub span: Option<Span>,
}

impl PlanError {
    fn new(code: impl Into<String>, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            message: message.into(),
            span: None,
        }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn diagnostic(&self) -> Diagnostic {
        let code = match self.code.as_str() {
            "E_VERSION" => DiagnosticCode::Version,
            "E_SYNTAX" | "E_RATIONAL" => DiagnosticCode::Syntax,
            "E_DUPLICATE_ID" => DiagnosticCode::DuplicateId,
            "E_DUPLICATE_FIELD" => DiagnosticCode::DuplicateField,
            "E_UNKNOWN_FIELD" => DiagnosticCode::UnknownField,
            "E_UNKNOWN_KIND" => DiagnosticCode::UnknownKind,
            "E_REFERENCE" => DiagnosticCode::Reference,
            "E_RANGE" => DiagnosticCode::Range,
            "E_TEMPO" => DiagnosticCode::Tempo,
            "E_INTERVAL" => DiagnosticCode::Interval,
            "E_TIME_PRECISION" => DiagnosticCode::TimePrecision,
            "E_SUBSAMPLE_NOTE" => DiagnosticCode::SubsampleNote,
            "E_AUTOMATION_WRITER" => DiagnosticCode::AutomationWriter,
            "E_CAPABILITY" => DiagnosticCode::Capability,
            "E_PORT_TYPE" => DiagnosticCode::PortType,
            "E_ALGEBRAIC_LOOP" => DiagnosticCode::AlgebraicLoop,
            "E_VOICE_LIMIT" => DiagnosticCode::VoiceLimit,
            "E_NONFINITE" => DiagnosticCode::Nonfinite,
            "E_RESOURCE_LIMIT" => DiagnosticCode::ResourceLimit,
            _ => DiagnosticCode::Range,
        };
        let mut diagnostic = Diagnostic::error(code, self.message.clone(), self.span);
        if !self.path.is_empty() {
            let (object_path, field_path) = diagnostic_paths(&self.path);
            if !object_path.is_empty() {
                diagnostic = diagnostic.object_path(object_path);
            }
            if !field_path.is_empty() {
                diagnostic = diagnostic.field_path(field_path);
            }
        }
        diagnostic
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}: {}", self.code, self.message)
        } else {
            write!(f, "{} at {}: {}", self.code, self.path, self.message)
        }
    }
}

impl std::error::Error for PlanError {}

fn err(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> PlanError {
    PlanError::new(code, path, message)
}

fn schema_error(message: impl Into<String>) -> PlanError {
    PlanError::new("E_SYNTAX", "", message)
}

fn serde_error_code(message: &str) -> &'static str {
    if message.contains("E_DUPLICATE_FIELD") || message.contains("duplicate field") {
        "E_DUPLICATE_FIELD"
    } else if message.contains("unknown field") {
        "E_UNKNOWN_FIELD"
    } else if message.contains("unknown variant") {
        "E_UNKNOWN_KIND"
    } else if message.contains("E_RESOURCE_LIMIT") {
        "E_RESOURCE_LIMIT"
    } else {
        "E_SYNTAX"
    }
}

fn diagnostic_paths(path: &str) -> (Vec<String>, Vec<String>) {
    let tokens = path_tokens(path);
    let Some(root) = tokens.first() else {
        return (Vec::new(), Vec::new());
    };
    let collection = matches!(
        root.as_str(),
        "nodes" | "connections" | "events" | "automation" | "regions" | "source_mappings"
    );
    let field_name = tokens.get(1).is_some_and(|token| {
        matches!(
            token.as_str(),
            "id" | "processor"
                | "params"
                | "from"
                | "to"
                | "address"
                | "source"
                | "target"
                | "kind"
                | "score_on_q"
                | "score_off_q"
                | "onset_offset_seconds"
                | "release_offset_seconds"
                | "on_seconds"
                | "off_seconds"
                | "release_velocity"
                | "on_frame"
                | "off_frame"
                | "clock"
                | "at"
                | "points"
                | "start_q"
                | "end_q"
                | "label"
                | "object"
                | "path"
                | "span"
        )
    });
    if collection && tokens.len() >= 2 && !field_name {
        return (tokens[..2].to_vec(), tokens[2..].to_vec());
    }
    (vec![root.clone()], tokens[1..].to_vec())
}

fn path_tokens(path: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for segment in path.split('.') {
        let mut rest = segment;
        while let Some(open) = rest.find('[') {
            let prefix = &rest[..open];
            if !prefix.is_empty() {
                tokens.push(prefix.to_owned());
            }
            let after_open = &rest[open + 1..];
            let Some(close) = after_open.find(']') else {
                if !after_open.is_empty() {
                    tokens.push(after_open.to_owned());
                }
                rest = "";
                break;
            };
            tokens.push(after_open[..close].to_owned());
            rest = &after_open[close + 1..];
        }
        if !rest.is_empty() {
            tokens.push(rest.to_owned());
        }
    }
    tokens
}

fn parse_wire_rational(input: &str) -> Result<Rational, PlanError> {
    // Do this before BigInt parsing.  A valid 4 MiB JSON document can still
    // contain a single pathological integer; parsing it first would defeat
    // the rational-bit resource bound.
    let (numerator, denominator) = input.split_once('/').ok_or_else(|| {
        err(
            "E_SYNTAX",
            "",
            "rational must use numerator/denominator form",
        )
    })?;
    let unsigned_numerator = numerator.strip_prefix('-').unwrap_or(numerator);
    if unsigned_numerator.len() > MAX_RATIONAL_DECIMAL_DIGITS
        || denominator.len() > MAX_RATIONAL_DECIMAL_DIGITS
    {
        return Err(err(
            "E_RESOURCE_LIMIT",
            "",
            "rational exceeds the bounded canonical decimal form",
        ));
    }
    if unsigned_numerator.is_empty()
        || denominator.is_empty()
        || !unsigned_numerator.bytes().all(|b| b.is_ascii_digit())
        || !denominator.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(err(
            "E_SYNTAX",
            "",
            "rational contains invalid decimal digits",
        ));
    }
    let value = parse_rational_with_limit(input, MAX_RATIONAL_BITS).map_err(|e| match e {
        RationalError::ResourceLimit => err("E_RESOURCE_LIMIT", "", e.to_string()),
        RationalError::ZeroDenominator => err("E_RANGE", "", e.to_string()),
        RationalError::InvalidSyntax => err("E_SYNTAX", "", e.to_string()),
    })?;
    let (n, d) = rational_parts(&value);
    if format!("{n}/{d}") != input {
        return Err(err(
            "E_SYNTAX",
            "",
            "rational must be reduced with a positive denominator",
        ));
    }
    Ok(value)
}

pub mod rational_serde {
    use super::*;

    pub fn serialize<S>(value: &Rational, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (n, d) = rational_parts(value);
        serializer.serialize_str(&format!("{n}/{d}"))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Rational, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        parse_wire_rational(&text).map_err(de::Error::custom)
    }
}

pub mod optional_rational_serde {
    use super::*;

    pub fn serialize<S>(value: &Option<Rational>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => rational_serde::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Rational>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        value
            .map(|text| parse_wire_rational(&text).map_err(de::Error::custom))
            .transpose()
    }
}

pub mod rational_map_serde {
    use super::*;

    pub fn serialize<S>(
        value: &BTreeMap<String, Rational>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire: BTreeMap<&str, String> = value
            .iter()
            .map(|(key, value)| {
                let (n, d) = rational_parts(value);
                (key.as_str(), format!("{n}/{d}"))
            })
            .collect();
        wire.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<String, Rational>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RationalMapVisitor;

        impl<'de> Visitor<'de> for RationalMapVisitor {
            type Value = BTreeMap<String, Rational>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object of parameter names to canonical rationals")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, text)) = map.next_entry::<String, String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(
                            "E_DUPLICATE_FIELD: duplicate parameter name",
                        ));
                    }
                    let value = parse_wire_rational(&text).map_err(de::Error::custom)?;
                    values.insert(key, value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_map(RationalMapVisitor)
    }
}

fn zero() -> Rational {
    Rational::from_integer(BigInt::zero())
}

fn one() -> Rational {
    Rational::from_integer(BigInt::one())
}

fn rational_bit_limit(
    value: &Rational,
    limits: &PlanLimits,
    path: impl Into<String>,
) -> Result<(), PlanError> {
    let path = path.into();
    // `BigRational::new_raw` is public and can construct an invalid ratio in
    // memory.  Reject it before comparisons or clock arithmetic: a zero
    // denominator otherwise reaches num-rational's arithmetic and can panic.
    if value.denom().is_zero() || value.denom().is_negative() {
        return Err(err(
            "E_NONFINITE",
            path,
            "rational denominator must be positive",
        ));
    }
    if value.numer().magnitude().bits() > limits.max_rational_bits
        || value.denom().magnitude().bits() > limits.max_rational_bits
    {
        return Err(err(
            "E_RESOURCE_LIMIT",
            path,
            "rational numerator or denominator exceeds the plan bit limit",
        ));
    }
    Ok(())
}

fn rational_ceil_nonnegative(value: &Rational) -> Option<u64> {
    if value.is_negative() {
        return None;
    }
    let (quotient, remainder) = value.numer().div_rem(value.denom());
    let rounded = if remainder.is_zero() {
        quotient
    } else {
        quotient + BigInt::one()
    };
    rounded.to_u64()
}

fn rational_to_f64(value: &Rational) -> Option<f64> {
    value
        .numer()
        .to_f64()
        .zip(value.denom().to_f64())
        .map(|(n, d)| n / d)
}

fn finite_engine_rational(value: &Rational, path: impl Into<String>) -> Result<(), PlanError> {
    if rational_to_f64(value).is_none_or(|value| !value.is_finite()) {
        return Err(err(
            "E_NONFINITE",
            path,
            "numeric value cannot be represented as a finite engine number",
        ));
    }
    Ok(())
}

/// A node/port address.  Port names are kept separate from object paths so a
/// parameter target cannot be confused with an audio connection.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRef {
    pub node: String,
    pub port: String,
}

impl PortRef {
    pub fn new(node: impl Into<String>, port: impl Into<String>) -> Result<Self, PlanError> {
        let value = Self {
            node: node.into(),
            port: port.into(),
        };
        validate_identifier(&value.node, "node")?;
        validate_identifier(&value.port, "port")?;
        Ok(value)
    }
}

pub type EventTarget = PortRef;

/// The core processors and reusable instrument instances supported by plans.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Processor {
    Sine {
        voices: u32,
    },
    #[serde(alias = "onepole")]
    OnePole {
        channels: u8,
    },
    Pan,
    Sum {
        channels: u8,
    },
    Instrument {
        program: String,
        voices: u32,
        channels: u8,
    },
}

impl Processor {
    pub fn sine(voices: u32) -> Self {
        Self::Sine { voices }
    }

    pub fn one_pole(channels: u8) -> Self {
        Self::OnePole { channels }
    }

    pub fn pan() -> Self {
        Self::Pan
    }

    pub fn sum(channels: u8) -> Self {
        Self::Sum { channels }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Sine { .. } => "sine",
            Self::OnePole { .. } => "onepole",
            Self::Pan => "pan",
            Self::Sum { .. } => "sum",
            Self::Instrument { .. } => "instrument",
        }
    }

    fn accepts_events(&self) -> bool {
        matches!(self, Self::Sine { .. } | Self::Instrument { .. })
    }

    fn parameter_allowed(&self, parameter: &str) -> bool {
        match self {
            Self::Sine { .. } => matches!(parameter, "attack" | "release" | "level"),
            Self::OnePole { .. } => parameter == "cutoff",
            Self::Pan => parameter == "pan",
            Self::Sum { .. } => false,
            Self::Instrument { .. } => false,
        }
    }

    fn voice_capacity(&self) -> u64 {
        match self {
            Self::Sine { voices } => u64::from(*voices),
            Self::Instrument { .. } => 0,
            _ => 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub processor: Processor,
    #[serde(default, with = "rational_map_serde")]
    pub params: BTreeMap<String, Rational>,
}

impl Node {
    pub fn new(id: impl Into<String>, processor: Processor) -> Result<Self, PlanError> {
        let id = id.into();
        validate_identifier(&id, "node.id")?;
        Ok(Self {
            id,
            processor,
            params: BTreeMap::new(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub from: PortRef,
    pub to: PortRef,
}

impl Connection {
    pub fn new(id: impl Into<String>, from: PortRef, to: PortRef) -> Result<Self, PlanError> {
        let id = id.into();
        validate_identifier(&id, "connection.id")?;
        Ok(Self { id, from, to })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Step,
    Linear,
    Exponential,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TempoPoint {
    #[serde(with = "rational_serde")]
    pub q: Rational,
    #[serde(with = "rational_serde")]
    pub bpm: Rational,
    pub shape: Interpolation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TempoMap {
    pub points: Vec<TempoPoint>,
}

impl TempoMap {
    /// Convert a plan map into the checked exact clock used by the musical
    /// resolver.  This keeps graph validation and compiler timing on one
    /// implementation of endpoint extension and inverse scheduling.
    pub fn checked(&self) -> Result<crate::music::TempoMap, PlanError> {
        let points = self
            .points
            .iter()
            .map(|point| {
                crate::music::TempoPoint::new(
                    point.q.clone(),
                    point.bpm.clone(),
                    match point.shape {
                        Interpolation::Step => crate::music::TempoShape::Step,
                        Interpolation::Linear => crate::music::TempoShape::Linear,
                        Interpolation::Exponential => crate::music::TempoShape::Linear,
                    },
                )
            })
            .collect();
        crate::music::TempoMap::new(points)
            .map_err(|error| err(error.code.as_str(), "tempo", error.message))
    }

    /// Exact score-to-seconds conversion for the standalone step-tempo
    /// profile. `T(0)=0`; callers subtract the project score-origin value.
    pub fn seconds_at(&self, q: &Rational) -> Result<Rational, PlanError> {
        self.checked().and_then(|tempo| {
            tempo
                .seconds_at(q)
                .map_err(|error| err(error.code.as_str(), "tempo", error.message))
        })
    }

    /// Exact inverse of [`TempoMap::seconds_at`] for step tempos.
    pub fn score_at(&self, seconds: &Rational) -> Result<Rational, PlanError> {
        self.checked().and_then(|tempo| {
            tempo
                .q_at(seconds)
                .map_err(|error| err(error.code.as_str(), "tempo", error.message))
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationClock {
    Score,
    Seconds,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationPoint {
    #[serde(with = "rational_serde")]
    pub position: Rational,
    #[serde(with = "rational_serde")]
    pub value: Rational,
    pub shape: Interpolation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Automation {
    pub id: String,
    pub target: PortRef,
    pub clock: AutomationClock,
    #[serde(with = "rational_serde")]
    pub at: Rational,
    pub points: Vec<AutomationPoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMapping {
    pub object: String,
    pub path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}

/// Optional source byte span retained alongside a resolved address.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventKind {
    Note {
        /// Resolved pitch is binary64 because equal temperament and cents
        /// transforms are generally irrational.  Source/timing quantities
        /// remain exact rationals elsewhere in the plan.
        pitch_hz: f64,
        #[serde(with = "rational_serde")]
        velocity: Rational,
    },
    Hit {
        key: String,
        #[serde(with = "rational_serde")]
        velocity: Rational,
    },
    Message {
        bytes: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedEvent {
    pub address: String,
    pub source: SourceMapping,
    pub target: EventTarget,
    pub kind: EventKind,
    #[serde(with = "rational_serde")]
    pub score_on_q: Rational,
    #[serde(default, with = "optional_rational_serde")]
    pub score_off_q: Option<Rational>,
    #[serde(with = "rational_serde")]
    pub onset_offset_seconds: Rational,
    #[serde(with = "rational_serde")]
    pub release_offset_seconds: Rational,
    #[serde(with = "rational_serde")]
    pub on_seconds: Rational,
    #[serde(default, with = "optional_rational_serde")]
    pub off_seconds: Option<Rational>,
    pub release_velocity: f64,
    pub on_frame: u64,
    #[serde(default)]
    pub off_frame: Option<u64>,
    #[serde(default)]
    pub order: i32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSettings {
    #[serde(with = "rational_serde")]
    pub score_start_q: Rational,
    #[serde(with = "rational_serde")]
    pub score_end_q: Rational,
    #[serde(with = "rational_serde")]
    pub tail_seconds: Rational,
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub total_frames: u64,
    pub output: PortRef,
}

impl OutputSettings {
    /// Naming used by the scheduling specification for the reset origin.
    pub fn score_origin_q(&self) -> &Rational {
        &self.score_start_q
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub id: String,
    #[serde(with = "rational_serde")]
    pub start_q: Rational,
    #[serde(with = "rational_serde")]
    pub end_q: Rational,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plan {
    pub version: u32,
    pub output: OutputSettings,
    pub tempo: TempoMap,
    #[serde(default)]
    pub events: Vec<ResolvedEvent>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub automation: Vec<Automation>,
    #[serde(default)]
    pub regions: Vec<Region>,
    #[serde(default)]
    pub source_mappings: Vec<SourceMapping>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruments: Option<InstrumentResources>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanWire {
    version: u32,
    output: OutputSettings,
    tempo: TempoMap,
    #[serde(default)]
    events: Vec<ResolvedEvent>,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    connections: Vec<Connection>,
    #[serde(default)]
    automation: Vec<Automation>,
    #[serde(default)]
    regions: Vec<Region>,
    #[serde(default)]
    source_mappings: Vec<SourceMapping>,
    #[serde(default)]
    instruments: Option<InstrumentResources>,
}

impl From<PlanWire> for Plan {
    fn from(value: PlanWire) -> Self {
        Self {
            version: value.version,
            output: value.output,
            tempo: value.tempo,
            events: value.events,
            nodes: value.nodes,
            connections: value.connections,
            automation: value.automation,
            regions: value.regions,
            source_mappings: value.source_mappings,
            instruments: value.instruments,
        }
    }
}

impl<'de> Deserialize<'de> for Plan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PlanWire::deserialize(deserializer)?;
        let plan: Plan = wire.into();
        plan.validate().map_err(de::Error::custom)?;
        Ok(plan)
    }
}

impl Plan {
    /// Decode an untrusted JSON plan.  The byte limit is checked before JSON
    /// allocation and rational strings are bounded before BigInt conversion.
    pub fn from_json(bytes: &[u8]) -> Result<Self, PlanError> {
        let limits = PlanLimits::default();
        if bytes.len() > limits.max_json_bytes {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "json",
                format!("plan JSON exceeds {} bytes", limits.max_json_bytes),
            ));
        }
        let wire: PlanWire = serde_json::from_slice(bytes).map_err(|e| {
            let message = e.to_string();
            PlanError::new(serde_error_code(&message), "json", message)
        })?;
        let plan: Plan = wire.into();
        plan.validate()?;
        Ok(plan)
    }

    pub fn from_json_str(input: &str) -> Result<Self, PlanError> {
        Self::from_json(input.as_bytes())
    }

    /// Serialize a validated plan in compact canonical JSON and enforce the
    /// same 4 MiB artifact bound used by import.
    pub fn to_json(&self) -> Result<Vec<u8>, PlanError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| schema_error(e.to_string()))?;
        if bytes.len() > PlanLimits::default().max_json_bytes {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "json",
                format!(
                    "serialized plan exceeds {} bytes",
                    PlanLimits::default().max_json_bytes
                ),
            ));
        }
        Ok(bytes)
    }

    pub fn to_json_string(&self) -> Result<String, PlanError> {
        String::from_utf8(self.to_json()?).map_err(|error| schema_error(error.to_string()))
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        self.validate_with_limits(&PlanLimits::default())
    }

    pub fn validate_with_limits(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let limits = limits.bounded();
        match self.version {
            LEGACY_PLAN_VERSION => {
                if self.instruments.is_some()
                    || self
                        .nodes
                        .iter()
                        .any(|node| matches!(node.processor, Processor::Instrument { .. }))
                {
                    return Err(err(
                        "E_VERSION",
                        "version",
                        "version 1 plans cannot contain version 2 instrument resources",
                    ));
                }
            }
            PLAN_VERSION => {
                if self.instruments.is_none() {
                    return Err(err(
                        "E_REFERENCE",
                        "instruments",
                        "version 2 plans require embedded instrument resources",
                    ));
                }
            }
            _ => {
                return Err(err(
                    "E_VERSION",
                    "version",
                    format!("unsupported performance-plan version {}", self.version),
                ));
            }
        }
        self.validate_counts(&limits)?;
        if let Some(resources) = &self.instruments {
            resources.validate()?;
        }
        // Structural/resource limits are cheap and must run before any exact
        // clock integration.  Tempo validation still precedes output/event
        // time arithmetic below, so malformed BPM values cannot reach a
        // clock operation.
        self.validate_tempo(&limits)?;
        self.validate_global_ids()?;
        self.validate_nodes(&limits)?;
        self.validate_output(&limits)?;
        self.validate_automation(&limits)?;
        self.validate_connections(&limits)?;
        self.validate_events(&limits)?;
        self.validate_regions(&limits)?;
        self.validate_source_mappings(&limits)?;
        self.validate_instrument_work(&limits)?;
        Ok(())
    }

    /// Look up a reusable graph program embedded in this standalone plan.
    pub fn instrument_program(&self, id: &str) -> Option<&InstrumentProgram> {
        self.instruments
            .as_ref()?
            .programs
            .iter()
            .find(|program| program.id == id)
    }

    /// Return the processor metadata for an instrument instance's public
    /// control. Core processors and unknown controls return `None`.
    pub fn instrument_control_spec(&self, node: &Node, control: &str) -> Option<ParameterSpec> {
        let Processor::Instrument { program, .. } = &node.processor else {
            return None;
        };
        self.instrument_program(program)?.control_spec(control)
    }

    /// Resolve all public instrument controls, filling omitted instance values
    /// from their program defaults. The node is validated as part of lookup.
    pub fn resolved_node_params(
        &self,
        node: &Node,
    ) -> Result<BTreeMap<String, Rational>, PlanError> {
        let Processor::Instrument { program, .. } = &node.processor else {
            return Ok(node.params.clone());
        };
        let program = self.instrument_program(program).ok_or_else(|| {
            err(
                "E_REFERENCE",
                format!("nodes.{}.processor.program", node.id),
                "instrument program does not exist",
            )
        })?;
        let mut params = program
            .controls
            .iter()
            .map(|(name, control)| (name.clone(), control.default.clone()))
            .collect::<BTreeMap<_, _>>();
        for (name, value) in &node.params {
            let spec = program.control_spec(name).ok_or_else(|| {
                err(
                    "E_UNKNOWN_FIELD",
                    format!("nodes.{}.params.{name}", node.id),
                    "instrument has no such public control",
                )
            })?;
            validate_parameter_spec(value, &spec, format!("nodes.{}.params.{name}", node.id))?;
            params.insert(name.clone(), value.clone());
        }
        Ok(params)
    }

    fn validate_counts(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let automation_points = self
            .automation
            .iter()
            .map(|lane| lane.points.len())
            .fold(0usize, usize::saturating_add);
        let parameter_count = self
            .nodes
            .iter()
            .map(|node| node.params.len())
            .fold(0usize, usize::saturating_add);
        let span_count = self
            .events
            .iter()
            .filter(|event| event.source.span.is_some())
            .count()
            .saturating_add(
                self.source_mappings
                    .iter()
                    .filter(|mapping| mapping.span.is_some())
                    .count(),
            );
        let path_component_count = self
            .events
            .iter()
            .map(|event| event.source.path.len())
            .fold(0usize, usize::saturating_add)
            .saturating_add(
                self.source_mappings
                    .iter()
                    .map(|mapping| mapping.path.len())
                    .fold(0usize, usize::saturating_add),
            );
        let binary_payload_bytes = self
            .events
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::Message { bytes } => Some(bytes.len()),
                _ => None,
            })
            .fold(0usize, usize::saturating_add);
        let mut resource_objects = 0usize;
        if let Some(resources) = &self.instruments {
            let graph_nodes = resources.programs.iter().fold(0usize, |total, program| {
                total
                    .saturating_add(program.voice.nodes.len())
                    .saturating_add(program.shared.as_ref().map_or(0, |graph| graph.nodes.len()))
            });
            let graph_edges = resources.programs.iter().fold(0usize, |total, program| {
                let count = |graph: &crate::graph::GraphProgram| {
                    graph
                        .connections
                        .len()
                        .saturating_add(graph.modulations.len())
                };
                total
                    .saturating_add(count(&program.voice))
                    .saturating_add(program.shared.as_ref().map_or(0, count))
            });
            let raw_samples = resources.wavetables.iter().fold(0usize, |total, table| {
                total.saturating_add(table.samples.len())
            });
            if resources.programs.len() > limits.max_instrument_programs
                || graph_nodes > limits.max_graph_nodes
                || graph_edges > limits.max_graph_edges
                || resources.wavetables.len() > limits.max_wavetables
                || raw_samples > limits.max_embedded_samples
                || resources
                    .programs
                    .iter()
                    .any(|program| program.controls.len() > limits.max_instrument_controls)
            {
                return Err(err(
                    "E_RESOURCE_LIMIT",
                    "instruments",
                    "instrument resource limit exceeded",
                ));
            }

            let voice_graph_states = self.nodes.iter().fold(0usize, |total, node| {
                let Processor::Instrument {
                    program, voices, ..
                } = &node.processor
                else {
                    return total;
                };
                let graph_nodes = resources
                    .programs
                    .iter()
                    .find(|candidate| candidate.id == *program)
                    .map_or(0, |program| program.voice.nodes.len());
                total.saturating_add(graph_nodes.saturating_mul(*voices as usize))
            });
            if voice_graph_states > limits.max_voice_graph_states {
                return Err(err(
                    "E_RESOURCE_LIMIT",
                    "instruments",
                    "declared instrument voice graph state exceeds the plan limit",
                ));
            }

            resource_objects = resources
                .programs
                .len()
                .saturating_add(resources.wavetables.len())
                .saturating_add(resources.wavetable_sources.len())
                .saturating_add(resources.source_files.len())
                .saturating_add(resources.dependencies.len())
                .saturating_add(resources.libraries.len())
                .saturating_add(graph_nodes)
                .saturating_add(graph_edges)
                .saturating_add(
                    resources
                        .programs
                        .iter()
                        .map(|program| {
                            let graph_params = std::iter::once(&program.voice)
                                .chain(program.shared.iter())
                                .map(|graph| {
                                    graph
                                        .nodes
                                        .iter()
                                        .map(|node| node.params.len())
                                        .fold(0usize, usize::saturating_add)
                                })
                                .fold(0usize, usize::saturating_add);
                            2usize
                                .saturating_add(program.shared.is_some() as usize)
                                .saturating_add(program.controls.len())
                                .saturating_add(graph_params)
                                .saturating_add(program.source.span.is_some() as usize)
                        })
                        .fold(0usize, usize::saturating_add),
                );

            for (program_index, program) in resources.programs.iter().enumerate() {
                if let Some(span) = program.source.span {
                    if span.start > span.end || span.end > limits.max_json_bytes {
                        return Err(err(
                            "E_RANGE",
                            format!("instruments.programs[{program_index}].source.span"),
                            "program source span is not a bounded half-open byte range",
                        ));
                    }
                }
                for (stage, graph) in std::iter::once(("voice", &program.voice))
                    .chain(program.shared.as_ref().map(|graph| ("shared", graph)))
                {
                    for (node_index, node) in graph.nodes.iter().enumerate() {
                        for (name, value) in &node.params {
                            rational_bit_limit(
                                value,
                                limits,
                                format!(
                                    "instruments.programs[{program_index}].{stage}.nodes[{node_index}].params.{name}"
                                ),
                            )?;
                        }
                    }
                    for (edge_index, modulation) in graph.modulations.iter().enumerate() {
                        rational_bit_limit(
                            &modulation.depth,
                            limits,
                            format!(
                                "instruments.programs[{program_index}].{stage}.modulations[{edge_index}].depth"
                            ),
                        )?;
                    }
                }
                for (name, control) in &program.controls {
                    rational_bit_limit(
                        &control.default,
                        limits,
                        format!("instruments.programs[{program_index}].controls.{name}.default"),
                    )?;
                }
            }
        }
        if binary_payload_bytes > limits.max_json_bytes {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "events",
                "event payload bytes exceed the plan limit",
            ));
        }
        if self.events.len() > limits.max_events
            || self.nodes.len() > limits.max_nodes
            || self.connections.len() > limits.max_connections
            || automation_points > limits.max_automation_points
            || self.tempo.points.len() > limits.max_tempo_points
            || self.regions.len() > limits.max_regions
            || self.source_mappings.len() > limits.max_source_mappings
        {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "plan",
                "plan object or collection limit exceeded",
            ));
        }
        let objects = self
            .nodes
            .len()
            .saturating_add(self.connections.len())
            .saturating_add(self.events.len())
            .saturating_add(self.automation.len())
            .saturating_add(self.regions.len())
            .saturating_add(self.source_mappings.len())
            .saturating_add(self.tempo.points.len())
            .saturating_add(automation_points)
            .saturating_add(parameter_count)
            .saturating_add(span_count)
            .saturating_add(path_component_count)
            .saturating_add(resource_objects);
        if objects > limits.max_objects {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "plan",
                "aggregate plan object limit exceeded",
            ));
        }
        // Work is intentionally a simple, published upper bound over all
        // graph and schedule records.  Processor-specific voice work is
        // checked separately in `validate_events`.
        let work = self
            .nodes
            .len()
            .saturating_add(self.connections.len())
            .saturating_add(self.events.len())
            .saturating_add(self.automation.len())
            .saturating_add(automation_points)
            .saturating_add(self.regions.len())
            .saturating_add(self.source_mappings.len())
            .saturating_add(self.tempo.points.len())
            .saturating_add(parameter_count)
            .saturating_add(span_count)
            .saturating_add(path_component_count)
            .saturating_add(resource_objects);
        if work > limits.max_work as usize {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "plan",
                "aggregate plan work exceeds the plan limit",
            ));
        }

        let mut string_bytes = 0usize;
        let mut count_string = |value: &str, path: String| -> Result<(), PlanError> {
            if value.len() > limits.max_string_bytes {
                return Err(err(
                    "E_RESOURCE_LIMIT",
                    path,
                    "string exceeds the plan string limit",
                ));
            }
            string_bytes = string_bytes.saturating_add(value.len());
            Ok(())
        };
        for node in &self.nodes {
            count_string(&node.id, format!("nodes.{}", node.id))?;
            if let Processor::Instrument { program, .. } = &node.processor {
                count_string(program, format!("nodes.{}.processor.program", node.id))?;
            }
            for key in node.params.keys() {
                count_string(key, format!("nodes.{}.params", node.id))?;
            }
        }
        for connection in &self.connections {
            count_string(&connection.id, format!("connections.{}", connection.id))?;
            count_string(&connection.from.node, "connections.from.node".into())?;
            count_string(&connection.from.port, "connections.from.port".into())?;
            count_string(&connection.to.node, "connections.to.node".into())?;
            count_string(&connection.to.port, "connections.to.port".into())?;
        }
        for event in &self.events {
            count_string(&event.address, "events.address".into())?;
            count_string(&event.source.object, "events.source.object".into())?;
            for component in &event.source.path {
                count_string(component, "events.source.path".into())?;
            }
            if let EventKind::Hit { key, .. } = &event.kind {
                count_string(key, "events.kind.key".into())?;
            }
            if let Some(span) = event.source.span {
                if span.start > span.end || span.end > limits.max_json_bytes {
                    return Err(err(
                        "E_RANGE",
                        "events.source.span",
                        "source span is not a bounded half-open byte range",
                    ));
                }
            }
            count_string(&event.target.node, "events.target.node".into())?;
            count_string(&event.target.port, "events.target.port".into())?;
        }
        for lane in &self.automation {
            count_string(&lane.id, format!("automation.{}", lane.id))?;
            count_string(&lane.target.node, "automation.target.node".into())?;
            count_string(&lane.target.port, "automation.target.port".into())?;
        }
        for mapping in &self.source_mappings {
            count_string(&mapping.object, "source_mappings.object".into())?;
            for component in &mapping.path {
                count_string(component, "source_mappings.path".into())?;
            }
            if let Some(span) = mapping.span {
                if span.start > span.end || span.end > limits.max_json_bytes {
                    return Err(err(
                        "E_RANGE",
                        "source_mappings.span",
                        "source span is not a bounded half-open byte range",
                    ));
                }
            }
        }
        for region in &self.regions {
            count_string(&region.id, format!("regions.{}", region.id))?;
            if let Some(label) = &region.label {
                count_string(label, format!("regions.{}.label", region.id))?;
            }
        }
        if let Some(resources) = &self.instruments {
            count_string(&resources.entry_source, "instruments.entry_source".into())?;
            for (program_index, program) in resources.programs.iter().enumerate() {
                let prefix = format!("instruments.programs[{program_index}]");
                count_string(&program.id, format!("{prefix}.id"))?;
                count_string(&program.source.file, format!("{prefix}.source.file"))?;
                count_string(&program.source.object, format!("{prefix}.source.object"))?;
                for (stage, graph) in std::iter::once(("voice", &program.voice))
                    .chain(program.shared.as_ref().map(|graph| ("shared", graph)))
                {
                    count_string(&graph.output.node, format!("{prefix}.{stage}.output.node"))?;
                    count_string(&graph.output.port, format!("{prefix}.{stage}.output.port"))?;
                    if let Some(amplitude) = &graph.amplitude {
                        count_string(amplitude, format!("{prefix}.{stage}.amplitude"))?;
                    }
                    for (node_index, node) in graph.nodes.iter().enumerate() {
                        let node_path = format!("{prefix}.{stage}.nodes[{node_index}]");
                        count_string(&node.id, format!("{node_path}.id"))?;
                        if let crate::graph::GraphProcessor::Wavetable { table } = &node.processor {
                            count_string(table, format!("{node_path}.processor.table"))?;
                        }
                        for name in node.params.keys() {
                            count_string(name, format!("{node_path}.params"))?;
                        }
                    }
                    for (edge_index, connection) in graph.connections.iter().enumerate() {
                        let edge_path = format!("{prefix}.{stage}.connections[{edge_index}]");
                        count_string(&connection.id, format!("{edge_path}.id"))?;
                        count_string(&connection.from.node, format!("{edge_path}.from.node"))?;
                        count_string(&connection.from.port, format!("{edge_path}.from.port"))?;
                        count_string(&connection.to.node, format!("{edge_path}.to.node"))?;
                        count_string(&connection.to.port, format!("{edge_path}.to.port"))?;
                    }
                    for (edge_index, modulation) in graph.modulations.iter().enumerate() {
                        let edge_path = format!("{prefix}.{stage}.modulations[{edge_index}]");
                        count_string(&modulation.id, format!("{edge_path}.id"))?;
                        count_string(&modulation.from.node, format!("{edge_path}.from.node"))?;
                        count_string(&modulation.from.port, format!("{edge_path}.from.port"))?;
                        count_string(&modulation.to.node, format!("{edge_path}.to.node"))?;
                        count_string(
                            &modulation.to.parameter,
                            format!("{edge_path}.to.parameter"),
                        )?;
                    }
                }
                for (name, control) in &program.controls {
                    let control_path = format!("{prefix}.controls.{name}");
                    count_string(name, control_path.clone())?;
                    count_string(&control.target.node, format!("{control_path}.target.node"))?;
                    count_string(
                        &control.target.parameter,
                        format!("{control_path}.target.parameter"),
                    )?;
                }
            }
            for (index, table) in resources.wavetables.iter().enumerate() {
                count_string(&table.id, format!("instruments.wavetables[{index}].id"))?;
            }
            for (index, source) in resources.wavetable_sources.iter().enumerate() {
                let prefix = format!("instruments.wavetable_sources[{index}]");
                count_string(&source.table, format!("{prefix}.table"))?;
                count_string(&source.file, format!("{prefix}.file"))?;
                count_string(&source.object, format!("{prefix}.object"))?;
                count_string(&source.path, format!("{prefix}.path"))?;
                count_string(&source.hash, format!("{prefix}.hash"))?;
            }
            for (index, source) in resources.source_files.iter().enumerate() {
                count_string(
                    &source.path,
                    format!("instruments.source_files[{index}].path"),
                )?;
                count_string(
                    &source.hash,
                    format!("instruments.source_files[{index}].hash"),
                )?;
            }
            for (index, dependency) in resources.dependencies.iter().enumerate() {
                let prefix = format!("instruments.dependencies[{index}]");
                count_string(&dependency.source, format!("{prefix}.source"))?;
                count_string(&dependency.alias, format!("{prefix}.alias"))?;
                count_string(&dependency.path, format!("{prefix}.path"))?;
                count_string(&dependency.hash, format!("{prefix}.hash"))?;
            }
            for (index, library) in resources.libraries.iter().enumerate() {
                let prefix = format!("instruments.libraries[{index}]");
                count_string(&library.file, format!("{prefix}.file"))?;
                count_string(&library.object, format!("{prefix}.object"))?;
                count_string(&library.version, format!("{prefix}.version"))?;
                if let Some(creator) = &library.creator {
                    count_string(creator, format!("{prefix}.creator"))?;
                }
                if let Some(license) = &library.license {
                    count_string(license, format!("{prefix}.license"))?;
                }
            }
        }
        count_string(&self.output.output.node, "output.output.node".into())?;
        count_string(&self.output.output.port, "output.output.port".into())?;
        if string_bytes > limits.max_total_string_bytes {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "plan",
                "aggregate plan string limit exceeded",
            ));
        }
        Ok(())
    }

    fn validate_output(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let output = &self.output;
        validate_rational_fields(
            [
                (&output.score_start_q, "output.score_start_q"),
                (&output.score_end_q, "output.score_end_q"),
                (&output.tail_seconds, "output.tail_seconds"),
            ],
            limits,
        )?;
        if output.score_start_q >= output.score_end_q {
            return Err(err(
                "E_INTERVAL",
                "output.score",
                "score end must be after score start",
            ));
        }
        if output.tail_seconds.is_negative() {
            return Err(err(
                "E_RANGE",
                "output.tail_seconds",
                "tail must be nonnegative",
            ));
        }
        if output.sample_rate_hz == 0 {
            return Err(err(
                "E_RANGE",
                "output.sample_rate_hz",
                "sample rate must be positive",
            ));
        }
        if output.sample_rate_hz != 48_000 || output.sample_rate_hz > limits.max_rate_hz {
            return Err(err(
                "E_CAPABILITY",
                "output.sample_rate_hz",
                "standalone plans require the 48 kHz foundation profile",
            ));
        }
        if output.channels == 0 || output.channels > limits.max_channels {
            return Err(err(
                "E_RANGE",
                "output.channels",
                "only mono and stereo output are supported",
            ));
        }
        validate_identifier_limit(
            &output.output.node,
            "output.output.node",
            limits.max_id_bytes,
        )?;
        validate_identifier_limit(
            &output.output.port,
            "output.output.port",
            limits.max_id_bytes,
        )?;
        let output_node = self
            .nodes
            .iter()
            .find(|node| node.id == output.output.node)
            .ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    "output.output.node",
                    "output node does not exist",
                )
            })?;
        let output_port =
            port_descriptor(output_node, &output.output.port, false).ok_or_else(|| {
                err(
                    "E_PORT_TYPE",
                    "output.output",
                    "output must name an audio output port",
                )
            })?;
        if output_port.kind != PortKind::Audio || output_port.channels != output.channels {
            return Err(err(
                "E_PORT_TYPE",
                "output.output",
                "output port channels do not match output settings",
            ));
        }
        let end_seconds = self.tempo.seconds_at(&output.score_end_q)?;
        let start_seconds = self.tempo.seconds_at(&output.score_start_q)?;
        let duration = end_seconds - start_seconds + output.tail_seconds.clone();
        if duration.is_negative() {
            return Err(err(
                "E_INTERVAL",
                "output",
                "render duration must be positive",
            ));
        }
        let max_duration = Rational::from_integer(BigInt::from(limits.max_duration_seconds));
        if duration > max_duration {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "output",
                "render duration exceeds the 30 minute limit",
            ));
        }
        let expected = rational_ceil_nonnegative(&(duration * BigInt::from(output.sample_rate_hz)))
            .ok_or_else(|| {
                err(
                    "E_TIME_PRECISION",
                    "output.total_frames",
                    "frame ceiling overflow",
                )
            })?;
        if output.total_frames != expected {
            return Err(err(
                "E_INTERVAL",
                "output.total_frames",
                format!("expected {expected} frames, found {}", output.total_frames),
            ));
        }
        Ok(())
    }

    fn validate_global_ids(&self) -> Result<(), PlanError> {
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            if !ids.insert(node.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("nodes.{}", node.id),
                    "duplicate plan object ID",
                ));
            }
        }
        for connection in &self.connections {
            if !ids.insert(connection.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("connections.{}", connection.id),
                    "duplicate plan object ID",
                ));
            }
        }
        for lane in &self.automation {
            if !ids.insert(lane.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("automation.{}", lane.id),
                    "duplicate plan object ID",
                ));
            }
        }
        for region in &self.regions {
            if !ids.insert(region.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("regions.{}", region.id),
                    "duplicate plan object ID",
                ));
            }
        }
        Ok(())
    }

    fn validate_tempo(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        if self.tempo.points.is_empty() {
            return Err(err(
                "E_TEMPO",
                "tempo.points",
                "tempo map must not be empty",
            ));
        }
        let mut previous: Option<&Rational> = None;
        for (index, point) in self.tempo.points.iter().enumerate() {
            rational_bit_limit(&point.q, limits, format!("tempo.points[{index}].q"))?;
            rational_bit_limit(&point.bpm, limits, format!("tempo.points[{index}].bpm"))?;
            if point.bpm <= zero() {
                return Err(err(
                    "E_TEMPO",
                    format!("tempo.points[{index}].bpm"),
                    "tempo must be positive",
                ));
            }
            if let Some(previous) = previous {
                if point.q <= *previous {
                    return Err(err(
                        "E_TEMPO",
                        "tempo.points",
                        "tempo positions must increase",
                    ));
                }
            }
            if point.shape != Interpolation::Step {
                return Err(err(
                    "E_CAPABILITY",
                    format!("tempo.points[{index}].shape"),
                    "standalone plans require step tempo for exact inverse scheduling",
                ));
            }
            previous = Some(&point.q);
        }
        self.tempo.checked()?;
        Ok(())
    }

    fn validate_nodes(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            validate_identifier_limit(&node.id, "nodes.id", limits.max_id_bytes)?;
            if !ids.insert(node.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("nodes.{}", node.id),
                    "duplicate node ID",
                ));
            }
            match &node.processor {
                Processor::Sine { voices } => {
                    if *voices == 0 {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor.voices", node.id),
                            "sine voice capacity must be positive",
                        ));
                    }
                    if *voices > limits.max_voices {
                        return Err(err(
                            "E_RESOURCE_LIMIT",
                            format!("nodes.{}.processor.voices", node.id),
                            "sine voice capacity exceeds the published limit",
                        ));
                    }
                }
                Processor::OnePole { channels } | Processor::Sum { channels } => {
                    if *channels == 0 || *channels > limits.max_channels {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor", node.id),
                            "channels must be mono or stereo",
                        ));
                    }
                }
                Processor::Pan => {}
                Processor::Instrument {
                    program,
                    voices,
                    channels,
                } => {
                    validate_identifier_limit(
                        program,
                        format!("nodes.{}.processor.program", node.id),
                        limits.max_id_bytes,
                    )?;
                    if *voices == 0 {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor.voices", node.id),
                            "instrument voice capacity must be positive",
                        ));
                    }
                    if *voices > limits.max_instrument_voices {
                        return Err(err(
                            "E_RESOURCE_LIMIT",
                            format!("nodes.{}.processor.voices", node.id),
                            "instrument voice capacity exceeds the published limit",
                        ));
                    }
                    if *channels == 0 || *channels > limits.max_channels {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor.channels", node.id),
                            "instrument channels must be mono or stereo",
                        ));
                    }
                    let program = self.instrument_program(program).ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            format!("nodes.{}.processor.program", node.id),
                            "instrument program does not exist",
                        )
                    })?;
                    if program.channels() != *channels {
                        return Err(err(
                            "E_PORT_TYPE",
                            format!("nodes.{}.processor.channels", node.id),
                            "instrument instance channels differ from its program",
                        ));
                    }
                }
            }
            for (parameter, value) in &node.params {
                rational_bit_limit(
                    value,
                    limits,
                    format!("nodes.{}.params.{}", node.id, parameter),
                )?;
                if let Processor::Instrument { program, .. } = &node.processor {
                    let spec = self
                        .instrument_program(program)
                        .and_then(|program| program.control_spec(parameter))
                        .ok_or_else(|| {
                            err(
                                "E_UNKNOWN_FIELD",
                                format!("nodes.{}.params.{}", node.id, parameter),
                                "instrument has no such public control",
                            )
                        })?;
                    validate_parameter_spec(
                        value,
                        &spec,
                        format!("nodes.{}.params.{}", node.id, parameter),
                    )?;
                } else {
                    if !node.processor.parameter_allowed(parameter) {
                        return Err(err(
                            "E_UNKNOWN_FIELD",
                            format!("nodes.{}.params.{}", node.id, parameter),
                            format!(
                                "parameter is not supported by core.{}",
                                node.processor.kind()
                            ),
                        ));
                    }
                    validate_parameter(
                        &node.processor,
                        parameter,
                        value,
                        &node.id,
                        self.output.sample_rate_hz,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn validate_connections(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let node_by_id: HashMap<&str, &Node> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let mut connection_ids = BTreeSet::new();
        let mut destinations: HashMap<(String, String), usize> = HashMap::new();
        let mut edges: Vec<(String, String)> = Vec::new();
        for connection in &self.connections {
            validate_identifier_limit(&connection.id, "connections.id", limits.max_id_bytes)?;
            if !connection_ids.insert(connection.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("connections.{}", connection.id),
                    "duplicate connection ID",
                ));
            }
            let from_node = node_by_id
                .get(connection.from.node.as_str())
                .ok_or_else(|| {
                    err(
                        "E_REFERENCE",
                        format!("connections.{}.from", connection.id),
                        "source node does not exist",
                    )
                })?;
            let to_node = node_by_id.get(connection.to.node.as_str()).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    format!("connections.{}.to", connection.id),
                    "destination node does not exist",
                )
            })?;
            let from =
                port_descriptor(from_node, &connection.from.port, false).ok_or_else(|| {
                    err(
                        "E_PORT_TYPE",
                        format!("connections.{}.from", connection.id),
                        "source port is not an output",
                    )
                })?;
            let to = port_descriptor(to_node, &connection.to.port, true).ok_or_else(|| {
                err(
                    "E_PORT_TYPE",
                    format!("connections.{}.to", connection.id),
                    "destination port is not an input",
                )
            })?;
            if from.kind != to.kind || from.channels != to.channels {
                return Err(err(
                    "E_PORT_TYPE",
                    format!("connections.{}", connection.id),
                    "connection port kinds or channels differ",
                ));
            }
            let destination = (connection.to.node.clone(), connection.to.port.clone());
            let count = destinations.entry(destination).or_default();
            *count += 1;
            if !to.summing && *count > 1 {
                return Err(err(
                    "E_PORT_TYPE",
                    format!("connections.{}.to", connection.id),
                    "single-input port has multiple connections",
                ));
            }
            if from.kind == PortKind::Audio {
                edges.push((connection.from.node.clone(), connection.to.node.clone()));
            }
        }
        for node in &self.nodes {
            let requires_audio_input =
                matches!(node.processor, Processor::OnePole { .. } | Processor::Pan);
            if requires_audio_input
                && !destinations.contains_key(&(node.id.clone(), "in".to_owned()))
            {
                return Err(err(
                    "E_PORT_TYPE",
                    format!("nodes.{}.in", node.id),
                    "required audio input is disconnected",
                ));
            }
        }
        detect_cycle(&self.nodes, &edges)
    }

    fn validate_events(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let node_by_id: HashMap<&str, &Node> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let origin_seconds = self.tempo.seconds_at(&self.output.score_start_q)?;
        let end_seconds = self.tempo.seconds_at(&self.output.score_end_q)?;
        let mut addresses = BTreeSet::new();
        let mut voice_work = 0u64;
        for (index, event) in self.events.iter().enumerate() {
            validate_event_address(&event.address, format!("events[{index}].address"), limits)?;
            if !addresses.insert(event.address.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("events[{index}].address"),
                    "event address must be unique and nonempty",
                ));
            }
            let target = node_by_id.get(event.target.node.as_str()).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    format!("events[{index}].target"),
                    "event target node does not exist",
                )
            })?;
            if !target.processor.accepts_events() || event.target.port != "events" {
                return Err(err(
                    "E_CAPABILITY",
                    format!("events[{index}].target"),
                    "target does not accept native note events",
                ));
            }
            rational_bit_limit(
                &event.score_on_q,
                limits,
                format!("events[{index}].score_on_q"),
            )?;
            if event.score_on_q < self.output.score_start_q
                || event.score_on_q >= self.output.score_end_q
            {
                return Err(err(
                    "E_INTERVAL",
                    format!("events[{index}].score_on_q"),
                    "event onset lies outside the score",
                ));
            }
            if let Some(score_off) = &event.score_off_q {
                rational_bit_limit(score_off, limits, format!("events[{index}].score_off_q"))?;
                if *score_off <= event.score_on_q {
                    return Err(err(
                        "E_INTERVAL",
                        format!("events[{index}].score_off_q"),
                        "event gate must be positive",
                    ));
                }
            }
            rational_bit_limit(
                &event.onset_offset_seconds,
                limits,
                format!("events[{index}].onset_offset_seconds"),
            )?;
            rational_bit_limit(
                &event.release_offset_seconds,
                limits,
                format!("events[{index}].release_offset_seconds"),
            )?;
            rational_bit_limit(
                &event.on_seconds,
                limits,
                format!("events[{index}].on_seconds"),
            )?;
            let expected_on_seconds =
                self.tempo.seconds_at(&event.score_on_q)? + event.onset_offset_seconds.clone();
            if event.on_seconds != expected_on_seconds {
                return Err(err(
                    "E_INTERVAL",
                    format!("events[{index}].on_seconds"),
                    "resolved onset does not match score time plus physical offset",
                ));
            }
            if event.on_seconds >= end_seconds {
                return Err(err(
                    "E_INTERVAL",
                    format!("events[{index}].on_seconds"),
                    "resolved event onset must occur before score end",
                ));
            }
            let on_delta = event.on_seconds.clone() - origin_seconds.clone();
            let expected_on = rational_ceil_nonnegative(
                &(on_delta.clone() * BigInt::from(self.output.sample_rate_hz)),
            )
            .ok_or_else(|| {
                err(
                    "E_INTERVAL",
                    format!("events[{index}].on_seconds"),
                    "event starts before reset origin",
                )
            })?;
            if expected_on != event.on_frame {
                return Err(err(
                    "E_INTERVAL",
                    format!("events[{index}].on_frame"),
                    "event frame is not the exact ceiling of physical time",
                ));
            }
            if event.on_frame >= self.output.total_frames {
                return Err(err(
                    "E_INTERVAL",
                    format!("events[{index}].on_frame"),
                    "event onset quantizes outside the rendered frame range",
                ));
            }
            if let Some(off_seconds) = &event.off_seconds {
                rational_bit_limit(off_seconds, limits, format!("events[{index}].off_seconds"))?;
                // Check the field's representation before deriving a frame
                // delta.  In-memory callers can construct an arbitrarily
                // wide BigRational even though imported values are bounded
                // by the wire deserializer.
                let off_delta = off_seconds.clone() - origin_seconds.clone();
                let score_off = event.score_off_q.as_ref().ok_or_else(|| {
                    err(
                        "E_INTERVAL",
                        format!("events[{index}].score_off_q"),
                        "note off time is required",
                    )
                })?;
                let expected_off_seconds = (self.tempo.seconds_at(score_off)?
                    + event.release_offset_seconds.clone())
                .min(end_seconds.clone());
                if *off_seconds != expected_off_seconds {
                    return Err(err(
                        "E_INTERVAL",
                        format!("events[{index}].off_seconds"),
                        "resolved off time does not match score time plus physical offset",
                    ));
                }
                if *off_seconds <= event.on_seconds || *off_seconds > end_seconds {
                    return Err(err(
                        "E_INTERVAL",
                        format!("events[{index}].off_seconds"),
                        "event off time is outside the effective score interval",
                    ));
                }
                let expected_off = rational_ceil_nonnegative(
                    &(off_delta * BigInt::from(self.output.sample_rate_hz)),
                )
                .ok_or_else(|| {
                    err(
                        "E_TIME_PRECISION",
                        format!("events[{index}].off_frame"),
                        "event frame ceiling overflow",
                    )
                })?;
                if event.off_frame != Some(expected_off) {
                    return Err(err(
                        "E_INTERVAL",
                        format!("events[{index}].off_frame"),
                        "event off frame is not the exact ceiling of physical time",
                    ));
                }
                if matches!(event.kind, EventKind::Note { .. }) && event.on_frame == expected_off {
                    return Err(err(
                        "E_SUBSAMPLE_NOTE",
                        format!("events[{index}]"),
                        "positive note gate collapsed to one frame",
                    ));
                }
            } else if event.off_frame.is_some() {
                return Err(err(
                    "E_INTERVAL",
                    format!("events[{index}].off_frame"),
                    "event without an off time cannot carry an off frame",
                ));
            }
            match &event.kind {
                EventKind::Note { pitch_hz, velocity } => {
                    if event.score_off_q.is_none() || event.off_seconds.is_none() {
                        return Err(err(
                            "E_INTERVAL",
                            format!("events[{index}]"),
                            "note events require score and physical off times",
                        ));
                    }
                    rational_bit_limit(velocity, limits, format!("events[{index}].velocity"))?;
                    finite_engine_rational(velocity, format!("events[{index}].velocity"))?;
                    if !pitch_hz.is_finite()
                        || *pitch_hz <= 0.0
                        || *pitch_hz >= f64::from(self.output.sample_rate_hz / 2)
                    {
                        return Err(err(
                            "E_RANGE",
                            format!("events[{index}].pitch_hz"),
                            "sine pitch must be positive and below Nyquist",
                        ));
                    }
                    if velocity.is_negative() || *velocity > one() {
                        return Err(err(
                            "E_RANGE",
                            format!("events[{index}].velocity"),
                            "velocity must be finite in [0,1]",
                        ));
                    }
                    if !event.release_velocity.is_finite()
                        || event.release_velocity < 0.0
                        || event.release_velocity > 1.0
                    {
                        return Err(err(
                            "E_RANGE",
                            format!("events[{index}].release_velocity"),
                            "release velocity must be finite in [0,1]",
                        ));
                    }
                    voice_work = voice_work.saturating_add(target.processor.voice_capacity());
                }
                EventKind::Hit { velocity, .. } => {
                    // Validate the payload's exact value before reporting the
                    // foundation capability error.  In-memory callers must
                    // receive the same rational resource boundary as imported
                    // plans even for an unsupported event kind.
                    rational_bit_limit(velocity, limits, format!("events[{index}].velocity"))?;
                    finite_engine_rational(velocity, format!("events[{index}].velocity"))?;
                    if velocity.is_negative() || *velocity > one() {
                        return Err(err(
                            "E_RANGE",
                            format!("events[{index}].velocity"),
                            "velocity must be finite in [0,1]",
                        ));
                    }
                    return Err(err(
                        "E_CAPABILITY",
                        format!("events[{index}].kind"),
                        "standalone core sine plans accept notes only",
                    ));
                }
                EventKind::Message { .. } => {
                    if event.score_off_q.is_some() || event.off_seconds.is_some() {
                        return Err(err(
                            "E_INTERVAL",
                            format!("events[{index}]"),
                            "hit and message events cannot carry note off times",
                        ));
                    }
                    return Err(err(
                        "E_CAPABILITY",
                        format!("events[{index}].kind"),
                        "standalone core sine plans accept notes only",
                    ));
                }
            }
        }
        if voice_work > limits.max_voice_work {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "events",
                "bounded voice work exceeds the plan limit",
            ));
        }
        Ok(())
    }

    fn validate_automation(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let node_by_id: HashMap<&str, &Node> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let mut targets = BTreeSet::new();
        let mut lane_ids = BTreeSet::new();
        let mut work = 0u64;
        for (index, lane) in self.automation.iter().enumerate() {
            validate_identifier_limit(
                &lane.id,
                format!("automation[{index}].id"),
                limits.max_id_bytes,
            )?;
            if !lane_ids.insert(lane.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("automation[{index}].id"),
                    "duplicate automation ID",
                ));
            }
            if !targets.insert((lane.target.node.clone(), lane.target.port.clone())) {
                return Err(err(
                    "E_AUTOMATION_WRITER",
                    format!("automation[{index}].target"),
                    "parameter has more than one replacement automation",
                ));
            }
            let node = node_by_id.get(lane.target.node.as_str()).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    format!("automation[{index}].target"),
                    "automation node does not exist",
                )
            })?;
            if let Processor::Instrument { program, .. } = &node.processor {
                let spec = self
                    .instrument_program(program)
                    .and_then(|program| program.control_spec(&lane.target.port))
                    .ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            format!("automation[{index}].target"),
                            "instrument public control does not exist",
                        )
                    })?;
                if spec.rate == ParameterRate::Reset {
                    return Err(err(
                        "E_CAPABILITY",
                        format!("automation[{index}].target"),
                        "reset-rate instrument controls cannot be automated",
                    ));
                }
                for (point_index, point) in lane.points.iter().enumerate() {
                    validate_parameter_spec(
                        &point.value,
                        &spec,
                        format!("automation[{index}].points[{point_index}].value"),
                    )?;
                }
            } else {
                if !node.processor.parameter_allowed(&lane.target.port) {
                    return Err(err(
                        "E_REFERENCE",
                        format!("automation[{index}].target"),
                        "automation parameter does not exist",
                    ));
                }
                validate_automation_parameter(
                    &node.processor,
                    &lane.target.port,
                    &lane.points,
                    limits,
                    &format!("automation[{index}]"),
                    self.output.sample_rate_hz,
                )?;
            }
            rational_bit_limit(&lane.at, limits, format!("automation[{index}].at"))?;
            if lane.points.is_empty() {
                return Err(err(
                    "E_RANGE",
                    format!("automation[{index}].points"),
                    "automation requires at least one point",
                ));
            }
            let mut previous: Option<&Rational> = None;
            for (point_index, point) in lane.points.iter().enumerate() {
                rational_bit_limit(
                    &point.position,
                    limits,
                    format!("automation[{index}].points[{point_index}].position"),
                )?;
                rational_bit_limit(
                    &point.value,
                    limits,
                    format!("automation[{index}].points[{point_index}].value"),
                )?;
                if point.position.is_negative()
                    || previous.is_some_and(|previous| point.position <= *previous)
                {
                    return Err(err(
                        "E_RANGE",
                        format!("automation[{index}].points"),
                        "curve positions must be nonnegative and increasing",
                    ));
                }
                if point_index == 0 && !point.position.is_zero() {
                    return Err(err(
                        "E_RANGE",
                        format!("automation[{index}].points[0]"),
                        "curve must begin at position zero",
                    ));
                }
                previous = Some(&point.position);
            }
            for point_index in 0..lane.points.len().saturating_sub(1) {
                let left = &lane.points[point_index];
                let right = &lane.points[point_index + 1];
                if left.shape == Interpolation::Exponential
                    && (left.value <= zero() || right.value <= zero())
                {
                    return Err(err(
                        "E_RANGE",
                        format!("automation[{index}].points[{point_index}]"),
                        "exponential interpolation endpoints must be positive",
                    ));
                }
            }
            if lane
                .points
                .last()
                .is_some_and(|point| point.shape != Interpolation::Step)
            {
                return Err(err(
                    "E_RANGE",
                    format!("automation[{index}].points"),
                    "last curve shape must be step",
                ));
            }
            work = work.saturating_add(lane.points.len() as u64);
        }
        if work > limits.max_work {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "automation",
                "automation work exceeds the plan limit",
            ));
        }
        Ok(())
    }

    fn validate_regions(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let mut ids = BTreeSet::new();
        for (index, region) in self.regions.iter().enumerate() {
            validate_identifier_limit(
                &region.id,
                format!("regions[{index}].id"),
                limits.max_id_bytes,
            )?;
            if !ids.insert(region.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("regions[{index}].id"),
                    "duplicate region ID",
                ));
            }
            rational_bit_limit(&region.start_q, limits, format!("regions[{index}].start_q"))?;
            rational_bit_limit(&region.end_q, limits, format!("regions[{index}].end_q"))?;
            if region.start_q < self.output.score_start_q
                || region.end_q > self.output.score_end_q
                || region.start_q >= region.end_q
            {
                return Err(err(
                    "E_INTERVAL",
                    format!("regions[{index}]"),
                    "region must be a positive interval inside the score",
                ));
            }
        }
        Ok(())
    }

    fn validate_source_mappings(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let addresses: HashSet<&str> = self
            .events
            .iter()
            .map(|event| event.address.as_str())
            .collect();
        let mut seen = BTreeSet::new();
        for (index, mapping) in self.source_mappings.iter().enumerate() {
            validate_identifier_limit(
                &mapping.object,
                format!("source_mappings[{index}].object"),
                limits.max_id_bytes,
            )?;
            if mapping.path.is_empty() {
                return Err(err(
                    "E_REFERENCE",
                    format!("source_mappings[{index}].path"),
                    "source mapping path must not be empty",
                ));
            }
            for component in &mapping.path {
                validate_identifier_limit(
                    component,
                    format!("source_mappings[{index}].path"),
                    limits.max_id_bytes,
                )?;
            }
            if !seen.insert(mapping.object.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("source_mappings[{index}].object"),
                    "duplicate source mapping",
                ));
            }
        }
        for event in &self.events {
            if event.source.path.is_empty() || !addresses.contains(event.address.as_str()) {
                return Err(err(
                    "E_REFERENCE",
                    "events.source",
                    "event source mapping is invalid",
                ));
            }
            validate_identifier_limit(
                &event.source.object,
                "events.source.object",
                limits.max_id_bytes,
            )?;
            for component in &event.source.path {
                validate_identifier_limit(component, "events.source.path", limits.max_id_bytes)?;
            }
        }
        Ok(())
    }

    fn validate_instrument_work(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let mut work = 0u64;
        for node in &self.nodes {
            let Processor::Instrument { program, .. } = &node.processor else {
                continue;
            };
            let program = self.instrument_program(program).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    format!("nodes.{}.processor.program", node.id),
                    "instrument program does not exist",
                )
            })?;
            let graph_cost = |graph: &crate::graph::GraphProgram| {
                graph
                    .nodes
                    .len()
                    .saturating_add(graph.connections.len())
                    .saturating_add(graph.modulations.len()) as u64
            };
            if let Some(shared) = &program.shared {
                work = work
                    .saturating_add(self.output.total_frames.saturating_mul(graph_cost(shared)));
            }

            let amplitude = program.voice.amplitude.as_deref().ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    format!("instruments.programs.{}.voice.amplitude", program.id),
                    "voice graph requires an amplitude envelope",
                )
            })?;
            let amplitude_node = program
                .voice
                .nodes
                .iter()
                .find(|candidate| candidate.id == amplitude)
                .ok_or_else(|| {
                    err(
                        "E_REFERENCE",
                        format!("instruments.programs.{}.voice.amplitude", program.id),
                        "amplitude node does not exist",
                    )
                })?;
            let mut release = amplitude_node
                .params
                .get("release")
                .cloned()
                .unwrap_or_else(zero);
            if let Some((control_name, control)) = program.controls.iter().find(|(_, control)| {
                control.target.graph == crate::graph::GraphStage::Voice
                    && control.target.node == amplitude
                    && control.target.parameter == "release"
            }) {
                release = node
                    .params
                    .get(control_name)
                    .cloned()
                    .unwrap_or_else(|| control.default.clone());
                for lane in self
                    .automation
                    .iter()
                    .filter(|lane| lane.target.node == node.id && lane.target.port == *control_name)
                {
                    for point in &lane.points {
                        if point.value > release {
                            release = point.value.clone();
                        }
                    }
                }
            }
            let release_frames =
                rational_ceil_nonnegative(&(release * BigInt::from(self.output.sample_rate_hz)))
                    .ok_or_else(|| {
                        err(
                            "E_RESOURCE_LIMIT",
                            format!("nodes.{}.params.release", node.id),
                            "instrument release duration cannot be represented as frames",
                        )
                    })?;
            let voice_cost = graph_cost(&program.voice);
            for event in self.events.iter().filter(|event| {
                event.target.node == node.id && matches!(event.kind, EventKind::Note { .. })
            }) {
                let release_start = event.off_frame.unwrap_or(self.output.total_frames);
                let active_end = release_start
                    .saturating_add(release_frames)
                    .min(self.output.total_frames);
                let active_frames = active_end.saturating_sub(event.on_frame);
                work = work.saturating_add(active_frames.saturating_mul(voice_cost));
            }
        }
        if work > limits.max_execution_work {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "instruments",
                "instrument sample execution work exceeds the plan limit",
            ));
        }
        Ok(())
    }
}

fn validate_identifier(value: &str, path: impl Into<String>) -> Result<(), PlanError> {
    validate_identifier_limit(value, path, PlanLimits::default().max_id_bytes)
}

fn validate_identifier_limit(
    value: &str,
    path: impl Into<String>,
    max_id_bytes: usize,
) -> Result<(), PlanError> {
    let path = path.into();
    if value.is_empty()
        || value.len() > max_id_bytes
        || !value.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_alphabetic() || byte == b'_'
            } else {
                byte.is_ascii_alphanumeric() || byte == b'_'
            }
        })
    {
        return Err(err(
            "E_RANGE",
            path,
            "identifier is not a bounded ASCII MaaC ID",
        ));
    }
    Ok(())
}

fn validate_event_address(
    value: &str,
    path: impl Into<String>,
    limits: &PlanLimits,
) -> Result<(), PlanError> {
    let path = path.into();
    if value.is_empty() || value.len() > limits.max_string_bytes {
        return Err(err("E_RANGE", path, "event address is empty or too long"));
    }
    for component in value.split('/') {
        if component.is_empty()
            || component.len() > limits.max_id_bytes
            || !component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(err(
                "E_RANGE",
                path,
                "event address contains an invalid structural component",
            ));
        }
    }
    Ok(())
}

fn validate_rational_fields<const N: usize>(
    values: [(&Rational, &str); N],
    limits: &PlanLimits,
) -> Result<(), PlanError> {
    for (value, path) in values {
        rational_bit_limit(value, limits, path)?;
    }
    Ok(())
}

fn validate_parameter(
    processor: &Processor,
    name: &str,
    value: &Rational,
    node: &str,
    sample_rate_hz: u32,
) -> Result<(), PlanError> {
    finite_engine_rational(value, format!("nodes.{node}.params.{name}"))?;
    match (processor, name) {
        (Processor::Sine { .. }, "attack" | "release" | "level") if value.is_negative() => {
            Err(err(
                "E_RANGE",
                format!("nodes.{node}.params.{name}"),
                "parameter must be nonnegative",
            ))
        }
        (Processor::OnePole { .. }, "cutoff")
            if *value <= zero()
                || *value >= Rational::from_integer(BigInt::from(sample_rate_hz / 2)) =>
        {
            Err(err(
                "E_RANGE",
                format!("nodes.{node}.params.cutoff"),
                "cutoff must be positive and below Nyquist",
            ))
        }
        // core.pan declares clamp policy.  Preserve the raw value here; the
        // renderer clamps it when evaluating the parameter at a sample.
        (Processor::Pan, "pan") => Ok(()),
        _ => Ok(()),
    }
}

fn validate_parameter_spec(
    value: &Rational,
    spec: &ParameterSpec,
    path: impl Into<String>,
) -> Result<(), PlanError> {
    let path = path.into();
    finite_engine_rational(value, path.clone())?;
    let below = if spec.min_open {
        value <= &spec.min
    } else {
        value < &spec.min
    };
    let above = if spec.max_open {
        value >= &spec.max
    } else {
        value > &spec.max
    };
    if below || above {
        return Err(err(
            "E_RANGE",
            path,
            "instrument control value is outside its declared range",
        ));
    }
    Ok(())
}

fn validate_automation_parameter(
    processor: &Processor,
    name: &str,
    points: &[AutomationPoint],
    limits: &PlanLimits,
    path: &str,
    sample_rate_hz: u32,
) -> Result<(), PlanError> {
    for (index, point) in points.iter().enumerate() {
        rational_bit_limit(
            &point.value,
            limits,
            format!("{path}.points[{index}].value"),
        )?;
        finite_engine_rational(&point.value, format!("{path}.points[{index}].value"))?;
        match (processor, name) {
            (Processor::Sine { .. }, "attack" | "release" | "level")
                if point.value.is_negative() =>
            {
                return Err(err(
                    "E_RANGE",
                    format!("{path}.points[{index}].value"),
                    "sine automation parameter must be nonnegative",
                ));
            }
            (Processor::OnePole { .. }, "cutoff")
                if point.value <= zero()
                    || point.value >= Rational::from_integer(BigInt::from(sample_rate_hz / 2)) =>
            {
                return Err(err(
                    "E_RANGE",
                    format!("{path}.points[{index}].value"),
                    "one-pole cutoff must be positive and below Nyquist",
                ));
            }
            (Processor::Pan, "pan") => {
                // core.pan's declared policy is clamp, so all finite raw
                // values are retained for the renderer to clamp.
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PortKind {
    Audio,
    Events,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PortDescriptor {
    kind: PortKind,
    channels: u8,
    summing: bool,
}

fn port_descriptor(node: &Node, port: &str, input: bool) -> Option<PortDescriptor> {
    match (&node.processor, input, port) {
        (Processor::Sine { .. }, true, "events") => Some(PortDescriptor {
            kind: PortKind::Events,
            channels: 0,
            summing: true,
        }),
        (Processor::Sine { .. }, false, "out") => Some(PortDescriptor {
            kind: PortKind::Audio,
            channels: 1,
            summing: false,
        }),
        (Processor::OnePole { channels }, true, "in")
        | (Processor::OnePole { channels }, false, "out") => Some(PortDescriptor {
            kind: PortKind::Audio,
            channels: *channels,
            summing: false,
        }),
        (Processor::Pan, true, "in") => Some(PortDescriptor {
            kind: PortKind::Audio,
            channels: 1,
            summing: false,
        }),
        (Processor::Pan, false, "out") => Some(PortDescriptor {
            kind: PortKind::Audio,
            channels: 2,
            summing: false,
        }),
        (Processor::Sum { channels }, true, "in") => Some(PortDescriptor {
            kind: PortKind::Audio,
            channels: *channels,
            summing: true,
        }),
        (Processor::Sum { channels }, false, "out") => Some(PortDescriptor {
            kind: PortKind::Audio,
            channels: *channels,
            summing: false,
        }),
        (Processor::Instrument { .. }, true, "events") => Some(PortDescriptor {
            kind: PortKind::Events,
            channels: 0,
            summing: true,
        }),
        (Processor::Instrument { channels, .. }, false, "out") => Some(PortDescriptor {
            kind: PortKind::Audio,
            channels: *channels,
            summing: false,
        }),
        _ => None,
    }
}

fn detect_cycle(nodes: &[Node], edges: &[(String, String)]) -> Result<(), PlanError> {
    let mut indegree: HashMap<&str, usize> =
        nodes.iter().map(|node| (node.id.as_str(), 0)).collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for (from, to) in edges {
        if from == to {
            return Err(err(
                "E_ALGEBRAIC_LOOP",
                "connections",
                "same-sample graph contains a self-cycle",
            ));
        }
        *indegree.entry(to.as_str()).or_default() += 1;
        outgoing.entry(from.as_str()).or_default().push(to.as_str());
    }
    let mut queue: VecDeque<&str> = indegree
        .iter()
        .filter_map(|(node, degree)| (*degree == 0).then_some(*node))
        .collect();
    let mut visited = 0usize;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if let Some(next) = outgoing.get(node) {
            for destination in next {
                let degree = indegree.get_mut(destination).expect("edge node exists");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(destination);
                }
            }
        }
    }
    if visited != nodes.len() {
        return Err(err(
            "E_ALGEBRAIC_LOOP",
            "connections",
            "same-sample graph contains a cycle",
        ));
    }
    Ok(())
}
