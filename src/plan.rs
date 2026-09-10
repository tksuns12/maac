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
pub use crate::production_eq::EqMode;
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
    /// Maximum f64 delay cells reserved by declared pluck voice capacities.
    pub max_pluck_delay_cells: usize,
    /// Aggregate native reverb history storage, in binary64 cells.
    pub max_production_delay_cells: usize,
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
            max_pluck_delay_cells: crate::graph::MAX_PLUCK_DELAY_CELLS,
            max_production_delay_cells: Self::MAX_PRODUCTION_DELAY_CELLS,
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
    /// Hard execution ceiling available through an explicit caller profile.
    pub const MAX_SONG_EXECUTION_WORK: u64 = 10_000_000_000;
    pub const MAX_PRODUCTION_DELAY_CELLS: usize = 4_194_304;
    pub const MAX_PLUCK_DELAY_CELLS: usize = crate::graph::MAX_PLUCK_DELAY_CELLS;

    /// Authorize song-sized execution work without changing any other limit.
    pub fn song() -> Self {
        Self {
            max_execution_work: Self::MAX_SONG_EXECUTION_WORK,
            ..Self::default()
        }
    }

    /// Apply the published ceilings to a caller-supplied profile. Hosts may
    /// tighten any limit; only explicit execution work can exceed the default
    /// allowance, up to the finite song ceiling.
    pub(crate) fn bounded(self) -> Self {
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
            max_execution_work: self.max_execution_work.min(Self::MAX_SONG_EXECUTION_WORK),
            max_pluck_delay_cells: self.max_pluck_delay_cells.min(Self::MAX_PLUCK_DELAY_CELLS),
            max_production_delay_cells: self
                .max_production_delay_cells
                .min(Self::MAX_PRODUCTION_DELAY_CELLS),
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

pub(crate) fn err(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> PlanError {
    PlanError::new(code, path, message)
}

fn schema_error(message: impl Into<String>) -> PlanError {
    PlanError::new("E_SYNTAX", "", message)
}

pub(crate) fn serde_error_code(message: &str) -> &'static str {
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
    Gain {
        channels: u8,
    },
    Fader {
        channels: u8,
    },
    #[serde(rename = "fx.eq/1")]
    Eq {
        channels: u8,
        mode: EqMode,
    },
    #[serde(rename = "fx.compressor/1")]
    Compressor {
        channels: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sidechain_channels: Option<u8>,
    },
    #[serde(rename = "fx.reverb/1")]
    Reverb {
        channels: u8,
        predelay_frames: u32,
        #[serde(with = "rational_serde")]
        damping: Rational,
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

    pub fn gain(channels: u8) -> Self {
        Self::Gain { channels }
    }

    pub fn fader(channels: u8) -> Self {
        Self::Fader { channels }
    }

    pub fn pan() -> Self {
        Self::Pan
    }

    pub fn sum(channels: u8) -> Self {
        Self::Sum { channels }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Sine { .. } => "sine",
            Self::OnePole { .. } => "onepole",
            Self::Gain { .. } => "gain",
            Self::Fader { .. } => "fader",
            Self::Eq { .. } => "fx.eq/1",
            Self::Compressor { .. } => "fx.compressor/1",
            Self::Reverb { .. } => "fx.reverb/1",
            Self::Pan => "pan",
            Self::Sum { .. } => "sum",
            Self::Instrument { .. } => "instrument",
        }
    }

    fn accepts_events(&self) -> bool {
        matches!(self, Self::Sine { .. } | Self::Instrument { .. })
    }

    pub(crate) fn parameter_allowed(&self, parameter: &str) -> bool {
        match self {
            Self::Sine { .. } => matches!(parameter, "attack" | "release" | "level"),
            Self::OnePole { .. } => parameter == "cutoff",
            Self::Gain { .. } => parameter == "gain",
            Self::Fader { .. } => parameter == "level",
            Self::Eq { mode, .. } => {
                parameter == "frequency"
                    || (parameter == "q"
                        && matches!(mode, EqMode::Peak | EqMode::LowPass | EqMode::HighPass))
                    || (parameter == "gain"
                        && matches!(mode, EqMode::Peak | EqMode::LowShelf | EqMode::HighShelf))
            }
            Self::Compressor { .. } => matches!(
                parameter,
                "threshold" | "ratio" | "knee" | "attack" | "release" | "makeup"
            ),
            Self::Reverb { .. } => matches!(parameter, "decay" | "mix"),
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

/// Shared per-note expression clock on the effective scheduled gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionClock {
    Score,
    Seconds,
    Normalized,
}

/// Compatibility name retained for existing pitch-expression callers.
pub type PitchExpressionClock = ExpressionClock;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PitchExpressionPoint {
    #[serde(with = "rational_serde")]
    pub position: Rational,
    #[serde(with = "rational_serde")]
    pub cents: Rational,
    pub shape: Interpolation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PitchExpression {
    pub clock: PitchExpressionClock,
    pub points: Vec<PitchExpressionPoint>,
}

impl ExpressionClock {
    /// Exact clock coordinate; release frames hold the effective gate endpoint.
    /// Requires a validated note and expression.
    #[cfg(test)]
    pub(crate) fn coordinate_at(&self, event: &ResolvedEvent, frame: u64, rate: u32) -> Rational {
        self.coordinate_for_view(&EventView::from(event), frame, rate)
    }

    pub(crate) fn coordinate_for_view(
        &self,
        event: &EventView<'_>,
        frame: u64,
        rate: u32,
    ) -> Rational {
        let gate = event.off_frame.expect("validated note off") - event.on_frame;
        let elapsed = frame.saturating_sub(event.on_frame).min(gate);
        match self {
            ExpressionClock::Seconds => Rational::new(elapsed.into(), rate.into()),
            ExpressionClock::Normalized => Rational::new(elapsed.into(), gate.into()),
            ExpressionClock::Score => {
                Rational::new(elapsed.into(), gate.into())
                    * (event.score_off_q.as_ref().expect("validated score off") - event.score_on_q)
            }
        }
    }
}

impl PitchExpression {
    /// Right-continuous step/linear evaluation with exact knot selection.
    /// Requires validated points; arithmetic stays exact until cents conversion.
    pub(crate) fn cents_at(&self, coordinate: &Rational) -> Rational {
        let right = expression_right_index(&self.points, coordinate, |point| &point.position);
        let left = &self.points[right.saturating_sub(1)];
        if right == 0 || right == self.points.len() || left.shape == Interpolation::Step {
            return left.cents.clone();
        }
        let next = &self.points[right];
        &left.cents
            + (&next.cents - &left.cents)
                * ((coordinate - &left.position) / (&next.position - &left.position))
    }

    fn validate_for_note(
        &self,
        event: &EventView<'_>,
        base_hz: f64,
        rate: u32,
        limits: &PlanLimits,
        path: &str,
    ) -> Result<(), PlanError> {
        if self.points.is_empty() {
            return Err(err("E_RANGE", path, "pitch expression requires points"));
        }
        for (index, point) in self.points.iter().enumerate() {
            for (name, value) in [("position", &point.position), ("cents", &point.cents)] {
                let field = format!("{path}.points[{index}].{name}");
                rational_bit_limit(value, limits, &field)?;
                if value.numer().gcd(value.denom()) != BigInt::one() {
                    return Err(err(
                        "E_RATIONAL",
                        field,
                        "pitch expression rationals must be canonical",
                    ));
                }
            }
            if point.cents.to_f64().is_none_or(|value| !value.is_finite()) {
                return Err(err(
                    "E_NONFINITE",
                    format!("{path}.points[{index}].cents"),
                    "pitch cents cannot be represented as a finite engine number",
                ));
            }
            if point.position.is_negative()
                || (index == 0 && !point.position.is_zero())
                || (index > 0 && point.position <= self.points[index - 1].position)
                || point.shape == Interpolation::Exponential
            {
                return Err(err("E_RANGE", path, "pitch points must start at zero, increase, and use step or linear interpolation"));
            }
        }
        let last = self.points.last().expect("nonempty points");
        if last.shape != Interpolation::Step
            || (self.clock == PitchExpressionClock::Normalized && last.position != one())
        {
            return Err(err(
                "E_RANGE",
                path,
                "final pitch point must be step and normalized curves must end at one",
            ));
        }
        let end = self.clock.coordinate_for_view(
            event,
            event.off_frame.expect("validated note off"),
            rate,
        );
        // Linear cents and their exponential frequency transform are monotone on
        // each segment. Knots in the domain plus its endpoint bound every value;
        // future knots only contribute through interpolation at the endpoint.
        let check = |cents: &Rational| -> Result<(), PlanError> {
            let cents = cents
                .to_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| err("E_NONFINITE", path, "evaluated pitch cents must be finite"))?;
            let frequency = base_hz * 2.0_f64.powf(cents / 1200.0);
            if !frequency.is_finite() || frequency <= 0.0 || frequency >= f64::from(rate) / 2.0 {
                return Err(err(
                    "E_RANGE",
                    path,
                    "expressed pitch must be finite, positive, and below Nyquist",
                ));
            }
            Ok(())
        };
        for point in self.points.iter().take_while(|point| point.position <= end) {
            check(&point.cents)?;
        }
        check(&self.cents_at(&end))
    }
}

/// A nonnegative per-note linear-amplitude multiplier, independent of pitch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GainExpression {
    pub clock: ExpressionClock,
    pub points: Vec<GainExpressionPoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GainExpressionPoint {
    #[serde(with = "rational_serde")]
    pub position: Rational,
    #[serde(with = "rational_serde")]
    pub gain: Rational,
    pub shape: Interpolation,
}

fn expression_right_index<T>(
    points: &[T],
    coordinate: &Rational,
    position: impl Fn(&T) -> &Rational,
) -> usize {
    points.partition_point(|point| position(point) <= coordinate)
}

// Positive rational endpoints may round to zero or imprecise subnormals. Their logarithm
// remains representable: retain the high mantissa bits and the binary exponent
// without first converting an overflowing numerator or underflowing quotient.
fn positive_rational_ln(value: &Rational) -> f64 {
    if let Some(number) = value
        .to_f64()
        .filter(|number| *number > 0.0 && number.is_normal())
    {
        return number.ln();
    }
    let parts = |integer: &BigInt| {
        let shift = integer.magnitude().bits().saturating_sub(53);
        let mantissa = (integer >> shift as usize)
            .to_f64()
            .expect("53-bit mantissa");
        (mantissa, shift as f64)
    };
    let (numerator, numerator_shift) = parts(value.numer());
    let (denominator, denominator_shift) = parts(value.denom());
    (numerator / denominator).ln() + (numerator_shift - denominator_shift) * std::f64::consts::LN_2
}

fn scalar_expression_at<T>(
    points: &[T],
    coordinate: &Rational,
    fields: impl Fn(&T) -> (&Rational, &Rational, Interpolation),
) -> f64 {
    let right = expression_right_index(points, coordinate, |point| fields(point).0);
    let left = &points[right.saturating_sub(1)];
    let (left_position, left_value, left_shape) = fields(left);
    let endpoint = |value: &Rational| {
        value
            .to_f64()
            .expect("validated finite scalar expression value")
    };
    if right == 0
        || right == points.len()
        || left_shape == Interpolation::Step
        || *coordinate == *left_position
    {
        return endpoint(left_value);
    }
    let next = &points[right];
    let (next_position, next_value, _) = fields(next);
    let fraction = (coordinate - left_position) / (next_position - left_position);
    if left_shape == Interpolation::Linear {
        return endpoint(&(left_value + (next_value - left_value) * fraction));
    }
    let left_weight = (one() - &fraction).to_f64().expect("unit fraction");
    let right_weight = fraction.to_f64().expect("unit fraction");
    let interpolated = (left_weight * positive_rational_ln(left_value)
        + right_weight * positive_rational_ln(next_value))
    .exp();
    let a = endpoint(left_value);
    let b = endpoint(next_value);
    // Exponential interpolation is bounded by its endpoints. Correct only
    // floating-point exp/log roundoff; this imposes no dynamic gain limit.
    interpolated.clamp(a.min(b), a.max(b))
}

impl GainExpression {
    /// Requires validated points and a nonnegative clock coordinate. Knots and
    /// step/linear arithmetic stay exact until the final engine conversion.
    pub(crate) fn gain_at(&self, coordinate: &Rational) -> f64 {
        scalar_expression_at(&self.points, coordinate, |point| {
            (&point.position, &point.gain, point.shape)
        })
    }

    fn validate(&self, limits: &PlanLimits, path: &str) -> Result<(), PlanError> {
        if self.points.is_empty() {
            return Err(err("E_RANGE", path, "gain expression requires points"));
        }
        // Validate every raw rational before comparisons or segment arithmetic.
        for (index, point) in self.points.iter().enumerate() {
            for (name, value) in [("position", &point.position), ("gain", &point.gain)] {
                let field = format!("{path}.points[{index}].{name}");
                rational_bit_limit(value, limits, &field)?;
                if value.numer().gcd(value.denom()) != BigInt::one() {
                    return Err(err(
                        "E_RATIONAL",
                        field,
                        "gain expression rationals must be canonical",
                    ));
                }
            }
        }
        for (index, point) in self.points.iter().enumerate() {
            if point.gain.to_f64().is_none_or(|value| !value.is_finite()) {
                return Err(err(
                    "E_NONFINITE",
                    format!("{path}.points[{index}].gain"),
                    "gain cannot be represented as a finite engine number",
                ));
            }
            if point.gain.is_negative()
                || point.position.is_negative()
                || (index == 0 && !point.position.is_zero())
                || (index > 0 && point.position <= self.points[index - 1].position)
            {
                return Err(err("E_RANGE", path, "gain must be nonnegative and positions must start at zero and strictly increase"));
            }
            if point.shape == Interpolation::Exponential
                && (point.gain.is_zero()
                    || self
                        .points
                        .get(index + 1)
                        .is_none_or(|next| next.gain <= zero()))
            {
                return Err(err(
                    "E_RANGE",
                    path,
                    "exponential gain segments require strictly positive endpoints",
                ));
            }
        }
        let last = self.points.last().expect("nonempty points");
        if last.shape != Interpolation::Step
            || (self.clock == ExpressionClock::Normalized && last.position != one())
        {
            return Err(err(
                "E_RANGE",
                path,
                "final gain point must be step and normalized curves must end at one",
            ));
        }
        Ok(())
    }
}

fn validate_unit_expression<T>(
    points: &[T],
    clock: ExpressionClock,
    kind: &str,
    fields: impl Fn(&T) -> (&Rational, &Rational, Interpolation),
    limits: &PlanLimits,
    path: &str,
) -> Result<(), PlanError> {
    if points.is_empty() {
        return Err(err(
            "E_RANGE",
            path,
            format!("{kind} expression requires points"),
        ));
    }
    // Validate every raw rational before comparisons or segment arithmetic.
    for (index, point) in points.iter().enumerate() {
        let (position, value, _) = fields(point);
        for (name, value) in [("position", position), ("value", value)] {
            let field = format!("{path}.points[{index}].{name}");
            rational_bit_limit(value, limits, &field)?;
            if value.numer().gcd(value.denom()) != BigInt::one() {
                return Err(err(
                    "E_RATIONAL",
                    field,
                    format!("{kind} expression rationals must be canonical"),
                ));
            }
        }
    }
    for (index, point) in points.iter().enumerate() {
        let (position, value, shape) = fields(point);
        if value.to_f64().is_none_or(|value| !value.is_finite()) {
            return Err(err(
                "E_NONFINITE",
                format!("{path}.points[{index}].value"),
                format!("{kind} value cannot be represented as a finite engine number"),
            ));
        }
        if value.is_negative()
            || *value > one()
            || position.is_negative()
            || (index == 0 && !position.is_zero())
            || (index > 0 && *position <= *fields(&points[index - 1]).0)
        {
            return Err(err("E_RANGE", path, format!("{kind} value must be in [0,1] and positions must start at zero and strictly increase")));
        }
        if shape == Interpolation::Exponential
            && (value.is_zero()
                || points
                    .get(index + 1)
                    .is_none_or(|next| *fields(next).1 <= zero()))
        {
            return Err(err(
                "E_RANGE",
                path,
                format!("exponential {kind} segments require strictly positive endpoints"),
            ));
        }
    }
    let last = points.last().expect("nonempty points");
    if fields(last).2 != Interpolation::Step
        || (clock == ExpressionClock::Normalized && *fields(last).0 != one())
    {
        return Err(err(
            "E_RANGE",
            path,
            format!("final {kind} point must be step and normalized curves must end at one"),
        ));
    }
    Ok(())
}

/// A per-note unit-interval timbre control for opted-in instruments.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimbreExpression {
    pub clock: ExpressionClock,
    pub points: Vec<TimbreExpressionPoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimbreExpressionPoint {
    #[serde(with = "rational_serde")]
    pub position: Rational,
    #[serde(with = "rational_serde")]
    pub value: Rational,
    pub shape: Interpolation,
}

impl TimbreExpression {
    /// Requires validated points and a nonnegative clock coordinate. Knots and
    /// step/linear arithmetic stay exact until the final engine conversion.
    pub(crate) fn value_at(&self, coordinate: &Rational) -> f64 {
        scalar_expression_at(&self.points, coordinate, |point| {
            (&point.position, &point.value, point.shape)
        })
    }

    fn validate(&self, limits: &PlanLimits, path: &str) -> Result<(), PlanError> {
        validate_unit_expression(
            &self.points,
            self.clock,
            "timbre",
            |point| (&point.position, &point.value, point.shape),
            limits,
            path,
        )
    }
}

/// A per-note unit-interval pressure control for opted-in instruments.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressureExpression {
    pub clock: ExpressionClock,
    pub points: Vec<PressureExpressionPoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressureExpressionPoint {
    #[serde(with = "rational_serde")]
    pub position: Rational,
    #[serde(with = "rational_serde")]
    pub value: Rational,
    pub shape: Interpolation,
}

impl PressureExpression {
    /// Requires validated points and a nonnegative clock coordinate. Knots and
    /// step/linear arithmetic stay exact until the final engine conversion.
    pub(crate) fn value_at(&self, coordinate: &Rational) -> f64 {
        scalar_expression_at(&self.points, coordinate, |point| {
            (&point.position, &point.value, point.shape)
        })
    }

    fn validate(&self, limits: &PlanLimits, path: &str) -> Result<(), PlanError> {
        validate_unit_expression(
            &self.points,
            self.clock,
            "pressure",
            |point| (&point.position, &point.value, point.shape),
            limits,
            path,
        )
    }
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pitch_expression: Option<PitchExpression>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_expression: Option<GainExpression>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timbre_expression: Option<TimbreExpression>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pressure_expression: Option<PressureExpression>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production: Option<crate::production_data::ProductionSettings>,
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
    #[serde(default)]
    production: Option<crate::production_data::ProductionSettings>,
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
            production: value.production,
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
        Self::from_json_with_limits(bytes, &PlanLimits::default())
    }

    /// Decode the same strict wire schema under an explicit caller allowance.
    pub fn from_json_with_limits(bytes: &[u8], limits: &PlanLimits) -> Result<Self, PlanError> {
        let limits = limits.bounded();
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
        plan.validate_with_limits(&limits)?;
        Ok(plan)
    }

    pub fn from_json_str(input: &str) -> Result<Self, PlanError> {
        Self::from_json(input.as_bytes())
    }

    pub fn from_json_str_with_limits(input: &str, limits: &PlanLimits) -> Result<Self, PlanError> {
        Self::from_json_with_limits(input.as_bytes(), limits)
    }

    /// Serialize a validated plan in compact canonical JSON and enforce the
    /// same 4 MiB artifact bound used by import.
    pub fn to_json(&self) -> Result<Vec<u8>, PlanError> {
        self.to_json_with_limits(&PlanLimits::default())
    }

    /// Encode a plan validated under explicit limits, retaining the artifact cap.
    pub fn to_json_with_limits(&self, limits: &PlanLimits) -> Result<Vec<u8>, PlanError> {
        let limits = limits.bounded();
        self.validate_with_limits(&limits)?;
        let bytes = serde_json::to_vec(self).map_err(|e| schema_error(e.to_string()))?;
        if bytes.len() > limits.max_json_bytes {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "json",
                format!("serialized plan exceeds {} bytes", limits.max_json_bytes),
            ));
        }
        Ok(bytes)
    }

    pub fn to_json_string(&self) -> Result<String, PlanError> {
        String::from_utf8(self.to_json()?).map_err(|error| schema_error(error.to_string()))
    }

    pub fn to_json_string_with_limits(&self, limits: &PlanLimits) -> Result<String, PlanError> {
        String::from_utf8(self.to_json_with_limits(limits)?)
            .map_err(|error| schema_error(error.to_string()))
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        self.validate_with_limits(&PlanLimits::default())
    }

    pub(crate) fn view(&self) -> PlanView<'_> {
        PlanView {
            version: self.version,
            output: &self.output,
            tempo: &self.tempo,
            events: EventSlice::Legacy(&self.events),
            nodes: NodeSlice::Legacy(&self.nodes),
            audio_assets: None,
            connections: &self.connections,
            modulations: &[],
            automation: AutomationSlice::Legacy(&self.automation),
            regions: &self.regions,
            source_mappings: &self.source_mappings,
            instruments: self.instruments.as_ref(),
            production: self.production.as_ref(),
        }
    }
    pub fn validate_with_limits(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        if !matches!(self.version, 1 | 2) {
            return Err(err("E_VERSION", "version", "Plan requires version 1 or 2"));
        }
        self.view().validate_with_limits(limits)
    }
    /// Resolve an explicit project-level audio output without pruning graph context.
    pub fn audio_output_channels(&self, output: &PortRef) -> Result<u8, PlanError> {
        self.view().audio_output_channels(output)
    }
    /// Look up a reusable graph program embedded in this standalone plan.
    pub fn instrument_program(&self, id: &str) -> Option<&InstrumentProgram> {
        self.view().instrument_program(id)
    }
    /// Return the processor metadata for an instrument instance's public
    /// control. Core processors and unknown controls return `None`.
    pub fn instrument_control_spec(&self, node: &Node, control: &str) -> Option<ParameterSpec> {
        self.view().instrument_control_spec(node.into(), control)
    }
    /// Resolve all public instrument controls, filling omitted instance values
    /// from their program defaults. The node is validated as part of lookup.
    pub fn resolved_node_params(
        &self,
        node: &Node,
    ) -> Result<BTreeMap<String, Rational>, PlanError> {
        self.view().resolved_node_params(node.into())
    }
}

