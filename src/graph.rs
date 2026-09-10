//! Typed reusable instrument graphs and validation for untrusted plans.
//!
//! This module owns graph structure and processor metadata. Source resolution,
//! wavetable lookup, aggregate bundle limits, and rendering live at later
//! boundaries.

use crate::exact::MAX_RATIONAL_BITS;
use crate::plan::{
    rational_map_serde, rational_serde, Connection, PlanError, PortRef, Rational, SourceSpan,
};
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const MAX_GRAPH_PROGRAMS: usize = 128;
pub const MAX_GRAPH_NODES: usize = 64;
pub const MAX_GRAPH_EDGES: usize = 256;
pub const MAX_INSTRUMENT_CONTROLS: usize = 64;
pub const MAX_TOTAL_GRAPH_NODES: usize = 1_024;
pub const MAX_TOTAL_GRAPH_EDGES: usize = 4_096;
pub const MAX_EMBEDDED_SAMPLES: usize = 262_144;
pub const MAX_ALLOCATED_VOICE_GRAPH_STATES: usize = 262_144;
pub const MAX_EXECUTION_WORK: u64 = 500_000_000;
pub const DEFAULT_NOISE_SEED: u32 = 1_831_565_813;
pub const DEFAULT_PLUCK_SEED: u32 = DEFAULT_NOISE_SEED;
pub const PLUCK_DELAY_CELLS: usize = crate::pluck::DELAY_CELLS;
pub const MAX_PLUCK_DELAY_CELLS: usize = 8_388_608;
pub const PLUCK_SAMPLE_WORK: u64 = 16;

pub(crate) fn pluck_delay_cells(capacity: usize, nodes: usize) -> Option<usize> {
    capacity.checked_mul(nodes)?.checked_mul(PLUCK_DELAY_CELLS)
}

const MAX_ID_BYTES: usize = 128;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

/// A processor in the version 1 synthesis palette.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum GraphProcessor {
    #[serde(rename = "synth.sine/1")]
    Sine,
    #[serde(rename = "synth.saw/1")]
    Saw,
    #[serde(rename = "synth.square/1")]
    Square,
    #[serde(rename = "synth.triangle/1")]
    Triangle,
    #[serde(rename = "synth.noise/1")]
    Noise { seed: u32 },
    #[serde(rename = "synth.pluck/1")]
    Pluck { seed: u32 },
    #[serde(rename = "synth.wavetable/1")]
    Wavetable { table: String },
    #[serde(rename = "synth.adsr/1")]
    Adsr,
    #[serde(rename = "synth.timbre/1")]
    Timbre,
    #[serde(rename = "synth.pressure/1")]
    Pressure,
    #[serde(rename = "synth.lfo/1")]
    Lfo,
    #[serde(rename = "synth.gain/1")]
    Gain { channels: u8 },
    #[serde(rename = "synth.onepole/1")]
    OnePole { channels: u8 },
    #[serde(rename = "synth.highpass/1")]
    HighPass { channels: u8 },
    #[serde(rename = "synth.mix/1")]
    Mix { channels: u8 },
    #[serde(rename = "synth.pan/1")]
    Pan,
}

#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum GraphProcessorWire {
    #[serde(rename = "synth.sine/1")]
    Sine {},
    #[serde(rename = "synth.saw/1")]
    Saw {},
    #[serde(rename = "synth.square/1")]
    Square {},
    #[serde(rename = "synth.triangle/1")]
    Triangle {},
    #[serde(rename = "synth.noise/1")]
    Noise { seed: u32 },
    #[serde(rename = "synth.pluck/1")]
    Pluck { seed: u32 },
    #[serde(rename = "synth.wavetable/1")]
    Wavetable { table: String },
    #[serde(rename = "synth.adsr/1")]
    Adsr {},
    #[serde(rename = "synth.timbre/1")]
    Timbre {},
    #[serde(rename = "synth.pressure/1")]
    Pressure {},
    #[serde(rename = "synth.lfo/1")]
    Lfo {},
    #[serde(rename = "synth.gain/1")]
    Gain { channels: u8 },
    #[serde(rename = "synth.onepole/1")]
    OnePole { channels: u8 },
    #[serde(rename = "synth.highpass/1")]
    HighPass { channels: u8 },
    #[serde(rename = "synth.mix/1")]
    Mix { channels: u8 },
    #[serde(rename = "synth.pan/1")]
    Pan {},
}

