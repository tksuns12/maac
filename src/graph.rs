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
use num_traits::{Signed, ToPrimitive, Zero};
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
/// Per-sample work of a `synth.svf/1` or `synth.drive/1` node. Measured at
/// 1.4 and 1.9 times a one-unit oscillator node, rounded up.
pub const SVF_SAMPLE_WORK: u64 = 2;
pub const DRIVE_SAMPLE_WORK: u64 = 2;
/// Per-sample work of a `synth.delay/1` counted in seconds, and in quarter
/// notes, which evaluates the tempo map and its inverse each frame. On an
/// Apple M1 in an effect, per node and frame, the first took 14 ns and the
/// second 34 ns, where `synth.drive/1` (2 units) took 67 ns.
pub const DELAY_SAMPLE_WORK: u64 = 1;
pub const SCORE_DELAY_SAMPLE_WORK: u64 = 2;
/// The longest a `synth.delay/1` line may be, in seconds.
pub const MAX_DELAY_SECONDS: i64 = 60;
/// Resonators of a `synth.modal/1` node per unit of per-sample work. One
/// resonator measured at about 1/6 of a one-unit oscillator node.
pub const MODAL_RESONATORS_PER_WORK: u64 = 4;

/// Per-sample work of a `synth.modal/1` node: one unit per four resonators,
/// rounded up.
pub const fn modal_sample_work(modes: u8, strings: u8) -> u64 {
    (modes as u64 * strings as u64).div_ceil(MODAL_RESONATORS_PER_WORK)
}

/// Per-sample work of one player of a `synth.bow/1` node: a string with its
/// bowing, vibrato and tuning, and a body of about 50 resonators. In a
/// release benchmark they measured 22.8 and 9.8 times a one-unit oscillator
/// node.
pub const BOW_PLAYER_WORK: u64 = 23;
pub const BOW_BODY_WORK: u64 = 10;
/// Delay cells per bowed string: two lines of 4,096.
pub const BOW_STRING_CELLS: usize = 8_192;
/// The most bowed-string delay cells one instrument instance may declare,
/// across its voices.
pub const MAX_BOW_STRING_CELLS: usize = 16_777_216;

/// Per-sample work of a `synth.bow/1` node.
pub const fn bow_sample_work(players: u8, body: crate::bow::BowBody) -> u64 {
    let body = match body {
        crate::bow::BowBody::None => 0,
        _ => BOW_BODY_WORK,
    };
    players as u64 * (BOW_PLAYER_WORK + body)
}

/// Delay cells for `capacity` voices of a graph whose bowed sections hold
/// `players` players in all.
pub(crate) fn bow_string_cells(capacity: usize, players: usize) -> Option<usize> {
    capacity.checked_mul(players)?.checked_mul(BOW_STRING_CELLS)
}

pub(crate) fn pluck_delay_cells(capacity: usize, nodes: usize) -> Option<usize> {
    capacity.checked_mul(nodes)?.checked_mul(PLUCK_DELAY_CELLS)
}

const MAX_ID_BYTES: usize = 128;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

fn mono() -> u8 {
    1
}

fn is_mono(channels: &u8) -> bool {
    *channels == 1
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// What an envelope does when a connected note re-articulates its voice:
/// continue, or enter its attack again from its present value (§8.3).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdsrJoin {
    #[default]
    Hold,
    Restart,
}

impl AdsrJoin {
    fn is_hold(&self) -> bool {
        *self == Self::Hold
    }
}

impl GraphProcessor {
    /// An envelope that holds through connected notes, the default.
    pub const ADSR: Self = Self::Adsr {
        on_join: AdsrJoin::Hold,
    };
}

