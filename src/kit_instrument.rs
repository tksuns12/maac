//! Kit instruments: other instruments as pieces, selected by hit key.
//!
//! A kit instance receives hits. Each hit starts a note on the piece whose
//! key matches, at the piece's pitch with the hit's velocity, and releases it
//! once the piece's gate has elapsed. A hit on a piece in a choke group also
//! releases the open gates of the group's other pieces. The kit's output is
//! the sum of its pieces' outputs in piece order.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use num_traits::{ToPrimitive, Zero};
use serde::{Deserialize, Serialize};

use crate::dsp::{RenderError, Result};
use crate::graph::{InstrumentProgram, ParameterRate, ParameterSpec, ProgramSource};
use crate::plan::{
    err, rational_map_serde, rational_serde, validate_identifier, PlanError, Rational,
};
use crate::voice::{CompiledInstrument, InstrumentRuntime};

/// The most pieces one kit may hold.
pub const MAX_KIT_PIECES: usize = 64;
/// The most public controls one kit may expose, as for instruments.
pub const MAX_KIT_CONTROLS: usize = 64;
/// The default voice capacity of one piece.
pub const DEFAULT_PIECE_VOICES: u32 = 8;
/// The most voices one piece may declare.
pub const MAX_PIECE_VOICES: u32 = 4096;
/// The longest gate a piece may declare, in seconds.
pub const MAX_PIECE_GATE_SECONDS: i64 = 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KitProgram {
    pub id: String,
    pub channels: u8,
    pub pieces: Vec<KitPiece>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub controls: BTreeMap<String, KitControl>,
    pub source: ProgramSource,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KitPiece {
    pub id: String,
    pub key: String,
    /// The instrument program this piece plays.
    pub program: String,
    pub voices: u32,
    pub pitch_hz: f64,
    #[serde(with = "rational_serde")]
    pub gate_seconds: Rational,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choke: Option<String>,
    /// Fixed settings of the piece instrument's public controls.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "rational_map_serde"
    )]
    pub params: BTreeMap<String, Rational>,
}

/// A kit control exposes one public control of one piece.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KitControl {
    pub piece: String,
    pub control: String,
    #[serde(with = "rational_serde")]
    pub default: Rational,
}

impl KitProgram {
    pub fn piece(&self, id: &str) -> Option<&KitPiece> {
        self.pieces.iter().find(|piece| piece.id == id)
    }

    pub fn piece_for_key(&self, key: &str) -> Option<&KitPiece> {
        self.pieces.iter().find(|piece| piece.key == key)
    }

    /// The total voice capacity of every piece.
    pub fn voices(&self) -> u32 {
        self.pieces
            .iter()
            .fold(0u32, |total, piece| total.saturating_add(piece.voices))
    }

    /// A kit control takes the descriptor of the piece control it exposes.
    pub fn control_spec(
        &self,
        programs: &[InstrumentProgram],
        name: &str,
    ) -> Option<ParameterSpec> {
        let control = self.controls.get(name)?;
        let piece = self.piece(&control.piece)?;
        program_for(programs, &piece.program)?.control_spec(&control.control)
    }