/// Borrowed common validation boundary for standalone plan versions.
#[derive(Clone, Copy)]
pub(crate) struct PlanView<'a> {
    pub(crate) version: u32,
    pub(crate) output: &'a OutputSettings,
    pub(crate) tempo: &'a TempoMap,
    pub(crate) events: EventSlice<'a>,
    pub(crate) nodes: NodeSlice<'a>,
    pub(crate) audio_assets: Option<&'a [crate::audio_asset::AudioAsset]>,
    pub(crate) connections: &'a [Connection],
    pub(crate) modulations: &'a [crate::plan_v7::ModulationV7],
    pub(crate) automation: AutomationSlice<'a>,
    pub(crate) regions: &'a [Region],
    pub(crate) source_mappings: &'a [SourceMapping],
    pub(crate) instruments: Option<&'a InstrumentResources>,
    pub(crate) production: Option<&'a crate::production_data::ProductionSettings>,
}

#[derive(Clone, Copy)]
pub(crate) enum NodeSlice<'a> {
    Legacy(&'a [Node]),
    V4(&'a [crate::plan_v4::NodeV4]),
    V5(&'a [crate::plan_v5::NodeV5]),
    V6(&'a [crate::plan_v6::NodeV6]),
    V7(&'a [crate::plan_v7::NodeV7]),
}
impl<'a> NodeSlice<'a> {
    pub(crate) fn len(self) -> usize {
        match self {
            Self::Legacy(nodes) => nodes.len(),
            Self::V4(nodes) => nodes.len(),
            Self::V5(nodes) => nodes.len(),
            Self::V6(nodes) => nodes.len(),
            Self::V7(nodes) => nodes.len(),
        }
    }
    pub(crate) fn iter(self) -> Box<dyn ExactSizeIterator<Item = NodeView<'a>> + 'a> {
        match self {
            Self::Legacy(nodes) => Box::new(nodes.iter().map(NodeView::from)),
            Self::V4(nodes) => Box::new(nodes.iter().map(NodeView::from)),
            Self::V5(nodes) => Box::new(nodes.iter().map(NodeView::from)),
            Self::V6(nodes) => Box::new(nodes.iter().map(NodeView::from)),
            Self::V7(nodes) => Box::new(nodes.iter().map(NodeView::from)),
        }
    }
    pub(crate) fn get(self, index: usize) -> Option<NodeView<'a>> {
        match self {
            Self::Legacy(nodes) => nodes.get(index).map(NodeView::from),
            Self::V4(nodes) => nodes.get(index).map(NodeView::from),
            Self::V5(nodes) => nodes.get(index).map(NodeView::from),
            Self::V6(nodes) => nodes.get(index).map(NodeView::from),
            Self::V7(nodes) => nodes.get(index).map(NodeView::from),
        }
    }
}
impl<'a> IntoIterator for NodeSlice<'a> {
    type Item = NodeView<'a>;
    type IntoIter = Box<dyn ExactSizeIterator<Item = NodeView<'a>> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ProcessorView<'a> {
    Lfo(&'a crate::plan_v7::LfoConfig),
    Constant,
    Audio(&'a crate::plan_v5::AudioClip),
    WarpRate(&'a crate::plan_v6::WarpClip),
    Core(&'a Processor),
    Kit {
        channels: u8,
        voices: u32,
        samples: &'a [crate::plan_v4::KitSampleRef],
    },
}
struct AudioTransportView<'a> {
    asset: &'a String,
    channels: u8,
    at: &'a Rational,
    speed: Option<&'a Rational>,
    source_start_frame: u64,
    source_end_frame: u64,
    gain: &'a Rational,
    fade_in_seconds: &'a Rational,
    fade_out_seconds: &'a Rational,
    source: &'a SourceMapping,
    track: &'a Option<String>,
    start_frame: u64,
    end_frame: u64,
}
impl<'a> ProcessorView<'a> {
    fn audio_transport(self) -> Option<AudioTransportView<'a>> {
        let view = match self {
            Self::Audio(clip) => AudioTransportView {
                asset: &clip.asset,
                channels: clip.channels,
                at: match &clip.at {
                    crate::plan_v3::AutomationAnchor::Score { q } => q,
                    crate::plan_v3::AutomationAnchor::Seconds { seconds } => seconds,
                },
                speed: Some(&clip.speed),
                source_start_frame: clip.source_start_frame,
                source_end_frame: clip.source_end_frame,
                gain: &clip.gain,
                fade_in_seconds: &clip.fade_in_seconds,
                fade_out_seconds: &clip.fade_out_seconds,
                source: &clip.source,
                track: &clip.track,
                start_frame: clip.start_frame,
                end_frame: clip.end_frame,
            },
            Self::WarpRate(clip) => AudioTransportView {
                asset: &clip.asset,
                channels: clip.channels,
                at: &clip.at_q,
                speed: None,
                source_start_frame: clip.source_start_frame,
                source_end_frame: clip.source_end_frame,
                gain: &clip.gain,
                fade_in_seconds: &clip.fade_in_seconds,
                fade_out_seconds: &clip.fade_out_seconds,
                source: &clip.source,
                track: &clip.track,
                start_frame: clip.start_frame,
                end_frame: clip.end_frame,
            },
            _ => return None,
        };
        Some(view)
    }

    pub(crate) fn core(self) -> Result<&'a Processor, PlanError> {
        match self {
            Self::Core(processor) => Ok(processor),
            Self::Lfo(_)
            | Self::Constant
            | Self::Kit { .. }
            | Self::Audio(_)
            | Self::WarpRate(_) => Err(err(
                "E_CAPABILITY",
                "nodes.processor",
                "non-core processor is not supported at this boundary yet",
            )),
        }
    }
}
#[derive(Clone, Copy)]
enum NodeWire<'a> {
    Legacy(&'a Node),
    V4(&'a crate::plan_v4::NodeV4),
    V5(&'a crate::plan_v5::NodeV5),
    V6(&'a crate::plan_v6::NodeV6),
    V7(&'a crate::plan_v7::NodeV7),
}
#[derive(Clone, Copy)]
pub(crate) struct NodeView<'a> {
    pub(crate) id: &'a String,
    pub(crate) processor: ProcessorView<'a>,
    pub(crate) params: &'a BTreeMap<String, Rational>,
    wire: NodeWire<'a>,
}
impl<'a> From<&'a Node> for NodeView<'a> {
    fn from(node: &'a Node) -> Self {
        Self {
            id: &node.id,
            processor: ProcessorView::Core(&node.processor),
            params: &node.params,
            wire: NodeWire::Legacy(node),
        }
    }
}
impl<'a> From<&'a crate::plan_v4::NodeV4> for NodeView<'a> {
    fn from(node: &'a crate::plan_v4::NodeV4) -> Self {
        let processor = match &node.processor {
            crate::plan_v4::ProcessorV4::Core { processor } => ProcessorView::Core(processor),
            crate::plan_v4::ProcessorV4::Kit {
                channels,
                voices,
                samples,
            } => ProcessorView::Kit {
                channels: *channels,
                voices: *voices,
                samples,
            },
        };
        Self {
            id: &node.id,
            processor,
            params: &node.params,
            wire: NodeWire::V4(node),
        }
    }
}
impl<'a> From<&'a crate::plan_v5::NodeV5> for NodeView<'a> {
    fn from(node: &'a crate::plan_v5::NodeV5) -> Self {
        let processor = match &node.processor {
            crate::plan_v5::ProcessorV5::Core { processor } => ProcessorView::Core(processor),
            crate::plan_v5::ProcessorV5::Kit {
                channels,
                voices,
                samples,
            } => ProcessorView::Kit {
                channels: *channels,
                voices: *voices,
                samples,
            },
            crate::plan_v5::ProcessorV5::Audio { clip } => ProcessorView::Audio(clip),
        };
        Self {
            id: &node.id,
            processor,
            params: &node.params,
            wire: NodeWire::V5(node),
        }
    }
}
impl<'a> From<&'a crate::plan_v6::NodeV6> for NodeView<'a> {
    fn from(node: &'a crate::plan_v6::NodeV6) -> Self {
        let processor = match &node.processor {
            crate::plan_v6::ProcessorV6::Core { processor } => ProcessorView::Core(processor),
            crate::plan_v6::ProcessorV6::Kit {
                channels,
                voices,
                samples,
            } => ProcessorView::Kit {
                channels: *channels,
                voices: *voices,
                samples,
            },
            crate::plan_v6::ProcessorV6::Audio { clip } => ProcessorView::Audio(clip),
            crate::plan_v6::ProcessorV6::WarpRate { clip } => ProcessorView::WarpRate(clip),
        };
        Self {
            id: &node.id,
            processor,
            params: &node.params,
            wire: NodeWire::V6(node),
        }
    }
}
impl<'a> From<&'a crate::plan_v7::NodeV7> for NodeView<'a> {
    fn from(node: &'a crate::plan_v7::NodeV7) -> Self {
        let processor = match &node.processor {
            crate::plan_v7::ProcessorV7::Lfo { config } => ProcessorView::Lfo(config),
            crate::plan_v7::ProcessorV7::Constant {} => ProcessorView::Constant,
            crate::plan_v7::ProcessorV7::Core { processor } => ProcessorView::Core(processor),
            crate::plan_v7::ProcessorV7::Kit {
                channels,
                voices,
                samples,
            } => ProcessorView::Kit {
                channels: *channels,
                voices: *voices,
                samples,
            },
            crate::plan_v7::ProcessorV7::Audio { clip } => ProcessorView::Audio(clip),
            crate::plan_v7::ProcessorV7::WarpRate { clip } => ProcessorView::WarpRate(clip),
        };
        Self {
            id: &node.id,
            processor,
            params: &node.params,
            wire: NodeWire::V7(node),
        }
    }
}
impl Serialize for NodeView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.wire {
            NodeWire::Legacy(node) => node.serialize(serializer),
            NodeWire::V4(node) => node.serialize(serializer),
            NodeWire::V5(node) => node.serialize(serializer),
            NodeWire::V6(node) => node.serialize(serializer),
            NodeWire::V7(node) => node.serialize(serializer),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum EventSlice<'a> {
    Legacy(&'a [ResolvedEvent]),
    V3(&'a [crate::plan_v3::ResolvedEventV3]),
}
#[derive(Clone, Copy)]
pub(crate) enum AutomationSlice<'a> {
    Legacy(&'a [Automation]),
    V3(&'a [crate::plan_v3::AutomationV3]),
}
#[derive(Clone, Copy)]
pub(crate) enum EventTimingView<'a> {
    ExactScore,
    Legacy {
        on_seconds: &'a Rational,
        off_seconds: &'a Option<Rational>,
    },
}
#[derive(Clone, Copy)]
pub(crate) struct EventView<'a> {
    pub(crate) address: &'a String,
    pub(crate) source: &'a SourceMapping,
    pub(crate) target: &'a EventTarget,
    pub(crate) kind: &'a EventKind,
    pub(crate) score_on_q: &'a Rational,
    pub(crate) score_off_q: &'a Option<Rational>,
    pub(crate) onset_offset_seconds: &'a Rational,
    pub(crate) release_offset_seconds: &'a Rational,
    pub(crate) timing: EventTimingView<'a>,
    pub(crate) release_velocity: f64,
    pub(crate) on_frame: u64,
    pub(crate) off_frame: Option<u64>,
    #[allow(dead_code)] // Retained for versioned scheduling consumers of the view.
    pub(crate) order: i32,
}
impl<'a> From<&'a ResolvedEvent> for EventView<'a> {
    fn from(event: &'a ResolvedEvent) -> Self {
        Self {
            address: &event.address,
            source: &event.source,
            target: &event.target,
            kind: &event.kind,
            score_on_q: &event.score_on_q,
            score_off_q: &event.score_off_q,
            onset_offset_seconds: &event.onset_offset_seconds,
            release_offset_seconds: &event.release_offset_seconds,
            timing: EventTimingView::Legacy {
                on_seconds: &event.on_seconds,
                off_seconds: &event.off_seconds,
            },
            release_velocity: event.release_velocity,
            on_frame: event.on_frame,
            off_frame: event.off_frame,
            order: event.order,
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) enum AutomationAnchorView<'a> {
    Score(&'a Rational),
    Seconds(&'a Rational),
}
#[derive(Clone, Copy)]
pub(crate) struct AutomationView<'a> {
    pub(crate) id: &'a String,
    pub(crate) target: &'a PortRef,
    pub(crate) clock: AutomationClock,
    pub(crate) anchor: AutomationAnchorView<'a>,
    pub(crate) points: &'a [AutomationPoint],
}
impl<'a> From<&'a Automation> for AutomationView<'a> {
    fn from(lane: &'a Automation) -> Self {
        Self {
            id: &lane.id,
            target: &lane.target,
            clock: lane.clock,
            anchor: match lane.clock {
                AutomationClock::Score => AutomationAnchorView::Score(&lane.at),
                AutomationClock::Seconds => AutomationAnchorView::Seconds(&lane.at),
            },
            points: &lane.points,
        }
    }
}
impl<'a> From<&'a crate::plan_v3::ResolvedEventV3> for EventView<'a> {
    fn from(event: &'a crate::plan_v3::ResolvedEventV3) -> Self {
        Self {
            address: &event.address,
            source: &event.source,
            target: &event.target,
            kind: &event.kind,
            score_on_q: &event.score_on_q,
            score_off_q: &event.score_off_q,
            onset_offset_seconds: &event.onset_offset_seconds,
            release_offset_seconds: &event.release_offset_seconds,
            timing: EventTimingView::ExactScore,
            release_velocity: event.release_velocity,
            on_frame: event.on_frame,
            off_frame: event.off_frame,
            order: event.order,
        }
    }
}
impl<'a> From<&'a crate::plan_v3::AutomationV3> for AutomationView<'a> {
    fn from(lane: &'a crate::plan_v3::AutomationV3) -> Self {
        use crate::plan_v3::AutomationAnchor;
        Self {
            id: &lane.id,
            target: &lane.target,
            clock: lane.clock,
            points: &lane.points,
            anchor: match &lane.at {
                AutomationAnchor::Score { q } => AutomationAnchorView::Score(q),
                AutomationAnchor::Seconds { seconds } => AutomationAnchorView::Seconds(seconds),
            },
        }
    }
}
impl<'a> PlanView<'a> {
    pub(crate) fn events(&self) -> Box<dyn ExactSizeIterator<Item = EventView<'a>> + 'a> {
        match self.events {
            EventSlice::Legacy(e) => Box::new(e.iter().map(EventView::from)),
            EventSlice::V3(e) => Box::new(e.iter().map(EventView::from)),
        }
    }
    pub(crate) fn automations(&self) -> Box<dyn ExactSizeIterator<Item = AutomationView<'a>> + 'a> {
        match self.automation {
            AutomationSlice::Legacy(a) => Box::new(a.iter().map(AutomationView::from)),
            AutomationSlice::V3(a) => Box::new(a.iter().map(AutomationView::from)),
        }
    }
    pub fn validate_with_limits(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        self.validate_and_timing(limits).map(|_| ())
    }

    pub(crate) fn validate_and_timing(
        &self,
        limits: &PlanLimits,
    ) -> Result<Option<crate::plan_v3::TimingContext>, PlanError> {
        if !matches!(self.version, 3..=7) {
            self.validate_with_optional_timing(limits, None)?;
            return Ok(None);
        }
        let limits = limits.bounded();
        self.preflight_timing(&limits)?;
        let timing =
            crate::plan_v3::TimingContext::new_with_limits(self.tempo, self.output, &limits)?;
        self.validate_with_timing(&limits, &timing)?;
        Ok(Some(timing))
    }

    pub(crate) fn validate_with_timing(
        &self,
        limits: &PlanLimits,
        timing: &crate::plan_v3::TimingContext,
    ) -> Result<(), PlanError> {
        if !matches!(self.version, 3..=7) {
            return Err(err(
                "E_VERSION",
                "version",
                "a certified timing context requires plan version 3, 4, 5 or 6",
            ));
        }
        self.validate_with_optional_timing(limits, Some(timing))
    }

    fn validate_with_optional_timing(
        &self,
        limits: &PlanLimits,
        timing: Option<&crate::plan_v3::TimingContext>,
    ) -> Result<(), PlanError> {
        if !matches!(
            (&self.events, &self.automation, self.version),
            (EventSlice::Legacy(_), AutomationSlice::Legacy(_), 1 | 2)
                | (EventSlice::V3(_), AutomationSlice::V3(_), 3..=7)
        ) {
            return Err(err(
                "E_VERSION",
                "version",
                "plan version and record schema disagree",
            ));
        }
        if !matches!(
            (self.nodes, self.audio_assets, self.version),
            (NodeSlice::Legacy(_), None, 1..=3)
                | (NodeSlice::V4(_), Some(_), 4)
                | (NodeSlice::V5(_), Some(_), 5)
                | (NodeSlice::V6(_), Some(_), 6)
                | (NodeSlice::V7(_), Some(_), 7)
        ) {
            return Err(err(
                "E_VERSION",
                "version",
                "plan version and node/asset schema disagree",
            ));
        }
        let limits = limits.bounded();
        match self.version {
            LEGACY_PLAN_VERSION => {
                if self.instruments.is_some()
                    || self.nodes.iter().any(|node| {
                        matches!(
                            node.processor,
                            ProcessorView::Core(Processor::Instrument { .. })
                        )
                    })
                {
                    return Err(err(
                        "E_VERSION",
                        "version",
                        "version 1 plans cannot contain version 2 instrument resources",
                    ));
                }
            }
            3..=7 => {
                if self.instruments.is_none()
                    && self.nodes.iter().any(|n| {
                        matches!(
                            n.processor,
                            ProcessorView::Core(Processor::Instrument { .. })
                        )
                    })
                {
                    return Err(err(
                        "E_REFERENCE",
                        "instruments",
                        "instrument nodes require embedded resources",
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
        if matches!(self.version, 3..=7) {
            self.preflight_timing(&limits)?;
        } else {
            self.validate_counts(&limits)?;
        }
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
        self.validate_output(&limits, timing)?;
        for node in self.nodes {
            if let ProcessorView::Lfo(config) = node.processor {
                let timing = timing
                    .ok_or_else(|| err("E_VERSION", "version", "LFO requires certified timing"))?;
                crate::core_control::prepare_lfo(config, self.output, timing, &limits).map_err(
                    |mut error| {
                        error.path = format!("nodes.{}.processor.config", node.id);
                        error
                    },
                )?;
            }
        }
        for node in self.nodes {
            if let ProcessorView::Audio(clip) = node.processor {
                let asset = self
                    .audio_assets
                    .unwrap_or(&[])
                    .iter()
                    .find(|asset| asset.id == clip.asset)
                    .ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            "nodes.audio.asset",
                            "clip asset does not exist",
                        )
                    })?;
                let timing = timing.ok_or_else(|| {
                    err("E_VERSION", "version", "audio requires certified timing")
                })?;
                crate::audio_clip::prepare_clip(
                    clip,
                    timing,
                    self.output,
                    asset.rate_hz,
                    asset.frames,
                )?;
            }
        }
        for node in self.nodes {
            if let ProcessorView::WarpRate(clip) = node.processor {
                let asset = self
                    .audio_assets
                    .unwrap_or(&[])
                    .iter()
                    .find(|asset| asset.id == clip.asset)
                    .ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            "nodes.warp.asset",
                            "clip asset does not exist",
                        )
                    })?;
                let timing = timing
                    .ok_or_else(|| err("E_VERSION", "version", "warp requires certified timing"))?;
                crate::warp_clip::prepare_clip(clip, timing, self.output, asset.frames)?;
            }
        }
        self.validate_automation(&limits, timing)?;
        self.validate_connections(&limits)?;
        self.validate_events(&limits, timing)?;
        self.validate_regions(&limits)?;
        self.validate_source_mappings(&limits)?;
        self.validate_instrument_work(&limits)?;
        if let Some(production) = &self.production {
            production.validate_for_view(self, &limits)?;
        }
        Ok(())
    }

    pub(crate) fn execution_work(&self, limits: &PlanLimits) -> Result<u64, PlanError> {
        self.validate_instrument_work(&limits.bounded())
    }

    /// Resolve an explicit project-level audio output without pruning graph context.
    pub fn audio_output_channels(&self, output: &PortRef) -> Result<u8, PlanError> {
        let node = self
            .nodes
            .iter()
            .find(|node| *node.id == output.node)
            .ok_or_else(|| err("E_REFERENCE", "output", "target node does not exist"))?;
        let port = port_descriptor(node, &output.port, false)
            .filter(|p| p.kind == PortKind::Audio)
            .ok_or_else(|| err("E_PORT_TYPE", "output", "target is not an audio output"))?;
        if !(1..=2).contains(&port.channels) {
            return Err(err("E_PORT_TYPE", "output", "target layout is unsupported"));
        }
        Ok(port.channels)
    }

    /// Look up a reusable graph program embedded in this standalone plan.
    pub fn instrument_program(&self, id: &str) -> Option<&'a InstrumentProgram> {
        self.instruments?
            .programs
            .iter()
            .find(|program| program.id == id)
    }

    /// Return the processor metadata for an instrument instance's public
    /// control. Core processors and unknown controls return `None`.
    pub fn instrument_control_spec(
        &self,
        node: NodeView<'_>,
        control: &str,
    ) -> Option<ParameterSpec> {
        let Processor::Instrument { program, .. } = node.processor.core().ok()? else {
            return None;
        };
        self.instrument_program(program)?.control_spec(control)
    }

    /// Resolve all public instrument controls, filling omitted instance values
    /// from their program defaults. The node is validated as part of lookup.
    pub fn resolved_node_params(
        &self,
        node: NodeView<'_>,
    ) -> Result<BTreeMap<String, Rational>, PlanError> {
        if matches!(node.processor, ProcessorView::Constant) {
            let mut params = node.params.clone();
            params.entry("value".into()).or_insert_with(zero);
            return Ok(params);
        }
        if let ProcessorView::Lfo(_) | ProcessorView::Audio(_) | ProcessorView::WarpRate(_) =
            node.processor
        {
            return Ok(node.params.clone());
        }
        if let ProcessorView::Kit { .. } = node.processor {
            let mut params = node.params.clone();
            params.entry("level".into()).or_insert_with(one);
            return Ok(params);
        }
        let Processor::Instrument { program, .. } = node.processor.core()? else {
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
        for (name, value) in node.params {
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

    fn timing_work(&self) -> u64 {
        let knots = self
            .automations()
            .fold(0u64, |n, a| n.saturating_add(a.points.len() as u64));
        // Conservative segment traversal units; each certified operation additionally
        // has the tempo evaluator's fixed refinement and logarithm-series caps.

        let warp_work = self.nodes.iter().fold(0u64, |total, node| {
            if let ProcessorView::WarpRate(clip) = node.processor {
                total.saturating_add(
                    (clip.warp.len() as u64)
                        .saturating_add(self.tempo.points.len() as u64)
                        .saturating_mul(32),
                )
            } else {
                total
            }
        });
        (self.tempo.points.len() as u64)
            .saturating_add(1)
            .saturating_mul(
                8u64.saturating_add(8u64.saturating_mul(self.events().len() as u64))
                    .saturating_add(
                        16u64.saturating_mul(
                            self.nodes
                                .iter()
                                .filter(|n| {
                                    matches!(
                                        n.processor,
                                        ProcessorView::Audio(_) | ProcessorView::WarpRate(_)
                                    )
                                })
                                .count() as u64,
                        ),
                    )
                    .saturating_add(
                        3u64.saturating_mul(
                            (self.tempo.points.len() as u64)
                                .saturating_add(self.automations().len() as u64)
                                .saturating_add(knots),
                        ),
                    ),
            )
            .saturating_add(warp_work)
    }

    pub(crate) fn preflight_timing(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        self.validate_counts(limits)?;
        if let Some(assets) = self.audio_assets {
            for asset in assets {
                validate_identifier_limit(&asset.id, "audio_assets.id", limits.max_id_bytes)?;
            }
            self.validate_global_ids()?;
            crate::audio_asset::validate_assets(assets)?;
        }
        for (value, path) in [
            (&self.output.score_start_q, "output.score_start_q"),
            (&self.output.score_end_q, "output.score_end_q"),
            (&self.output.tail_seconds, "output.tail_seconds"),
        ] {
            rational_bit_limit(value, limits, path)?;
        }
        if self.output.score_start_q >= self.output.score_end_q || self.output.tail_seconds < zero()
        {
            return Err(err("E_INTERVAL", "output", "invalid score span or tail"));
        }
        for event in self.events() {
            for value in [
                event.score_on_q,
                event.onset_offset_seconds,
                event.release_offset_seconds,
            ] {
                rational_bit_limit(value, limits, "events")?;
            }
            if let Some(off) = event.score_off_q {
                rational_bit_limit(off, limits, "events.score_off_q")?;
            }
        }
        for lane in self.automations() {
            let at = match lane.anchor {
                AutomationAnchorView::Score(q) | AutomationAnchorView::Seconds(q) => q,
            };
            rational_bit_limit(at, limits, "automation.at")?;
            for point in lane.points {
                rational_bit_limit(&point.position, limits, "automation.points.position")?;
                rational_bit_limit(&point.value, limits, "automation.points.value")?;
            }
        }
        self.validate_tempo(limits)
    }
    fn validate_counts(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        for node in self.nodes {
            if let ProcessorView::Lfo(config) = node.processor {
                let path = format!("nodes.{}.processor.config", node.id);
                rational_bit_limit(&config.period, limits, format!("{path}.period"))?;
                rational_bit_limit(&config.phase, limits, format!("{path}.phase"))?;
                if config.phase < zero() || config.phase >= one() {
                    return Err(err(
                        "E_RANGE",
                        format!("{path}.phase"),
                        "retained LFO phase must be canonical in [0,1)",
                    ));
                }
                if config.period <= zero() {
                    return Err(err(
                        "E_RANGE",
                        format!("{path}.period"),
                        "LFO period must be positive",
                    ));
                }
            }
            if matches!(
                node.processor,
                ProcessorView::Constant | ProcessorView::Lfo(_)
            ) {
                for value in node.params.values() {
                    rational_bit_limit(value, limits, "nodes.params")?;
                }
            }
        }
        for edge in self.modulations {
            rational_bit_limit(&edge.amount, limits, "modulations.amount")?;
        }
        let mut control_preparation_work = 0u64;
        for node in self.nodes {
            if let ProcessorView::Lfo(config) = node.processor {
                let candidates = crate::core_control::candidate_bound(
                    config,
                    self.output,
                    self.tempo.points.len(),
                    limits,
                )
                .map_err(|mut error| {
                    error.path = format!("nodes.{}.processor.config", node.id);
                    error
                })?;
                control_preparation_work = candidates
                    .checked_mul(128)
                    .and_then(|cost| control_preparation_work.checked_add(cost))
                    .ok_or_else(|| {
                        err(
                            "E_RESOURCE_LIMIT",
                            "nodes.lfo",
                            "control preparation work overflow",
                        )
                    })?;
            }
        }
        let control_objects = self
            .nodes
            .iter()
            .filter(|node| matches!(node.processor, ProcessorView::Lfo(_)))
            .count()
            .saturating_mul(4);
        let production_usage = self
            .production
            .as_ref()
            .map(|settings| settings.resource_usage(limits))
            .transpose()?;
        let (expression_count, expression_points) =
            self.events()
                .fold((0usize, 0usize), |(count, points), event| {
                    match &event.kind {
                        EventKind::Note {
                            pitch_expression,
                            gain_expression,
                            timbre_expression,
                            pressure_expression,
                            ..
                        } => (
                            count
                                .saturating_add(usize::from(pitch_expression.is_some()))
                                .saturating_add(usize::from(gain_expression.is_some()))
                                .saturating_add(usize::from(timbre_expression.is_some()))
                                .saturating_add(usize::from(pressure_expression.is_some())),
                            points
                                .saturating_add(
                                    pitch_expression.as_ref().map_or(0, |e| e.points.len()),
                                )
                                .saturating_add(
                                    gain_expression.as_ref().map_or(0, |e| e.points.len()),
                                )
                                .saturating_add(
                                    timbre_expression.as_ref().map_or(0, |e| e.points.len()),
                                )
                                .saturating_add(
                                    pressure_expression.as_ref().map_or(0, |e| e.points.len()),
                                ),
                        ),
                        _ => (count, points),
                    }
                });
        let mut warp_points = 0usize;
        for node in self.nodes {
            if let ProcessorView::WarpRate(clip) = node.processor {
                if clip.warp.len() > 4096 {
                    return Err(err(
                        "E_RESOURCE_LIMIT",
                        "nodes.warp",
                        "warp anchor allowance exceeded",
                    ));
                }
                warp_points = warp_points.saturating_add(clip.warp.len());
                if warp_points > limits.max_automation_points {
                    return Err(err(
                        "E_RESOURCE_LIMIT",
                        "nodes.warp",
                        "aggregate warp anchor allowance exceeded",
                    ));
                }
                rational_bit_limit(&clip.at_q, limits, "nodes.warp.at_q")?;
                for anchor in &clip.warp {
                    rational_bit_limit(&anchor.q, limits, "nodes.warp.q")?;
                }
            }
        }
        let automation_points = self.automations().map(|lane| lane.points.len()).fold(
            expression_points.saturating_add(warp_points),
            usize::saturating_add,
        );
        let parameter_count = self
            .nodes
            .iter()
            .map(|node| node.params.len())
            .fold(0usize, usize::saturating_add);
        let span_count = self
            .events()
            .filter(|event| event.source.span.is_some())
            .count()
            .saturating_add(
                self.source_mappings
                    .iter()
                    .filter(|mapping| mapping.span.is_some())
                    .count(),
            );
        let path_component_count = self
            .events()
            .map(|event| event.source.path.len())
            .fold(0usize, usize::saturating_add)
            .saturating_add(
                self.source_mappings
                    .iter()
                    .map(|mapping| mapping.path.len())
                    .fold(0usize, usize::saturating_add),
            );
        let binary_payload_bytes = self
            .events()
            .filter_map(|event| match &event.kind {
                EventKind::Message { bytes } => Some(bytes.len()),
                _ => None,
            })
            .fold(0usize, usize::saturating_add);
        let assets = self.audio_assets.unwrap_or(&[]);
        let asset_bytes = assets
            .iter()
            .try_fold(0usize, |total, asset| total.checked_add(asset.bytes.len()))
            .ok_or_else(|| {
                err(
                    "E_RESOURCE_LIMIT",
                    "audio_assets",
                    "asset byte count overflow",
                )
            })?;
        if assets.len() > crate::bundle::MAX_BUNDLE_ASSETS
            || asset_bytes > crate::bundle::MAX_BUNDLE_ASSET_BYTES
            || assets
                .iter()
                .any(|asset| asset.bytes.len() > crate::bundle::MAX_BUNDLE_FILE_BYTES)
        {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "audio_assets",
                "audio asset storage limit exceeded",
            ));
        }
        let kit_objects = self.nodes.iter().fold(assets.len(), |total, node| {
            total.saturating_add(match node.processor {
                ProcessorView::Kit { samples, .. } => samples.len(),
                ProcessorView::WarpRate(clip) => 4usize
                    .saturating_add(clip.source.path.len())
                    .saturating_add(usize::from(clip.source.span.is_some())),
                ProcessorView::Audio(clip) => 4usize
                    .saturating_add(clip.source.path.len())
                    .saturating_add(usize::from(clip.source.span.is_some())),
                _ => 0,
            })
        });
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

            let mut pluck_cells = 0usize;
            for node in self.nodes {
                let ProcessorView::Core(Processor::Instrument {
                    program, voices, ..
                }) = node.processor
                else {
                    continue;
                };
                let count = resources
                    .programs
                    .iter()
                    .find(|candidate| candidate.id == *program)
                    .map_or(0, |program| program.pluck_node_count());
                let cells = crate::graph::pluck_delay_cells(*voices as usize, count)
                    .and_then(|cells| pluck_cells.checked_add(cells))
                    .ok_or_else(|| {
                        err(
                            "E_RESOURCE_LIMIT",
                            "instruments",
                            "pluck storage arithmetic overflow",
                        )
                    })?;
                if cells > limits.max_pluck_delay_cells {
                    return Err(err(
                        "E_RESOURCE_LIMIT",
                        "instruments",
                        "declared pluck delay storage exceeds the plan limit",
                    ));
                }
                pluck_cells = cells;
            }

            let voice_graph_states = self.nodes.iter().fold(0usize, |total, node| {
                let ProcessorView::Core(Processor::Instrument {
                    program, voices, ..
                }) = node.processor
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
        resource_objects = resource_objects.saturating_add(kit_objects);
        if binary_payload_bytes > limits.max_json_bytes {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "events",
                "event payload bytes exceed the plan limit",
            ));
        }
        if self.events().len() > limits.max_events
            || self.nodes.len() > limits.max_nodes
            || self
                .connections
                .len()
                .saturating_add(self.modulations.len())
                > limits.max_connections
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
            .saturating_add(self.modulations.len().saturating_mul(4))
            .saturating_add(control_objects)
            .saturating_add(self.events().len())
            .saturating_add(self.automations().len())
            .saturating_add(self.regions.len())
            .saturating_add(self.source_mappings.len())
            .saturating_add(self.tempo.points.len())
            .saturating_add(automation_points)
            .saturating_add(expression_count)
            .saturating_add(parameter_count)
            .saturating_add(span_count)
            .saturating_add(path_component_count)
            .saturating_add(resource_objects)
            .saturating_add(production_usage.as_ref().map_or(0, |usage| usage.objects));
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
            .saturating_add(self.modulations.len().saturating_mul(4))
            .saturating_add(control_objects)
            .saturating_add(self.events().len())
            .saturating_add(self.automations().len())
            .saturating_add(automation_points)
            .saturating_add(expression_count)
            .saturating_add(self.regions.len())
            .saturating_add(self.source_mappings.len())
            .saturating_add(self.tempo.points.len())
            .saturating_add(parameter_count)
            .saturating_add(span_count)
            .saturating_add(path_component_count)
            .saturating_add(resource_objects)
            .saturating_add(production_usage.as_ref().map_or(0, |usage| usage.objects));
        let timing_work = if matches!(self.version, 3..=7) {
            self.timing_work()
        } else {
            0
        };
        if (work as u64)
            .saturating_add(timing_work)
            .saturating_add(control_preparation_work)
            .saturating_add(asset_bytes as u64)
            > limits.max_work
        {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "plan",
                "aggregate plan work exceeds the plan limit",
            ));
        }

        let mut string_bytes = production_usage
            .as_ref()
            .map_or(0, |usage| usage.string_bytes);
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
        for edge in self.modulations {
            for value in [
                &edge.id,
                &edge.from.node,
                &edge.from.port,
                &edge.target.node,
                &edge.target.port,
            ] {
                count_string(value, "modulations".into())?;
                validate_identifier_limit(value, "modulations", limits.max_id_bytes)?;
            }
        }
        for node in self.nodes {
            if let ProcessorView::Lfo(config) = node.processor {
                count_string(
                    match config.clock {
                        crate::plan_v7::ControlClock::Score => "score",
                        crate::plan_v7::ControlClock::Seconds => "seconds",
                    },
                    "nodes.lfo.clock".into(),
                )?;
                count_string(
                    match config.wave {
                        crate::plan_v7::LfoWave::Sine => "sine",
                        crate::plan_v7::LfoWave::Triangle => "triangle",
                        crate::plan_v7::LfoWave::Saw => "saw",
                        crate::plan_v7::LfoWave::Square => "square",
                    },
                    "nodes.lfo.wave".into(),
                )?;
            }
        }
        for asset in assets {
            count_string(&asset.id, "audio_assets.id".into())?;
            count_string(&asset.format, "audio_assets.format".into())?;
            count_string(&asset.hash, "audio_assets.hash".into())?;
            if asset.id.len() > limits.max_id_bytes {
                return Err(err(
                    "E_RESOURCE_LIMIT",
                    "audio_assets.id",
                    "asset ID exceeds caller limit",
                ));
            }
        }
        for node in self.nodes {
            if let Some(clip) = node.processor.audio_transport() {
                if std::iter::once(node.id)
                    .chain([clip.asset, &clip.source.object])
                    .chain(clip.source.path.iter())
                    .chain(clip.track.iter())
                    .any(|id| id.len() > limits.max_id_bytes)
                {
                    return Err(err(
                        "E_RESOURCE_LIMIT",
                        "nodes.audio",
                        "clip identifier exceeds caller limit",
                    ));
                }
                validate_identifier_limit(node.id, "nodes.id", limits.max_id_bytes)?;
                for value in [clip.asset, &clip.source.object] {
                    validate_identifier_limit(value, "nodes.audio.source", limits.max_id_bytes)?;
                    count_string(value, "nodes.audio.source".into())?;
                }
                if clip.source.path.is_empty() {
                    return Err(err(
                        "E_REFERENCE",
                        "nodes.audio.source.path",
                        "clip source path must not be empty",
                    ));
                }
                for component in &clip.source.path {
                    validate_identifier_limit(
                        component,
                        "nodes.audio.source.path",
                        limits.max_id_bytes,
                    )?;
                    count_string(component, "nodes.audio.source.path".into())?;
                }
                if let Some(track) = &clip.track {
                    validate_identifier_limit(track, "nodes.audio.track", limits.max_id_bytes)?;
                    count_string(track, "nodes.audio.track".into())?;
                }
                if let Some(span) = clip.source.span {
                    if span.start > span.end || span.end > limits.max_json_bytes {
                        return Err(err(
                            "E_RANGE",
                            "nodes.audio.source.span",
                            "source span must be a bounded half-open byte range",
                        ));
                    }
                }
                for value in [
                    clip.at,
                    clip.gain,
                    clip.fade_in_seconds,
                    clip.fade_out_seconds,
                ]
                .into_iter()
                .chain(clip.speed)
                {
                    rational_bit_limit(value, limits, "nodes.audio")?;
                }
                for value in node.params.values() {
                    rational_bit_limit(value, limits, "nodes.params")?;
                }
            }
            if let ProcessorView::Kit { samples, .. } = node.processor {
                for sample in samples {
                    count_string(&sample.key, "nodes.processor.samples.key".into())?;
                    count_string(&sample.asset, "nodes.processor.samples.asset".into())?;
                }
            }
            count_string(node.id, format!("nodes.{}", node.id))?;
            if let ProcessorView::Core(Processor::Instrument { program, .. }) = node.processor {
                count_string(program, format!("nodes.{}.processor.program", node.id))?;
            }
            for key in node.params.keys() {
                count_string(key, format!("nodes.{}.params", node.id))?;
            }
        }
        for connection in self.connections {
            count_string(&connection.id, format!("connections.{}", connection.id))?;
            count_string(&connection.from.node, "connections.from.node".into())?;
            count_string(&connection.from.port, "connections.from.port".into())?;
            count_string(&connection.to.node, "connections.to.node".into())?;
            count_string(&connection.to.port, "connections.to.port".into())?;
        }
        for event in self.events() {
            count_string(event.address, "events.address".into())?;
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
        for lane in self.automations() {
            count_string(lane.id, format!("automation.{}", lane.id))?;
            count_string(&lane.target.node, "automation.target.node".into())?;
            count_string(&lane.target.port, "automation.target.port".into())?;
        }
        for mapping in self.source_mappings {
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
        for region in self.regions {
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

    fn validate_output(
        &self,
        limits: &PlanLimits,
        timing: Option<&crate::plan_v3::TimingContext>,
    ) -> Result<(), PlanError> {
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
            .find(|node| *node.id == output.output.node)
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
        if let Some(timing) = timing {
            return timing.validate_output(output, limits);
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
        for node in self.nodes {
            if !ids.insert(node.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("nodes.{}", node.id),
                    "duplicate plan object ID",
                ));
            }
        }
        for edge in self.modulations {
            if !ids.insert(edge.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    "modulations.id",
                    "duplicate plan object ID",
                ));
            }
        }
        for connection in self.connections {
            if !ids.insert(connection.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("connections.{}", connection.id),
                    "duplicate plan object ID",
                ));
            }
        }
        for lane in self.automations() {
            if !ids.insert(lane.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("automation.{}", lane.id),
                    "duplicate plan object ID",
                ));
            }
        }
        for region in self.regions {
            if !ids.insert(region.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("regions.{}", region.id),
                    "duplicate plan object ID",
                ));
            }
        }
        for asset in self.audio_assets.unwrap_or(&[]) {
            if !ids.insert(asset.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("audio_assets.{}", asset.id),
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
            if point.shape != Interpolation::Step
                && (!matches!(self.version, 3..=7) || point.shape != Interpolation::Linear)
            {
                return Err(err(
                    "E_CAPABILITY",
                    format!("tempo.points[{index}].shape"),
                    "standalone plans require step tempo for exact inverse scheduling",
                ));
            }
            previous = Some(&point.q);
        }
        if !matches!(self.version, 3..=7) {
            self.tempo.checked()?;
        } else if self
            .tempo
            .points
            .last()
            .is_some_and(|p| p.shape != Interpolation::Step)
        {
            return Err(err(
                "E_TEMPO",
                "tempo.points",
                "last tempo point must be step",
            ));
        }
        Ok(())
    }

    fn validate_nodes(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let mut ids = BTreeSet::new();
        for node in self.nodes {
            validate_identifier_limit(node.id, "nodes.id", limits.max_id_bytes)?;
            if !ids.insert(node.id.clone()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("nodes.{}", node.id),
                    "duplicate node ID",
                ));
            }
            if matches!(
                node.processor,
                ProcessorView::Lfo(_) | ProcessorView::Constant
            ) {
                for (name, value) in node.params {
                    if !matches!(node.processor, ProcessorView::Constant) || name != "value" {
                        return Err(err(
                            "E_UNKNOWN_FIELD",
                            "nodes.params",
                            "control parameter does not exist",
                        ));
                    }
                    rational_bit_limit(value, limits, "nodes.params.value")?;
                    finite_engine_rational(value, "nodes.params.value")?;
                }
                if let ProcessorView::Lfo(config) = node.processor {
                    if config.phase < zero() || config.phase >= one() {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor.config.phase", node.id),
                            "retained LFO phase must be canonical in [0,1)",
                        ));
                    }
                    if config.period <= zero() {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor.config.period", node.id),
                            "LFO period must be positive",
                        ));
                    }
                }
                continue;
            }
            if let Some(clip) = node.processor.audio_transport() {
                if !node.params.is_empty() {
                    return Err(err(
                        "E_UNKNOWN_FIELD",
                        "nodes.audio.params",
                        "audio clips have no parameters",
                    ));
                }
                if clip.channels == 0 || clip.channels > limits.max_channels {
                    return Err(err(
                        "E_RANGE",
                        "nodes.audio.channels",
                        "clip channels must be mono or stereo",
                    ));
                }
                let asset = self
                    .audio_assets
                    .unwrap_or(&[])
                    .iter()
                    .find(|asset| &asset.id == clip.asset)
                    .ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            "nodes.audio.asset",
                            "clip asset does not exist",
                        )
                    })?;
                if asset.channels != clip.channels {
                    return Err(err(
                        "E_PORT_TYPE",
                        "nodes.audio.channels",
                        "clip and asset channels differ",
                    ));
                }
                if clip.source_start_frame >= clip.source_end_frame
                    || clip.source_end_frame > asset.frames
                {
                    return Err(err(
                        "E_RANGE",
                        "nodes.audio.source",
                        "clip source requires 0 <= a < b <= asset.frames",
                    ));
                }
                if clip.speed.is_some_and(|speed| speed <= &zero())
                    || clip.gain < &zero()
                    || clip.fade_in_seconds < &zero()
                    || clip.fade_out_seconds < &zero()
                {
                    return Err(err(
                        "E_RANGE",
                        "nodes.audio",
                        "clip speed must be positive and gain/fades nonnegative",
                    ));
                }
                continue;
            }
            if let ProcessorView::Kit {
                channels,
                voices,
                samples,
            } = node.processor
            {
                if channels == 0 || channels > limits.max_channels {
                    return Err(err(
                        "E_RANGE",
                        format!("nodes.{}.processor.channels", node.id),
                        "kit channels must be mono or stereo",
                    ));
                }
                if voices == 0 {
                    return Err(err(
                        "E_RANGE",
                        format!("nodes.{}.processor.voices", node.id),
                        "kit voice capacity must be positive",
                    ));
                }
                if voices > limits.max_voices {
                    return Err(err(
                        "E_RESOURCE_LIMIT",
                        format!("nodes.{}.processor.voices", node.id),
                        "kit voice capacity exceeds caller limit",
                    ));
                }
                if samples.is_empty() {
                    return Err(err(
                        "E_RANGE",
                        format!("nodes.{}.processor.samples", node.id),
                        "kit requires at least one sample mapping",
                    ));
                }
                let mut keys = BTreeSet::new();
                for sample in samples {
                    if !keys.insert(&sample.key) {
                        return Err(err(
                            "E_DUPLICATE_ID",
                            format!("nodes.{}.processor.samples", node.id),
                            "duplicate kit sample key",
                        ));
                    }
                    let asset = self
                        .audio_assets
                        .unwrap_or(&[])
                        .iter()
                        .find(|asset| asset.id == sample.asset)
                        .ok_or_else(|| {
                            err(
                                "E_REFERENCE",
                                format!("nodes.{}.processor.samples", node.id),
                                "kit sample asset does not exist",
                            )
                        })?;
                    if asset.channels != channels {
                        return Err(err(
                            "E_PORT_TYPE",
                            format!("nodes.{}.processor.samples", node.id),
                            "kit and asset channel counts differ",
                        ));
                    }
                }
                for (parameter, value) in node.params {
                    if parameter != "level" {
                        return Err(err(
                            "E_UNKNOWN_FIELD",
                            format!("nodes.{}.params.{parameter}", node.id),
                            "kit supports only level",
                        ));
                    }
                    validate_kit_level(value, limits, format!("nodes.{}.params.level", node.id))?;
                }
                continue;
            }
            match node.processor.core()? {
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
                Processor::OnePole { channels }
                | Processor::Gain { channels }
                | Processor::Eq { channels, .. }
                | Processor::Compressor { channels, .. }
                | Processor::Reverb { channels, .. }
                | Processor::Sum { channels } => {
                    if *channels == 0 || *channels > limits.max_channels {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor", node.id),
                            "channels must be mono or stereo",
                        ));
                    }
                }
                Processor::Fader { channels } => {
                    if *channels == 0 {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor.channels", node.id),
                            "fader channels must be positive",
                        ));
                    }
                    if *channels > 2 {
                        return Err(err(
                            "E_CAPABILITY",
                            format!("nodes.{}.processor.channels", node.id),
                            "core.fader/1 supports only mono or stereo",
                        ));
                    }
                    if *channels > limits.max_channels {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor.channels", node.id),
                            "channels exceed the caller channel limit",
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
            match node.processor.core()? {
                Processor::Compressor {
                    sidechain_channels: Some(channels),
                    ..
                } if *channels == 0 || *channels > limits.max_channels => {
                    return Err(err(
                        "E_RANGE",
                        format!("nodes.{}.processor.sidechain_channels", node.id),
                        "sidechain must be mono or stereo",
                    ))
                }
                Processor::Reverb {
                    predelay_frames,
                    damping,
                    ..
                } => {
                    rational_bit_limit(
                        damping,
                        limits,
                        format!("nodes.{}.processor.damping", node.id),
                    )?;
                    finite_engine_rational(
                        damping,
                        format!("nodes.{}.processor.damping", node.id),
                    )?;
                    if *predelay_frames > 12000 || damping < &zero() || damping > &one() {
                        return Err(err(
                            "E_RANGE",
                            format!("nodes.{}.processor", node.id),
                            "reverb configuration is outside its range",
                        ));
                    }
                }
                _ => {}
            }
            for (parameter, value) in node.params {
                rational_bit_limit(
                    value,
                    limits,
                    format!("nodes.{}.params.{}", node.id, parameter),
                )?;
                if let Processor::Instrument { program, .. } = node.processor.core()? {
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
                    if !node.processor.core()?.parameter_allowed(parameter) {
                        return Err(err(
                            "E_UNKNOWN_FIELD",
                            format!("nodes.{}.params.{}", node.id, parameter),
                            format!(
                                "parameter is not supported by core.{}",
                                node.processor.core()?.kind()
                            ),
                        ));
                    }
                    validate_parameter(
                        node.processor.core()?,
                        parameter,
                        value,
                        node.id,
                        self.output.sample_rate_hz,
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn validate_modulation_target(
        &self,
        node: NodeView<'_>,
        name: &str,
    ) -> Result<(), PlanError> {
        let missing = || {
            err(
                "E_REFERENCE",
                "modulations.target",
                "continuous parameter does not exist",
            )
        };
        match node.processor {
            ProcessorView::Constant if name == "value" => Ok(()),
            ProcessorView::Kit { .. } if name == "level" => Ok(()),
            ProcessorView::Core(Processor::Instrument { program, .. }) => {
                self.instrument_program(program)
                    .and_then(|p| p.control_spec(name))
                    .ok_or_else(missing)?;
                Ok(())
            }
            ProcessorView::Core(processor) if processor.parameter_allowed(name) => Ok(()),
            _ => Err(missing()),
        }
    }

    fn validate_connections(&self, limits: &PlanLimits) -> Result<(), PlanError> {
        let node_by_id: HashMap<&str, NodeView<'_>> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let mut connection_ids = BTreeSet::new();
        let mut destinations: HashMap<(String, String), usize> = HashMap::new();
        let mut edges: Vec<(String, String)> = Vec::new();
        for connection in self.connections {
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
                port_descriptor(*from_node, &connection.from.port, false).ok_or_else(|| {
                    err(
                        "E_PORT_TYPE",
                        format!("connections.{}.from", connection.id),
                        "source port is not an output",
                    )
                })?;
            let to = port_descriptor(*to_node, &connection.to.port, true).ok_or_else(|| {
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
        for node in self.nodes {
            let requires_audio_input = matches!(
                node.processor,
                ProcessorView::Core(
                    Processor::OnePole { .. }
                        | Processor::Gain { .. }
                        | Processor::Fader { .. }
                        | Processor::Eq { .. }
                        | Processor::Compressor { .. }
                        | Processor::Reverb { .. }
                        | Processor::Pan
                )
            );
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
        for node in self.nodes {
            if matches!(
                node.processor,
                ProcessorView::Core(Processor::Compressor {
                    sidechain_channels: Some(_),
                    ..
                })
            ) && !destinations.contains_key(&(node.id.clone(), "sidechain".into()))
            {
                return Err(err(
                    "E_PORT_TYPE",
                    format!("nodes.{}.sidechain", node.id),
                    "external detector input is disconnected",
                ));
            }
        }
        for edge in self.modulations {
            let from = node_by_id.get(edge.from.node.as_str()).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    "modulations.from",
                    "source node does not exist",
                )
            })?;
            if !port_descriptor(*from, &edge.from.port, false)
                .is_some_and(|p| p.kind == PortKind::Control)
            {
                return Err(err(
                    "E_PORT_TYPE",
                    "modulations.from",
                    "source must be a control output",
                ));
            }
            let target = node_by_id.get(edge.target.node.as_str()).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    "modulations.target",
                    "target node does not exist",
                )
            })?;
            self.validate_modulation_target(*target, &edge.target.port)?;
            finite_engine_rational(&edge.amount, "modulations.amount")?;
            edges.push((edge.from.node.clone(), edge.target.node.clone()));
        }
        detect_cycle(self.nodes, &edges)
    }

    fn validate_events(
        &self,
        limits: &PlanLimits,
        timing: Option<&crate::plan_v3::TimingContext>,
    ) -> Result<(), PlanError> {
        let node_by_id: HashMap<&str, NodeView<'_>> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let legacy_times = if timing.is_none() {
            Some((
                self.tempo.seconds_at(&self.output.score_start_q)?,
                self.tempo.seconds_at(&self.output.score_end_q)?,
            ))
        } else {
            None
        };
        let mut addresses = BTreeSet::new();
        let mut voice_work = 0u64;
        for (index, event) in self.events().enumerate() {
            validate_event_address(event.address, format!("events[{index}].address"), limits)?;
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
            if !match target.processor {
                ProcessorView::Core(processor) => processor.accepts_events(),
                ProcessorView::Kit { .. } => matches!(event.kind, EventKind::Hit { .. }),
                ProcessorView::Lfo(_)
                | ProcessorView::Constant
                | ProcessorView::Audio(_)
                | ProcessorView::WarpRate(_) => false,
            } || event.target.port != "events"
            {
                return Err(err(
                    "E_CAPABILITY",
                    format!("events[{index}].target"),
                    "target does not accept native note events",
                ));
            }
            rational_bit_limit(
                event.score_on_q,
                limits,
                format!("events[{index}].score_on_q"),
            )?;
            if *event.score_on_q < self.output.score_start_q
                || *event.score_on_q >= self.output.score_end_q
            {
                return Err(err(
                    "E_INTERVAL",
                    format!("events[{index}].score_on_q"),
                    "event onset lies outside the score",
                ));
            }
            if let Some(score_off) = &event.score_off_q {
                rational_bit_limit(score_off, limits, format!("events[{index}].score_off_q"))?;
                if score_off <= event.score_on_q {
                    return Err(err(
                        "E_INTERVAL",
                        format!("events[{index}].score_off_q"),
                        "event gate must be positive",
                    ));
                }
            }
            rational_bit_limit(
                event.onset_offset_seconds,
                limits,
                format!("events[{index}].onset_offset_seconds"),
            )?;
            rational_bit_limit(
                event.release_offset_seconds,
                limits,
                format!("events[{index}].release_offset_seconds"),
            )?;
            let has_off_time = match event.timing {
                EventTimingView::Legacy {
                    on_seconds,
                    off_seconds,
                } => {
                    let (origin_seconds, end_seconds) =
                        legacy_times.as_ref().expect("legacy timing");
                    rational_bit_limit(on_seconds, limits, format!("events[{index}].on_seconds"))?;
                    let expected_on_seconds = self.tempo.seconds_at(event.score_on_q)?
                        + event.onset_offset_seconds.clone();
                    if *on_seconds != expected_on_seconds {
                        return Err(err(
                            "E_INTERVAL",
                            format!("events[{index}].on_seconds"),
                            "resolved onset does not match score time plus physical offset",
                        ));
                    }
                    if on_seconds >= end_seconds {
                        return Err(err(
                            "E_INTERVAL",
                            format!("events[{index}].on_seconds"),
                            "resolved event onset must occur before score end",
                        ));
                    }
                    let on_delta = on_seconds.clone() - origin_seconds.clone();
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
                    if let Some(off_seconds) = &off_seconds {
                        rational_bit_limit(
                            off_seconds,
                            limits,
                            format!("events[{index}].off_seconds"),
                        )?;
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
                        if *off_seconds <= *on_seconds || off_seconds > end_seconds {
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
                        if matches!(event.kind, EventKind::Note { .. })
                            && event.on_frame == expected_off
                        {
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
                    off_seconds.is_some()
                }
                EventTimingView::ExactScore => {
                    let (on, off) = timing
                        .expect("v3 timing")
                        .schedule_view(&event, u64::from(self.output.sample_rate_hz))?;
                    if on != event.on_frame || off != event.off_frame {
                        return Err(err(
                            "E_INTERVAL",
                            format!("events[{index}]"),
                            "retained frames differ from certified timing",
                        ));
                    }
                    if on >= self.output.total_frames {
                        return Err(err(
                            "E_INTERVAL",
                            format!("events[{index}].on_frame"),
                            "event onset outside render range",
                        ));
                    }
                    off.is_some()
                }
            };
            match &event.kind {
                EventKind::Note {
                    pitch_hz,
                    velocity,
                    pitch_expression,
                    gain_expression,
                    timbre_expression,
                    pressure_expression,
                } => {
                    if event.score_off_q.is_none() || !has_off_time {
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
                            "note pitch must be positive and below Nyquist",
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
                    if let Some(expression) = pitch_expression {
                        if !matches!(
                            target.processor.core()?,
                            Processor::Sine { .. } | Processor::Instrument { .. }
                        ) {
                            return Err(err(
                                "E_CAPABILITY",
                                format!("events[{index}].pitch_expression"),
                                "pitch expression requires core.sine/1 or an instrument",
                            ));
                        }
                        expression.validate_for_note(
                            &event,
                            *pitch_hz,
                            self.output.sample_rate_hz,
                            limits,
                            &format!("events[{index}].pitch_expression"),
                        )?;
                    }
                    if let Some(expression) = gain_expression {
                        if !matches!(
                            target.processor.core()?,
                            Processor::Sine { .. } | Processor::Instrument { .. }
                        ) {
                            return Err(err(
                                "E_CAPABILITY",
                                format!("events[{index}].gain_expression"),
                                "gain expression requires core.sine/1 or an instrument",
                            ));
                        }
                        expression.validate(limits, &format!("events[{index}].gain_expression"))?;
                    }
                    if let Some(expression) = timbre_expression {
                        let supported = match target.processor.core()? {
                            Processor::Instrument { program, .. } => self
                                .instrument_program(program)
                                .is_some_and(|program| program.supports_timbre()),
                            _ => false,
                        };
                        if !supported {
                            return Err(err(
                                "E_CAPABILITY",
                                format!("events[{index}].timbre_expression"),
                                "timbre expression requires an instrument with synth.timbre/1",
                            ));
                        }
                        expression
                            .validate(limits, &format!("events[{index}].timbre_expression"))?;
                    }
                    if let Some(expression) = pressure_expression {
                        let supported = match target.processor.core()? {
                            Processor::Instrument { program, .. } => self
                                .instrument_program(program)
                                .is_some_and(|program| program.supports_pressure()),
                            _ => false,
                        };
                        if !supported {
                            return Err(err(
                                "E_CAPABILITY",
                                format!("events[{index}].pressure_expression"),
                                "pressure expression requires an instrument with synth.pressure/1",
                            ));
                        }
                        expression
                            .validate(limits, &format!("events[{index}].pressure_expression"))?;
                    }
                    voice_work =
                        voice_work.saturating_add(target.processor.core()?.voice_capacity());
                }
                EventKind::Hit { key, velocity } => {
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
                    let ProcessorView::Kit {
                        samples, voices, ..
                    } = target.processor
                    else {
                        return Err(err(
                            "E_CAPABILITY",
                            format!("events[{index}].kind"),
                            "hit requires a kit receiver",
                        ));
                    };
                    if event.score_off_q.is_some()
                        || has_off_time
                        || event.off_frame.is_some()
                        || !event.release_offset_seconds.is_zero()
                        || event.release_velocity.to_bits() != 0.0_f64.to_bits()
                    {
                        return Err(err(
                            "E_INTERVAL",
                            format!("events[{index}]"),
                            "hit must carry onset-only timing and zero unused release fields",
                        ));
                    }
                    if !samples.iter().any(|sample| sample.key == *key) {
                        return Err(err(
                            "E_REFERENCE",
                            format!("events[{index}].kind.key"),
                            "kit has no such sample key",
                        ));
                    }
                    let clock = timing.expect("kit requires certified timing");
                    let end_frame = clock.frame_at_score(
                        &self.output.score_end_q,
                        &zero(),
                        u64::from(self.output.sample_rate_hz),
                    )?;
                    if BigInt::from(event.on_frame) >= end_frame {
                        return Err(err(
                            "E_INTERVAL",
                            format!("events[{index}].on_frame"),
                            "hit onset quantizes into the tail",
                        ));
                    }
                    voice_work = voice_work.saturating_add(u64::from(voices));
                }
                EventKind::Message { .. } => {
                    if event.score_off_q.is_some() || has_off_time {
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

    fn validate_automation(
        &self,
        limits: &PlanLimits,
        timing: Option<&crate::plan_v3::TimingContext>,
    ) -> Result<(), PlanError> {
        let node_by_id: HashMap<&str, NodeView<'_>> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let mut targets = BTreeSet::new();
        let mut lane_ids = BTreeSet::new();
        let mut work = 0u64;
        for (index, lane) in self.automations().enumerate() {
            validate_identifier_limit(
                lane.id,
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
            if let ProcessorView::Audio(_) | ProcessorView::WarpRate(_) = node.processor {
                return Err(err(
                    "E_REFERENCE",
                    "automation.target",
                    "audio clips have no parameter ports",
                ));
            }
            if matches!(
                node.processor,
                ProcessorView::Constant | ProcessorView::Lfo(_)
            ) {
                if !matches!(node.processor, ProcessorView::Constant) || lane.target.port != "value"
                {
                    return Err(err(
                        "E_REFERENCE",
                        "automation.target",
                        "control parameter does not exist",
                    ));
                }
                for point in lane.points {
                    finite_engine_rational(&point.value, "automation.points.value")?;
                }
            } else if let ProcessorView::Kit { .. } = node.processor {
                if lane.target.port != "level" {
                    return Err(err(
                        "E_REFERENCE",
                        format!("automation[{index}].target"),
                        "kit supports only level",
                    ));
                }
                for point in lane.points {
                    validate_kit_level(
                        &point.value,
                        limits,
                        format!("automation[{index}].points.value"),
                    )?;
                }
            } else if let Processor::Instrument { program, .. } = node.processor.core()? {
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
                if !node.processor.core()?.parameter_allowed(&lane.target.port) {
                    return Err(err(
                        "E_REFERENCE",
                        format!("automation[{index}].target"),
                        "automation parameter does not exist",
                    ));
                }
                validate_automation_parameter(
                    node.processor.core()?,
                    &lane.target.port,
                    lane.points,
                    limits,
                    &format!("automation[{index}]"),
                    self.output.sample_rate_hz,
                )?;
            }
            if matches!(self.version, 3..=7)
                && lane.clock == AutomationClock::Score
                && matches!(lane.anchor, AutomationAnchorView::Seconds(_))
            {
                return Err(err(
                    "E_INTERVAL",
                    format!("automation[{index}].at"),
                    "score clock requires score anchor",
                ));
            }
            rational_bit_limit(
                match lane.anchor {
                    AutomationAnchorView::Score(at) | AutomationAnchorView::Seconds(at) => at,
                },
                limits,
                format!("automation[{index}].at"),
            )?;
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
            if let Some(timing) = timing {
                timing.validate_automation(&lane, u64::from(self.output.sample_rate_hz))?;
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
        let addresses: HashSet<&str> = self.events().map(|event| event.address.as_str()).collect();
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
        for event in self.events() {
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

    pub(crate) fn control_execution_work(&self, limits: &PlanLimits) -> Result<u64, PlanError> {
        let overflow = || {
            err(
                "E_RESOURCE_LIMIT",
                "modulations",
                "control execution work overflow",
            )
        };
        let control_weight = |node: NodeView<'_>| -> Result<u64, PlanError> {
            match node.processor {
                ProcessorView::Constant => Ok(8),
                ProcessorView::Lfo(config) => {
                    let candidates = crate::core_control::candidate_bound(
                        config,
                        self.output,
                        self.tempo.points.len(),
                        limits,
                    )
                    .map_err(|mut error| {
                        error.path = format!("nodes.{}.processor.config", node.id);
                        error
                    })?;
                    Ok(128
                        + u64::from(
                            u64::BITS - candidates.max(1).saturating_sub(1).leading_zeros(),
                        ))
                }
                _ => Ok(0),
            }
        };
        let mut per_frame = (self.modulations.len() as u64)
            .checked_mul(8)
            .ok_or_else(overflow)?;
        for node in self.nodes {
            let weight = control_weight(node)?;
            per_frame = per_frame.checked_add(weight).ok_or_else(overflow)?;
        }
        let frame_work = per_frame
            .checked_mul(self.output.total_frames)
            .ok_or_else(overflow)?;
        let node_indices: HashMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        let mut required_controls = vec![false; self.nodes.len()];
        let mut incoming_sources = vec![Vec::new(); self.nodes.len()];
        let mut reset_edge_count = 0usize;
        for edge in self.modulations {
            let target_index = *node_indices.get(edge.target.node.as_str()).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    "modulations.target",
                    "reset modulation target node does not exist",
                )
            })?;
            let source_index = *node_indices.get(edge.from.node.as_str()).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    "modulations.from",
                    "modulation source node does not exist",
                )
            })?;
            incoming_sources[target_index].push(source_index);
            let target = self.nodes.get(target_index).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    "modulations.target",
                    "reset modulation target node does not exist",
                )
            })?;
            let is_reset = matches!(
                target.processor,
                ProcessorView::Core(Processor::Instrument { .. })
            ) && self
                .instrument_control_spec(target, &edge.target.port)
                .is_some_and(|spec| spec.rate == ParameterRate::Reset);
            if is_reset {
                required_controls[source_index] = true;
                reset_edge_count = reset_edge_count.checked_add(1).ok_or_else(overflow)?;
            }
        }
        if reset_edge_count == 0 {
            return Ok(frame_work);
        }

        // Reset targets consume the transitive control-only dependency closure
        // once during renderer preparation. A reverse adjacency walk keeps the
        // charge independent of declaration order while validation guarantees
        // the same-sample graph is acyclic.
        let mut pending = required_controls
            .iter()
            .enumerate()
            .filter_map(|(index, required)| (*required).then_some(index))
            .collect::<Vec<_>>();
        while let Some(target_index) = pending.pop() {
            for source_index in incoming_sources[target_index].iter().copied() {
                if !required_controls[source_index] {
                    required_controls[source_index] = true;
                    pending.push(source_index);
                }
            }
        }

        let mut preparation = 0u64;
        let mut automation_lookup = vec![0u64; self.nodes.len()];
        for lane in self.automations() {
            let node_index = *node_indices.get(lane.target.node.as_str()).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    "automation.target",
                    "automation node does not exist",
                )
            })?;
            let points = lane.points.len().max(1);
            let lookup = 17u64
                .checked_add(u64::from(usize::BITS - (points - 1).leading_zeros()))
                .ok_or_else(overflow)?;
            automation_lookup[node_index] = automation_lookup[node_index]
                .checked_add(lookup)
                .ok_or_else(overflow)?;
        }
        for (index, required) in required_controls.iter().copied().enumerate() {
            if !required {
                continue;
            }
            let node = self.nodes.get(index).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    "modulations.from",
                    "reset modulation source node does not exist",
                )
            })?;
            preparation = preparation
                .checked_add(control_weight(node)?)
                .and_then(|work| work.checked_add(automation_lookup[index]))
                .ok_or_else(overflow)?;
        }
        let closure_edges = required_controls
            .iter()
            .enumerate()
            .filter(|(_, required)| **required)
            .try_fold(0usize, |count, (index, _)| {
                count
                    .checked_add(incoming_sources[index].len())
                    .ok_or_else(overflow)
            })?;
        let edge_count = closure_edges
            .checked_add(reset_edge_count)
            .ok_or_else(overflow)?;
        preparation = preparation
            .checked_add((edge_count as u64).checked_mul(8).ok_or_else(overflow)?)
            .ok_or_else(overflow)?;
        frame_work.checked_add(preparation).ok_or_else(overflow)
    }

    fn audio_execution_work(&self) -> Result<u64, PlanError> {
        let overflow = || {
            err(
                "E_RESOURCE_LIMIT",
                "nodes.audio",
                "audio execution work overflow or invalid interval",
            )
        };
        let mut work = 0u64;
        for node in self.nodes {
            let Some(clip) = node.processor.audio_transport() else {
                continue;
            };
            let phase_cost = if let ProcessorView::WarpRate(warp) = node.processor {
                let segments = warp
                    .warp
                    .len()
                    .checked_add(self.tempo.points.len())
                    .ok_or_else(overflow)?;
                let search = if segments <= 1 {
                    0
                } else {
                    usize::BITS - (segments - 1).leading_zeros()
                };
                96 + u64::from(search)
            } else {
                32
            };
            let active = clip
                .end_frame
                .min(self.output.total_frames)
                .checked_sub(clip.start_frame)
                .ok_or_else(overflow)?;
            let base = self
                .output
                .total_frames
                .checked_mul(4 + u64::from(clip.channels))
                .ok_or_else(overflow)?;
            let cost = active
                .checked_mul(phase_cost + 8 * u64::from(clip.channels))
                .ok_or_else(overflow)?;
            work = work
                .checked_add(base)
                .and_then(|n| n.checked_add(cost))
                .ok_or_else(overflow)?;
        }
        Ok(work)
    }

    fn kit_execution_work(&self) -> Result<u64, PlanError> {
        let overflow = || {
            err(
                "E_RESOURCE_LIMIT",
                "nodes.kit",
                "kit execution work overflow",
            )
        };
        let mut work = 0u64;
        for node in self.nodes {
            let ProcessorView::Kit {
                channels, samples, ..
            } = node.processor
            else {
                continue;
            };
            let base = self
                .output
                .total_frames
                .checked_mul(4 + u64::from(channels))
                .ok_or_else(overflow)?;
            work = work.checked_add(base).ok_or_else(overflow)?;
            for event in self.events().filter(|event| event.target.node == *node.id) {
                let EventKind::Hit { key, .. } = event.kind else {
                    continue;
                };
                let mapping = samples
                    .iter()
                    .find(|sample| sample.key == *key)
                    .ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            "events.kind.key",
                            "kit has no such sample key",
                        )
                    })?;
                let asset = self
                    .audio_assets
                    .unwrap_or(&[])
                    .iter()
                    .find(|asset| asset.id == mapping.asset)
                    .ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            "audio_assets",
                            "kit sample asset does not exist",
                        )
                    })?;
                if asset.rate_hz == 0 {
                    return Err(err(
                        "E_ASSET",
                        "audio_assets.rate_hz",
                        "asset rate must be positive",
                    ));
                }
                let scaled = asset
                    .frames
                    .checked_mul(u64::from(self.output.sample_rate_hz))
                    .ok_or_else(overflow)?;
                let rate = u64::from(asset.rate_hz);
                let lifetime = (scaled / rate)
                    .checked_add(u64::from(scaled % rate != 0))
                    .ok_or_else(overflow)?;
                let remaining = self
                    .output
                    .total_frames
                    .checked_sub(event.on_frame)
                    .ok_or_else(overflow)?;
                let cost = lifetime
                    .min(remaining)
                    .checked_mul(4 + 8 * u64::from(channels))
                    .ok_or_else(overflow)?;
                work = work.checked_add(cost).ok_or_else(overflow)?;
            }
        }
        Ok(work)
    }

    fn validate_instrument_work(&self, limits: &PlanLimits) -> Result<u64, PlanError> {
        let control_work = self.control_execution_work(limits)?;
        let mut work = self
            .kit_execution_work()?
            .checked_add(self.audio_execution_work()?)
            .and_then(|work| work.checked_add(control_work))
            .ok_or_else(|| {
                err(
                    "E_RESOURCE_LIMIT",
                    "nodes.audio",
                    "aggregate execution work overflow",
                )
            })?;
        let mut cells = 0usize;
        for node in self.nodes {
            if matches!(
                node.processor,
                ProcessorView::Lfo(_)
                    | ProcessorView::Constant
                    | ProcessorView::Kit { .. }
                    | ProcessorView::Audio(_)
                    | ProcessorView::WarpRate(_)
            ) {
                continue;
            }
            let cost = match node.processor.core()? {
                Processor::Eq { channels, .. } => 32 + 10 * u64::from(*channels),
                Processor::Fader { channels } => 128 + u64::from(*channels),
                Processor::Compressor {
                    channels,
                    sidechain_channels,
                } => 24 + u64::from(*channels) + u64::from(sidechain_channels.unwrap_or(*channels)),
                Processor::Reverb {
                    channels,
                    predelay_frames,
                    ..
                } => {
                    let count = 15562usize
                        .checked_add(
                            usize::from(*channels)
                                .checked_mul(*predelay_frames as usize + 360)
                                .ok_or_else(|| {
                                    err("E_RESOURCE_LIMIT", "production", "native storage overflow")
                                })?,
                        )
                        .and_then(|n| n.checked_add(8))
                        .ok_or_else(|| {
                            err("E_RESOURCE_LIMIT", "production", "native storage overflow")
                        })?;
                    cells = cells.checked_add(count).ok_or_else(|| {
                        err("E_RESOURCE_LIMIT", "production", "native storage overflow")
                    })?;
                    96 + 40 * u64::from(*channels)
                }
                _ => 0,
            };
            work =
                work.checked_add(self.output.total_frames.checked_mul(cost).ok_or_else(|| {
                    err("E_RESOURCE_LIMIT", "production", "native work overflow")
                })?)
                .ok_or_else(|| err("E_RESOURCE_LIMIT", "production", "native work overflow"))?;
        }
        if cells > limits.max_production_delay_cells {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "production",
                "aggregate native reverb history exceeds the caller limit",
            ));
        }
        for node in self.nodes {
            let ProcessorView::Core(Processor::Instrument { program, .. }) = node.processor else {
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
                    .iter()
                    .fold(0u64, |cost, node| {
                        cost.saturating_add(
                            if matches!(node.processor, crate::graph::GraphProcessor::Pluck { .. })
                            {
                                crate::graph::PLUCK_SAMPLE_WORK
                            } else {
                                1
                            },
                        )
                    })
                    .saturating_add(graph.connections.len() as u64)
                    .saturating_add(graph.modulations.len() as u64)
            };
            if let Some(shared) = &program.shared {
                let shared_cost = graph_cost(shared);
                work = work.saturating_add(self.output.total_frames.saturating_mul(shared_cost));

                // A reset-rate shared LFO phase edge is captured once while a
                // runtime instance is constructed (and again only when its
                // public reset path explicitly reconstructs that state). The
                // preview is independent of notes and shared output routing,
                // so charge it at the instrument-instance boundary. Its
                // bounded sweep pays the full shared graph cost plus one unit
                // per node and modulation edge, including zero-depth edges.
                let mut has_reset_modulation = false;
                for (edge_index, modulation) in shared.modulations.iter().enumerate() {
                    let target = shared
                        .nodes
                        .iter()
                        .find(|candidate| candidate.id == modulation.to.node)
                        .ok_or_else(|| {
                            err(
                                "E_REFERENCE",
                                format!(
                                    "instruments.programs.{}.shared.modulations[{edge_index}].to.node",
                                    program.id
                                ),
                                "shared graph modulation target node does not exist",
                            )
                        })?;
                    let spec = crate::graph::parameter_descriptor_for_stage(
                        &target.processor,
                        &modulation.to.parameter,
                        crate::graph::GraphStage::Shared,
                    )
                    .ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            format!(
                                "instruments.programs.{}.shared.modulations[{edge_index}].to.parameter",
                                program.id
                            ),
                            "shared graph modulation target has no parameter descriptor",
                        )
                    })?;
                    if spec.rate == ParameterRate::Reset {
                        has_reset_modulation = true;
                    }
                }
                if has_reset_modulation {
                    let reset_preview_cost = shared_cost
                        .checked_add(shared.nodes.len() as u64)
                        .and_then(|cost| cost.checked_add(shared.modulations.len() as u64))
                        .ok_or_else(|| {
                            err(
                                "E_RESOURCE_LIMIT",
                                format!("instruments.programs.{}.shared", program.id),
                                "shared reset preview work overflow",
                            )
                        })?;
                    work = work.checked_add(reset_preview_cost).ok_or_else(|| {
                        err(
                            "E_RESOURCE_LIMIT",
                            format!("instruments.programs.{}.shared", program.id),
                            "shared reset preview work overflow",
                        )
                    })?;
                }
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
            let mut internal_note_on = false;
            let mut internal_note_off = false;
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
                for lane in self.automations().filter(|lane| {
                    lane.target.node == *node.id && lane.target.port == *control_name
                }) {
                    for point in lane.points {
                        if point.value > release {
                            release = point.value.clone();
                        }
                    }
                }
                if self
                    .modulations
                    .iter()
                    .any(|edge| edge.target.node == *node.id && edge.target.port == *control_name)
                {
                    // Bound signal-dependent releases by the descriptor
                    // maximum. Apply this conservative policy to all matching
                    // edges, including zero-amount edges.
                    let spec = program.control_spec(control_name).ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            format!(
                                "instruments.programs.{}.controls.{}",
                                program.id, control_name
                            ),
                            "instrument control descriptor does not exist",
                        )
                    })?;
                    release = spec.max;
                }
            }
            for (edge_index, modulation) in program.voice.modulations.iter().enumerate() {
                let target = program
                    .voice
                    .nodes
                    .iter()
                    .find(|candidate| candidate.id == modulation.to.node)
                    .ok_or_else(|| {
                        err(
                            "E_REFERENCE",
                            format!(
                                "instruments.programs.{}.voice.modulations[{edge_index}].to.node",
                                program.id
                            ),
                            "instrument graph modulation target node does not exist",
                        )
                    })?;
                let spec = crate::graph::parameter_descriptor_for_stage(
                    &target.processor,
                    &modulation.to.parameter,
                    crate::graph::GraphStage::Voice,
                )
                .ok_or_else(|| {
                    err(
                        "E_REFERENCE",
                        format!(
                            "instruments.programs.{}.voice.modulations[{edge_index}].to.parameter",
                            program.id
                        ),
                        "instrument graph modulation target has no parameter descriptor",
                    )
                })?;
                match spec.rate {
                    ParameterRate::NoteOn => internal_note_on = true,
                    ParameterRate::NoteOff => internal_note_off = true,
                    ParameterRate::Sample | ParameterRate::Reset => {}
                }
                if modulation.to.node == amplitude && modulation.to.parameter == "release" {
                    // Any internal edge makes the designated amplitude release
                    // signal-dependent. Use its descriptor maximum even when
                    // the edge has zero depth or no public release control.
                    if spec.max > release {
                        release = spec.max;
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
            let initialization_cost = (program.pluck_node_count() as u64)
                .saturating_mul(crate::graph::PLUCK_DELAY_CELLS as u64);
            let capture_cost = voice_cost
                .checked_add(program.voice.nodes.len() as u64)
                .and_then(|cost| cost.checked_add(program.voice.modulations.len() as u64))
                .ok_or_else(|| {
                    err(
                        "E_RESOURCE_LIMIT",
                        format!("instruments.programs.{}.voice", program.id),
                        "instrument event capture work overflow",
                    )
                })?;
            for event in self.events().filter(|event| {
                event.target.node == *node.id && matches!(event.kind, EventKind::Note { .. })
            }) {
                let release_start = event.off_frame.unwrap_or(self.output.total_frames);
                let active_end = release_start
                    .saturating_add(release_frames)
                    .min(self.output.total_frames);
                let active_frames = active_end.saturating_sub(event.on_frame);
                work = work.saturating_add(active_frames.saturating_mul(voice_cost));
                work = work.saturating_add(initialization_cost);
                if let EventKind::Note {
                    pitch_expression,
                    gain_expression,
                    timbre_expression,
                    pressure_expression,
                    ..
                } = &event.kind
                {
                    // Each expression executes independently throughout the conservative
                    // voice window, including silent frames and release tails.
                    for (kind, points) in [
                        ("pitch", pitch_expression.as_ref().map(|e| e.points.len())),
                        ("gain", gain_expression.as_ref().map(|e| e.points.len())),
                        ("timbre", timbre_expression.as_ref().map(|e| e.points.len())),
                        (
                            "pressure",
                            pressure_expression.as_ref().map(|e| e.points.len()),
                        ),
                    ] {
                        if let Some(points) = points {
                            let lookup = usize::BITS - (points - 1).leading_zeros();
                            let cost = 17 + u64::from(lookup);
                            work = active_frames
                                .checked_mul(cost)
                                .and_then(|expression_work| work.checked_add(expression_work))
                                .ok_or_else(|| {
                                    err(
                                        "E_RESOURCE_LIMIT",
                                        "instruments",
                                        format!("{kind} expression work overflow"),
                                    )
                                })?;
                        }
                    }
                }
                if let EventKind::Note {
                    pitch_expression,
                    timbre_expression,
                    pressure_expression,
                    ..
                } = &event.kind
                {
                    // Internal event-rate edges perform one bounded graph
                    // preview at each note boundary they use. The preview
                    // reads the pitch, timbre, and pressure expression values;
                    // gain is applied by the voice path and is not previewed.
                    for (performed, boundary) in [
                        (internal_note_on, "note-on"),
                        (internal_note_off, "note-off"),
                    ] {
                        if !performed {
                            continue;
                        }
                        work = work.checked_add(capture_cost).ok_or_else(|| {
                            err(
                                "E_RESOURCE_LIMIT",
                                format!("nodes.{}.events", node.id),
                                format!("instrument {boundary} capture work overflow"),
                            )
                        })?;
                        for (kind, points) in [
                            ("pitch", pitch_expression.as_ref().map(|e| e.points.len())),
                            ("timbre", timbre_expression.as_ref().map(|e| e.points.len())),
                            (
                                "pressure",
                                pressure_expression.as_ref().map(|e| e.points.len()),
                            ),
                        ] {
                            if let Some(points) = points {
                                let lookup = usize::BITS - (points - 1).leading_zeros();
                                let cost = 17 + u64::from(lookup);
                                work = work.checked_add(cost).ok_or_else(|| {
                                    err(
                                        "E_RESOURCE_LIMIT",
                                        "instruments",
                                        format!("{kind} expression preview work overflow"),
                                    )
                                })?;
                            }
                        }
                    }
                }
            }
        }
        if work > limits.max_execution_work {
            return Err(err(
                "E_RESOURCE_LIMIT",
                "instruments",
                format!(
                    "instrument sample execution work {work} exceeds the selected limit {}",
                    limits.max_execution_work
                ),
            ));
        }
        Ok(work)
    }
}