/// How a `synth.delay/1` counts its length: in seconds, or in quarter notes
/// of score time.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelayClock {
    #[default]
    Seconds,
    Score,
}

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
    /// White noise from `seed`, or, with `per_note`, from `seed` mixed with
    /// each voice's note address.
    #[serde(rename = "synth.noise/1")]
    Noise {
        seed: u32,
        #[serde(skip_serializing_if = "is_false")]
        per_note: bool,
    },
    #[serde(rename = "synth.pluck/1")]
    Pluck { seed: u32 },
    /// A constant per note in [-1, 1), drawn from `seed` and the note's
    /// address.
    #[serde(rename = "synth.random/1")]
    Random { seed: u32 },
    /// A bank of decaying resonators at stretched harmonics, excited by its
    /// input.
    #[serde(rename = "synth.modal/1")]
    Modal { modes: u8, strings: u8 },
    /// A section of bowed strings playing the voice's line, each player with
    /// a body.
    #[serde(rename = "synth.bow/1")]
    Bow {
        players: u8,
        body: crate::bow::BowBody,
        channels: u8,
        seed: u32,
    },
    #[serde(rename = "synth.wavetable/1")]
    Wavetable { table: String },
    /// Pitched playback of embedded samples chosen by key zone.
    #[serde(rename = "synth.sample/1")]
    Sample {
        /// Output channels, equal to every zone sample's. Absent on the wire
        /// means mono, so mono payloads are unchanged.
        #[serde(skip_serializing_if = "is_mono")]
        channels: u8,
        zones: Vec<crate::sample_instrument::SampleZone>,
        #[serde(
            default,
            skip_serializing_if = "crate::sample_instrument::SampleFadeShape::is_linear"
        )]
        fade_shape: crate::sample_instrument::SampleFadeShape,
    },
    /// An envelope; `on_join` says whether a re-articulated connected note
    /// restarts it.
    #[serde(rename = "synth.adsr/1")]
    Adsr {
        #[serde(skip_serializing_if = "AdsrJoin::is_hold")]
        on_join: AdsrJoin,
    },
    #[serde(rename = "synth.timbre/1")]
    Timbre,
    #[serde(rename = "synth.pressure/1")]
    Pressure,
    /// The note's velocity, constant for the voice's life.
    #[serde(rename = "synth.velocity/1")]
    Velocity,
    /// The voice's base frequency in octaves from C4.
    #[serde(rename = "synth.key/1")]
    Key,
    #[serde(rename = "synth.lfo/1")]
    Lfo,
    #[serde(rename = "synth.gain/1")]
    Gain { channels: u8 },
    #[serde(rename = "synth.onepole/1")]
    OnePole { channels: u8 },
    #[serde(rename = "synth.highpass/1")]
    HighPass { channels: u8 },
    /// A resonant two-pole state-variable filter.
    #[serde(rename = "synth.svf/1")]
    Svf {
        channels: u8,
        mode: crate::synth::SvfMode,
    },
    /// Anti-aliased `tanh` saturation with an offset.
    #[serde(rename = "synth.drive/1")]
    Drive { channels: u8 },
    #[serde(rename = "synth.mix/1")]
    Mix { channels: u8 },
    #[serde(rename = "synth.pan/1")]
    Pan,
    /// A stereo balance: the side away from it is lowered.
    #[serde(rename = "synth.balance/1")]
    Balance,
    /// A stereo width, applied to the mid and side signals.
    #[serde(rename = "synth.width/1")]
    Width,
    /// A delay line whose length is a parameter, with feedback inside the
    /// node; `max` seconds bounds the length and sets the line's memory.
    #[serde(rename = "synth.delay/1")]
    Delay {
        channels: u8,
        clock: DelayClock,
        #[serde(with = "rational_serde")]
        max: Rational,
    },
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
    Noise {
        seed: u32,
        #[serde(default)]
        per_note: bool,
    },
    #[serde(rename = "synth.pluck/1")]
    Pluck { seed: u32 },
    #[serde(rename = "synth.random/1")]
    Random { seed: u32 },
    #[serde(rename = "synth.modal/1")]
    Modal { modes: u8, strings: u8 },
    #[serde(rename = "synth.bow/1")]
    Bow {
        players: u8,
        body: crate::bow::BowBody,
        channels: u8,
        seed: u32,
    },
    #[serde(rename = "synth.wavetable/1")]
    Wavetable { table: String },
    #[serde(rename = "synth.sample/1")]
    Sample {
        #[serde(default = "mono")]
        channels: u8,
        zones: Vec<crate::sample_instrument::SampleZone>,
        #[serde(default)]
        fade_shape: crate::sample_instrument::SampleFadeShape,
    },
    #[serde(rename = "synth.adsr/1")]
    Adsr {
        #[serde(default)]
        on_join: AdsrJoin,
    },
    #[serde(rename = "synth.timbre/1")]
    Timbre {},
    #[serde(rename = "synth.pressure/1")]
    Pressure {},
    #[serde(rename = "synth.velocity/1")]
    Velocity {},
    #[serde(rename = "synth.key/1")]
    Key {},
    #[serde(rename = "synth.lfo/1")]
    Lfo {},
    #[serde(rename = "synth.gain/1")]
    Gain { channels: u8 },
    #[serde(rename = "synth.onepole/1")]
    OnePole { channels: u8 },
    #[serde(rename = "synth.highpass/1")]
    HighPass { channels: u8 },
    #[serde(rename = "synth.svf/1")]
    Svf {
        channels: u8,
        mode: crate::synth::SvfMode,
    },
    #[serde(rename = "synth.drive/1")]
    Drive { channels: u8 },
    #[serde(rename = "synth.mix/1")]
    Mix { channels: u8 },
    #[serde(rename = "synth.pan/1")]
    Pan {},
    #[serde(rename = "synth.balance/1")]
    Balance {},
    #[serde(rename = "synth.width/1")]
    Width {},
    #[serde(rename = "synth.delay/1")]
    Delay {
        channels: u8,
        clock: DelayClock,
        #[serde(with = "rational_serde")]
        max: Rational,
    },
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
            GraphProcessorWire::Noise { seed: 0, .. } => {
                return Err(serde::de::Error::custom("noise seed must be nonzero"));
            }
            GraphProcessorWire::Noise { seed, per_note } => Self::Noise { seed, per_note },
            GraphProcessorWire::Random { seed: 0 } => {
                return Err(serde::de::Error::custom("random seed must be nonzero"));
            }
            GraphProcessorWire::Random { seed } => Self::Random { seed },
            GraphProcessorWire::Pluck { seed: 0 } => {
                return Err(serde::de::Error::custom("pluck seed must be nonzero"));
            }
            GraphProcessorWire::Pluck { seed } => Self::Pluck { seed },
            GraphProcessorWire::Modal { modes, strings } => Self::Modal { modes, strings },
            GraphProcessorWire::Bow { seed: 0, .. } => {
                return Err(serde::de::Error::custom("bow seed must be nonzero"));
            }
            GraphProcessorWire::Bow {
                players,
                body,
                channels,
                seed,
            } => Self::Bow {
                players,
                body,
                channels,
                seed,
            },
            GraphProcessorWire::Wavetable { table } => Self::Wavetable { table },
            GraphProcessorWire::Sample {
                channels,
                zones,
                fade_shape,
            } => Self::Sample {
                channels,
                zones,
                fade_shape,
            },
            GraphProcessorWire::Adsr { on_join } => Self::Adsr { on_join },
            GraphProcessorWire::Timbre {} => Self::Timbre,
            GraphProcessorWire::Pressure {} => Self::Pressure,
            GraphProcessorWire::Velocity {} => Self::Velocity,
            GraphProcessorWire::Key {} => Self::Key,
            GraphProcessorWire::Lfo {} => Self::Lfo,
            GraphProcessorWire::Gain { channels } => Self::Gain { channels },
            GraphProcessorWire::OnePole { channels } => Self::OnePole { channels },
            GraphProcessorWire::HighPass { channels } => Self::HighPass { channels },
            GraphProcessorWire::Svf { channels, mode } => Self::Svf { channels, mode },
            GraphProcessorWire::Drive { channels } => Self::Drive { channels },
            GraphProcessorWire::Mix { channels } => Self::Mix { channels },
            GraphProcessorWire::Pan {} => Self::Pan,
            GraphProcessorWire::Balance {} => Self::Balance,
            GraphProcessorWire::Width {} => Self::Width,
            GraphProcessorWire::Delay {
                channels,
                clock,
                max,
            } => Self::Delay {
                channels,
                clock,
                max,
            },
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
            Self::Random { .. } => "synth.random/1",
            Self::Modal { .. } => "synth.modal/1",
            Self::Bow { .. } => "synth.bow/1",
            Self::Wavetable { .. } => "synth.wavetable/1",
            Self::Sample { .. } => "synth.sample/1",
            Self::Adsr { .. } => "synth.adsr/1",
            Self::Timbre => "synth.timbre/1",
            Self::Pressure => "synth.pressure/1",
            Self::Velocity => "synth.velocity/1",
            Self::Key => "synth.key/1",
            Self::Lfo => "synth.lfo/1",
            Self::Gain { .. } => "synth.gain/1",
            Self::OnePole { .. } => "synth.onepole/1",
            Self::HighPass { .. } => "synth.highpass/1",
            Self::Svf { .. } => "synth.svf/1",
            Self::Drive { .. } => "synth.drive/1",
            Self::Mix { .. } => "synth.mix/1",
            Self::Pan => "synth.pan/1",
            Self::Balance => "synth.balance/1",
            Self::Width => "synth.width/1",
            Self::Delay { .. } => "synth.delay/1",
        }
    }

    fn output_channels(&self) -> u8 {
        match self {
            Self::Gain { channels }
            | Self::OnePole { channels }
            | Self::HighPass { channels }
            | Self::Svf { channels, .. }
            | Self::Drive { channels }
            | Self::Mix { channels }
            | Self::Sample { channels, .. }
            | Self::Bow { channels, .. }
            | Self::Delay { channels, .. } => *channels,
            Self::Pan | Self::Balance | Self::Width => 2,
            _ => 1,
        }
    }

    fn required_input_channels(&self) -> Option<u8> {
        match self {
            Self::Gain { channels }
            | Self::OnePole { channels }
            | Self::HighPass { channels }
            | Self::Svf { channels, .. }
            | Self::Drive { channels }
            | Self::Mix { channels }
            | Self::Delay { channels, .. } => Some(*channels),
            Self::Pan | Self::Modal { .. } => Some(1),
            Self::Balance | Self::Width => Some(2),
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
                | Self::Random { .. }
                | Self::Pluck { .. }
                | Self::Modal { .. }
                | Self::Bow { .. }
                | Self::Wavetable { .. }
                | Self::Sample { .. }
                | Self::Adsr { .. }
                | Self::Timbre
                | Self::Pressure
                | Self::Velocity
                | Self::Key
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
    /// Whether the voice plays connected notes (§8.3).
    #[serde(default, skip_serializing_if = "is_false")]
    pub connected: bool,
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

    /// Whether notes that touch on its track join (§8.3): its voice says
    /// so, or holds a bowed section.
    pub(crate) fn plays_connected(&self) -> bool {
        self.voice.connected || self.bow_player_count() > 0
    }

    /// The players of every bowed section in the voice graph.
    pub(crate) fn bow_player_count(&self) -> usize {
        self.voice
            .nodes
            .iter()
            .map(|node| match node.processor {
                GraphProcessor::Bow { players, .. } => usize::from(players),
                _ => 0,
            })
            .sum()
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
        for (stage, graph) in std::iter::once(("voice", &self.voice))
            .chain(self.shared.as_ref().map(|graph| ("shared", graph)))
        {
            if let Some(node) = graph
                .nodes
                .iter()
                .find(|node| matches!(node.processor, GraphProcessor::Delay { .. }))
            {
                return Err(error(
                    "E_CAPABILITY",
                    format!("{stage}.nodes.{}.processor", node.id),
                    "synth.delay/1 is allowed only in an effect",
                ));
            }
        }
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

/// An effect: one graph, under the rules of an instrument's shared graph,
/// that processes the audio arriving at its node's `in` (`&input:out`)
/// behind named controls. It has no voices.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectProgram {
    pub id: String,
    /// The width of the audio at `in`; the graph's own `channels` is `out`'s.
    pub input_channels: u8,
    pub graph: GraphProgram,
    #[serde(default, with = "control_map_serde")]
    pub controls: BTreeMap<String, Control>,
    pub source: ProgramSource,
}

impl EffectProgram {
    pub fn channels(&self) -> u8 {
        self.graph.channels
    }

    pub fn control_spec(&self, name: &str) -> Option<ParameterSpec> {
        let control = self.controls.get(name)?;
        if control.target.graph != GraphStage::Shared {
            return None;
        }
        let node = self
            .graph
            .nodes
            .iter()
            .find(|node| node.id == control.target.node)?;
        parameter_descriptor_for_stage(
            &node.processor,
            &control.target.parameter,
            GraphStage::Shared,
        )
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        if self.controls.len() > MAX_INSTRUMENT_CONTROLS {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "controls",
                "effect control limit exceeded",
            ));
        }
        validate_graph_resources(&self.graph, "graph")?;
        validate_identifier(&self.id, "id")?;
        validate_source(&self.source)?;
        if !(1..=2).contains(&self.input_channels) {
            return Err(error(
                "E_PORT_TYPE",
                "input_channels",
                "effect input must have one or two channels",
            ));
        }
        validate_graph_inner(
            &self.graph,
            GraphStage::Shared,
            Some(self.input_channels),
            "graph",
        )?;
        let mut targets = BTreeSet::new();
        for (name, control) in &self.controls {
            let path = format!("controls.{name}");
            validate_identifier(name, &path)?;
            validate_identifier(&control.target.node, format!("{path}.target.node"))?;
            validate_identifier(
                &control.target.parameter,
                format!("{path}.target.parameter"),
            )?;
            if control.target.graph != GraphStage::Shared {
                return Err(error(
                    "E_REFERENCE",
                    format!("{path}.target.graph"),
                    "an effect control must target its graph",
                ));
            }
            if !targets.insert(control.target.clone()) {
                return Err(error(
                    "E_AUTOMATION_WRITER",
                    format!("{path}.target"),
                    "two controls cannot expose the same graph parameter",
                ));
            }
            let node = self
                .graph
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
                GraphStage::Shared,
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
    /// Quarter notes of score time (`q`).
    Quarter,
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

/// Every parameter name a `synth.*/1` descriptor accepts. Semantic
/// normalization expands each one a node's descriptor defines, so a new
/// parameter is listed here with its descriptor; debug builds check that.
pub(crate) const PARAMETER_NAMES: [&str; 30] = [
    "attack",
    "balance",
    "bias",
    "curve",
    "cutoff",
    "damping",
    "decay",
    "detune",
    "drive",
    "dynamics",
    "feedback",
    "frequency",
    "hardness",
    "level",
    "pan",
    "phase",
    "pickup",
    "position",
    "q",
    "rate",
    "ratio",
    "reference",
    "release",
    "spread",
    "stretch",
    "sustain",
    "time",
    "timing",
    "vibrato",
    "width",
];

/// Processor-owned parameter metadata. LFO phase has voice semantics here;
/// use [`parameter_descriptor_for_stage`] for a shared graph.
pub fn parameter_descriptor(processor: &GraphProcessor, name: &str) -> Option<ParameterSpec> {
    let result = descriptor(processor, name);
    debug_assert!(
        result.is_none() || PARAMETER_NAMES.contains(&name),
        "parameter `{name}` of `{}` is missing from PARAMETER_NAMES",
        processor.identity()
    );
    result
}

fn descriptor(processor: &GraphProcessor, name: &str) -> Option<ParameterSpec> {
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
        // Oscillator frequency controls without a phase: playback starts at
        // the first sample frame.
        GraphProcessor::Sample { .. } => (name != "phase").then(oscillator).flatten(),
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
            "hardness" => {
                let mut result = dimensionless(ParameterRate::NoteOn, 1, 0, 1);
                result.min = Rational::new(1.into(), 100.into());
                Some(result)
            }
            "position" => {
                let mut result = dimensionless(ParameterRate::NoteOn, 0, 0, 1);
                result.max = Rational::new(1.into(), 2.into());
                Some(result)
            }
            "pickup" => {
                let mut result = dimensionless(ParameterRate::Sample, 0, 0, 1);
                result.max = Rational::new(1.into(), 2.into());
                Some(result)
            }
            _ => None,
        },
        GraphProcessor::Bow { .. } => {
            let fraction = |rate, default: (i64, i64), min: (i64, i64), max: (i64, i64)| {
                let mut result = dimensionless(rate, 0, 0, 1);
                result.default = Rational::new(default.0.into(), default.1.into());
                result.min = Rational::new(min.0.into(), min.1.into());
                result.max = Rational::new(max.0.into(), max.1.into());
                result
            };
            match name {
                "ratio" => Some(fraction(ParameterRate::Sample, (1, 1), (1, 8), (8, 1))),
                "dynamics" => Some(fraction(ParameterRate::Sample, (1, 2), (0, 1), (1, 1))),
                "position" => Some(fraction(ParameterRate::Sample, (1, 8), (1, 10), (1, 7))),
                // Numbers of cents: graph parameters have no cents unit.
                "vibrato" => Some(fraction(ParameterRate::Sample, (25, 2), (0, 1), (50, 1))),
                "rate" => {
                    let mut result = spec(
                        GraphUnit::Hertz,
                        ParameterRate::Sample,
                        0,
                        3,
                        8,
                        false,
                        false,
                    );
                    result.default = Rational::new(28.into(), 5.into());
                    Some(result)
                }
                "attack" => {
                    let mut result = spec(
                        GraphUnit::Seconds,
                        ParameterRate::Sample,
                        0,
                        0,
                        1,
                        false,
                        false,
                    );
                    result.default = Rational::new(2.into(), 25.into());
                    result.min = Rational::new(1.into(), 200.into());
                    result.max = Rational::new(1.into(), 2.into());
                    Some(result)
                }
                "timing" => {
                    let mut result = spec(
                        GraphUnit::Seconds,
                        ParameterRate::Sample,
                        0,
                        0,
                        1,
                        false,
                        false,
                    );
                    result.default = Rational::new(9.into(), 500.into());
                    result.max = Rational::new(1.into(), 25.into());
                    Some(result)
                }
                "spread" => Some(fraction(ParameterRate::Sample, (1, 1), (0, 1), (1, 1))),
                "width" => Some(fraction(ParameterRate::Sample, (7, 10), (0, 1), (2, 1))),
                "pan" => Some(fraction(ParameterRate::Sample, (0, 1), (-1, 1), (1, 1))),
                "level" => Some(dimensionless(ParameterRate::Sample, 1, 0, 16)),
                _ => None,
            }
        }
        GraphProcessor::Modal { .. } => match name {
            "ratio" => {
                let mut result = dimensionless(ParameterRate::Sample, 1, 0, 8);
                result.min = Rational::new(1.into(), 8.into());
                Some(result)
            }
            "stretch" => {
                let mut result = dimensionless(ParameterRate::Sample, 0, 0, 1);
                result.max = Rational::new(1.into(), 16.into());
                Some(result)
            }
            "decay" => {
                let mut result = spec(
                    GraphUnit::Seconds,
                    ParameterRate::Sample,
                    2,
                    0,
                    60,
                    false,
                    false,
                );
                result.min = Rational::new(1.into(), 100.into());
                Some(result)
            }
            "damping" => Some(dimensionless(ParameterRate::Sample, 0, 0, 4)),
            "position" => {
                let mut result = dimensionless(ParameterRate::Sample, 0, 0, 1);
                result.max = Rational::new(1.into(), 2.into());
                Some(result)
            }
            // A number of cents: graph parameters have no cents unit.
            "detune" => Some(dimensionless(ParameterRate::Sample, 0, 0, 50)),
            "level" => Some(dimensionless(ParameterRate::Sample, 1, 0, 16)),
            _ => None,
        },
        GraphProcessor::Adsr { .. } => match name {
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
            "curve" => Some(dimensionless(ParameterRate::NoteOn, 0, 0, 16)),
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
        GraphProcessor::Svf { .. } => match name {
            "cutoff" => Some(spec(
                GraphUnit::Hertz,
                ParameterRate::Sample,
                1_000,
                0,
                24_000,
                false,
                true,
            )),
            "ratio" => Some(dimensionless(ParameterRate::Sample, 0, 0, 64)),
            "q" => {
                let mut result = dimensionless(ParameterRate::Sample, 0, 0, 40);
                result.default = Rational::new(707.into(), 1_000.into());
                result.min = Rational::new(1.into(), 10.into());
                Some(result)
            }
            _ => None,
        },
        GraphProcessor::Drive { .. } => match name {
            "drive" => Some(dimensionless(ParameterRate::Sample, 1, 0, 64)),
            "bias" => Some(dimensionless(ParameterRate::Sample, 0, -4, 4)),
            "level" => Some(dimensionless(ParameterRate::Sample, 1, 0, 16)),
            "reference" => Some(dimensionless(ParameterRate::Sample, 0, 0, 1)),
            _ => None,
        },
        GraphProcessor::Pan => {
            (name == "pan").then(|| dimensionless(ParameterRate::Sample, 0, -1, 1))
        }
        GraphProcessor::Balance => {
            (name == "balance").then(|| dimensionless(ParameterRate::Sample, 0, -1, 1))
        }
        GraphProcessor::Width => {
            (name == "width").then(|| dimensionless(ParameterRate::Sample, 1, -4, 4))
        }
        GraphProcessor::Delay { clock, .. } => match name {
            // The length's range here is static; the node also checks each
            // frame that it is at least one frame and at most `max`.
            "time" => Some(match clock {
                DelayClock::Seconds => {
                    let mut result = spec(
                        GraphUnit::Seconds,
                        ParameterRate::Sample,
                        0,
                        0,
                        MAX_DELAY_SECONDS,
                        true,
                        false,
                    );
                    result.default = Rational::new(1.into(), 2.into());
                    result
                }
                DelayClock::Score => spec(
                    GraphUnit::Quarter,
                    ParameterRate::Sample,
                    1,
                    0,
                    256,
                    true,
                    false,
                ),
            }),
            "feedback" => {
                let mut result = dimensionless(ParameterRate::Sample, 0, 0, 1);
                result.max = Rational::new(99.into(), 100.into());
                Some(result)
            }
            "damping" => Some(dimensionless(ParameterRate::Sample, 0, 0, 1)),
            _ => None,
        },
        GraphProcessor::Mix { .. }
        | GraphProcessor::Timbre
        | GraphProcessor::Pressure
        | GraphProcessor::Velocity
        | GraphProcessor::Random { .. }
        | GraphProcessor::Key => None,
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
    // A shared graph has no note, so its filters cannot track one.
    if stage == GraphStage::Shared
        && matches!(processor, GraphProcessor::Svf { .. })
        && name == "ratio"
    {
        result.max = Rational::from_integer(BigInt::zero());
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
/// modulation edges both participate; node IDs break ties. An audio
/// connection into a `synth.delay/1` does not: the line's output in a frame
/// was written in earlier frames, so a cycle may close through its `in`.
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
    let mut incoming = vec![Vec::new(); graph.nodes.len()];
    for (path, from, to, audio) in graph
        .connections
        .iter()
        .map(|edge| {
            (
                format!("connections.{}", edge.id),
                &edge.from.node,
                &edge.to.node,
                true,
            )
        })
        .chain(graph.modulations.iter().map(|edge| {
            (
                format!("modulations.{}", edge.id),
                &edge.from.node,
                &edge.to.node,
                false,
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
        if audio
            && matches!(
                graph.nodes[to_index].processor,
                GraphProcessor::Delay { .. }
            )
        {
            continue;
        }
        indegree[to_index] = indegree[to_index].saturating_add(1);
        outgoing[from_index].push(to_index);
        incoming[to_index].push(from_index);
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
        let cycle = same_sample_cycle(graph, &indegree, &incoming);
        return Err(error(
            "E_ALGEBRAIC_LOOP",
            "graph",
            format!(
                "same-sample cycle {}: a cycle must pass through a delay line's `in`",
                cycle.join(" -> ")
            ),
        ));
    }
    Ok(order)
}

/// One cycle among the nodes that a topological sort left unordered, as node
/// IDs from its least ID around to that ID again. Each such node still has
/// an unordered predecessor, so walking back from one must repeat a node.
fn same_sample_cycle(
    graph: &GraphProgram,
    indegree: &[usize],
    incoming: &[Vec<usize>],
) -> Vec<String> {
    let id = |index: usize| graph.nodes[index].id.as_str();
    let unordered = |index: &usize| indegree[*index] > 0;
    let Some(start) = (0..graph.nodes.len())
        .filter(unordered)
        .min_by_key(|&i| id(i))
    else {
        return Vec::new();
    };
    let mut walk = vec![start];
    let mut seen = BTreeMap::from([(start, 0usize)]);
    let first = loop {
        let current = *walk.last().expect("the walk starts with a node");
        let Some(previous) = incoming[current]
            .iter()
            .copied()
            .filter(unordered)
            .min_by_key(|&i| id(i))
        else {
            return vec![id(start).to_owned()];
        };
        if let Some(&position) = seen.get(&previous) {
            break position;
        }
        seen.insert(previous, walk.len());
        walk.push(previous);
    };
    // The walk went against the edges; the cycle runs the other way.
    let mut cycle: Vec<usize> = walk[first..].iter().rev().copied().collect();
    let least = (0..cycle.len())
        .min_by_key(|&i| id(cycle[i]))
        .expect("a cycle has a node");
    cycle.rotate_left(least);
    cycle.push(cycle[0]);
    cycle
        .into_iter()
        .map(|index| id(index).to_owned())
        .collect()
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
                GraphProcessor::Adsr { .. }
                    | GraphProcessor::Sine
                    | GraphProcessor::Saw
                    | GraphProcessor::Square
                    | GraphProcessor::Triangle
                    | GraphProcessor::Wavetable { .. }
                    | GraphProcessor::Lfo
                    | GraphProcessor::Pluck { .. }
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
            if !matches!(node.processor, GraphProcessor::Adsr { .. }) {
                return Err(error(
                    "E_PORT_TYPE",
                    format!("{path}.amplitude"),
                    "amplitude node must be an ADSR",
                ));
            }
        }
        GraphStage::Shared if graph.connected => {
            return Err(error(
                "E_RANGE",
                format!("{path}.connected"),
                "only a voice graph plays connected notes",
            ));
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
        GraphProcessor::Noise { seed: 0, .. } => {
            return Err(error(
                "E_RANGE",
                format!("{path}.seed"),
                "noise seed must be nonzero",
            ));
        }
        GraphProcessor::Random { seed: 0 } => {
            return Err(error(
                "E_RANGE",
                format!("{path}.seed"),
                "random seed must be nonzero",
            ));
        }
        GraphProcessor::Bow {
            players,
            channels,
            seed,
            ..
        } => {
            if !(1..=crate::bow::MAX_PLAYERS).contains(players) {
                return Err(error(
                    "E_RANGE",
                    format!("{path}.players"),
                    "bowed section needs 1 to 16 players",
                ));
            }
            if !(1..=2).contains(channels) {
                return Err(error(
                    "E_RANGE",
                    format!("{path}.channels"),
                    "bowed section needs 1 or 2 channels",
                ));
            }
            if *seed == 0 {
                return Err(error(
                    "E_RANGE",
                    format!("{path}.seed"),
                    "bowed section seed must be nonzero",
                ));
            }
        }
        GraphProcessor::Modal { modes, strings } => {
            if !(1..=crate::modal::MAX_MODES).contains(modes) {
                return Err(error(
                    "E_RANGE",
                    format!("{path}.modes"),
                    "modal bank needs 1 to 64 modes",
                ));
            }
            if !(1..=crate::modal::MAX_STRINGS).contains(strings) {
                return Err(error(
                    "E_RANGE",
                    format!("{path}.strings"),
                    "modal bank needs 1 to 3 strings",
                ));
            }
        }
        GraphProcessor::Gain { channels }
        | GraphProcessor::OnePole { channels }
        | GraphProcessor::HighPass { channels }
        | GraphProcessor::Mix { channels } => validate_channels(*channels, path)?,
        GraphProcessor::Wavetable { table } => validate_identifier(table, format!("{path}.table"))?,
        GraphProcessor::Sample {
            channels, zones, ..
        } => {
            validate_channels(*channels, format!("{path}.channels"))?;
            for (index, zone) in zones.iter().enumerate() {
                validate_identifier(&zone.sample, format!("{path}.zones[{index}].sample"))?;
            }
            // Sample channel counts are checked against the plan's resources.
            crate::sample_instrument::validate_zones(zones, *channels, &path, |_| Some(*channels))?;
        }
        GraphProcessor::Delay { channels, max, .. } => {
            validate_channels(*channels, format!("{path}.channels"))?;
            validate_rational(max, &format!("{path}.max"))?;
            if !max.is_positive() || max > &Rational::from_integer(BigInt::from(MAX_DELAY_SECONDS))
            {
                return Err(error(
                    "E_RANGE",
                    format!("{path}.max"),
                    format!("delay max must be above 0 and at most {MAX_DELAY_SECONDS} s"),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// The frames a `synth.delay/1` line holds: its max at 48 kHz, rounded up,
/// plus two for interpolation.
pub fn delay_line_frames(max: &Rational) -> Option<usize> {
    let frames = (max * Rational::from_integer(BigInt::from(48_000))).ceil();
    frames.to_integer().to_usize()?.checked_add(2)
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
        GraphUnit::Quarter => 256,
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