    pub fn validate(&self, programs: &[InstrumentProgram]) -> std::result::Result<(), PlanError> {
        let path = format!("instruments.kits.{}", self.id);
        validate_identifier(&self.id, format!("{path}.id"))?;
        if !(1..=2).contains(&self.channels) {
            return Err(err(
                "E_RANGE",
                format!("{path}.channels"),
                "kit channels must be 1 or 2",
            ));
        }
        if self.pieces.is_empty() || self.pieces.len() > MAX_KIT_PIECES {
            return Err(err(
                "E_RANGE",
                format!("{path}.pieces"),
                format!("a kit has 1 through {MAX_KIT_PIECES} pieces"),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut keys = BTreeSet::new();
        for piece in &self.pieces {
            let piece_path = format!("{path}.pieces.{}", piece.id);
            validate_identifier(&piece.id, format!("{piece_path}.id"))?;
            if !ids.insert(piece.id.as_str()) {
                return Err(err("E_DUPLICATE_ID", piece_path, "duplicate kit piece"));
            }
            if piece.key.is_empty() || !keys.insert(piece.key.as_str()) {
                return Err(err(
                    "E_DUPLICATE_ID",
                    format!("{piece_path}.key"),
                    "kit keys must be nonempty and unique",
                ));
            }
            let program = program_for(programs, &piece.program).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    format!("{piece_path}.program"),
                    "kit piece names an unknown instrument program",
                )
            })?;
            if program.channels() != self.channels {
                return Err(err(
                    "E_PORT_TYPE",
                    format!("{piece_path}.program"),
                    "kit piece channels differ from the kit's",
                ));
            }
            if !(1..=MAX_PIECE_VOICES).contains(&piece.voices) {
                return Err(err(
                    "E_RANGE",
                    format!("{piece_path}.voices"),
                    format!("piece voices must be 1 through {MAX_PIECE_VOICES}"),
                ));
            }
            if !piece.pitch_hz.is_finite() || piece.pitch_hz <= 0.0 {
                return Err(err(
                    "E_RANGE",
                    format!("{piece_path}.pitch_hz"),
                    "piece pitch must be a finite positive frequency",
                ));
            }
            if piece.gate_seconds <= Rational::zero()
                || piece.gate_seconds > Rational::from_integer(MAX_PIECE_GATE_SECONDS.into())
            {
                return Err(err(
                    "E_RANGE",
                    format!("{piece_path}.gate_seconds"),
                    format!("piece gate must be positive and at most {MAX_PIECE_GATE_SECONDS} s"),
                ));
            }
            if piece.choke.as_deref() == Some("") {
                return Err(err(
                    "E_RANGE",
                    format!("{piece_path}.choke"),
                    "a choke group name must be nonempty",
                ));
            }
            for (name, value) in &piece.params {
                let spec = program.control_spec(name).ok_or_else(|| {
                    err(
                        "E_UNKNOWN_FIELD",
                        format!("{piece_path}.params.{name}"),
                        "kit piece sets an unknown instrument control",
                    )
                })?;
                spec.validate(value).map_err(|mut error| {
                    error.path = format!("{piece_path}.params.{name}");
                    error
                })?;
            }
        }
        if self.controls.len() > MAX_KIT_CONTROLS {
            return Err(err(
                "E_RESOURCE_LIMIT",
                format!("{path}.controls"),
                format!("a kit exposes at most {MAX_KIT_CONTROLS} controls"),
            ));
        }
        let mut targets = BTreeSet::new();
        for (name, control) in &self.controls {
            let control_path = format!("{path}.controls.{name}");
            validate_identifier(name, control_path.clone())?;
            let piece = self.piece(&control.piece).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    format!("{control_path}.piece"),
                    "kit control names an unknown piece",
                )
            })?;
            if piece.params.contains_key(&control.control) {
                return Err(err(
                    "E_CONFLICT",
                    format!("{control_path}.control"),
                    "a kit control cannot expose a piece control the piece already sets",
                ));
            }
            if !targets.insert((control.piece.as_str(), control.control.as_str())) {
                return Err(err(
                    "E_AUTOMATION_WRITER",
                    format!("{control_path}.control"),
                    "two kit controls expose the same piece control",
                ));
            }
            let spec = self.control_spec(programs, name).ok_or_else(|| {
                err(
                    "E_REFERENCE",
                    format!("{control_path}.control"),
                    "kit control names an unknown piece control",
                )
            })?;
            if spec.rate == ParameterRate::Reset {
                return Err(err(
                    "E_CAPABILITY",
                    format!("{control_path}.control"),
                    "a kit control cannot expose a reset-rate control",
                ));
            }
            spec.validate(&control.default).map_err(|mut error| {
                error.path = format!("{control_path}.default");
                error
            })?;
        }
        Ok(())
    }
}

fn program_for<'a>(programs: &'a [InstrumentProgram], id: &str) -> Option<&'a InstrumentProgram> {
    programs.iter().find(|program| program.id == id)
}

/// The gate of a piece in whole frames, rounded up so a gate never ends
/// early: `ceil(gate_seconds * rate)`.
pub fn gate_frames(gate_seconds: &Rational, rate: u32) -> u64 {
    let frames = gate_seconds * Rational::from_integer(rate.into());
    frames.ceil().to_integer().to_u64().unwrap_or(u64::MAX)
}

#[derive(Debug)]
struct PieceRuntime {
    key: String,
    runtime: InstrumentRuntime,
    pitch_hz: f64,
    gate_frames: u64,
    choke: Option<String>,
    /// Fixed control settings, overlaid by the kit controls that expose
    /// this piece.
    fixed: BTreeMap<String, f64>,
    /// Kit control name to this piece's control name.
    exposed: Vec<(String, String)>,
}

/// One kit instance: a polyphonic instrument runtime per piece.
#[derive(Debug)]
pub struct KitInstrumentRuntime {
    pieces: Vec<PieceRuntime>,
    channels: usize,
    /// Open gates: piece index, voice address, and release frame.
    open: Vec<(usize, String, u64)>,
    /// The kit controls last applied, so unchanged frames skip the pieces.
    applied: BTreeMap<String, f64>,
    output: [f64; 2],
}