impl<'de> Deserialize<'de> for GraphProcessor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match GraphProcessorWire::deserialize(deserializer)? {
            GraphProcessorWire::Sine {} => Self::Sine,
            GraphProcessorWire::Saw {} => Self::Saw,
            GraphProcessorWire::Square {} => Self::Square,
            GraphProcessorWire::Triangle {} => Self::Triangle,
            GraphProcessorWire::Noise { seed: 0 } => {
                return Err(serde::de::Error::custom("noise seed must be nonzero"));
            }
            GraphProcessorWire::Noise { seed } => Self::Noise { seed },
            GraphProcessorWire::Pluck { seed: 0 } => {
                return Err(serde::de::Error::custom("pluck seed must be nonzero"));
            }
            GraphProcessorWire::Pluck { seed } => Self::Pluck { seed },
            GraphProcessorWire::Wavetable { table } => Self::Wavetable { table },
            GraphProcessorWire::Adsr {} => Self::Adsr,
            GraphProcessorWire::Timbre {} => Self::Timbre,
            GraphProcessorWire::Pressure {} => Self::Pressure,
            GraphProcessorWire::Lfo {} => Self::Lfo,
            GraphProcessorWire::Gain { channels } => Self::Gain { channels },
            GraphProcessorWire::OnePole { channels } => Self::OnePole { channels },
            GraphProcessorWire::HighPass { channels } => Self::HighPass { channels },
            GraphProcessorWire::Mix { channels } => Self::Mix { channels },
            GraphProcessorWire::Pan {} => Self::Pan,
        })
    }
}