fn validate_kit_level(
    value: &Rational,
    limits: &PlanLimits,
    path: String,
) -> Result<(), PlanError> {
    rational_bit_limit(value, limits, &path)?;
    finite_engine_rational(value, &path)?;
    if value.is_negative() {
        return Err(err("E_RANGE", path, "kit level must be nonnegative"));
    }
    Ok(())
}

pub(crate) fn validate_identifier(value: &str, path: impl Into<String>) -> Result<(), PlanError> {
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

pub(crate) fn validate_parameter(
    processor: &Processor,
    name: &str,
    value: &Rational,
    node: &str,
    sample_rate_hz: u32,
) -> Result<(), PlanError> {
    finite_engine_rational(value, format!("nodes.{node}.params.{name}"))?;
    if let Some((min, max, open)) = production_parameter_bounds(processor, name) {
        if if open {
            value <= &min || value >= &max
        } else {
            value < &min || value > &max
        } {
            return Err(err(
                "E_RANGE",
                format!("nodes.{node}.params.{name}"),
                "production parameter is outside its range",
            ));
        }
    }
    match (processor, name) {
        (Processor::Sine { .. }, "attack" | "release" | "level")
        | (Processor::Gain { .. }, "gain")
            if value.is_negative() =>
        {
            Err(err(
                "E_RANGE",
                format!("nodes.{node}.params.{name}"),
                "parameter must be nonnegative",
            ))
        }
        (Processor::Fader { .. }, "level") => Ok(()),
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
        validate_parameter(processor, name, &point.value, path, sample_rate_hz)?;
        if ((matches!(
            processor,
            Processor::Eq { .. } | Processor::Compressor { .. }
        ) && matches!(name, "gain" | "threshold" | "knee" | "makeup"))
            || matches!((processor, name), (Processor::Fader { .. }, "level")))
            && point.shape == Interpolation::Exponential
        {
            return Err(err(
                "E_RANGE",
                path,
                "exponential interpolation is forbidden for decibels",
            ));
        }
        match (processor, name) {
            (Processor::Sine { .. }, "attack" | "release" | "level")
            | (Processor::Gain { .. }, "gain")
                if point.value.is_negative() =>
            {
                return Err(err(
                    "E_RANGE",
                    format!("{path}.points[{index}].value"),
                    "automation parameter must be nonnegative",
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
    Control,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PortDescriptor {
    kind: PortKind,
    channels: u8,
    summing: bool,
}

fn port_descriptor(node: NodeView<'_>, port: &str, input: bool) -> Option<PortDescriptor> {
    if matches!(
        node.processor,
        ProcessorView::Lfo(_) | ProcessorView::Constant
    ) {
        return (!input && port == "out").then_some(PortDescriptor {
            kind: PortKind::Control,
            channels: 0,
            summing: false,
        });
    }
    if let Some(clip) = node.processor.audio_transport() {
        return (!input && port == "out").then_some(PortDescriptor {
            kind: PortKind::Audio,
            channels: clip.channels,
            summing: false,
        });
    }
    if let ProcessorView::Kit { channels, .. } = node.processor {
        return match (input, port) {
            (true, "events") => Some(PortDescriptor {
                kind: PortKind::Events,
                channels: 0,
                summing: true,
            }),
            (false, "out") => Some(PortDescriptor {
                kind: PortKind::Audio,
                channels,
                summing: false,
            }),
            _ => None,
        };
    }
    match (node.processor.core().ok()?, input, port) {
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
        | (Processor::OnePole { channels }, false, "out")
        | (Processor::Gain { channels }, true, "in")
        | (Processor::Gain { channels }, false, "out")
        | (Processor::Fader { channels }, true, "in")
        | (Processor::Fader { channels }, false, "out")
        | (Processor::Eq { channels, .. }, true, "in")
        | (Processor::Eq { channels, .. }, false, "out")
        | (Processor::Compressor { channels, .. }, true, "in")
        | (Processor::Compressor { channels, .. }, false, "out")
        | (Processor::Reverb { channels, .. }, true, "in")
        | (Processor::Reverb { channels, .. }, false, "out") => Some(PortDescriptor {
            kind: PortKind::Audio,
            channels: *channels,
            summing: false,
        }),
        (
            Processor::Compressor {
                sidechain_channels: Some(channels),
                ..
            },
            true,
            "sidechain",
        ) => Some(PortDescriptor {
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

fn detect_cycle(nodes: NodeSlice<'_>, edges: &[(String, String)]) -> Result<(), PlanError> {
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

/// Canonical native parameter defaults; immutable mode determines applicability.
pub(crate) fn production_defaults(processor: &Processor) -> BTreeMap<String, Rational> {
    let values: &[(&str, i64, i64)] = match processor {
        Processor::Eq { .. } => &[("frequency", 1000, 1), ("q", 1, 1), ("gain", 0, 1)],
        Processor::Compressor { .. } => &[
            ("threshold", -18, 1),
            ("ratio", 4, 1),
            ("knee", 6, 1),
            ("attack", 1, 100),
            ("release", 1, 10),
            ("makeup", 0, 1),
        ],
        Processor::Reverb { .. } => &[("decay", 3, 2), ("mix", 1, 5)],
        _ => &[],
    };
    values
        .iter()
        .filter(|(name, _, _)| processor.parameter_allowed(name))
        .map(|(name, n, d)| (name.to_string(), Rational::new((*n).into(), (*d).into())))
        .collect()
}

pub(crate) fn production_parameter_bounds(
    processor: &Processor,
    name: &str,
) -> Option<(Rational, Rational, bool)> {
    let (lo, hi, den, open) = match (processor, name) {
        (Processor::Eq { .. }, "frequency") => (0, 24000, 1, true),
        (Processor::Eq { .. }, "q") => (1, 180, 10, false),
        (Processor::Eq { .. }, "gain") => (-24, 24, 1, false),
        (Processor::Compressor { .. }, "threshold") => (-120, 24, 1, false),
        (Processor::Compressor { .. }, "ratio") => (1, 100, 1, false),
        (Processor::Compressor { .. }, "knee") => (0, 24, 1, false),
        (Processor::Compressor { .. }, "attack") => (0, 10, 1, false),
        (Processor::Compressor { .. }, "release") => (0, 30, 1, false),
        (Processor::Compressor { .. }, "makeup") => (-24, 24, 1, false),
        (Processor::Reverb { .. }, "decay") => (1, 300, 10, false),
        (Processor::Reverb { .. }, "mix") => (0, 1, 1, false),
        _ => return None,
    };
    Some((
        Rational::new(lo.into(), den.into()),
        Rational::new(hi.into(), den.into()),
        open,
    ))
}

#[cfg(test)]
mod gain_expression_tests {
    use super::*;

    fn r(n: i64, d: i64) -> Rational {
        Rational::new(n.into(), d.into())
    }
    fn curve(a: Rational, b: Rational, shape: Interpolation) -> GainExpression {
        GainExpression {
            clock: ExpressionClock::Normalized,
            points: vec![
                GainExpressionPoint {
                    position: zero(),
                    gain: a,
                    shape,
                },
                GainExpressionPoint {
                    position: one(),
                    gain: b,
                    shape: Interpolation::Step,
                },
            ],
        }
    }
    fn close(actual: f64, expected: f64) {
        assert!(
            (actual / expected - 1.0).abs() < 2e-12,
            "actual {actual:e}, expected {expected:e}"
        );
    }
    #[test]
    fn analytic_step_linear_exponential_and_exact_knots() {
        let step = curve(r(2, 1), r(8, 1), Interpolation::Step);
        assert_eq!(step.gain_at(&r(1, 2)), 2.0);
        assert_eq!(step.gain_at(&one()), 8.0);
        let linear = curve(zero(), r(8, 1), Interpolation::Linear);
        assert_eq!(linear.gain_at(&r(1, 4)), 2.0);
        assert_eq!(linear.gain_at(&zero()), 0.0);
        let exponential = curve(r(2, 1), r(8, 1), Interpolation::Exponential);
        close(exponential.gain_at(&r(1, 2)), 4.0);
        assert_eq!(exponential.gain_at(&zero()), 2.0);
        assert_eq!(exponential.gain_at(&one()), 8.0);
        assert_eq!(exponential.gain_at(&r(2, 1)), 8.0);
        // Distinct rational positions that collapse onto the same f64 still
        // select the correct right-continuous step at the exact knot.
        let epsilon = Rational::new(1.into(), BigInt::one() << 200);
        assert_eq!(step.gain_at(&(one() - &epsilon)), 2.0);
        assert_eq!(step.gain_at(&(one() + &epsilon)), 8.0);
    }
    #[test]
    fn exponential_extremes_use_rational_logs_without_endpoint_ratio_overflow() {
        let ten = BigInt::from(10);
        for (power, expected) in [(400, 1e-50), (323, 10.0_f64.powf(-11.5))] {
            let tiny = Rational::new(1.into(), ten.pow(power));
            let huge = Rational::from_integer(ten.pow(300));
            let expression = curve(tiny.clone(), huge.clone(), Interpolation::Exponential);
            expression.validate(&PlanLimits::default(), "test").unwrap();
            close(expression.gain_at(&r(1, 2)), expected);
            let descending = curve(huge, tiny.clone(), Interpolation::Exponential);
            close(descending.gain_at(&r(1, 2)), expected);
            assert_eq!(expression.gain_at(&zero()), tiny.to_f64().unwrap());
        }
        let tiny = Rational::new(1.into(), ten.pow(400));
        assert_eq!(
            curve(tiny.clone(), tiny, Interpolation::Exponential).gain_at(&r(1, 2)),
            0.0
        );
        let max = Rational::from_float(f64::MAX).unwrap();
        assert_eq!(
            curve(max.clone(), max, Interpolation::Exponential).gain_at(&r(1, 2)),
            f64::MAX
        );
    }
    #[test]
    fn linear_extremes_interpolate_exact_rationals_before_conversion() {
        let huge = Rational::from_integer(BigInt::from(10).pow(300));
        let tiny = Rational::new(1.into(), BigInt::from(10).pow(400));
        close(
            curve(tiny, huge, Interpolation::Linear).gain_at(&r(1, 2)),
            5e299,
        );
        let epsilon = Rational::new(1.into(), BigInt::one() << 200);
        let mut expression = curve(zero(), r(2, 1), Interpolation::Linear);
        expression.points[0].position = one() - &epsilon;
        expression.points[1].position = one() + &epsilon;
        assert_eq!(expression.gain_at(&one()), 1.0);
    }
    #[test]
    fn shared_clock_uses_effective_gate_and_holds_release_endpoint() {
        let event = ResolvedEvent {
            address: "note".into(),
            source: SourceMapping {
                object: "note".into(),
                path: vec!["note".into()],
                span: None,
            },
            target: EventTarget::new("sine", "events").unwrap(),
            kind: EventKind::Note {
                pitch_hz: 440.0,
                velocity: one(),
                pitch_expression: None,
                gain_expression: None,
                timbre_expression: None,
                pressure_expression: None,
            },
            score_on_q: r(10, 1),
            score_off_q: Some(r(13, 1)),
            onset_offset_seconds: zero(),
            release_offset_seconds: zero(),
            on_seconds: r(1, 1),
            off_seconds: Some(r(3, 2)),
            release_velocity: 0.0,
            on_frame: 48_000,
            off_frame: Some(72_000),
            order: 0,
        };
        for (clock, midpoint, end) in [
            (ExpressionClock::Seconds, r(1, 4), r(1, 2)),
            (ExpressionClock::Score, r(3, 2), r(3, 1)),
            (ExpressionClock::Normalized, r(1, 2), one()),
        ] {
            assert_eq!(clock.coordinate_at(&event, 47_999, 48_000), zero());
            assert_eq!(clock.coordinate_at(&event, 60_000, 48_000), midpoint);
            assert_eq!(clock.coordinate_at(&event, 72_000, 48_000), end);
            assert_eq!(clock.coordinate_at(&event, 80_000, 48_000), end);
        }
    }
}

#[cfg(test)]
mod timbre_expression_tests {
    use super::*;

    fn curve(a: Rational, b: Rational, shape: Interpolation) -> TimbreExpression {
        TimbreExpression {
            clock: ExpressionClock::Normalized,
            points: vec![
                TimbreExpressionPoint {
                    position: zero(),
                    value: a,
                    shape,
                },
                TimbreExpressionPoint {
                    position: one(),
                    value: b,
                    shape: Interpolation::Step,
                },
            ],
        }
    }
    #[test]
    fn exact_knots_and_bounded_scalar_interpolation() {
        let half = Rational::new(1.into(), 2.into());
        let epsilon = Rational::new(1.into(), BigInt::one() << 200);
        let step = curve(zero(), one(), Interpolation::Step);
        assert_eq!(step.value_at(&(one() - &epsilon)), 0.0);
        assert_eq!(step.value_at(&one()), 1.0);
        assert_eq!(step.value_at(&(one() + &epsilon)), 1.0);
        let linear = curve(zero(), one(), Interpolation::Linear);
        assert_eq!(linear.value_at(&half), 0.5);
        let exponential = curve(
            Rational::new(1.into(), 4.into()),
            one(),
            Interpolation::Exponential,
        );
        assert!((exponential.value_at(&half) - 0.5).abs() < 1e-15);
        for power in [323, 400] {
            let tiny = Rational::new(1.into(), BigInt::from(10).pow(power));
            for (a, b) in [(tiny.clone(), one()), (one(), tiny.clone())] {
                let expression = curve(a, b, Interpolation::Exponential);
                expression.validate(&PlanLimits::default(), "test").unwrap();
                let expected = 10.0_f64.powf(-(power as f64) / 2.0);
                assert!((expression.value_at(&half) / expected - 1.0).abs() < 2e-12);
            }
            assert_eq!(
                curve(tiny.clone(), tiny.clone(), Interpolation::Exponential).value_at(&half),
                tiny.to_f64().unwrap()
            );
        }
    }
}

#[cfg(test)]
mod pressure_expression_tests {
    use super::*;

    fn curve(a: Rational, b: Rational, shape: Interpolation) -> PressureExpression {
        PressureExpression {
            clock: ExpressionClock::Normalized,
            points: vec![
                PressureExpressionPoint {
                    position: zero(),
                    value: a,
                    shape,
                },
                PressureExpressionPoint {
                    position: one(),
                    value: b,
                    shape: Interpolation::Step,
                },
            ],
        }
    }
    #[test]
    fn exact_knots_and_bounded_scalar_interpolation() {
        let half = Rational::new(1.into(), 2.into());
        let epsilon = Rational::new(1.into(), BigInt::one() << 200);
        let step = curve(zero(), one(), Interpolation::Step);
        assert_eq!(step.value_at(&(one() - &epsilon)), 0.0);
        assert_eq!(step.value_at(&one()), 1.0);
        assert_eq!(step.value_at(&(one() + &epsilon)), 1.0);
        let linear = curve(zero(), one(), Interpolation::Linear);
        assert_eq!(linear.value_at(&half), 0.5);
        let exponential = curve(
            Rational::new(1.into(), 4.into()),
            one(),
            Interpolation::Exponential,
        );
        assert!((exponential.value_at(&half) - 0.5).abs() < 1e-15);
        for power in [323, 400] {
            let tiny = Rational::new(1.into(), BigInt::from(10).pow(power));
            for (a, b) in [(tiny.clone(), one()), (one(), tiny.clone())] {
                let expression = curve(a, b, Interpolation::Exponential);
                expression.validate(&PlanLimits::default(), "test").unwrap();
                let expected = 10.0_f64.powf(-(power as f64) / 2.0);
                assert!((expression.value_at(&half) / expected - 1.0).abs() < 2e-12);
            }
            assert_eq!(
                curve(tiny.clone(), tiny.clone(), Interpolation::Exponential).value_at(&half),
                tiny.to_f64().unwrap()
            );
        }
    }
}

#[cfg(test)]
mod modulation_admission_tests {
    #[test]
    fn lfo_admission_consumes_the_existing_timing_budget() {
        let bundle = crate::SourceBundle::new(
            "score.maac",
            r#"maac 1;
project p {score=[0q,1q];tail=0s;rate=48000Hz;tempo=&clock;meter=&metre;output=&sound:out;}
tempo clock {points=[(0q,60bpm,linear),(1q,120bpm,step)];} meter metre {points=[(0q,4,4)];}
node sound {type="core.sine/1";} node motion {type="core.lfo/1";config={period=1q;};}
"#,
        );
        let artifact = crate::compiler::compile_bundle_artifact(&bundle).unwrap();
        let view = artifact.view();
        let limits = super::PlanLimits::default();
        let depleted = crate::plan_v3::TimingContext::new_with_limits(
            view.tempo,
            view.output,
            &super::PlanLimits {
                max_work: 0,
                ..limits
            },
        )
        .unwrap();
        let error = view.validate_with_timing(&limits, &depleted).unwrap_err();
        assert_eq!(error.code, "E_RESOURCE_LIMIT");
    }
}