impl KitInstrumentRuntime {
    pub(crate) fn new(
        kit: &KitProgram,
        programs: &BTreeMap<String, Arc<CompiledInstrument>>,
        rate: u32,
        controls: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        let mut pieces = Vec::with_capacity(kit.pieces.len());
        for piece in &kit.pieces {
            let program = programs.get(&piece.program).cloned().ok_or_else(|| {
                RenderError::RenderState(format!(
                    "kit piece program {} is not compiled",
                    piece.program
                ))
            })?;
            let mut fixed = BTreeMap::new();
            for (name, value) in &piece.params {
                fixed.insert(name.clone(), rational_f64(value)?);
            }
            let exposed = kit
                .controls
                .iter()
                .filter(|(_, control)| control.piece == piece.id)
                .map(|(name, control)| (name.clone(), control.control.clone()))
                .collect::<Vec<_>>();
            let initial = piece_controls(&fixed, &exposed, controls);
            pieces.push(PieceRuntime {
                key: piece.key.clone(),
                runtime: InstrumentRuntime::new(program, piece.voices, f64::from(rate), &initial)?,
                pitch_hz: piece.pitch_hz,
                gate_frames: gate_frames(&piece.gate_seconds, rate),
                choke: piece.choke.clone(),
                fixed,
                exposed,
            });
        }
        Ok(Self {
            pieces,
            channels: usize::from(kit.channels),
            open: Vec::new(),
            applied: controls.clone(),
            output: [0.0; 2],
        })
    }

    /// Apply the kit's public controls for the current frame. The pieces only
    /// see a change, so a frame without automation costs one comparison.
    pub(crate) fn update_controls(&mut self, controls: &BTreeMap<String, f64>) -> Result<()> {
        if &self.applied == controls {
            return Ok(());
        }
        for piece in &mut self.pieces {
            let values = piece_controls(&piece.fixed, &piece.exposed, controls);
            piece.runtime.update_controls(&values)?;
        }
        self.applied.clone_from(controls);
        Ok(())
    }

    /// Release every gate that ends at `frame`, then retire finished release
    /// tails. The engine calls this before the frame's hits.
    pub(crate) fn prune_finished(&mut self, frame: u64) -> Result<()> {
        let mut index = 0;
        while index < self.open.len() {
            if self.open[index].2 <= frame {
                let (piece, address, _) = self.open.remove(index);
                self.pieces[piece].runtime.note_off(&address, frame)?;
            } else {
                index += 1;
            }
        }
        for piece in &mut self.pieces {
            piece.runtime.prune_finished(frame)?;
        }
        Ok(())
    }

    /// Start one hit: choke the group's other pieces, then open a gate on the
    /// matching piece.
    pub(crate) fn hit(
        &mut self,
        frame: u64,
        key: &str,
        velocity: f64,
        address: &str,
    ) -> Result<()> {
        let index = self
            .pieces
            .iter()
            .position(|piece| piece.key == key)
            .ok_or_else(|| {
                RenderError::RenderState(format!("kit has no piece with key {key:?}"))
            })?;
        if let Some(group) = self.pieces[index].choke.clone() {
            let mut position = 0;
            while position < self.open.len() {
                let other = self.open[position].0;
                if other != index && self.pieces[other].choke.as_deref() == Some(group.as_str()) {
                    let (piece, voice, _) = self.open.remove(position);
                    self.pieces[piece].runtime.note_off(&voice, frame)?;
                } else {
                    position += 1;
                }
            }
        }
        let piece = &mut self.pieces[index];
        piece
            .runtime
            .note_on(address, piece.pitch_hz, velocity, frame)?;
        let release = frame.saturating_add(piece.gate_frames);
        self.open.push((index, address.to_owned(), release));
        Ok(())
    }

    /// Sum every piece's output for this frame, in piece order.
    pub(crate) fn render(&mut self, frame: u64) -> Result<&[f64]> {
        let mut sum = [0.0; 2];
        for piece in &mut self.pieces {
            let output = piece.runtime.render(frame)?;
            for (total, sample) in sum.iter_mut().zip(output) {
                *total += sample;
            }
        }
        self.output = sum;
        Ok(&self.output[..self.channels])
    }

    pub(crate) fn reset_state(&mut self) {
        self.open.clear();
        self.output = [0.0; 2];
        for piece in &mut self.pieces {
            piece.runtime.reset_state();
        }
    }

    pub fn active_voice_count(&self) -> usize {
        self.pieces
            .iter()
            .map(|piece| piece.runtime.active_voice_count())
            .sum()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.pieces.iter().map(|piece| piece.key.as_str())
    }
}

fn piece_controls(
    fixed: &BTreeMap<String, f64>,
    exposed: &[(String, String)],
    controls: &BTreeMap<String, f64>,
) -> BTreeMap<String, f64> {
    let mut values = fixed.clone();
    for (kit_control, piece_control) in exposed {
        if let Some(value) = controls.get(kit_control) {
            values.insert(piece_control.clone(), *value);
        }
    }
    values
}

fn rational_f64(value: &Rational) -> Result<f64> {
    value
        .to_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| RenderError::Nonfinite("kit piece control is not finite".into()))
}
