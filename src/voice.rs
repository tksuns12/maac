//! Compiled reusable instrument graphs and their per-instance DSP state.

use crate::dsp::{one_pole_step, pan_sample, RenderError, Result};
use crate::expression::ExpressionRuntime;
use crate::graph::{
    parameter_descriptor_for_stage, topological_order, GraphProcessor, GraphProgram, GraphStage,
    InstrumentProgram, ParameterRate, ParameterSpec, MAX_GRAPH_NODES,
};
use crate::plan::{GainExpression, PitchExpression, PressureExpression, TimbreExpression};
use crate::sample_instrument::{SampleFadeShape, SamplePlayback, SampleZone};
use crate::synth::{Adsr, DriveState, Oscillator, SvfCoefficients, SvfMode, Waveform};
use crate::wavetable::TableBank;
use num_traits::ToPrimitive;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const MAX_PARAMETER_SLOTS: usize = 5;

/// Immutable executable code shared by every instance of one instrument.
#[derive(Debug)]
pub struct CompiledInstrument {
    id: String,
    voice: CompiledGraph,
    shared: Option<CompiledGraph>,
    controls: Vec<CompiledControl>,
    control_indices: BTreeMap<String, usize>,
    channels: usize,
    pluck_nodes: usize,
}

/// Mutable polyphonic state for one instrument node.
#[derive(Debug)]
pub struct InstrumentRuntime {
    program: Arc<CompiledInstrument>,
    capacity: usize,
    rate: f64,
    controls: Vec<f64>,
    voices: Vec<VoiceState>,
    shared: Option<GraphState>,
    shared_reset: Option<GraphState>,
    output: [f64; 2],
    /// Frames rendered ahead of the caller, starting at `buffered_start`.
    buffered: Vec<[f64; 2]>,
    buffered_start: u64,
    /// The error found at the frame after the buffered ones.
    buffered_error: Option<RenderError>,
    /// The frame after the last one the caller received.
    next_frame: u64,
    /// Node outputs of the graph being rendered, one block per node.
    scratch: Vec<[f64; 2]>,
    /// Each voice's output for the block being rendered.
    voice_frames: Vec<[f64; 2]>,
    /// Per-frame pitch, timbre and pressure of the voice being rendered.
    expressions: Vec<(f64, f64, f64)>,
}

/// The most frames an instrument renders ahead at once.
const BLOCK_FRAMES: usize = 128;

#[derive(Debug)]
struct CompiledGraph {
    nodes: Vec<CompiledNode>,
    order: Vec<usize>,
    has_note_on_modulations: bool,
    has_note_off_modulations: bool,
    has_reset_modulations: bool,
    note_on_required: [bool; MAX_GRAPH_NODES],
    note_off_required: [bool; MAX_GRAPH_NODES],
    reset_required: [bool; MAX_GRAPH_NODES],
    output: usize,
    amplitude: Option<usize>,
    channels: usize,
}

#[derive(Debug)]
struct CompiledNode {
    id: String,
    processor: ProcessorCode,
    base_params: Vec<f64>,
    specs: Vec<CompiledBounds>,
    incoming: Vec<Source>,
    modulations: Vec<ModulationBinding>,
    has_note_on_modulations: bool,
    has_reset_modulations: bool,
    /// Whether any modulation changes a parameter every sample; without one
    /// the parameters change only when a control does.
    has_sample_modulations: bool,
    controls: Vec<ControlBinding>,
}

#[derive(Debug)]
enum ProcessorCode {
    Oscillator(Waveform),
    Noise(u32),
    Pluck(u32),
    Wavetable(Arc<TableBank>),
    Sample(Arc<CompiledSampleNode>),
    Adsr,
    Lfo,
    Timbre,
    Pressure,
    Velocity,
    Key,
    Gain(usize),
    OnePole(usize),
    HighPass(usize),
    Svf(usize, SvfMode),
    Drive(usize),
    Mix(usize),
    Pan,
}

/// A `synth.sample/1` node's channels, zones, bound sample data, and fade
/// curve.
#[derive(Debug)]
struct CompiledSampleNode {
    channels: usize,
    zones: Vec<SampleZone>,
    playbacks: Vec<Arc<SamplePlayback>>,
    fade_shape: SampleFadeShape,
}

/// One sounding zone of a sample voice with its fixed gain and position.
#[derive(Clone, Debug)]
struct SampleLayer {
    zone: usize,
    gain: f64,
    position: f64,
}