impl GraphProcessor {
    pub const fn identity(&self) -> &'static str {
        match self {
            Self::Sine => "synth.sine/1",
            Self::Saw => "synth.saw/1",
            Self::Square => "synth.square/1",
            Self::Triangle => "synth.triangle/1",
            Self::Noise { .. } => "synth.noise/1",
            Self::Pluck { .. } => "synth.pluck/1",
            Self::Wavetable { .. } => "synth.wavetable/1",
            Self::Adsr => "synth.adsr/1",
            Self::Timbre => "synth.timbre/1",
            Self::Pressure => "synth.pressure/1",
            Self::Lfo => "synth.lfo/1",
            Self::Gain { .. } => "synth.gain/1",
            Self::OnePole { .. } => "synth.onepole/1",
            Self::HighPass { .. } => "synth.highpass/1",
            Self::Mix { .. } => "synth.mix/1",
            Self::Pan => "synth.pan/1",
        }
    }

    fn output_channels(&self) -> u8 {
        match self {
            Self::Gain { channels }
            | Self::OnePole { channels }
            | Self::HighPass { channels }
            | Self::Mix { channels } => *channels,
            Self::Pan => 2,
            _ => 1,
        }
    }

    fn required_input_channels(&self) -> Option<u8> {
        match self {
            Self::Gain { channels }
            | Self::OnePole { channels }
            | Self::HighPass { channels }
            | Self::Mix { channels } => Some(*channels),
            Self::Pan => Some(1),
            _ => None,
        }
    }

    fn accepts_many_inputs(&self) -> bool {
        matches!(self, Self::Mix { .. })
    }

    fn forbidden_in_shared(&self) -> bool {
        matches!(
            self,
            Self::Sine
                | Self::Saw
                | Self::Square
                | Self::Triangle
                | Self::Noise { .. }
                | Self::Pluck { .. }
                | Self::Wavetable { .. }
                | Self::Adsr
                | Self::Timbre
                | Self::Pressure
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphNode {
    pub id: String,
    pub processor: GraphProcessor,
    #[serde(default, with = "rational_map_serde")]
    pub params: BTreeMap<String, Rational>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterTarget {
    pub node: String,
    pub parameter: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modulation {
    pub id: String,
    pub from: PortRef,
    pub to: ParameterTarget,
    #[serde(with = "rational_serde")]
    pub depth: Rational,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphProgram {
    pub channels: u8,
    pub nodes: Vec<GraphNode>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub modulations: Vec<Modulation>,
    pub output: PortRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amplitude: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphStage {
    Voice,
    Shared,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlTarget {
    pub graph: GraphStage,
    pub node: String,
    pub parameter: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub target: ControlTarget,
    #[serde(with = "rational_serde")]
    pub default: Rational,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramSource {
    pub file: String,
    pub object: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentProgram {
    pub id: String,
    pub voice: GraphProgram,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<GraphProgram>,
    #[serde(default, with = "control_map_serde")]
    pub controls: BTreeMap<String, Control>,
    pub source: ProgramSource,
}

impl InstrumentProgram {
    /// Whether the voice graph opts into per-note timbre expression.
    pub fn supports_timbre(&self) -> bool {
        self.voice
            .nodes
            .iter()
            .any(|node| matches!(node.processor, GraphProcessor::Timbre))
    }

    /// Whether the voice graph opts into per-note pressure expression.
    pub fn supports_pressure(&self) -> bool {
        self.voice
            .nodes
            .iter()
            .any(|node| matches!(node.processor, GraphProcessor::Pressure))
    }

    pub(crate) fn pluck_node_count(&self) -> usize {
        self.voice
            .nodes
            .iter()
            .filter(|node| matches!(node.processor, GraphProcessor::Pluck { .. }))
            .count()
    }

    pub fn channels(&self) -> u8 {
        self.shared
            .as_ref()
            .map_or(self.voice.channels, |graph| graph.channels)
    }

    pub fn control_spec(&self, name: &str) -> Option<ParameterSpec> {
        let control = self.controls.get(name)?;
        let graph = match control.target.graph {
            GraphStage::Voice => &self.voice,
            GraphStage::Shared => self.shared.as_ref()?,
        };
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id == control.target.node)?;
        parameter_descriptor_for_stage(
            &node.processor,
            &control.target.parameter,
            control.target.graph,
        )
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        // Local collection ceilings are deliberately checked before walking
        // references or exact numeric values.
        if self.controls.len() > MAX_INSTRUMENT_CONTROLS {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "controls",
                "instrument control limit exceeded",
            ));
        }
        validate_graph_resources(&self.voice, "voice")?;
        if let Some(shared) = &self.shared {
            validate_graph_resources(shared, "shared")?;
        }

        validate_identifier(&self.id, "id")?;
        validate_source(&self.source)?;
        validate_graph_inner(&self.voice, GraphStage::Voice, None, "voice")?;
        if let Some(shared) = &self.shared {
            validate_graph_inner(
                shared,
                GraphStage::Shared,
                Some(self.voice.channels),
                "shared",
            )?;
        }

        let mut targets = BTreeSet::new();
        for (name, control) in &self.controls {
            let path = format!("controls.{name}");
            validate_identifier(name, &path)?;
            validate_identifier(&control.target.node, format!("{path}.target.node"))?;
            validate_identifier(
                &control.target.parameter,
                format!("{path}.target.parameter"),
            )?;
            if !targets.insert(control.target.clone()) {
                return Err(error(
                    "E_AUTOMATION_WRITER",
                    format!("{path}.target"),
                    "two controls cannot expose the same graph parameter",
                ));
            }
            let graph = match control.target.graph {
                GraphStage::Voice => &self.voice,
                GraphStage::Shared => self.shared.as_ref().ok_or_else(|| {
                    error(
                        "E_REFERENCE",
                        format!("{path}.target.graph"),
                        "control refers to a missing shared graph",
                    )
                })?,
            };
            let node = graph
                .nodes
                .iter()
                .find(|node| node.id == control.target.node)
                .ok_or_else(|| {
                    error(
                        "E_REFERENCE",
                        format!("{path}.target.node"),
                        "control target node does not exist",
                    )
                })?;
            let spec = parameter_descriptor_for_stage(
                &node.processor,
                &control.target.parameter,
                control.target.graph,
            )
            .ok_or_else(|| {
                error(
                    "E_REFERENCE",
                    format!("{path}.target.parameter"),
                    "control target parameter does not exist",
                )
            })?;
            validate_parameter_value(&control.default, &spec, format!("{path}.default"))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphUnit {
    Dimensionless,
    Seconds,
    Hertz,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterRate {
    Sample,
    NoteOn,
    NoteOff,
    Reset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParameterSpec {
    pub unit: GraphUnit,
    pub rate: ParameterRate,
    pub default: Rational,
    pub min: Rational,
    pub max: Rational,
    pub min_open: bool,
    pub max_open: bool,
}

impl ParameterSpec {
    pub fn validate(&self, value: &Rational) -> Result<(), PlanError> {
        validate_rational(value, "parameter")?;
        let below = if self.min_open {
            value <= &self.min
        } else {
            value < &self.min
        };
        let above = if self.max_open {
            value >= &self.max
        } else {
            value > &self.max
        };
        if below || above {
            return Err(error(
                "E_RANGE",
                "parameter",
                "parameter value is outside its allowed range",
            ));
        }
        Ok(())
    }
}

/// Processor-owned parameter metadata. LFO phase has voice semantics here;
/// use [`parameter_descriptor_for_stage`] for a shared graph.
pub fn parameter_descriptor(processor: &GraphProcessor, name: &str) -> Option<ParameterSpec> {
    let dimensionless = |rate, default, min, max| {
        spec(
            GraphUnit::Dimensionless,
            rate,
            default,
            min,
            max,
            false,
            false,
        )
    };
    let oscillator = || match name {
        "ratio" => Some(dimensionless(ParameterRate::Sample, 1, -64, 64)),
        "frequency" => Some(spec(
            GraphUnit::Hertz,
            ParameterRate::Sample,
            0,
            -24_000,
            24_000,
            false,
            false,
        )),
        "phase" => Some(dimensionless(ParameterRate::NoteOn, 0, 0, 1)),
        "level" => Some(dimensionless(ParameterRate::Sample, 1, 0, 16)),
        _ => None,
    };

    match processor {
        GraphProcessor::Sine
        | GraphProcessor::Saw
        | GraphProcessor::Square
        | GraphProcessor::Triangle => oscillator(),
        GraphProcessor::Wavetable { .. } => oscillator().or_else(|| {
            (name == "position").then(|| dimensionless(ParameterRate::Sample, 0, 0, 1))
        }),
        GraphProcessor::Pluck { .. } => match name {
            "ratio" => {
                let mut result = dimensionless(ParameterRate::Sample, 1, 0, 8);
                result.min = Rational::new(1.into(), 8.into());
                Some(result)
            }
            "decay" => {
                let mut result = spec(
                    GraphUnit::Seconds,
                    ParameterRate::Sample,
                    3,
                    0,
                    30,
                    false,
                    false,
                );
                result.min = Rational::new(1.into(), 20.into());
                Some(result)
            }
            "damping" => {
                let mut result = dimensionless(ParameterRate::Sample, 0, 0, 1);
                result.default = Rational::new(1.into(), 2.into());
                Some(result)
            }
            "level" => Some(dimensionless(ParameterRate::Sample, 1, 0, 16)),
            _ => None,
        },
        GraphProcessor::Adsr => match name {
            "attack" => Some(spec(
                GraphUnit::Seconds,
                ParameterRate::NoteOn,
                0,
                0,
                1_800,
                false,
                false,
            )),
            "decay" => Some(spec(
                GraphUnit::Seconds,
                ParameterRate::NoteOn,
                0,
                0,
                1_800,
                false,
                false,
            )),
            "sustain" => Some(dimensionless(ParameterRate::NoteOn, 1, 0, 1)),
            "release" => Some(spec(
                GraphUnit::Seconds,
                ParameterRate::NoteOff,
                0,
                0,
                1_800,
                false,
                false,
            )),
            _ => None,
        },
        GraphProcessor::Lfo => match name {
            "frequency" => Some(spec(
                GraphUnit::Hertz,
                ParameterRate::Sample,
                1,
                -200,
                200,
                false,
                false,
            )),
            "phase" => Some(dimensionless(ParameterRate::NoteOn, 0, 0, 1)),
            "level" => Some(dimensionless(ParameterRate::Sample, 1, 0, 16)),
            _ => None,
        },
        GraphProcessor::Gain { .. } | GraphProcessor::Noise { .. } => {
            (name == "level").then(|| dimensionless(ParameterRate::Sample, 1, 0, 16))
        }
        GraphProcessor::OnePole { .. } | GraphProcessor::HighPass { .. } => (name == "cutoff")
            .then(|| {
                spec(
                    GraphUnit::Hertz,
                    ParameterRate::Sample,
                    1_000,
                    0,
                    24_000,
                    true,
                    true,
                )
            }),
        GraphProcessor::Pan => {
            (name == "pan").then(|| dimensionless(ParameterRate::Sample, 0, -1, 1))
        }
        GraphProcessor::Mix { .. } | GraphProcessor::Timbre | GraphProcessor::Pressure => None,
    }
}

pub fn parameter_descriptor_for_stage(
    processor: &GraphProcessor,
    name: &str,
    stage: GraphStage,
) -> Option<ParameterSpec> {
    let mut result = parameter_descriptor(processor, name)?;
    if stage == GraphStage::Shared && matches!(processor, GraphProcessor::Lfo) && name == "phase" {
        result.rate = ParameterRate::Reset;
    }
    Some(result)
}

pub fn validate_graph(
    graph: &GraphProgram,
    is_voice: bool,
    input_channels: Option<u8>,
) -> Result<(), PlanError> {
    validate_graph_resources(graph, "graph")?;
    let stage = if is_voice {
        GraphStage::Voice
    } else {
        GraphStage::Shared
    };
    validate_graph_inner(graph, stage, input_channels, "graph")
}

/// Return node indices in deterministic dependency order. Audio and
/// modulation edges both participate; node IDs break ties.
pub fn topological_order(graph: &GraphProgram) -> Result<Vec<usize>, PlanError> {
    validate_graph_resources(graph, "graph")?;
    let mut index = HashMap::with_capacity(graph.nodes.len());
    for (node_index, node) in graph.nodes.iter().enumerate() {
        if index.insert(node.id.as_str(), node_index).is_some() {
            return Err(error(
                "E_DUPLICATE_ID",
                format!("nodes.{}", node.id),
                "duplicate graph node ID",
            ));
        }
    }
    let mut indegree = vec![0usize; graph.nodes.len()];
    let mut outgoing = vec![Vec::new(); graph.nodes.len()];
    for (path, from, to) in graph
        .connections
        .iter()
        .map(|edge| {
            (
                format!("connections.{}", edge.id),
                &edge.from.node,
                &edge.to.node,
            )
        })
        .chain(graph.modulations.iter().map(|edge| {
            (
                format!("modulations.{}", edge.id),
                &edge.from.node,
                &edge.to.node,
            )
        }))
    {
        if from == "input" {
            continue;
        }
        let Some(&from_index) = index.get(from.as_str()) else {
            return Err(error(
                "E_REFERENCE",
                format!("{path}.from.node"),
                "source node does not exist",
            ));
        };
        let Some(&to_index) = index.get(to.as_str()) else {
            return Err(error(
                "E_REFERENCE",
                format!("{path}.to.node"),
                "target node does not exist",
            ));
        };
        indegree[to_index] = indegree[to_index].saturating_add(1);
        outgoing[from_index].push(to_index);
    }

    let mut ready = BTreeSet::new();
    for (node_index, count) in indegree.iter().enumerate() {
        if *count == 0 {
            ready.insert((graph.nodes[node_index].id.as_str(), node_index));
        }
    }
    let mut order = Vec::with_capacity(graph.nodes.len());
    while let Some(&(id, node_index)) = ready.iter().next() {
        ready.remove(&(id, node_index));
        order.push(node_index);
        for &target in &outgoing[node_index] {
            indegree[target] -= 1;
            if indegree[target] == 0 {
                ready.insert((graph.nodes[target].id.as_str(), target));
            }
        }
    }
    if order.len() != graph.nodes.len() {
        return Err(error(
            "E_ALGEBRAIC_LOOP",
            "graph",
            "audio and modulation graph must be acyclic",
        ));
    }
    Ok(order)
}

fn validate_graph_resources(graph: &GraphProgram, path: &str) -> Result<(), PlanError> {
    if graph.nodes.len() > MAX_GRAPH_NODES {
        return Err(error(
            "E_RESOURCE_LIMIT",
            format!("{path}.nodes"),
            "graph node limit exceeded",
        ));
    }
    if graph
        .connections
        .len()
        .saturating_add(graph.modulations.len())
        > MAX_GRAPH_EDGES
    {
        return Err(error(
            "E_RESOURCE_LIMIT",
            format!("{path}.edges"),
            "graph edge limit exceeded",
        ));
    }
    Ok(())
}

fn validate_graph_inner(
    graph: &GraphProgram,
    stage: GraphStage,
    input_channels: Option<u8>,
    path: &str,
) -> Result<(), PlanError> {
    validate_channels(graph.channels, format!("{path}.channels"))?;
    if stage == GraphStage::Shared {
        validate_channels(
            input_channels.ok_or_else(|| {
                error(
                    "E_REFERENCE",
                    format!("{path}.input"),
                    "shared graph input channels are required",
                )
            })?,
            format!("{path}.input"),
        )?;
        if graph.amplitude.is_some() {
            return Err(error(
                "E_CAPABILITY",
                format!("{path}.amplitude"),
                "shared graph cannot declare an amplitude envelope",
            ));
        }
    } else if input_channels.is_some() {
        return Err(error(
            "E_CAPABILITY",
            format!("{path}.input"),
            "voice graph cannot receive the reserved shared input",
        ));
    }

    let mut node_indices = HashMap::with_capacity(graph.nodes.len());
    for (index, node) in graph.nodes.iter().enumerate() {
        let node_path = format!("{path}.nodes.{}", node.id);
        validate_identifier(&node.id, &node_path)?;
        if node.id == "input" {
            return Err(error(
                "E_DUPLICATE_ID",
                node_path,
                "input is a reserved graph source ID",
            ));
        }
        if node_indices.insert(node.id.as_str(), index).is_some() {
            return Err(error(
                "E_DUPLICATE_ID",
                node_path,
                "duplicate graph node ID",
            ));
        }
        validate_processor(&node.processor, stage, format!("{node_path}.processor"))?;
        for (name, value) in &node.params {
            validate_identifier(name, format!("{node_path}.params.{name}"))?;
            let spec =
                parameter_descriptor_for_stage(&node.processor, name, stage).ok_or_else(|| {
                    error(
                        "E_RANGE",
                        format!("{node_path}.params.{name}"),
                        "processor has no such parameter",
                    )
                })?;
            validate_parameter_value(value, &spec, format!("{node_path}.params.{name}"))?;
        }
    }

    let mut edge_ids = BTreeSet::new();
    let mut input_counts = vec![0usize; graph.nodes.len()];
    for connection in &graph.connections {
        let edge_path = format!("{path}.connections.{}", connection.id);
        validate_identifier(&connection.id, &edge_path)?;
        validate_identifier(&connection.from.node, format!("{edge_path}.from.node"))?;
        validate_identifier(&connection.from.port, format!("{edge_path}.from.port"))?;
        validate_identifier(&connection.to.node, format!("{edge_path}.to.node"))?;
        validate_identifier(&connection.to.port, format!("{edge_path}.to.port"))?;
        if !edge_ids.insert(connection.id.as_str()) {
            return Err(error(
                "E_DUPLICATE_ID",
                edge_path,
                "duplicate graph edge ID",
            ));
        }
        let source_channels = source_channels(
            &connection.from,
            graph,
            &node_indices,
            stage,
            input_channels,
            format!("{edge_path}.from"),
        )?;
        if connection.to.port != "in" {
            return Err(error(
                "E_PORT_TYPE",
                format!("{edge_path}.to.port"),
                "audio target port must be in",
            ));
        }
        let &target_index = node_indices
            .get(connection.to.node.as_str())
            .ok_or_else(|| {
                error(
                    "E_REFERENCE",
                    format!("{edge_path}.to.node"),
                    "audio target node does not exist",
                )
            })?;
        let target = &graph.nodes[target_index];
        let expected = target.processor.required_input_channels().ok_or_else(|| {
            error(
                "E_PORT_TYPE",
                format!("{edge_path}.to"),
                "processor has no audio input",
            )
        })?;
        if source_channels != expected {
            return Err(error(
                "E_PORT_TYPE",
                edge_path,
                "audio connection channel counts do not match",
            ));
        }
        input_counts[target_index] = input_counts[target_index].saturating_add(1);
    }
    for (index, node) in graph.nodes.iter().enumerate() {
        if node.processor.required_input_channels().is_some()
            && !node.processor.accepts_many_inputs()
            && input_counts[index] != 1
        {
            return Err(error(
                "E_PORT_TYPE",
                format!("{path}.nodes.{}.in", node.id),
                "processor requires exactly one audio input",
            ));
        }
    }

    for modulation in &graph.modulations {
        let edge_path = format!("{path}.modulations.{}", modulation.id);
        validate_identifier(&modulation.id, &edge_path)?;
        validate_identifier(&modulation.from.node, format!("{edge_path}.from.node"))?;
        validate_identifier(&modulation.from.port, format!("{edge_path}.from.port"))?;
        validate_identifier(&modulation.to.node, format!("{edge_path}.to.node"))?;
        validate_identifier(
            &modulation.to.parameter,
            format!("{edge_path}.to.parameter"),
        )?;
        if !edge_ids.insert(modulation.id.as_str()) {
            return Err(error(
                "E_DUPLICATE_ID",
                edge_path,
                "duplicate graph edge ID",
            ));
        }
        let channels = source_channels(
            &modulation.from,
            graph,
            &node_indices,
            stage,
            input_channels,
            format!("{edge_path}.from"),
        )?;
        if channels != 1 {
            return Err(error(
                "E_PORT_TYPE",
                format!("{edge_path}.from"),
                "modulation source must be mono",
            ));
        }
        let &target_index = node_indices
            .get(modulation.to.node.as_str())
            .ok_or_else(|| {
                error(
                    "E_REFERENCE",
                    format!("{edge_path}.to.node"),
                    "modulation target node does not exist",
                )
            })?;
        let target = &graph.nodes[target_index];
        let spec =
            parameter_descriptor_for_stage(&target.processor, &modulation.to.parameter, stage)
                .ok_or_else(|| {
                    error(
                        "E_REFERENCE",
                        format!("{edge_path}.to.parameter"),
                        "modulation target parameter does not exist",
                    )
                })?;
        let voice_event_rate = stage == GraphStage::Voice
            && matches!(
                target.processor,
                GraphProcessor::Adsr
                    | GraphProcessor::Sine
                    | GraphProcessor::Saw
                    | GraphProcessor::Square
                    | GraphProcessor::Triangle
                    | GraphProcessor::Wavetable { .. }
                    | GraphProcessor::Lfo
            )
            && matches!(spec.rate, ParameterRate::NoteOn | ParameterRate::NoteOff);
        let shared_lfo_reset = stage == GraphStage::Shared
            && matches!(target.processor, GraphProcessor::Lfo)
            && spec.rate == ParameterRate::Reset;
        if spec.rate != ParameterRate::Sample && !voice_event_rate && !shared_lfo_reset {
            return Err(error(
                "E_PORT_TYPE",
                format!("{edge_path}.to.parameter"),
                "modulation requires a sample-rate parameter or supported event-rate parameter",
            ));
        }
        validate_modulation_depth(&modulation.depth, spec.unit, format!("{edge_path}.depth"))?;
    }

    validate_identifier(&graph.output.node, format!("{path}.output.node"))?;
    validate_identifier(&graph.output.port, format!("{path}.output.port"))?;
    if graph.output.port != "out" {
        return Err(error(
            "E_PORT_TYPE",
            format!("{path}.output.port"),
            "graph output port must be out",
        ));
    }
    let output = graph
        .nodes
        .iter()
        .find(|node| node.id == graph.output.node)
        .ok_or_else(|| {
            error(
                "E_REFERENCE",
                format!("{path}.output.node"),
                "graph output node does not exist",
            )
        })?;
    if output.processor.output_channels() != graph.channels {
        return Err(error(
            "E_PORT_TYPE",
            format!("{path}.output"),
            "graph output channel count does not match declaration",
        ));
    }

    match stage {
        GraphStage::Voice => {
            let amplitude = graph.amplitude.as_ref().ok_or_else(|| {
                error(
                    "E_REFERENCE",
                    format!("{path}.amplitude"),
                    "voice graph requires an amplitude ADSR",
                )
            })?;
            validate_identifier(amplitude, format!("{path}.amplitude"))?;
            let node = graph
                .nodes
                .iter()
                .find(|node| node.id == *amplitude)
                .ok_or_else(|| {
                    error(
                        "E_REFERENCE",
                        format!("{path}.amplitude"),
                        "amplitude node does not exist",
                    )
                })?;
            if !matches!(node.processor, GraphProcessor::Adsr) {
                return Err(error(
                    "E_PORT_TYPE",
                    format!("{path}.amplitude"),
                    "amplitude node must be an ADSR",
                ));
            }
        }
        GraphStage::Shared => {}
    }

    topological_order(graph)?;
    Ok(())
}

fn source_channels(
    source: &PortRef,
    graph: &GraphProgram,
    node_indices: &HashMap<&str, usize>,
    stage: GraphStage,
    input_channels: Option<u8>,
    path: String,
) -> Result<u8, PlanError> {
    if source.port != "out" {
        return Err(error(
            "E_PORT_TYPE",
            format!("{path}.port"),
            "audio source port must be out",
        ));
    }
    if source.node == "input" {
        return if stage == GraphStage::Shared {
            input_channels.ok_or_else(|| {
                error(
                    "E_REFERENCE",
                    path,
                    "shared graph input channels are missing",
                )
            })
        } else {
            Err(error(
                "E_REFERENCE",
                path,
                "reserved input is only available in a shared graph",
            ))
        };
    }
    let &index = node_indices.get(source.node.as_str()).ok_or_else(|| {
        error(
            "E_REFERENCE",
            format!("{path}.node"),
            "source node does not exist",
        )
    })?;
    Ok(graph.nodes[index].processor.output_channels())
}

fn validate_processor(
    processor: &GraphProcessor,
    stage: GraphStage,
    path: String,
) -> Result<(), PlanError> {
    if stage == GraphStage::Shared && processor.forbidden_in_shared() {
        return Err(error("E_CAPABILITY", path, "processor is voice-only"));
    }
    match processor {
        GraphProcessor::Pluck { seed: 0 } => {
            return Err(error(
                "E_RANGE",
                format!("{path}.seed"),
                "pluck seed must be nonzero",
            ));
        }
        GraphProcessor::Noise { seed: 0 } => {
            return Err(error(
                "E_RANGE",
                format!("{path}.seed"),
                "noise seed must be nonzero",
            ));
        }
        GraphProcessor::Gain { channels }
        | GraphProcessor::OnePole { channels }
        | GraphProcessor::HighPass { channels }
        | GraphProcessor::Mix { channels } => validate_channels(*channels, path)?,
        GraphProcessor::Wavetable { table } => validate_identifier(table, format!("{path}.table"))?,
        _ => {}
    }
    Ok(())
}

fn validate_parameter_value(
    value: &Rational,
    spec: &ParameterSpec,
    path: String,
) -> Result<(), PlanError> {
    spec.validate(value).map_err(|mut error| {
        error.path = path;
        error
    })
}

fn validate_modulation_depth(
    value: &Rational,
    unit: GraphUnit,
    path: String,
) -> Result<(), PlanError> {
    validate_rational(value, &path)?;
    let bound = match unit {
        GraphUnit::Hertz => 48_000,
        GraphUnit::Seconds => 1_800,
        GraphUnit::Dimensionless => 128,
    };
    let bound = Rational::from_integer(BigInt::from(bound));
    if value < &-bound.clone() || value > &bound {
        return Err(error(
            "E_RANGE",
            path,
            "modulation depth exceeds the unit bound",
        ));
    }
    Ok(())
}

fn validate_rational(value: &Rational, path: &str) -> Result<(), PlanError> {
    if value.denom().is_zero() || value.denom().is_negative() {
        return Err(error(
            "E_NONFINITE",
            path,
            "rational denominator must be positive",
        ));
    }
    if value.numer().magnitude().bits() > MAX_RATIONAL_BITS
        || value.denom().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(error(
            "E_RESOURCE_LIMIT",
            path,
            "rational exceeds the exact-value bit limit",
        ));
    }
    Ok(())
}

fn validate_channels(channels: u8, path: String) -> Result<(), PlanError> {
    if !(1..=2).contains(&channels) {
        return Err(error(
            "E_RANGE",
            path,
            "only mono and stereo graphs are supported",
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, path: impl Into<String>) -> Result<(), PlanError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_alphabetic() || byte == b'_'
            } else {
                byte.is_ascii_alphanumeric() || byte == b'_'
            }
        })
    {
        return Err(error(
            "E_RANGE",
            path,
            "identifier is not a bounded ASCII MaaC ID",
        ));
    }
    Ok(())
}

fn validate_source(source: &ProgramSource) -> Result<(), PlanError> {
    if source.file.is_empty()
        || source.file.len() > MAX_PATH_BYTES
        || source.file.starts_with('/')
        || source.file.contains('\\')
        || source
            .file
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(error(
            "E_RANGE",
            "source.file",
            "source file must be a bounded normalized project-relative POSIX path",
        ));
    }
    if source.object.is_empty() || source.object.len() > MAX_PATH_BYTES {
        return Err(error(
            "E_RANGE",
            "source.object",
            "source object provenance must be nonempty and bounded",
        ));
    }
    if let Some(span) = source.span {
        if span.start > span.end || span.end > MAX_SOURCE_BYTES {
            return Err(error(
                "E_RANGE",
                "source.span",
                "source span must be a bounded half-open byte range",
            ));
        }
    }
    Ok(())
}

fn spec(
    unit: GraphUnit,
    rate: ParameterRate,
    default: i64,
    min: i64,
    max: i64,
    min_open: bool,
    max_open: bool,
) -> ParameterSpec {
    ParameterSpec {
        unit,
        rate,
        default: Rational::from_integer(BigInt::from(default)),
        min: Rational::from_integer(BigInt::from(min)),
        max: Rational::from_integer(BigInt::from(max)),
        min_open,
        max_open,
    }
}

fn error(code: &str, path: impl Into<String>, message: impl Into<String>) -> PlanError {
    PlanError {
        code: code.into(),
        path: path.into(),
        message: message.into(),
        span: None,
    }
}

mod control_map_serde {
    use super::*;
    use serde::de::{self, MapAccess, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S>(value: &BTreeMap<String, Control>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<String, Control>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ControlMapVisitor;

        impl<'de> Visitor<'de> for ControlMapVisitor {
            type Value = BTreeMap<String, Control>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object of unique public controls")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut controls = BTreeMap::new();
                while let Some((name, control)) = map.next_entry::<String, Control>()? {
                    if controls.insert(name, control).is_some() {
                        return Err(de::Error::custom(
                            "E_DUPLICATE_FIELD: duplicate public control name",
                        ));
                    }
                }
                Ok(controls)
            }
        }

        deserializer.deserialize_map(ControlMapVisitor)
    }
}

#[cfg(test)]
mod pluck_resource_tests {
    use super::*;

    #[test]
    fn pluck_storage_arithmetic_never_wraps_before_resource_validation() {
        assert_eq!(pluck_delay_cells(3492, 1), Some(8_387_784));
        assert_eq!(pluck_delay_cells(3493, 1), Some(8_390_186));
        assert_eq!(pluck_delay_cells(usize::MAX, 2), None);
        assert_eq!(
            pluck_delay_cells(usize::MAX / PLUCK_DELAY_CELLS + 1, 1),
            None
        );
        assert_eq!(pluck_delay_cells(usize::MAX, 0), Some(0));
    }
}