#[derive(Clone, Copy, Debug)]
enum Source {
    Input,
    Node(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreviewCapture {
    None,
    NoteOn,
    Reset,
}

#[derive(Debug)]
struct ModulationBinding {
    id: String,
    source: Source,
    parameter: usize,
    depth: f64,
    rate: ParameterRate,
}

#[derive(Debug)]
struct ControlBinding {
    control: usize,
    parameter: usize,
    rate: ParameterRate,
}

#[derive(Debug)]
struct CompiledControl {
    name: String,
    default: f64,
    rate: ParameterRate,
    bounds: CompiledBounds,
}

#[derive(Debug)]
struct CompiledBounds {
    min: f64,
    max: f64,
    min_open: bool,
    max_open: bool,
    context: String,
}

#[derive(Debug)]
struct VoiceState {
    address: String,
    pitch_hz: f64,
    velocity: f64,
    pitch_expression: Option<ExpressionRuntime<PitchExpression>>,
    gain_expression: Option<ExpressionRuntime<GainExpression>>,
    timbre_expression: Option<ExpressionRuntime<TimbreExpression>>,
    pressure_expression: Option<ExpressionRuntime<PressureExpression>>,
    on_frame: u64,
    released: bool,
    graph: GraphState,
}

#[derive(Clone, Debug)]
struct GraphState {
    nodes: Vec<RuntimeNode>,
    /// The controls that the parameters of nodes without sample-rate
    /// modulation were last computed from; `None` before the first sample.
    settled_controls: Option<Vec<f64>>,
}

#[derive(Clone, Debug)]
struct RuntimeNode {
    state: ProcessorState,
    params: Vec<f64>,
}

#[derive(Clone, Debug)]
enum ProcessorState {
    Oscillator(Oscillator),
    Noise(u32),
    Pluck(crate::pluck::Pluck),
    Wavetable {
        phase: f64,
    },
    /// Layers are chosen at the voice's first rendered frame from the note
    /// frequency and the note-on velocity captured here.
    Sample {
        velocity: f64,
        layers: Option<Vec<SampleLayer>>,
    },
    Adsr(Adsr),
    Lfo(Oscillator),
    OnePole([f64; 2]),
    /// Each channel's state-variable filter integrators `[s1, s2]`.
    Svf([[f64; 2]; 2]),
    /// Each channel's drive state.
    Drive([DriveState; 2]),
    /// The note-on velocity a `synth.velocity/1` node emits.
    Velocity(f64),
    Stateless,
}

impl CompiledInstrument {
    /// Validate and compile one standalone program against prebuilt tables.
    pub fn compile(
        program: &InstrumentProgram,
        tables: &BTreeMap<String, Arc<TableBank>>,
    ) -> Result<Self> {
        program.validate().map_err(RenderError::Plan)?;
        Self::compile_validated(program, tables)
    }

    /// Compile a program already covered by the enclosing plan validation.
    pub(crate) fn compile_validated(
        program: &InstrumentProgram,
        tables: &BTreeMap<String, Arc<TableBank>>,
    ) -> Result<Self> {
        Self::compile_with_samples(program, tables, &BTreeMap::new())
    }

    /// Compile a validated program against prebuilt tables and embedded samples.
    pub(crate) fn compile_with_samples(
        program: &InstrumentProgram,
        tables: &BTreeMap<String, Arc<TableBank>>,
        samples: &BTreeMap<String, Arc<SamplePlayback>>,
    ) -> Result<Self> {
        let mut voice = compile_graph(&program.voice, GraphStage::Voice, tables, samples)?;
        let mut shared = program
            .shared
            .as_ref()
            .map(|graph| compile_graph(graph, GraphStage::Shared, tables, samples))
            .transpose()?;

        let controls: Vec<CompiledControl> = program
            .controls
            .iter()
            .map(|(name, control)| {
                let spec = program.control_spec(name).ok_or_else(|| {
                    RenderError::RenderState(format!(
                        "instrument {} control {name} has no parameter descriptor",
                        program.id
                    ))
                })?;
                Ok(CompiledControl {
                    name: name.clone(),
                    default: rational_f64(&control.default, "control default")?,
                    rate: spec.rate,
                    bounds: compile_bounds(&spec, format!("control {name}"))?,
                })
            })
            .collect::<Result<_>>()?;
        let control_indices: BTreeMap<String, usize> = controls
            .iter()
            .enumerate()
            .map(|(index, control)| (control.name.clone(), index))
            .collect();

        for (name, control) in &program.controls {
            let control_index = control_indices[name];
            let graph = match control.target.graph {
                GraphStage::Voice => &mut voice,
                GraphStage::Shared => shared.as_mut().ok_or_else(|| {
                    RenderError::RenderState(format!(
                        "instrument {} control {name} targets a missing shared graph",
                        program.id
                    ))
                })?,
            };
            let node = graph
                .nodes
                .iter_mut()
                .find(|node| node.id == control.target.node)
                .ok_or_else(|| {
                    RenderError::RenderState(format!(
                        "instrument {} control {name} targets a missing node",
                        program.id
                    ))
                })?;
            let parameter =
                parameter_slot(&node.processor, &control.target.parameter).ok_or_else(|| {
                    RenderError::RenderState(format!(
                        "instrument {} control {name} targets a missing parameter",
                        program.id
                    ))
                })?;
            node.controls.push(ControlBinding {
                control: control_index,
                parameter,
                rate: controls[control_index].rate,
            });
        }

        Ok(Self {
            id: program.id.clone(),
            channels: program.channels() as usize,
            pluck_nodes: program.pluck_node_count(),
            voice,
            shared,
            controls,
            control_indices,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub(crate) fn default_controls(&self) -> BTreeMap<String, f64> {
        self.controls
            .iter()
            .map(|control| (control.name.clone(), control.default))
            .collect()
    }
}

impl InstrumentRuntime {
    pub fn new(
        program: Arc<CompiledInstrument>,
        capacity: u32,
        rate: f64,
        controls: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        validate_rate(rate)?;
        if capacity == 0 {
            return Err(RenderError::RenderState(
                "instrument voice capacity must be positive".into(),
            ));
        }
        let mut voices = Vec::new();
        if program.pluck_nodes != 0 {
            let cells = crate::graph::pluck_delay_cells(capacity as usize, program.pluck_nodes)
                .ok_or_else(|| {
                    pluck_resource_error("declared pluck storage arithmetic overflow")
                })?;
            if cells > crate::graph::MAX_PLUCK_DELAY_CELLS {
                return Err(pluck_resource_error(
                    "declared pluck delay storage exceeds the runtime limit",
                ));
            }
            voices
                .try_reserve_exact(capacity as usize)
                .map_err(|_| pluck_resource_error("cannot reserve pluck voice capacity"))?;
        }
        let resolved = resolve_controls(&program, controls)?;
        let shared = program
            .shared
            .as_ref()
            .map(|graph| GraphState::new_shared(graph, &resolved, rate))
            .transpose()?;
        Ok(Self {
            program,
            capacity: capacity as usize,
            rate,
            controls: resolved,
            voices,
            shared_reset: shared.clone(),
            shared,
            output: [0.0; 2],
            buffered: Vec::new(),
            buffered_start: 0,
            buffered_error: None,
            next_frame: 0,
            scratch: Vec::new(),
            voice_frames: Vec::new(),
            expressions: Vec::new(),
        })
    }

    /// Refuse a change at `frame` while frames from it on were rendered ahead
    /// but not yet handed out; those frames were computed without it. Frames
    /// already handed out are final, as they always were.
    fn check_nothing_ahead(&self, frame: u64) -> Result<()> {
        if self.buffered_start + self.buffered.len() as u64 > frame.max(self.next_frame) {
            return Err(RenderError::RenderState(format!(
                "instrument {} changed inside a block rendered ahead",
                self.program.id
            )));
        }
        Ok(())
    }

    fn clear_ahead(&mut self) {
        self.buffered.clear();
        self.buffered_start = 0;
        self.buffered_error = None;
        self.next_frame = 0;
    }

    /// Replace the instance's public control cache for the current frame.
    pub fn update_controls(&mut self, controls: &BTreeMap<String, f64>) -> Result<()> {
        if self.buffered_start + self.buffered.len() as u64 > self.next_frame {
            let before = self.controls.clone();
            update_resolved_controls(&self.program, controls, &mut self.controls)?;
            if before != self.controls {
                return Err(RenderError::RenderState(format!(
                    "instrument {} controls changed inside a block rendered ahead",
                    self.program.id
                )));
            }
            return Ok(());
        }
        update_resolved_controls(&self.program, controls, &mut self.controls)
    }

    pub fn note_on(
        &mut self,
        address: impl Into<String>,
        pitch_hz: f64,
        velocity: f64,
        frame: u64,
    ) -> Result<()> {
        self.note_on_with_expressions(address, pitch_hz, velocity, frame, None, None, None, None)
    }

    // Keep the public note-on contract unchanged while carrying the optional curves.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn note_on_with_expressions(
        &mut self,
        address: impl Into<String>,
        pitch_hz: f64,
        velocity: f64,
        frame: u64,
        pitch_expression: Option<ExpressionRuntime<PitchExpression>>,
        gain_expression: Option<ExpressionRuntime<GainExpression>>,
        timbre_expression: Option<ExpressionRuntime<TimbreExpression>>,
        pressure_expression: Option<ExpressionRuntime<PressureExpression>>,
    ) -> Result<()> {
        if !pitch_hz.is_finite() || !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
            return Err(RenderError::Nonfinite(
                "instrument note pitch must be finite and velocity must be within 0..=1".into(),
            ));
        }
        self.check_nothing_ahead(frame)?;
        if self.voices.len() >= self.capacity {
            return Err(RenderError::VoiceLimit {
                node: self.program.id.clone(),
                address: address.into(),
            });
        }
        let mut voice = VoiceState {
            address: address.into(),
            pitch_hz,
            velocity,
            pitch_expression,
            gain_expression,
            timbre_expression,
            pressure_expression,
            on_frame: frame,
            released: false,
            graph: GraphState::new_voice(&self.program.voice, &self.controls, frame, velocity)?,
        };
        if self.program.voice.has_note_on_modulations {
            let (pitch_hz, timbre, pressure) = voice.expression_inputs(frame, self.rate)?;
            voice.graph.capture_note_on(
                &self.program.voice,
                &self.controls,
                frame,
                self.rate,
                pitch_hz,
                timbre,
                pressure,
            )?;
        }
        let insertion = match self
            .voices
            .binary_search_by(|existing| existing.address.as_bytes().cmp(voice.address.as_bytes()))
        {
            Ok(_) => {
                return Err(RenderError::RenderState(format!(
                    "instrument voice address {} is already active",
                    voice.address
                )))
            }
            Err(position) => position,
        };
        self.voices.insert(insertion, voice);
        Ok(())
    }

    pub fn note_off(&mut self, address: &str, frame: u64) -> Result<()> {
        self.check_nothing_ahead(frame)?;
        let position = self
            .voices
            .binary_search_by(|voice| voice.address.as_bytes().cmp(address.as_bytes()))
            .map_err(|_| {
                RenderError::RenderState(format!(
                    "note-off for {address} has no allocated instrument voice"
                ))
            })?;
        let voice = &mut self.voices[position];
        if voice.released {
            return Err(RenderError::RenderState(format!(
                "note-off for {address} was delivered twice"
            )));
        }
        if self.program.voice.has_note_off_modulations {
            let (pitch_hz, timbre, pressure) = voice.expression_inputs(frame, self.rate)?;
            voice.graph.release_modulated(
                &self.program.voice,
                &self.controls,
                frame,
                self.rate,
                pitch_hz,
                timbre,
                pressure,
            )?;
        } else {
            voice
                .graph
                .release(&self.program.voice, &self.controls, frame, self.rate)?;
        }
        voice.released = true;
        Ok(())
    }

    /// Remove release tails that are zero at this sample instant.
    pub fn prune_finished(&mut self, frame: u64) -> Result<()> {
        let graph = &self.program.voice;
        let rate = self.rate;
        let mut error = None;
        self.voices.retain(|voice| {
            if !voice.released {
                return true;
            }
            match voice.graph.finished(graph, frame, rate) {
                Ok(finished) => !finished,
                Err(value) => {
                    error = Some(value);
                    true
                }
            }
        });
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Render one frame after the caller has applied note-off, pruning, and
    /// note-on in that order.
    pub fn render(&mut self, frame: u64) -> Result<&[f64]> {
        self.render_until(frame, frame + 1)
    }

    /// Render `frame`, rendering ahead up to `horizon` (exclusive). The caller
    /// promises to send no note, control or reset to this instrument at a
    /// frame below `horizon`. Every frame equals what frame-by-frame rendering
    /// gives, and an error is reported at the same frame.
    pub(crate) fn render_until(&mut self, frame: u64, horizon: u64) -> Result<&[f64]> {
        let end = self.buffered_start + self.buffered.len() as u64;
        if !(self.buffered_start..end).contains(&frame) {
            if frame == end {
                if let Some(error) = self.buffered_error.take() {
                    return Err(error);
                }
            }
            let frames = horizon.saturating_sub(frame).clamp(1, BLOCK_FRAMES as u64) as usize;
            self.fill(frame, frames);
            if self.buffered.is_empty() {
                return Err(self.buffered_error.take().unwrap_or_else(|| {
                    RenderError::RenderState("instrument block rendered no frame".into())
                }));
            }
        }
        self.output = self.buffered[(frame - self.buffered_start) as usize];
        self.next_frame = frame + 1;
        Ok(&self.output[..self.program.channels])
    }

    /// Render `frames` frames from `start` into the buffer. Each voice graph
    /// runs node by node over the block; the voices and the shared graph are
    /// then combined frame by frame in the same order as [`Self::render`]
    /// combined them. A voice retires at the frame where pruning would have
    /// removed it.
    fn fill(&mut self, start: u64, frames: usize) {
        let program = Arc::clone(&self.program);
        let graph = &program.voice;
        let rate = self.rate;
        let stride = BLOCK_FRAMES;
        let scratch_len = stride
            * program.voice.nodes.len().max(
                program
                    .shared
                    .as_ref()
                    .map_or(0, |shared| shared.nodes.len()),
            );
        if self.scratch.len() < scratch_len {
            self.scratch.resize(scratch_len, [0.0; 2]);
        }
        if self.voice_frames.len() < self.voices.len() * stride {
            self.voice_frames
                .resize(self.voices.len() * stride, [0.0; 2]);
        }
        if self.expressions.len() < stride {
            self.expressions.resize(stride, (0.0, 0.0, 0.0));
        }

        // Each voice: frames before its retirement, and its first error.
        let mut active = Vec::with_capacity(self.voices.len());
        let mut voice_errors: Vec<Option<(usize, RenderError)>> =
            Vec::with_capacity(self.voices.len());
        for (index, voice) in self.voices.iter_mut().enumerate() {
            let mut length = frames;
            let mut error = None;
            if voice.released {
                for offset in 1..frames {
                    match voice.graph.finished(graph, start + offset as u64, rate) {
                        Ok(true) => {
                            length = offset;
                            break;
                        }
                        Ok(false) => {}
                        Err(failure) => {
                            length = offset;
                            error = Some((offset, failure));
                            break;
                        }
                    }
                }
            }
            active.push(length);
            let mut rendered = length;
            for offset in 0..length {
                match voice.expression_inputs(start + offset as u64, rate) {
                    Ok(inputs) => self.expressions[offset] = inputs,
                    Err(failure) => {
                        rendered = offset;
                        error = Some((offset, failure));
                        break;
                    }
                }
            }
            let (graph_frames, graph_error) = voice.graph.render_block(
                graph,
                &self.controls,
                start,
                rendered,
                rate,
                &self.expressions[..rendered],
                None,
                &mut self.scratch,
                stride,
            );
            if let Some(failure) = graph_error {
                error = Some((graph_frames, failure));
            }
            let output = graph.output * stride;
            self.voice_frames[index * stride..index * stride + graph_frames]
                .copy_from_slice(&self.scratch[output..output + graph_frames]);
            voice_errors.push(error);
        }

        // Combine the voices frame by frame, in voice order.
        let voice_channels = graph.channels;
        let mut sums = vec![[0.0; 2]; frames];
        let mut limit = frames;
        let mut error = None;
        'frames: for (offset, sum) in sums.iter_mut().enumerate() {
            let frame = start + offset as u64;
            let mut voice_sum = [0.0; 2];
            for (index, voice) in self.voices.iter().enumerate() {
                if offset >= active[index] {
                    if let Some((at, _)) = &voice_errors[index] {
                        if *at == offset {
                            limit = offset;
                            error = voice_errors[index].take().map(|(_, failure)| failure);
                            break 'frames;
                        }
                    }
                    continue;
                }
                if let Some((at, _)) = &voice_errors[index] {
                    if *at == offset {
                        limit = offset;
                        error = voice_errors[index].take().map(|(_, failure)| failure);
                        break 'frames;
                    }
                }
                let sample = self.voice_frames[index * stride + offset];
                let amplitude = match voice.graph.amplitude(graph, frame, rate) {
                    Ok(amplitude) => amplitude,
                    Err(failure) => {
                        limit = offset;
                        error = Some(failure);
                        break 'frames;
                    }
                };
                let gain = voice.gain_expression.as_ref().map(|expression| {
                    expression
                        .curve
                        .gain_at(&expression.coordinate_at(voice.on_frame, frame))
                });
                if gain.is_some_and(|value| !value.is_finite() || value < 0.0) {
                    limit = offset;
                    error = Some(RenderError::Nonfinite(format!(
                        "gain expression at {} produced an invalid value",
                        voice.address
                    )));
                    break 'frames;
                }
                for channel in 0..voice_channels {
                    let contribution = sample[channel] * amplitude * voice.velocity;
                    voice_sum[channel] += match gain {
                        Some(gain) => contribution * gain,
                        None => contribution,
                    };
                    if !voice_sum[channel].is_finite() {
                        limit = offset;
                        error = Some(RenderError::Nonfinite(format!(
                            "instrument {} voice sum is nonfinite",
                            program.id
                        )));
                        break 'frames;
                    }
                }
            }
            *sum = voice_sum;
        }

        // The shared graph runs over the frames every voice completed.
        self.buffered.clear();
        self.buffered_start = start;
        if let (Some(compiled), Some(shared)) = (program.shared.as_ref(), self.shared.as_mut()) {
            for expression in &mut self.expressions[..limit] {
                *expression = (0.0, 0.0, 0.0);
            }
            let (shared_frames, shared_error) = shared.render_block(
                compiled,
                &self.controls,
                start,
                limit,
                rate,
                &self.expressions[..limit],
                Some(&sums[..limit]),
                &mut self.scratch,
                stride,
            );
            if shared_error.is_some() {
                limit = shared_frames;
                error = shared_error;
            }
            let output = compiled.output * stride;
            self.buffered
                .extend_from_slice(&self.scratch[output..output + limit]);
        } else {
            self.buffered.extend_from_slice(&sums[..limit]);
        }
        self.buffered_error = error;

        // Pruning would have removed these voices at their retirement frames.
        if self.buffered_error.is_none() {
            let mut index = 0;
            self.voices.retain(|_| {
                let keep = active[index] == frames;
                index += 1;
                keep
            });
        }
    }

    pub fn reset(&mut self, controls: &BTreeMap<String, f64>) -> Result<()> {
        let resolved = resolve_controls(&self.program, controls)?;
        let shared = self
            .program
            .shared
            .as_ref()
            .map(|graph| GraphState::new_shared(graph, &resolved, self.rate))
            .transpose()?;
        let shared_reset = shared.clone();

        self.controls = resolved;
        self.voices.clear();
        self.output = [0.0; 2];
        self.shared = shared;
        self.shared_reset = shared_reset;
        self.clear_ahead();
        Ok(())
    }

    pub(crate) fn reset_state(&mut self) {
        self.voices.clear();
        self.output = [0.0; 2];
        self.shared.clone_from(&self.shared_reset);
        self.clear_ahead();
    }

    pub fn active_voice_count(&self) -> usize {
        self.voices.len()
    }
}

impl VoiceState {
    fn expression_inputs(&self, frame: u64, rate: f64) -> Result<(f64, f64, f64)> {
        let pitch_hz = if let Some(expression) = &self.pitch_expression {
            let cents = expression
                .curve
                .cents_at(&expression.coordinate_at(self.on_frame, frame))
                .to_f64()
                .ok_or_else(|| {
                    RenderError::Nonfinite("pitch expression cents cannot be represented".into())
                })?;
            let frequency = self.pitch_hz * 2.0_f64.powf(cents / 1200.0);
            if !frequency.is_finite() || frequency <= 0.0 || frequency >= rate / 2.0 {
                return Err(RenderError::RenderState(format!(
                    "pitch expression at {} is outside (0, Nyquist)",
                    self.address
                )));
            }
            frequency
        } else {
            self.pitch_hz
        };
        let timbre = self.timbre_expression.as_ref().map_or(0.0, |expression| {
            expression
                .curve
                .value_at(&expression.coordinate_at(self.on_frame, frame))
        });
        if !timbre.is_finite() || !(0.0..=1.0).contains(&timbre) {
            return Err(RenderError::Nonfinite(format!(
                "timbre expression at {} produced an invalid value",
                self.address
            )));
        }
        let pressure = self.pressure_expression.as_ref().map_or(0.0, |expression| {
            expression
                .curve
                .value_at(&expression.coordinate_at(self.on_frame, frame))
        });
        if !pressure.is_finite() || !(0.0..=1.0).contains(&pressure) {
            return Err(RenderError::Nonfinite(format!(
                "pressure expression at {} produced an invalid value",
                self.address
            )));
        }
        Ok((pitch_hz, timbre, pressure))
    }
}

impl GraphState {
    fn new_voice(
        graph: &CompiledGraph,
        controls: &[f64],
        frame: u64,
        velocity: f64,
    ) -> Result<Self> {
        Self::new(graph, controls, Some(frame), velocity)
    }

    fn new_shared(graph: &CompiledGraph, controls: &[f64], rate: f64) -> Result<Self> {
        // Shared graphs contain no velocity-sensitive (voice-only) nodes.
        let mut state = Self::new(graph, controls, None, 0.0)?;
        if graph.has_reset_modulations {
            state.capture_reset(graph, controls, rate)?;
        }
        Ok(state)
    }

    fn new(
        graph: &CompiledGraph,
        controls: &[f64],
        on_frame: Option<u64>,
        velocity: f64,
    ) -> Result<Self> {
        let mut nodes = Vec::with_capacity(graph.nodes.len());
        for node in &graph.nodes {
            let mut params = node.base_params.clone();
            apply_controls(node, controls, ParameterRate::NoteOn, &mut params);
            apply_controls(node, controls, ParameterRate::Reset, &mut params);
            let state = match &node.processor {
                ProcessorCode::Oscillator(waveform) => {
                    ProcessorState::Oscillator(Oscillator::new(*waveform, params[2])?)
                }
                ProcessorCode::Noise(seed) => ProcessorState::Noise(*seed),
                ProcessorCode::Pluck(seed) => {
                    ProcessorState::Pluck(crate::pluck::Pluck::new(*seed)?)
                }
                ProcessorCode::Wavetable(_) => ProcessorState::Wavetable { phase: params[2] },
                ProcessorCode::Sample(_) => ProcessorState::Sample {
                    velocity,
                    layers: None,
                },
                ProcessorCode::Adsr => ProcessorState::Adsr(Adsr::new_curved(
                    on_frame.unwrap_or(0),
                    params[0],
                    params[1],
                    params[2],
                    params[4],
                )?),
                ProcessorCode::Lfo => {
                    ProcessorState::Lfo(Oscillator::new(Waveform::Sine, params[1])?)
                }
                ProcessorCode::OnePole(_) | ProcessorCode::HighPass(_) => {
                    ProcessorState::OnePole([0.0; 2])
                }
                ProcessorCode::Svf(..) => ProcessorState::Svf([[0.0; 2]; 2]),
                ProcessorCode::Drive(_) => ProcessorState::Drive([DriveState::default(); 2]),
                ProcessorCode::Velocity => ProcessorState::Velocity(velocity),
                ProcessorCode::Gain(_)
                | ProcessorCode::Key
                | ProcessorCode::Mix(_)
                | ProcessorCode::Pan
                | ProcessorCode::Timbre
                | ProcessorCode::Pressure => ProcessorState::Stateless,
            };
            nodes.push(RuntimeNode { state, params });
        }
        Ok(Self {
            nodes,
            settled_controls: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_note_on(
        &mut self,
        graph: &CompiledGraph,
        controls: &[f64],
        frame: u64,
        rate: f64,
        pitch_hz: f64,
        timbre: f64,
        pressure: f64,
    ) -> Result<()> {
        self.preview_required(
            graph,
            controls,
            frame,
            rate,
            pitch_hz,
            timbre,
            pressure,
            &graph.note_on_required,
            PreviewCapture::NoteOn,
        )?;
        Ok(())
    }

    fn capture_reset(&mut self, graph: &CompiledGraph, controls: &[f64], rate: f64) -> Result<()> {
        self.preview_required(
            graph,
            controls,
            0,
            rate,
            0.0,
            0.0,
            0.0,
            &graph.reset_required,
            PreviewCapture::Reset,
        )?;
        Ok(())
    }

    fn release(
        &mut self,
        graph: &CompiledGraph,
        controls: &[f64],
        frame: u64,
        rate: f64,
    ) -> Result<()> {
        for (compiled, runtime) in graph.nodes.iter().zip(&mut self.nodes) {
            if let ProcessorState::Adsr(envelope) = &mut runtime.state {
                let mut release = compiled.base_params[3];
                for binding in &compiled.controls {
                    if binding.rate == ParameterRate::NoteOff && binding.parameter == 3 {
                        release = controls[binding.control];
                    }
                }
                envelope.release(frame, rate, release)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn release_modulated(
        &mut self,
        graph: &CompiledGraph,
        controls: &[f64],
        frame: u64,
        rate: f64,
        pitch_hz: f64,
        timbre: f64,
        pressure: f64,
    ) -> Result<()> {
        let outputs = self.preview_required(
            graph,
            controls,
            frame,
            rate,
            pitch_hz,
            timbre,
            pressure,
            &graph.note_off_required,
            PreviewCapture::None,
        )?;

        // Validate every release against the same pre-release graph state.
        // Only after all reductions succeed may any envelope be mutated.
        let mut releases = [0.0; MAX_GRAPH_NODES];
        let mut has_release = [false; MAX_GRAPH_NODES];
        for (index, compiled) in graph.nodes.iter().enumerate() {
            if !matches!(compiled.processor, ProcessorCode::Adsr) {
                continue;
            }
            let mut params = parameter_scratch(compiled);
            apply_controls(
                compiled,
                controls,
                ParameterRate::NoteOff,
                &mut params[..compiled.base_params.len()],
            );
            apply_event_modulations(
                compiled,
                ParameterRate::NoteOff,
                &outputs,
                [0.0; 2],
                &mut params,
            )?;
            validate_event_parameter(compiled, 3, params[3])?;
            releases[index] = params[3];
            has_release[index] = true;
        }
        for index in 0..graph.nodes.len() {
            if !has_release[index] {
                continue;
            }
            match &mut self.nodes[index].state {
                ProcessorState::Adsr(envelope) => {
                    envelope.release(frame, rate, releases[index])?;
                }
                _ => {
                    return Err(RenderError::RenderState(format!(
                        "instrument graph node {} has mismatched compiled state",
                        graph.nodes[index].id
                    )))
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn preview_required(
        &mut self,
        graph: &CompiledGraph,
        controls: &[f64],
        frame: u64,
        rate: f64,
        pitch_hz: f64,
        timbre: f64,
        pressure: f64,
        required: &[bool; MAX_GRAPH_NODES],
        capture: PreviewCapture,
    ) -> Result<[[f64; 2]; MAX_GRAPH_NODES]> {
        let mut outputs = [[0.0; 2]; MAX_GRAPH_NODES];
        for &node_index in &graph.order {
            if !required[node_index] {
                continue;
            }
            let compiled = &graph.nodes[node_index];
            if capture == PreviewCapture::NoteOn
                && (matches!(compiled.processor, ProcessorCode::Adsr)
                    || compiled.has_note_on_modulations)
            {
                let mut event_params = parameter_scratch(compiled);
                apply_controls(
                    compiled,
                    controls,
                    ParameterRate::NoteOn,
                    &mut event_params[..compiled.base_params.len()],
                );
                apply_event_modulations(
                    compiled,
                    ParameterRate::NoteOn,
                    &outputs,
                    [0.0; 2],
                    &mut event_params,
                )?;
                match (&compiled.processor, &mut self.nodes[node_index].state) {
                    (ProcessorCode::Adsr, state) => {
                        for parameter in [0, 1, 2, 4] {
                            validate_event_parameter(compiled, parameter, event_params[parameter])?;
                        }
                        *state = ProcessorState::Adsr(Adsr::new_curved(
                            frame,
                            event_params[0],
                            event_params[1],
                            event_params[2],
                            event_params[4],
                        )?);
                    }
                    (ProcessorCode::Oscillator(_), ProcessorState::Oscillator(oscillator)) => {
                        validate_event_parameter(compiled, 2, event_params[2])?;
                        oscillator.reset(event_params[2])?;
                    }
                    (ProcessorCode::Wavetable(_), ProcessorState::Wavetable { phase }) => {
                        validate_event_parameter(compiled, 2, event_params[2])?;
                        *phase = canonical_event_phase(event_params[2]);
                    }
                    (ProcessorCode::Lfo, ProcessorState::Lfo(oscillator)) => {
                        validate_event_parameter(compiled, 1, event_params[1])?;
                        oscillator.reset(event_params[1])?;
                    }
                    _ => {
                        return Err(RenderError::RenderState(format!(
                            "instrument graph node {} has mismatched note-on state",
                            compiled.id
                        )))
                    }
                }
            } else if capture == PreviewCapture::Reset && compiled.has_reset_modulations {
                let mut event_params = parameter_scratch(compiled);
                apply_controls(
                    compiled,
                    controls,
                    ParameterRate::Reset,
                    &mut event_params[..compiled.base_params.len()],
                );
                apply_event_modulations(
                    compiled,
                    ParameterRate::Reset,
                    &outputs,
                    [0.0; 2],
                    &mut event_params,
                )?;
                match (&compiled.processor, &mut self.nodes[node_index].state) {
                    (ProcessorCode::Lfo, ProcessorState::Lfo(oscillator)) => {
                        validate_event_parameter(compiled, 1, event_params[1])?;
                        oscillator.reset(event_params[1])?;
                    }
                    _ => {
                        return Err(RenderError::RenderState(format!(
                            "instrument graph node {} has mismatched reset state",
                            compiled.id
                        )))
                    }
                }
            }

            let mut params = parameter_scratch(compiled);
            apply_controls(
                compiled,
                controls,
                ParameterRate::Sample,
                &mut params[..compiled.base_params.len()],
            );
            apply_sample_modulations(compiled, &outputs, [0.0; 2], &mut params)?;
            validate_params(compiled, &params[..compiled.base_params.len()])?;
            let audio_input = sum_scratch_inputs(&outputs, &compiled.incoming, [0.0; 2])?;
            outputs[node_index] = process_graph_node(
                compiled,
                &mut self.nodes[node_index].state,
                &params[..compiled.base_params.len()],
                frame,
                rate,
                pitch_hz,
                timbre,
                pressure,
                audio_input,
                false,
            )?;
        }
        Ok(outputs)
    }

    fn finished(&self, graph: &CompiledGraph, frame: u64, rate: f64) -> Result<bool> {
        let amplitude = graph.amplitude.ok_or_else(|| {
            RenderError::RenderState("voice graph has no amplitude envelope".into())
        })?;
        match &self.nodes[amplitude].state {
            ProcessorState::Adsr(envelope) => envelope.finished(frame, rate),
            _ => Err(RenderError::RenderState(
                "voice amplitude state is not an ADSR".into(),
            )),
        }
    }

    fn amplitude(&self, graph: &CompiledGraph, frame: u64, rate: f64) -> Result<f64> {
        let amplitude = graph.amplitude.ok_or_else(|| {
            RenderError::RenderState("voice graph has no amplitude envelope".into())
        })?;
        match &self.nodes[amplitude].state {
            ProcessorState::Adsr(envelope) => envelope.value(frame, rate),
            _ => Err(RenderError::RenderState(
                "voice amplitude state is not an ADSR".into(),
            )),
        }
    }

    /// Render frames `start .. start + frames` node by node, writing node
    /// `n`'s output for frame offset `i` to `scratch[n * stride + i]`. Each
    /// node does at each frame exactly what frame-by-frame evaluation does.
    /// When a node fails at some frame, later nodes stop before that frame, so
    /// the error is the one frame-by-frame evaluation would meet first.
    /// Returns the frames completed and the error at the next frame, if any.
    #[allow(clippy::too_many_arguments)]
    fn render_block(
        &mut self,
        graph: &CompiledGraph,
        controls: &[f64],
        start: u64,
        frames: usize,
        rate: f64,
        expressions: &[(f64, f64, f64)],
        input: Option<&[[f64; 2]]>,
        scratch: &mut [[f64; 2]],
        stride: usize,
    ) -> (usize, Option<RenderError>) {
        // A node without sample-rate modulation computes the same parameters
        // until a control changes, so it keeps the values it already checked.
        let settled = self.settled_controls.as_deref() == Some(controls);
        let input_at = |offset: usize| input.map_or([0.0; 2], |input| input[offset]);
        let mut limit = frames;
        let mut error = None;
        for &node_index in &graph.order {
            let compiled = &graph.nodes[node_index];
            let per_sample = compiled.has_sample_modulations;
            if !settled && !per_sample && limit > 0 {
                let params = &mut self.nodes[node_index].params;
                params.clone_from_slice(&compiled.base_params);
                apply_controls(compiled, controls, ParameterRate::Sample, params);
                if let Err(failure) = validate_params(compiled, params) {
                    limit = 0;
                    error = Some(failure);
                    continue;
                }
            }
            let mut stopped = None;
            for offset in 0..limit {
                if per_sample {
                    let params = &mut self.nodes[node_index].params;
                    params.clone_from_slice(&compiled.base_params);
                    apply_controls(compiled, controls, ParameterRate::Sample, params);
                    let mut failure = None;
                    for modulation in &compiled.modulations {
                        if modulation.rate != ParameterRate::Sample {
                            continue;
                        }
                        let source = match modulation.source {
                            Source::Input => input_at(offset)[0],
                            Source::Node(index) => scratch[index * stride + offset][0],
                        };
                        let product = source * modulation.depth;
                        if !product.is_finite() {
                            failure = Some(RenderError::Nonfinite(format!(
                                "modulation {} product is nonfinite",
                                modulation.id
                            )));
                            break;
                        }
                        let sum = params[modulation.parameter] + product;
                        if !sum.is_finite() {
                            failure = Some(RenderError::Nonfinite(format!(
                                "modulation {} sum is nonfinite",
                                modulation.id
                            )));
                            break;
                        }
                        params[modulation.parameter] = sum;
                    }
                    if let Some(failure) =
                        failure.or_else(|| validate_params(compiled, params).err())
                    {
                        stopped = Some((offset, failure));
                        break;
                    }
                }
                let mut audio_input = [0.0; 2];
                let mut failure = None;
                'sum: for source in &compiled.incoming {
                    let source = match source {
                        Source::Input => input_at(offset),
                        Source::Node(index) => scratch[index * stride + offset],
                    };
                    for channel in 0..2 {
                        audio_input[channel] += source[channel];
                        if !audio_input[channel].is_finite() {
                            failure = Some(RenderError::Nonfinite(
                                "instrument graph input sum is nonfinite".into(),
                            ));
                            break 'sum;
                        }
                    }
                }
                if let Some(failure) = failure {
                    stopped = Some((offset, failure));
                    break;
                }
                let (pitch_hz, timbre, pressure) = expressions[offset];
                let runtime = &mut self.nodes[node_index];
                match process_graph_node(
                    compiled,
                    &mut runtime.state,
                    &runtime.params,
                    start + offset as u64,
                    rate,
                    pitch_hz,
                    timbre,
                    pressure,
                    audio_input,
                    true,
                ) {
                    Ok(output) => scratch[node_index * stride + offset] = output,
                    Err(failure) => {
                        stopped = Some((offset, failure));
                        break;
                    }
                }
            }
            if let Some((offset, failure)) = stopped {
                limit = offset;
                error = Some(failure);
            }
        }
        if !settled && error.is_none() && frames > 0 {
            match &mut self.settled_controls {
                Some(previous) => previous.clone_from_slice(controls),
                None => self.settled_controls = Some(controls.to_vec()),
            }
        }
        (limit, error)
    }
}

fn capture_dependency_masks(
    nodes: &[CompiledNode],
    order: &[usize],
) -> (
    [bool; MAX_GRAPH_NODES],
    [bool; MAX_GRAPH_NODES],
    [bool; MAX_GRAPH_NODES],
) {
    let mut note_on = [false; MAX_GRAPH_NODES];
    let mut note_off = [false; MAX_GRAPH_NODES];
    let mut reset = [false; MAX_GRAPH_NODES];
    for (index, node) in nodes.iter().enumerate() {
        if matches!(node.processor, ProcessorCode::Adsr) {
            note_on[index] = true;
            for binding in &node.modulations {
                if binding.rate == ParameterRate::NoteOff {
                    mark_source(&mut note_off, binding.source);
                }
            }
        }
        if node.has_note_on_modulations {
            note_on[index] = true;
        }
        if node.has_reset_modulations {
            reset[index] = true;
        }
    }
    expand_capture_dependencies(nodes, order, &mut note_on, Some(ParameterRate::NoteOn));
    expand_capture_dependencies(nodes, order, &mut note_off, None);
    expand_capture_dependencies(nodes, order, &mut reset, Some(ParameterRate::Reset));
    (note_on, note_off, reset)
}

fn expand_capture_dependencies(
    nodes: &[CompiledNode],
    order: &[usize],
    required: &mut [bool; MAX_GRAPH_NODES],
    capture_rate: Option<ParameterRate>,
) {
    for &index in order.iter().rev() {
        if !required[index] {
            continue;
        }
        let node = &nodes[index];
        for &source in &node.incoming {
            mark_source(required, source);
        }
        for binding in &node.modulations {
            if binding.rate == ParameterRate::Sample
                || capture_rate.is_some_and(|rate| binding.rate == rate)
            {
                mark_source(required, binding.source);
            }
        }
    }
}

fn mark_source(required: &mut [bool; MAX_GRAPH_NODES], source: Source) {
    if let Source::Node(index) = source {
        required[index] = true;
    }
}

fn parameter_scratch(node: &CompiledNode) -> [f64; MAX_PARAMETER_SLOTS] {
    let mut params = [0.0; MAX_PARAMETER_SLOTS];
    params[..node.base_params.len()].copy_from_slice(&node.base_params);
    params
}

fn canonical_event_phase(phase: f64) -> f64 {
    if phase == 1.0 {
        0.0
    } else {
        phase
    }
}

fn apply_sample_modulations(
    node: &CompiledNode,
    outputs: &[[f64; 2]; MAX_GRAPH_NODES],
    input: [f64; 2],
    params: &mut [f64; MAX_PARAMETER_SLOTS],
) -> Result<()> {
    for binding in &node.modulations {
        if binding.rate != ParameterRate::Sample {
            continue;
        }
        apply_modulation(binding, outputs, input, params)?;
    }
    Ok(())
}

fn apply_event_modulations(
    node: &CompiledNode,
    rate: ParameterRate,
    outputs: &[[f64; 2]; MAX_GRAPH_NODES],
    input: [f64; 2],
    params: &mut [f64; MAX_PARAMETER_SLOTS],
) -> Result<()> {
    for binding in &node.modulations {
        if binding.rate != rate {
            continue;
        }
        apply_modulation(binding, outputs, input, params)?;
    }
    Ok(())
}

fn apply_modulation(
    binding: &ModulationBinding,
    outputs: &[[f64; 2]; MAX_GRAPH_NODES],
    input: [f64; 2],
    params: &mut [f64; MAX_PARAMETER_SLOTS],
) -> Result<()> {
    let source = scratch_source_sample(outputs, binding.source, input)[0];
    let product = source * binding.depth;
    if !product.is_finite() {
        return Err(RenderError::Nonfinite(format!(
            "modulation {} product is nonfinite",
            binding.id
        )));
    }
    let sum = params[binding.parameter] + product;
    if !sum.is_finite() {
        return Err(RenderError::Nonfinite(format!(
            "modulation {} sum is nonfinite",
            binding.id
        )));
    }
    params[binding.parameter] = sum;
    Ok(())
}

fn validate_event_parameter(node: &CompiledNode, parameter: usize, value: f64) -> Result<()> {
    validate_value(value, &node.specs[parameter]).map_err(|_| {
        crate::plan::err(
            "E_RANGE",
            "instrument.modulations.target",
            "combined event-rate parameter is outside its declared range",
        )
        .into()
    })
}

fn scratch_source_sample(
    outputs: &[[f64; 2]; MAX_GRAPH_NODES],
    source: Source,
    input: [f64; 2],
) -> [f64; 2] {
    match source {
        Source::Input => input,
        Source::Node(index) => outputs[index],
    }
}

fn sum_scratch_inputs(
    outputs: &[[f64; 2]; MAX_GRAPH_NODES],
    incoming: &[Source],
    input: [f64; 2],
) -> Result<[f64; 2]> {
    let mut sum = [0.0; 2];
    for &source in incoming {
        let source = scratch_source_sample(outputs, source, input);
        for channel in 0..2 {
            sum[channel] += source[channel];
            if !sum[channel].is_finite() {
                return Err(RenderError::Nonfinite(
                    "instrument graph input sum is nonfinite".into(),
                ));
            }
        }
    }
    Ok(sum)
}

#[allow(clippy::too_many_arguments)]
fn process_graph_node(
    compiled: &CompiledNode,
    state: &mut ProcessorState,
    params: &[f64],
    frame: u64,
    rate: f64,
    pitch_hz: f64,
    timbre: f64,
    pressure: f64,
    audio_input: [f64; 2],
    commit: bool,
) -> Result<[f64; 2]> {
    let mut output = [0.0; 2];
    match (&compiled.processor, state) {
        (ProcessorCode::Noise(_), ProcessorState::Noise(state)) => {
            // Version 1 fixes xorshift32's width, shifts and advance-before-output order.
            let mut next = *state;
            next ^= next << 13;
            next ^= next >> 17;
            next ^= next << 5;
            output[0] = (next as f64 / 2147483648.0 - 1.0) * params[0];
            if commit {
                *state = next;
            }
        }
        (ProcessorCode::Pluck(_), ProcessorState::Pluck(pluck)) => {
            output[0] = if commit {
                pluck.sample(pitch_hz * params[0], params[1], params[2], params[3])?
            } else {
                pluck.preview(pitch_hz * params[0], params[1], params[2], params[3])?
            };
        }
        (ProcessorCode::Oscillator(_), ProcessorState::Oscillator(oscillator)) => {
            let frequency = pitch_hz * params[0] + params[1];
            validate_frequency(frequency)?;
            if commit {
                output[0] = oscillator.sample(frequency, rate)? * params[3];
            } else {
                let mut preview = oscillator.clone();
                output[0] = preview.sample(frequency, rate)? * params[3];
            }
        }
        (ProcessorCode::Wavetable(table), ProcessorState::Wavetable { phase }) => {
            let frequency = pitch_hz * params[0] + params[1];
            validate_frequency(frequency)?;
            output[0] = table.sample(*phase, params[4], frequency, rate)? * params[3];
            if commit {
                *phase = (*phase + frequency / rate).rem_euclid(1.0);
            }
        }
        (ProcessorCode::Sample(node), ProcessorState::Sample { velocity, layers }) => {
            let frequency = pitch_hz * params[0] + params[1];
            validate_frequency(frequency)?;
            if frequency <= 0.0 {
                return Err(RenderError::Nonfinite(format!(
                    "sample node {} playback frequency {frequency} Hz must be positive",
                    compiled.id
                )));
            }
            let mut chosen = None;
            let current = match layers {
                Some(current) => current,
                None => {
                    let gains = crate::sample_instrument::zone_gains(
                        &node.zones,
                        pitch_hz,
                        *velocity,
                        node.fade_shape,
                    );
                    if gains.is_empty() {
                        return Err(RenderError::Plan(crate::plan::err(
                            "E_RANGE",
                            "instruments.sample",
                            format!(
                                "note at {pitch_hz} Hz and velocity {velocity} is outside every zone of sample node {}",
                                compiled.id
                            ),
                        )));
                    }
                    chosen.insert(
                        gains
                            .into_iter()
                            .map(|(zone, gain)| SampleLayer {
                                zone,
                                gain,
                                position: 0.0,
                            })
                            .collect::<Vec<_>>(),
                    )
                }
            };
            // Every channel reads the same position, so stereo images stay
            // aligned through bends and loops.
            let mut sums = [0.0; 2];
            for layer in current.iter_mut() {
                let playback = &node.playbacks[layer.zone];
                for (channel, sum) in sums.iter_mut().enumerate().take(node.channels) {
                    *sum += layer.gain
                        * playback
                            .value(layer.position, channel)
                            .map_err(RenderError::Plan)?;
                }
                if commit {
                    let recorded = playback.sample();
                    let step = frequency / recorded.root_hz() * f64::from(recorded.rate_hz) / rate;
                    layer.position = playback.advance(layer.position, step);
                }
            }
            for (output, sum) in output.iter_mut().zip(sums).take(node.channels) {
                *output = sum * params[2];
            }
            if commit {
                if let Some(chosen) = chosen {
                    *layers = Some(chosen);
                }
            }
        }
        (ProcessorCode::Adsr, ProcessorState::Adsr(envelope)) => {
            output[0] = envelope.value(frame, rate)?;
        }
        (ProcessorCode::Lfo, ProcessorState::Lfo(oscillator)) => {
            if commit {
                output[0] = oscillator.sample(params[0], rate)? * params[2];
            } else {
                let mut preview = oscillator.clone();
                output[0] = preview.sample(params[0], rate)? * params[2];
            }
        }
        (ProcessorCode::Timbre, ProcessorState::Stateless) => output[0] = timbre,
        (ProcessorCode::Pressure, ProcessorState::Stateless) => output[0] = pressure,
        (ProcessorCode::Velocity, ProcessorState::Velocity(velocity)) => output[0] = *velocity,
        // Octaves from C4 = 440 Hz * 2^(-9/12).
        (ProcessorCode::Key, ProcessorState::Stateless) => {
            output[0] = (pitch_hz / 440.0).log2() + 0.75;
        }
        (ProcessorCode::Gain(channels), ProcessorState::Stateless) => {
            for (output, input) in output.iter_mut().zip(audio_input).take(*channels) {
                *output = input * params[0];
            }
        }
        (
            ProcessorCode::OnePole(channels) | ProcessorCode::HighPass(channels),
            ProcessorState::OnePole(previous),
        ) => {
            let mut next = *previous;
            for ((output, previous), input) in output
                .iter_mut()
                .zip(next.iter_mut())
                .zip(audio_input)
                .take(*channels)
            {
                let lowpass = one_pole_step(input, previous, params[0], rate)?;
                *output = if matches!(compiled.processor, ProcessorCode::HighPass(_)) {
                    input - lowpass
                } else {
                    lowpass
                };
            }
            if commit {
                *previous = next;
            }
        }
        (ProcessorCode::Svf(channels, mode), ProcessorState::Svf(states)) => {
            // A shared graph renders with zero pitch, and its ratio is zero.
            let cutoff = pitch_hz * params[1] + params[0];
            let coefficients = SvfCoefficients::new(cutoff, params[2], rate).map_err(|_| {
                RenderError::Nonfinite(format!(
                    "state-variable filter {} cutoff {cutoff} Hz must be strictly within 0..{} Hz",
                    compiled.id,
                    rate / 2.0
                ))
            })?;
            let mut next = *states;
            for ((output, state), input) in output
                .iter_mut()
                .zip(next.iter_mut())
                .zip(audio_input)
                .take(*channels)
            {
                *output = coefficients.step(*mode, input, state);
            }
            if commit {
                *states = next;
            }
        }
        (ProcessorCode::Drive(channels), ProcessorState::Drive(states)) => {
            let mut next = *states;
            for ((output, state), input) in output
                .iter_mut()
                .zip(next.iter_mut())
                .zip(audio_input)
                .take(*channels)
            {
                let (value, after) = state.sample(input, params[0], params[1], params[2]);
                *output = value;
                *state = after;
            }
            if commit {
                *states = next;
            }
        }
        (ProcessorCode::Mix(channels), ProcessorState::Stateless) => {
            output[..*channels].copy_from_slice(&audio_input[..*channels]);
        }
        (ProcessorCode::Pan, ProcessorState::Stateless) => {
            output = pan_sample(audio_input[0], params[0])?;
        }
        _ => {
            return Err(RenderError::RenderState(format!(
                "instrument graph node {} has mismatched compiled state",
                compiled.id
            )))
        }
    }
    if output.iter().any(|sample| !sample.is_finite()) {
        return Err(RenderError::Nonfinite(format!(
            "instrument graph node {} produced a nonfinite sample",
            compiled.id
        )));
    }
    Ok(output)
}

fn compile_graph(
    graph: &GraphProgram,
    stage: GraphStage,
    tables: &BTreeMap<String, Arc<TableBank>>,
    samples: &BTreeMap<String, Arc<SamplePlayback>>,
) -> Result<CompiledGraph> {
    let indices: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    let mut nodes = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        let (names, processor) = compile_processor(&node.processor, tables, samples)?;
        let mut base_params = Vec::with_capacity(names.len());
        let mut specs = Vec::with_capacity(names.len());
        for name in names {
            let spec =
                parameter_descriptor_for_stage(&node.processor, name, stage).ok_or_else(|| {
                    RenderError::RenderState(format!(
                        "graph node {} has no descriptor for {name}",
                        node.id
                    ))
                })?;
            let value = node.params.get(*name).map_or_else(
                || rational_f64(&spec.default, "parameter default"),
                |value| rational_f64(value, "graph parameter"),
            )?;
            base_params.push(value);
            specs.push(compile_bounds(
                &spec,
                format!("node {} parameter {name}", node.id),
            )?);
        }
        nodes.push(CompiledNode {
            id: node.id.clone(),
            processor,
            base_params,
            specs,
            incoming: Vec::new(),
            modulations: Vec::new(),
            has_note_on_modulations: false,
            has_reset_modulations: false,
            has_sample_modulations: false,
            controls: Vec::new(),
        });
    }

    let mut connections: Vec<_> = graph.connections.iter().collect();
    connections.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
    for connection in connections {
        let target = indices[connection.to.node.as_str()];
        nodes[target]
            .incoming
            .push(compile_source(&connection.from.node, &indices)?);
    }

    let mut modulations: Vec<_> = graph.modulations.iter().collect();
    modulations.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
    for modulation in modulations {
        let target = indices[modulation.to.node.as_str()];
        let parameter = parameter_slot(&nodes[target].processor, &modulation.to.parameter)
            .ok_or_else(|| {
                RenderError::RenderState(format!(
                    "modulation {} targets a missing parameter",
                    modulation.id
                ))
            })?;
        let rate = parameter_descriptor_for_stage(
            &graph.nodes[target].processor,
            &modulation.to.parameter,
            stage,
        )
        .ok_or_else(|| {
            RenderError::RenderState(format!(
                "modulation {} targets a missing parameter descriptor",
                modulation.id
            ))
        })?
        .rate;
        nodes[target].modulations.push(ModulationBinding {
            id: modulation.id.clone(),
            source: compile_source(&modulation.from.node, &indices)?,
            parameter,
            depth: rational_f64(&modulation.depth, "modulation depth")?,
            rate,
        });
        if rate == ParameterRate::NoteOn {
            nodes[target].has_note_on_modulations = true;
        } else if rate == ParameterRate::Reset {
            nodes[target].has_reset_modulations = true;
        } else if rate == ParameterRate::Sample {
            nodes[target].has_sample_modulations = true;
        }
    }

    let order = topological_order(graph).map_err(RenderError::Plan)?;
    let has_note_on_modulations = nodes.iter().any(|node| {
        node.modulations
            .iter()
            .any(|binding| binding.rate == ParameterRate::NoteOn)
    });
    let has_note_off_modulations = nodes.iter().any(|node| {
        node.modulations
            .iter()
            .any(|binding| binding.rate == ParameterRate::NoteOff)
    });
    let has_reset_modulations = nodes.iter().any(|node| node.has_reset_modulations);
    let (note_on_required, note_off_required, reset_required) =
        capture_dependency_masks(&nodes, &order);
    Ok(CompiledGraph {
        order,
        has_note_on_modulations,
        has_note_off_modulations,
        has_reset_modulations,
        note_on_required,
        note_off_required,
        reset_required,
        output: indices[graph.output.node.as_str()],
        amplitude: graph.amplitude.as_ref().map(|node| indices[node.as_str()]),
        channels: graph.channels as usize,
        nodes,
    })
}

fn compile_processor(
    processor: &GraphProcessor,
    tables: &BTreeMap<String, Arc<TableBank>>,
    samples: &BTreeMap<String, Arc<SamplePlayback>>,
) -> Result<(&'static [&'static str], ProcessorCode)> {
    const OSCILLATOR: &[&str] = &["ratio", "frequency", "phase", "level"];
    Ok(match processor {
        GraphProcessor::Sine => (OSCILLATOR, ProcessorCode::Oscillator(Waveform::Sine)),
        GraphProcessor::Saw => (OSCILLATOR, ProcessorCode::Oscillator(Waveform::Saw)),
        GraphProcessor::Square => (OSCILLATOR, ProcessorCode::Oscillator(Waveform::Square)),
        GraphProcessor::Triangle => (OSCILLATOR, ProcessorCode::Oscillator(Waveform::Triangle)),
        GraphProcessor::Noise { seed } => (&["level"], ProcessorCode::Noise(*seed)),
        GraphProcessor::Pluck { seed } => (
            &["ratio", "decay", "damping", "level"],
            ProcessorCode::Pluck(*seed),
        ),
        GraphProcessor::Wavetable { table } => (
            &["ratio", "frequency", "phase", "level", "position"],
            ProcessorCode::Wavetable(tables.get(table).cloned().ok_or_else(|| {
                RenderError::RenderState(format!("wavetable {table} has no prebuilt table bank"))
            })?),
        ),
        GraphProcessor::Sample {
            channels,
            zones,
            fade_shape,
        } => (
            &["ratio", "frequency", "level"],
            ProcessorCode::Sample(Arc::new(CompiledSampleNode {
                channels: usize::from(*channels),
                zones: zones.clone(),
                playbacks: zones
                    .iter()
                    .map(|zone| {
                        samples.get(&zone.sample).cloned().ok_or_else(|| {
                            RenderError::RenderState(format!(
                                "sample {} is not embedded",
                                zone.sample
                            ))
                        })
                    })
                    .collect::<Result<_>>()?,
                fade_shape: *fade_shape,
            })),
        ),
        GraphProcessor::Adsr => (
            &["attack", "decay", "sustain", "release", "curve"],
            ProcessorCode::Adsr,
        ),
        GraphProcessor::Timbre => (&[], ProcessorCode::Timbre),
        GraphProcessor::Pressure => (&[], ProcessorCode::Pressure),
        GraphProcessor::Velocity => (&[], ProcessorCode::Velocity),
        GraphProcessor::Key => (&[], ProcessorCode::Key),
        GraphProcessor::Lfo => (&["frequency", "phase", "level"], ProcessorCode::Lfo),
        GraphProcessor::Gain { channels } => (&["level"], ProcessorCode::Gain(*channels as usize)),
        GraphProcessor::OnePole { channels } => {
            (&["cutoff"], ProcessorCode::OnePole(*channels as usize))
        }
        GraphProcessor::HighPass { channels } => {
            (&["cutoff"], ProcessorCode::HighPass(*channels as usize))
        }
        GraphProcessor::Svf { channels, mode } => (
            &["cutoff", "ratio", "q"],
            ProcessorCode::Svf(*channels as usize, *mode),
        ),
        GraphProcessor::Drive { channels } => (
            &["drive", "bias", "level"],
            ProcessorCode::Drive(*channels as usize),
        ),
        GraphProcessor::Mix { channels } => (&[], ProcessorCode::Mix(*channels as usize)),
        GraphProcessor::Pan => (&["pan"], ProcessorCode::Pan),
    })
}

fn parameter_slot(processor: &ProcessorCode, name: &str) -> Option<usize> {
    let names: &[&str] = match processor {
        ProcessorCode::Oscillator(_) => &["ratio", "frequency", "phase", "level"],
        ProcessorCode::Pluck(_) => &["ratio", "decay", "damping", "level"],
        ProcessorCode::Wavetable(_) => &["ratio", "frequency", "phase", "level", "position"],
        ProcessorCode::Sample(_) => &["ratio", "frequency", "level"],
        ProcessorCode::Adsr => &["attack", "decay", "sustain", "release", "curve"],
        ProcessorCode::Lfo => &["frequency", "phase", "level"],
        ProcessorCode::Gain(_) | ProcessorCode::Noise(_) => &["level"],
        ProcessorCode::OnePole(_) | ProcessorCode::HighPass(_) => &["cutoff"],
        ProcessorCode::Svf(..) => &["cutoff", "ratio", "q"],
        ProcessorCode::Drive(_) => &["drive", "bias", "level"],
        ProcessorCode::Mix(_)
        | ProcessorCode::Timbre
        | ProcessorCode::Pressure
        | ProcessorCode::Velocity
        | ProcessorCode::Key => &[],
        ProcessorCode::Pan => &["pan"],
    };
    names.iter().position(|candidate| *candidate == name)
}

fn compile_source(source: &str, indices: &HashMap<&str, usize>) -> Result<Source> {
    if source == "input" {
        Ok(Source::Input)
    } else {
        indices
            .get(source)
            .copied()
            .map(Source::Node)
            .ok_or_else(|| {
                RenderError::RenderState(format!("graph source node {source} does not exist"))
            })
    }
}

fn apply_controls(node: &CompiledNode, controls: &[f64], rate: ParameterRate, params: &mut [f64]) {
    for binding in &node.controls {
        if binding.rate == rate {
            params[binding.parameter] = controls[binding.control];
        }
    }
}

fn resolve_controls(
    program: &CompiledInstrument,
    supplied: &BTreeMap<String, f64>,
) -> Result<Vec<f64>> {
    for name in supplied.keys() {
        if !program.control_indices.contains_key(name) {
            return Err(RenderError::RenderState(format!(
                "instrument {} has no public control {name}",
                program.id
            )));
        }
    }
    program
        .controls
        .iter()
        .map(|control| {
            let value = supplied
                .get(&control.name)
                .copied()
                .unwrap_or(control.default);
            validate_value(value, &control.bounds)?;
            Ok(value)
        })
        .collect()
}

fn update_resolved_controls(
    program: &CompiledInstrument,
    supplied: &BTreeMap<String, f64>,
    resolved: &mut [f64],
) -> Result<()> {
    for name in supplied.keys() {
        if !program.control_indices.contains_key(name) {
            return Err(RenderError::RenderState(format!(
                "instrument {} has no public control {name}",
                program.id
            )));
        }
    }
    for control in &program.controls {
        let value = supplied
            .get(&control.name)
            .copied()
            .unwrap_or(control.default);
        validate_value(value, &control.bounds)?;
    }
    for (index, control) in program.controls.iter().enumerate() {
        let value = supplied
            .get(&control.name)
            .copied()
            .unwrap_or(control.default);
        resolved[index] = value;
    }
    Ok(())
}

fn validate_params(node: &CompiledNode, params: &[f64]) -> Result<()> {
    for (value, bounds) in params.iter().zip(&node.specs) {
        validate_value(*value, bounds)?;
    }
    Ok(())
}

fn compile_bounds(spec: &ParameterSpec, context: String) -> Result<CompiledBounds> {
    Ok(CompiledBounds {
        min: rational_f64(&spec.min, "parameter minimum")?,
        max: rational_f64(&spec.max, "parameter maximum")?,
        min_open: spec.min_open,
        max_open: spec.max_open,
        context,
    })
}

fn validate_value(value: f64, bounds: &CompiledBounds) -> Result<()> {
    let below = if bounds.min_open {
        value <= bounds.min
    } else {
        value < bounds.min
    };
    let above = if bounds.max_open {
        value >= bounds.max
    } else {
        value > bounds.max
    };
    if !value.is_finite() || below || above {
        return Err(RenderError::Nonfinite(format!(
            "{} value {value} is outside its finite range",
            bounds.context
        )));
    }
    Ok(())
}

fn validate_frequency(frequency: f64) -> Result<()> {
    if !frequency.is_finite() || !(-24_000.0..=24_000.0).contains(&frequency) {
        return Err(RenderError::Nonfinite(format!(
            "instrument oscillator frequency {frequency} is outside -24000..=24000 Hz"
        )));
    }
    Ok(())
}

fn validate_rate(rate: f64) -> Result<()> {
    if !rate.is_finite() || rate != 48_000.0 {
        return Err(RenderError::Nonfinite(format!(
            "instrument sample rate {rate} must be 48000 Hz"
        )));
    }
    Ok(())
}

fn rational_f64(value: &crate::plan::Rational, context: &str) -> Result<f64> {
    let result = value.to_f64().ok_or_else(|| {
        RenderError::Nonfinite(format!("{context} cannot be represented as binary64"))
    })?;
    if !result.is_finite() {
        return Err(RenderError::Nonfinite(format!(
            "{context} cannot be represented as a finite value"
        )));
    }
    Ok(result)
}

fn pluck_resource_error(message: &str) -> RenderError {
    RenderError::Plan(crate::plan::PlanError {
        code: "E_RESOURCE_LIMIT".into(),
        path: "instrument.pluck".into(),
        message: message.into(),
        span: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rat(value: i64) -> crate::plan::Rational {
        crate::plan::Rational::from_integer(value.into())
    }

    #[test]
    fn zero_velocity_voice_still_advances_noise_state() {
        use crate::graph::{GraphNode, ProgramSource};
        let program = InstrumentProgram {
            id: "noise".into(),
            voice: GraphProgram {
                channels: 1,
                nodes: vec![
                    GraphNode {
                        id: "amp".into(),
                        processor: GraphProcessor::Adsr,
                        params: BTreeMap::new(),
                    },
                    GraphNode {
                        id: "noise".into(),
                        processor: GraphProcessor::Noise { seed: 1 },
                        params: BTreeMap::new(),
                    },
                ],
                connections: Vec::new(),
                modulations: Vec::new(),
                output: crate::plan::PortRef::new("noise", "out").unwrap(),
                amplitude: Some("amp".into()),
            },
            shared: None,
            controls: BTreeMap::new(),
            source: ProgramSource {
                file: "test.maac".into(),
                object: "noise".into(),
                span: None,
            },
        };
        let compiled = Arc::new(CompiledInstrument::compile(&program, &BTreeMap::new()).unwrap());
        let mut runtime = InstrumentRuntime::new(compiled, 1, 48_000.0, &BTreeMap::new()).unwrap();
        runtime.note_on("silent", 440.0, 0.0, 0).unwrap();
        assert_eq!(runtime.render(0).unwrap(), &[0.0]);
        assert_eq!(runtime.render(1).unwrap(), &[0.0]);
        let state = runtime.voices[0]
            .graph
            .nodes
            .iter()
            .find_map(|n| match n.state {
                ProcessorState::Noise(state) => Some(state),
                _ => None,
            });
        assert_eq!(state, Some(67634689));
    }

    #[test]
    fn release_modulation_reads_every_adsr_before_releasing_any_adsr() {
        use crate::graph::{GraphNode, Modulation, ParameterTarget, ProgramSource};
        let adsr = |id: &str| GraphNode {
            id: id.into(),
            processor: GraphProcessor::Adsr,
            params: BTreeMap::from([
                ("attack".into(), rat(0)),
                ("decay".into(), rat(0)),
                ("sustain".into(), rat(1)),
                ("release".into(), rat(0)),
            ]),
        };
        let program = InstrumentProgram {
            id: "adsr_release_order".into(),
            voice: GraphProgram {
                channels: 1,
                nodes: vec![
                    adsr("driver"),
                    adsr("amp"),
                    GraphNode {
                        id: "tone".into(),
                        processor: GraphProcessor::Sine,
                        params: BTreeMap::from([
                            ("ratio".into(), rat(0)),
                            ("frequency".into(), rat(0)),
                            (
                                "phase".into(),
                                crate::plan::Rational::new(1.into(), 4.into()),
                            ),
                            ("level".into(), rat(1)),
                        ]),
                    },
                ],
                connections: Vec::new(),
                modulations: vec![Modulation {
                    id: "driver_release".into(),
                    from: crate::plan::PortRef::new("driver", "out").unwrap(),
                    to: ParameterTarget {
                        node: "amp".into(),
                        parameter: "release".into(),
                    },
                    depth: crate::plan::Rational::new(1.into(), 48_000.into()),
                }],
                output: crate::plan::PortRef::new("tone", "out").unwrap(),
                amplitude: Some("amp".into()),
            },
            shared: None,
            controls: BTreeMap::new(),
            source: ProgramSource {
                file: "test.maac".into(),
                object: "adsr_release_order".into(),
                span: None,
            },
        };
        let compiled = Arc::new(CompiledInstrument::compile(&program, &BTreeMap::new()).unwrap());
        let mut runtime = InstrumentRuntime::new(compiled, 1, 48_000.0, &BTreeMap::new()).unwrap();
        runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
        assert_eq!(runtime.render(0).unwrap(), &[1.0]);

        runtime.note_off("voice", 1).unwrap();
        assert_eq!(runtime.render(1).unwrap(), &[1.0]);
        assert_eq!(runtime.active_voice_count(), 1);
        runtime.prune_finished(2).unwrap();
        assert_eq!(runtime.active_voice_count(), 0);
    }

    #[test]
    fn note_on_modulations_reduce_in_id_order() {
        use crate::graph::{GraphNode, Modulation, ParameterTarget, ProgramSource};
        let source = crate::plan::PortRef::new("tone", "out").unwrap();
        let target = || ParameterTarget {
            node: "amp".into(),
            parameter: "sustain".into(),
        };
        let mut program = InstrumentProgram {
            id: "ordered_event_sum".into(),
            voice: GraphProgram {
                channels: 1,
                nodes: vec![
                    GraphNode {
                        id: "amp".into(),
                        processor: GraphProcessor::Adsr,
                        params: BTreeMap::from([("sustain".into(), rat(0))]),
                    },
                    GraphNode {
                        id: "tone".into(),
                        processor: GraphProcessor::Sine,
                        params: BTreeMap::from([
                            ("ratio".into(), rat(0)),
                            ("frequency".into(), rat(0)),
                            (
                                "phase".into(),
                                crate::plan::Rational::new(1.into(), 4.into()),
                            ),
                            ("level".into(), rat(1)),
                        ]),
                    },
                ],
                connections: Vec::new(),
                modulations: vec![
                    Modulation {
                        id: "c_small".into(),
                        from: source.clone(),
                        to: target(),
                        depth: crate::plan::Rational::new(
                            1.into(),
                            1_152_921_504_606_846_976_i64.into(),
                        ),
                    },
                    Modulation {
                        id: "b_negative".into(),
                        from: source.clone(),
                        to: target(),
                        depth: rat(-128),
                    },
                    Modulation {
                        id: "a_positive".into(),
                        from: source.clone(),
                        to: target(),
                        depth: rat(128),
                    },
                ],
                output: source,
                amplitude: Some("amp".into()),
            },
            shared: None,
            controls: BTreeMap::new(),
            source: ProgramSource {
                file: "test.maac".into(),
                object: "ordered_event_sum".into(),
                span: None,
            },
        };
        let render = |program: &InstrumentProgram| {
            let compiled =
                Arc::new(CompiledInstrument::compile(program, &BTreeMap::new()).unwrap());
            let mut runtime =
                InstrumentRuntime::new(compiled, 1, 48_000.0, &BTreeMap::new()).unwrap();
            runtime.note_on("voice", 440.0, 1.0, 0).unwrap();
            runtime.render(0).unwrap()[0]
        };
        let expected = 2.0_f64.powi(-60);
        assert_eq!(render(&program).to_bits(), expected.to_bits());
        program.voice.modulations.reverse();
        assert_eq!(render(&program).to_bits(), expected.to_bits());
    }

    fn phase_program(modulations: Vec<crate::graph::Modulation>) -> InstrumentProgram {
        use crate::graph::{GraphNode, ProgramSource};
        let adsr = |id: &str, sustain: i64| GraphNode {
            id: id.into(),
            processor: GraphProcessor::Adsr,
            params: BTreeMap::from([("sustain".into(), rat(sustain))]),
        };
        InstrumentProgram {
            id: "phase_capture".into(),
            voice: GraphProgram {
                channels: 1,
                nodes: vec![
                    adsr("amp", 1),
                    adsr("driver", 1),
                    GraphNode {
                        id: "tone".into(),
                        processor: GraphProcessor::Sine,
                        params: BTreeMap::from([
                            ("ratio".into(), rat(0)),
                            ("frequency".into(), rat(0)),
                            ("phase".into(), rat(0)),
                            ("level".into(), rat(1)),
                        ]),
                    },
                ],
                connections: Vec::new(),
                modulations,
                output: crate::plan::PortRef::new("tone", "out").unwrap(),
                amplitude: Some("amp".into()),
            },
            shared: None,
            controls: BTreeMap::new(),
            source: ProgramSource {
                file: "test.maac".into(),
                object: "phase_capture".into(),
                span: None,
            },
        }
    }

    fn phase_modulation(
        id: &str,
        from: &str,
        to: &str,
        parameter: &str,
        depth: crate::plan::Rational,
    ) -> crate::graph::Modulation {
        crate::graph::Modulation {
            id: id.into(),
            from: crate::plan::PortRef::new(from, "out").unwrap(),
            to: crate::graph::ParameterTarget {
                node: to.into(),
                parameter: parameter.into(),
            },
            depth,
        }
    }

    fn render_phase_program(program: &InstrumentProgram) -> Result<f64> {
        let compiled = Arc::new(CompiledInstrument::compile(program, &BTreeMap::new())?);
        let mut runtime = InstrumentRuntime::new(compiled, 1, 48_000.0, &BTreeMap::new())?;
        runtime.note_on("voice", 440.0, 1.0, 0)?;
        Ok(runtime.render(0)?[0])
    }

    #[test]
    fn note_on_phase_initializes_before_downstream_adsr_capture() {
        let mut program = phase_program(vec![
            phase_modulation(
                "a_phase",
                "driver",
                "tone",
                "phase",
                crate::plan::Rational::new(1.into(), 4.into()),
            ),
            phase_modulation("b_sustain", "tone", "amp", "sustain", rat(1)),
        ]);
        program
            .voice
            .nodes
            .iter_mut()
            .find(|node| node.id == "amp")
            .unwrap()
            .params
            .insert("sustain".into(), rat(0));

        assert_eq!(render_phase_program(&program).unwrap(), 1.0);
    }

    #[test]
    fn phase_capture_uses_id_order_and_rejects_final_range() {
        let source = "driver";
        let mut ordered = phase_program(vec![
            phase_modulation(
                "c_small",
                source,
                "tone",
                "phase",
                crate::plan::Rational::new(1.into(), 1_152_921_504_606_846_976_i64.into()),
            ),
            phase_modulation("b_negative", source, "tone", "phase", rat(-128)),
            phase_modulation("a_positive", source, "tone", "phase", rat(128)),
        ]);
        let first = render_phase_program(&ordered).unwrap();
        assert!(first > 0.0);
        ordered.voice.modulations.reverse();
        assert_eq!(
            render_phase_program(&ordered).unwrap().to_bits(),
            first.to_bits()
        );

        let mut invalid = phase_program(vec![phase_modulation(
            "phase",
            source,
            "tone",
            "phase",
            crate::plan::Rational::new(1.into(), 2.into()),
        )]);
        invalid
            .voice
            .nodes
            .iter_mut()
            .find(|node| node.id == "tone")
            .unwrap()
            .params
            .insert(
                "phase".into(),
                crate::plan::Rational::new(3.into(), 4.into()),
            );
        let compiled = Arc::new(CompiledInstrument::compile(&invalid, &BTreeMap::new()).unwrap());
        let mut runtime = InstrumentRuntime::new(compiled, 1, 48_000.0, &BTreeMap::new()).unwrap();
        assert_eq!(
            runtime.note_on("voice", 440.0, 1.0, 0).unwrap_err().code(),
            "E_RANGE"
        );
    }

    fn shared_reset_program(
        shared: GraphProgram,
        controls: BTreeMap<String, crate::graph::Control>,
    ) -> InstrumentProgram {
        use crate::graph::{GraphNode, ProgramSource};
        InstrumentProgram {
            id: "shared_reset_capture".into(),
            voice: GraphProgram {
                channels: 1,
                nodes: vec![
                    GraphNode {
                        id: "amp".into(),
                        processor: GraphProcessor::Adsr,
                        params: BTreeMap::new(),
                    },
                    GraphNode {
                        id: "tone".into(),
                        processor: GraphProcessor::Sine,
                        params: BTreeMap::new(),
                    },
                ],
                connections: Vec::new(),
                modulations: Vec::new(),
                output: crate::plan::PortRef::new("tone", "out").unwrap(),
                amplitude: Some("amp".into()),
            },
            shared: Some(shared),
            controls,
            source: ProgramSource {
                file: "test.maac".into(),
                object: "shared_reset_capture".into(),
                span: None,
            },
        }
    }

    #[test]
    fn shared_reset_preview_does_not_advance_filter_history() {
        use crate::graph::{GraphNode, Modulation, ParameterTarget};
        use crate::plan::Connection;
        let out = |node| crate::plan::PortRef::new(node, "out").unwrap();
        let input = |node| crate::plan::PortRef::new(node, "in").unwrap();
        let shared = GraphProgram {
            channels: 1,
            nodes: vec![
                GraphNode {
                    id: "source".into(),
                    processor: GraphProcessor::Lfo,
                    params: BTreeMap::from([
                        ("frequency".into(), rat(100)),
                        ("phase".into(), rat(0)),
                        ("level".into(), rat(1)),
                    ]),
                },
                GraphNode {
                    id: "filter".into(),
                    processor: GraphProcessor::OnePole { channels: 1 },
                    params: BTreeMap::from([("cutoff".into(), rat(12_000))]),
                },
                GraphNode {
                    id: "target".into(),
                    processor: GraphProcessor::Lfo,
                    params: BTreeMap::from([("frequency".into(), rat(0))]),
                },
                GraphNode {
                    id: "mix".into(),
                    processor: GraphProcessor::Mix { channels: 1 },
                    params: BTreeMap::new(),
                },
                GraphNode {
                    id: "driver".into(),
                    processor: GraphProcessor::Lfo,
                    params: BTreeMap::from([
                        (
                            "phase".into(),
                            crate::plan::Rational::new(1.into(), 4.into()),
                        ),
                        ("level".into(), rat(1)),
                    ]),
                },
            ],
            connections: vec![
                Connection::new("source_filter", out("source"), input("filter")).unwrap(),
                Connection::new("filter_mix", out("filter"), input("mix")).unwrap(),
                Connection::new("target_mix", out("target"), input("mix")).unwrap(),
            ],
            modulations: vec![
                Modulation {
                    id: "driver_source_phase".into(),
                    from: out("driver"),
                    to: ParameterTarget {
                        node: "source".into(),
                        parameter: "phase".into(),
                    },
                    depth: crate::plan::Rational::new(1.into(), 4.into()),
                },
                Modulation {
                    id: "filter_phase".into(),
                    from: out("filter"),
                    to: ParameterTarget {
                        node: "target".into(),
                        parameter: "phase".into(),
                    },
                    depth: rat(1),
                },
            ],
            output: out("mix"),
            amplitude: None,
        };
        let program = shared_reset_program(shared, BTreeMap::new());
        let compiled = Arc::new(CompiledInstrument::compile(&program, &BTreeMap::new()).unwrap());
        let mut runtime = InstrumentRuntime::new(compiled, 1, 48_000.0, &BTreeMap::new()).unwrap();

        let a = (-std::f64::consts::PI / 2.0).exp();
        let mut source = Oscillator::new(Waveform::Sine, 0.25).unwrap();
        let source_first = source.sample(100.0, 48_000.0).unwrap();
        let first_filter = (1.0 - a) * source_first;
        let mut target = Oscillator::new(Waveform::Sine, first_filter).unwrap();
        let captured_target = target.sample(0.0, 48_000.0).unwrap();
        let first = first_filter + captured_target;
        let source_second = source.sample(100.0, 48_000.0).unwrap();
        let second_filter = (1.0 - a) * source_second + a * first_filter;
        let second = second_filter + target.sample(0.0, 48_000.0).unwrap();
        assert!((runtime.render(0).unwrap()[0] - first).abs() < 1.0e-12);
        assert!((runtime.render(1).unwrap()[0] - second).abs() < 1.0e-12);

        runtime.reset(&BTreeMap::new()).unwrap();
        assert!((runtime.render(0).unwrap()[0] - first).abs() < 1.0e-12);
    }

    #[test]
    fn failed_public_reset_capture_preserves_the_previous_runtime() {
        use crate::graph::{
            Control, ControlTarget, GraphNode, GraphStage, Modulation, ParameterTarget,
        };
        let shared = GraphProgram {
            channels: 1,
            nodes: vec![
                GraphNode {
                    id: "source".into(),
                    processor: GraphProcessor::Lfo,
                    params: BTreeMap::from([
                        ("frequency".into(), rat(0)),
                        (
                            "phase".into(),
                            crate::plan::Rational::new(1.into(), 4.into()),
                        ),
                    ]),
                },
                GraphNode {
                    id: "target".into(),
                    processor: GraphProcessor::Lfo,
                    params: BTreeMap::from([("frequency".into(), rat(0))]),
                },
            ],
            connections: Vec::new(),
            modulations: vec![Modulation {
                id: "source_phase".into(),
                from: crate::plan::PortRef::new("source", "out").unwrap(),
                to: ParameterTarget {
                    node: "target".into(),
                    parameter: "phase".into(),
                },
                depth: crate::plan::Rational::new(1.into(), 4.into()),
            }],
            output: crate::plan::PortRef::new("target", "out").unwrap(),
            amplitude: None,
        };
        let controls = BTreeMap::from([(
            "phase".into(),
            Control {
                target: ControlTarget {
                    graph: GraphStage::Shared,
                    node: "target".into(),
                    parameter: "phase".into(),
                },
                default: rat(0),
            },
        )]);
        let program = shared_reset_program(shared, controls);
        let compiled = Arc::new(CompiledInstrument::compile(&program, &BTreeMap::new()).unwrap());
        let mut runtime = InstrumentRuntime::new(compiled, 1, 48_000.0, &BTreeMap::new()).unwrap();
        assert_eq!(runtime.render(0).unwrap(), &[1.0]);

        let error = runtime
            .reset(&BTreeMap::from([("phase".into(), 1.0)]))
            .unwrap_err();
        assert_eq!(error.code(), "E_RANGE");
        assert_eq!(runtime.render(1).unwrap(), &[1.0]);

        runtime.reset(&BTreeMap::new()).unwrap();
        assert_eq!(runtime.render(0).unwrap(), &[1.0]);
    }
}

#[cfg(test)]
mod pluck_state_tests {
    use super::*;

    #[test]
    fn zero_velocity_and_zero_amplitude_still_advance_pluck_state() {
        for (velocity, sustain) in [(0.0, 1), (1.0, 0)] {
            let source = format!(
                r#"maac 1;
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument string {{ channels = 1; voice v {{ channels = 1; amplitude = &amp; output = &string:out;
node amp {{ type = "synth.adsr/1"; params = {{ sustain = {sustain}; }}; }}
node string {{ type = "synth.pluck/1"; config = {{ seed = 1; }}; }}
}} }}
node sound {{ instrument = &string; }}
"#
            );
            let mut plan =
                crate::compile_bundle(&crate::SourceBundle::new("silent.maac", source)).unwrap();
            let program = plan.instruments.as_mut().unwrap().programs.remove(0);
            let compiled =
                Arc::new(CompiledInstrument::compile(&program, &BTreeMap::new()).unwrap());
            let mut runtime =
                InstrumentRuntime::new(compiled, 1, 48000.0, &BTreeMap::new()).unwrap();
            runtime.note_on("silent", 480.0, velocity, 0).unwrap();
            let mut expected = crate::pluck::Pluck::new(1).unwrap();
            for frame in 0..128 {
                assert_eq!(runtime.render(frame).unwrap(), &[0.0]);
                expected.sample(480.0, 3.0, 0.5, 1.0).unwrap();
            }
            // Private state inspection distinguishes silent-but-running from a
            // shortcut that skipped the generator under velocity/envelope mute.
            let actual = runtime.voices[0]
                .graph
                .nodes
                .iter_mut()
                .find_map(|node| {
                    if let ProcessorState::Pluck(pluck) = &mut node.state {
                        Some(pluck)
                    } else {
                        None
                    }
                })
                .unwrap();
            assert_eq!(
                actual.sample(480.0, 3.0, 0.5, 1.0).unwrap(),
                expected.sample(480.0, 3.0, 0.5, 1.0).unwrap()
            );
        }
    }
}

#[cfg(test)]
mod block_tests {
    use super::*;

    fn program(source: &str, name: &str) -> Arc<CompiledInstrument> {
        let plan = crate::compile_bundle(&crate::SourceBundle::new("main.maac", source)).unwrap();
        let resources = plan.instruments.as_ref().unwrap();
        let program = resources
            .programs
            .iter()
            .find(|program| program.source.object == name)
            .unwrap();
        Arc::new(CompiledInstrument::compile(program, &BTreeMap::new()).unwrap())
    }

    /// Notes on and off at fixed frames; returns every rendered frame, or the
    /// frame and error where rendering stopped. `ahead` renders up to the next
    /// event instead of one frame at a time.
    fn play(
        compiled: &Arc<CompiledInstrument>,
        frames: u64,
        ahead: bool,
    ) -> (Vec<Vec<f64>>, Option<(u64, RenderError)>, usize) {
        let ons = [
            (0, "a", 220.0, 0.9),
            (100, "b", 330.0, 0.4),
            (2100, "c", 880.0, 1.0),
        ];
        let offs = [(700, "a"), (2000, "b"), (2400, "c")];
        let events: Vec<u64> = vec![0, 100, 700, 2000, 2100, 2400];
        let mut runtime =
            InstrumentRuntime::new(Arc::clone(compiled), 8, 48000.0, &BTreeMap::new()).unwrap();
        let mut rendered = Vec::new();
        for frame in 0..frames {
            for (at, address) in offs {
                if at == frame {
                    runtime.note_off(address, frame).unwrap();
                }
            }
            runtime.prune_finished(frame).unwrap();
            for (at, address, pitch, velocity) in ons {
                if at == frame {
                    runtime.note_on(address, pitch, velocity, frame).unwrap();
                }
            }
            let horizon = if ahead {
                events
                    .iter()
                    .copied()
                    .find(|&event| event > frame)
                    .unwrap_or(frames)
            } else {
                frame + 1
            };
            match runtime.render_until(frame, horizon) {
                Ok(output) => rendered.push(output.to_vec()),
                Err(error) => return (rendered, Some((frame, error)), runtime.voices.len()),
            }
        }
        let voices = runtime.voices.len();
        (rendered, None, voices)
    }

    #[test]
    fn rendering_ahead_matches_frame_by_frame_rendering() {
        let source = r#"maac 1;
project p { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &keys:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
import studio { builtin = "std/studio/1.0.0"; }
node keys { instrument = &studio.electric_piano; }
"#;
        let compiled = program(source, "electric_piano");
        // The last release ends near frame 2400 + 300 ms, so the voices retire
        // inside a block.
        let (frame_by_frame, error, voices) = play(&compiled, 20_000, false);
        assert!(error.is_none());
        let (ahead, error, voices_ahead) = play(&compiled, 20_000, true);
        assert!(error.is_none());
        assert_eq!(voices, 0);
        assert_eq!(voices_ahead, 0);
        assert_eq!(frame_by_frame.len(), ahead.len());
        for (frame, (expected, actual)) in frame_by_frame.iter().zip(&ahead).enumerate() {
            let bits = |frame: &Vec<f64>| frame.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
            assert_eq!(bits(expected), bits(actual), "frame {frame}");
        }
    }

    #[test]
    fn rendering_ahead_reports_the_same_error_at_the_same_frame() {
        // A 3 Hz LFO swings the cutoff below zero about 70 ms into each note.
        let source = r#"maac 1;
project p { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument swept { channels = 1;
  voice v { channels = 1; amplitude = &amp; output = &tone:out;
    node amp { type = "synth.adsr/1"; params = { sustain = 1; release = 50ms; }; }
    node osc { type = "synth.saw/1"; }
    node sweep { type = "synth.lfo/1"; params = { frequency = 3Hz; }; }
    node tone { type = "synth.svf/1"; config = { channels = 1; mode = lowpass; }; params = { cutoff = 1000Hz; }; }
    connect osc_tone { from = &osc:out; to = &tone:in; }
    modulate swing { from = &sweep:out; to = &tone.params.cutoff; depth = -2000Hz; }
  }
}
node sound { instrument = &swept; }
"#;
        let compiled = program(source, "swept");
        let (frame_by_frame, expected, _) = play(&compiled, 20_000, false);
        let (ahead, actual, _) = play(&compiled, 20_000, true);
        let (expected_frame, expected_error) = expected.expect("the sweep fails");
        let (actual_frame, actual_error) = actual.expect("the sweep fails");
        assert_eq!(expected_frame, actual_frame);
        assert_eq!(expected_error, actual_error);
        assert_eq!(frame_by_frame, ahead);
    }

    #[test]
    fn a_change_inside_a_block_rendered_ahead_is_refused() {
        let source = r#"maac 1;
project p { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &keys:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
import studio { builtin = "std/studio/1.0.0"; }
node keys { instrument = &studio.electric_piano; }
"#;
        let compiled = program(source, "electric_piano");
        let mut runtime = InstrumentRuntime::new(compiled, 4, 48000.0, &BTreeMap::new()).unwrap();
        runtime.note_on("a", 220.0, 0.5, 0).unwrap();
        runtime.render_until(0, 64).unwrap();
        assert!(matches!(
            runtime.note_on("b", 330.0, 0.5, 10),
            Err(RenderError::RenderState(_))
        ));
        assert!(matches!(
            runtime.note_off("a", 10),
            Err(RenderError::RenderState(_))
        ));
    }
}
