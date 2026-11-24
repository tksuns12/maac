//! Compiled reusable instrument graphs and their per-instance DSP state.

use crate::dsp::{one_pole_step, pan_sample, RenderError, Result};
use crate::expression::ExpressionRuntime;
use crate::graph::{
    parameter_descriptor_for_stage, topological_order, Control, EffectProgram, GraphProcessor,
    GraphProgram, GraphStage, InstrumentProgram, ParameterRate, ParameterSpec, MAX_GRAPH_NODES,
};
use crate::plan::{GainExpression, PitchExpression, PressureExpression, TimbreExpression};
use crate::processor::Declaration;
use crate::sample_instrument::{SampleFadeShape, SamplePlayback, SampleZone};
use crate::synth::{Adsr, DriveState, Oscillator, SvfCoefficients, SvfMode, Waveform};
use crate::wavetable::TableBank;
use num_traits::ToPrimitive;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The most parameter slots any processor has: `synth.bow/1`'s eleven. The
/// note-on, reset and note-off captures evaluate parameters in a scratch of
/// this size.
const MAX_PARAMETER_SLOTS: usize = crate::processor::BOW.params.len();

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
    /// Whether the voice graph holds a `synth.bow/1` section, which plays
    /// lines.
    plays_lines: bool,
}

/// Mutable polyphonic state for one instrument node.
#[derive(Debug, Clone)]
pub struct InstrumentRuntime {
    program: Arc<CompiledInstrument>,
    capacity: usize,
    rate: f64,
    controls: Vec<f64>,
    controls_generation: u64,
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
    expressions: Vec<[f64; 3]>,
    /// Reusable per-block scratch buffers for `fill`.
    active_scratch: Vec<usize>,
    voice_errors_scratch: Vec<Option<(usize, RenderError)>>,
    sums_scratch: Vec<[f64; 2]>,
    /// How long the last block took to render, to decide whether handing the
    /// next one to other threads pays. Not part of the instrument's state.
    last_fill_nanos: u64,
}

/// The most frames an instrument renders ahead at once.
const BLOCK_FRAMES: usize = 128;

#[derive(Debug)]
struct CompiledGraph {
    nodes: Vec<CompiledNode>,
    order: Vec<usize>,
    /// The `synth.delay/1` nodes, in `order`. Each reads its line in its
    /// place in the order and writes its input after the rest of the graph.
    delays: Vec<usize>,
    has_note_on_modulations: bool,
    has_note_off_modulations: bool,
    has_reset_modulations: bool,
    note_on_required: [bool; MAX_GRAPH_NODES],
    note_off_required: [bool; MAX_GRAPH_NODES],
    reset_required: [bool; MAX_GRAPH_NODES],
    output: usize,
    amplitude: Option<usize>,
    channels: usize,
    #[cfg(feature = "jit")]
    jit: Option<Arc<crate::jit::JitCompiledGraph>>,
}

#[derive(Debug)]
struct CompiledNode {
    id: String,
    declaration: &'static Declaration,
    /// Whether it reads its audio input from earlier frames, as a delay
    /// line does: its input is summed and written after the graph has run.
    reads_input_later: bool,
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
    /// The seed, and whether each note mixes its address into it.
    Noise(u32, bool),
    Random(u32),
    Pluck(u32),
    /// Modes and strings.
    Modal(u8, u8),
    /// Players, body, channels and seed.
    Bow(u8, crate::bow::BowBody, u8, u32),
    Wavetable(Arc<TableBank>),
    Sample(Arc<CompiledSampleNode>),
    /// Whether a re-articulated connected note restarts the envelope.
    Adsr(bool),
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
    Balance,
    Width,
    /// Channels, whether the length counts quarter notes, and the frames
    /// the line holds.
    Delay(usize, bool, usize),
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

#[derive(Debug, Clone)]
struct VoiceState {
    address: String,
    pitch_hz: f64,
    velocity: f64,
    /// A joined note's velocity moves from this one over 100 ms from the
    /// frame it joined.
    velocity_glide: Option<(f64, u64)>,
    /// A sliding joined note's base pitch moves from this one, in cents,
    /// over this many frames from the frame it joined.
    pitch_glide: Option<(f64, u64, f64)>,
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
    /// The `controls_generation` that the parameters of nodes without
    /// sample-rate modulation were last computed from; `None` before the first sample.
    settled_generation: Option<u64>,
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
    Modal(crate::modal::Modal),
    Bow(Box<crate::bow::Bow>),
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
    /// The velocity a `synth.velocity/1` node emits: the note-on velocity,
    /// or after a join the glide to the joined note's.
    Velocity(f64, Option<(f64, u64)>),
    /// The value a `synth.random/1` node emits for its note, gliding like a
    /// velocity after a join.
    Random(f64, Option<(f64, u64)>),
    Delay(Box<crate::delay::DelayLine>),
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

        let (controls, control_indices) = compile_controls(
            "instrument",
            &program.id,
            &program.controls,
            &|name| program.control_spec(name),
            Some(&mut voice),
            shared.as_mut(),
        )?;

        #[cfg(feature = "jit")]
        if !jit_disabled_for_test() {
            voice.jit = jit_bridge::compile_graph(&voice);
            if let Some(shared_graph) = shared.as_mut() {
                shared_graph.jit = jit_bridge::compile_graph(shared_graph);
            }
        }

        Ok(Self {
            id: program.id.clone(),
            channels: program.channels() as usize,
            pluck_nodes: program.pluck_node_count(),
            plays_lines: voice
                .nodes
                .iter()
                .any(|node| matches!(node.processor, ProcessorCode::Bow(..))),
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
        let players = program
            .voice
            .nodes
            .iter()
            .map(|node| match node.processor {
                ProcessorCode::Bow(players, ..) => usize::from(players),
                _ => 0,
            })
            .sum();
        if crate::graph::bow_string_cells(capacity as usize, players)
            .is_none_or(|cells| cells > crate::graph::MAX_BOW_STRING_CELLS)
        {
            return Err(RenderError::Plan(crate::plan::PlanError {
                code: "E_RESOURCE_LIMIT".into(),
                path: "instrument.voices".into(),
                message: "declared bowed string storage exceeds the instance limit".into(),
                span: None,
            }));
        }
        let resolved = resolve_controls(
            &program.id,
            &program.controls,
            &program.control_indices,
            controls,
        )?;
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
            controls_generation: 1,
            voices,
            shared_reset: shared.clone(),
            shared,
            output: [0.0; 2],
            buffered: Vec::new(),
            buffered_start: 0,
            buffered_error: None,
            last_fill_nanos: 0,
            next_frame: 0,
            scratch: Vec::new(),
            voice_frames: Vec::new(),
            expressions: Vec::new(),
            active_scratch: Vec::new(),
            voice_errors_scratch: Vec::new(),
            sums_scratch: Vec::new(),
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

    /// Whether frames from `frame` on were rendered already.
    pub(crate) fn has_frames_ahead(&self, frame: u64) -> bool {
        self.buffered_start + self.buffered.len() as u64 > frame
    }

    fn clear_ahead(&mut self) {
        self.buffered.clear();
        self.buffered_start = 0;
        self.buffered_error = None;
        self.next_frame = 0;
    }

    /// Replace the instance's public control cache for the current frame.
    pub fn update_controls(&mut self, controls: &BTreeMap<String, f64>) -> Result<()> {
        let changed = update_resolved_controls(
            &self.program.id,
            &self.program.controls,
            &self.program.control_indices,
            controls,
            &mut self.controls,
        )?;
        if changed {
            self.controls_generation += 1;
            if self.buffered_start + self.buffered.len() as u64 > self.next_frame {
                return Err(RenderError::RenderState(format!(
                    "instrument {} controls changed inside a block rendered ahead",
                    self.program.id
                )));
            }
        }
        Ok(())
    }

    pub fn note_on(
        &mut self,
        address: impl Into<String>,
        pitch_hz: f64,
        velocity: f64,
        frame: u64,
    ) -> Result<()> {
        self.note_on_with_expressions(
            address, pitch_hz, velocity, frame, None, None, None, None, None, 0,
        )
    }

    /// Whether this instrument plays lines: a voice graph with a bowed
    /// section, which reads the notes its voice will play.
    pub(crate) fn plays_lines(&self) -> bool {
        self.program.plays_lines
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
        line: Option<Arc<crate::bow::Line>>,
        expression_shift: u64,
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
        let address = address.into();
        let line = self.program.plays_lines.then(|| {
            line.unwrap_or_else(|| {
                // A voice without a plan plays one note of unknown length.
                Arc::new(crate::bow::Line {
                    notes: vec![crate::bow::LineNote {
                        on_frame: frame,
                        off_frame: u64::MAX,
                        pitch_hz,
                        slur: false,
                        slide_seconds: 0.0,
                    }],
                    address: address.clone(),
                })
            })
        });
        let graph = GraphState::new_voice(
            &self.program.voice,
            &self.controls,
            frame,
            velocity,
            &address,
        )?;
        let mut voice = VoiceState {
            address,
            pitch_hz,
            velocity,
            velocity_glide: None,
            pitch_glide: None,
            pitch_expression,
            gain_expression,
            timbre_expression,
            pressure_expression,
            on_frame: frame,
            released: false,
            graph,
        };
        if let Some(line) = &line {
            voice.graph.play_line(line)?;
        }
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
        if voice.graph.has_unmodulated_excitation(&self.program.voice) {
            let (pitch_hz, _, _) = voice.expression_inputs(frame, self.rate)?;
            voice
                .graph
                .excite_unmodulated(&self.program.voice, &self.controls, pitch_hz)?;
        }
        // A preview restarts a note that began `expression_shift` frames
        // earlier: from here its curves continue from that point, but what it
        // captured at note-on above was read at the note's own onset.
        if expression_shift > 0 {
            voice.pitch_expression = voice
                .pitch_expression
                .take()
                .map(|expression| expression.shifted(expression_shift));
            voice.gain_expression = voice
                .gain_expression
                .take()
                .map(|expression| expression.shifted(expression_shift));
            voice.timbre_expression = voice
                .timbre_expression
                .take()
                .map(|expression| expression.shifted(expression_shift));
            voice.pressure_expression = voice
                .pressure_expression
                .take()
                .map(|expression| expression.shifted(expression_shift));
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

    /// Continue the voice of the note at `from` with a joined note (§8.3).
    /// The voice keeps every node's state and takes the joined note's
    /// address, pitch and expressions; its velocity glides to the joined
    /// note's over 100 ms. Outside a bowed section, a note without `slur`
    /// restarts the envelopes that restart on a join, and `slide` moves the
    /// base pitch over that many seconds.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn note_join(
        &mut self,
        from: &str,
        address: String,
        pitch_hz: f64,
        velocity: f64,
        frame: u64,
        (slur, slide_seconds): (bool, f64),
        pitch_expression: Option<ExpressionRuntime<PitchExpression>>,
        gain_expression: Option<ExpressionRuntime<GainExpression>>,
        timbre_expression: Option<ExpressionRuntime<TimbreExpression>>,
        pressure_expression: Option<ExpressionRuntime<PressureExpression>>,
    ) -> Result<()> {
        if !pitch_hz.is_finite() || !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
            return Err(RenderError::Nonfinite(
                "joined note pitch must be finite and velocity must be within 0..=1".into(),
            ));
        }
        if !slide_seconds.is_finite() || slide_seconds < 0.0 {
            return Err(RenderError::Nonfinite(
                "joined note slide must be finite and nonnegative".into(),
            ));
        }
        self.check_nothing_ahead(frame)?;
        let position = self
            .voices
            .binary_search_by(|voice| voice.address.as_bytes().cmp(from.as_bytes()))
            .map_err(|_| {
                RenderError::RenderState(format!("joined note {address} has no voice for {from}"))
            })?;
        if self.voices[position].released {
            return Err(RenderError::RenderState(format!(
                "joined note {address} continues a released voice"
            )));
        }
        let mut voice = self.voices.remove(position);
        let previous = voice.velocity_at(frame);
        voice.velocity_glide = Some((previous, frame));
        voice.velocity = velocity;
        voice
            .graph
            .glide_velocity(&self.program.voice, previous, velocity, &address, frame);
        // A bowed section plays its own slurs and slides from the line.
        if !self.program.plays_lines {
            voice.pitch_glide = (slide_seconds > 0.0)
                .then(|| (voice.base_pitch_at(frame), frame, slide_seconds * self.rate));
            if !slur {
                voice
                    .graph
                    .restart_envelopes(&self.program.voice, frame, self.rate)?;
            }
        }
        voice.address = address;
        voice.pitch_hz = pitch_hz;
        voice.on_frame = frame;
        voice.pitch_expression = pitch_expression;
        voice.gain_expression = gain_expression;
        voice.timbre_expression = timbre_expression;
        voice.pressure_expression = pressure_expression;
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
        voice.graph.lift_bows(frame);
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
        if self.needs_refill(frame) {
            self.refill(frame, horizon)?;
        }
        self.output = self.buffered[(frame - self.buffered_start) as usize];
        self.next_frame = frame + 1;
        Ok(&self.output[..self.program.channels])
    }

    /// Whether `frame` is outside the frames rendered ahead, so that
    /// [`Self::render_until`] would render a block.
    pub(crate) fn needs_refill(&self, frame: u64) -> bool {
        let end = self.buffered_start + self.buffered.len() as u64;
        !(self.buffered_start..end).contains(&frame)
    }

    /// The block `render_until` renders when `frame` is not buffered.
    fn refill(&mut self, frame: u64, horizon: u64) -> Result<()> {
        let end = self.buffered_start + self.buffered.len() as u64;
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
        Ok(())
    }

    /// Render the block `render_until(frame, horizon)` is about to render, if
    /// it has to, so that the call finds it done. An error is kept where
    /// `render_until` reports it, at the frame it belongs to. This is what
    /// lets a block be rendered on another thread, ahead of the call, with
    /// the same result as rendering it in the call.
    pub(crate) fn prefill(&mut self, frame: u64, horizon: u64) {
        if self.needs_refill(frame) {
            if let Err(error) = self.refill(frame, horizon) {
                self.buffered_error = Some(error);
            }
        }
    }

    /// How long the last block took to render, in nanoseconds: a guess at
    /// what the next one costs, for deciding whether handing instruments to
    /// other threads pays.
    pub(crate) fn cost_hint(&self) -> u64 {
        self.last_fill_nanos
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
            self.expressions.resize(stride, [0.0, 0.0, 0.0]);
        }

        // Each voice: frames before its retirement, and its first error.
        self.active_scratch.clear();
        self.voice_errors_scratch.clear();
        let timer = std::time::Instant::now();
        for (index, voice) in self.voices.iter_mut().enumerate() {
            let (length, error) = render_voice(
                voice,
                &program,
                &self.controls,
                self.controls_generation,
                start,
                frames,
                rate,
                &mut self.expressions,
                &mut self.scratch,
                &mut self.voice_frames[index * stride..(index + 1) * stride],
            );
            self.active_scratch.push(length);
            self.voice_errors_scratch.push(error);
        }
        self.last_fill_nanos = timer.elapsed().as_nanos() as u64;
        // Combine the voices frame by frame, in voice order.
        let voice_channels = graph.channels;
        self.sums_scratch.resize(frames, [0.0; 2]);
        let mut limit = frames;
        let mut error = None;
        'frames: for (offset, sum) in self.sums_scratch[..frames].iter_mut().enumerate() {
            let frame = start + offset as u64;
            let mut voice_sum = [0.0; 2];
            for (index, voice) in self.voices.iter().enumerate() {
                if offset >= self.active_scratch[index] {
                    if let Some((at, _)) = &self.voice_errors_scratch[index] {
                        if *at == offset {
                            limit = offset;
                            error = self.voice_errors_scratch[index]
                                .take()
                                .map(|(_, failure)| failure);
                            break 'frames;
                        }
                    }
                    continue;
                }
                if let Some((at, _)) = &self.voice_errors_scratch[index] {
                    if *at == offset {
                        limit = offset;
                        error = self.voice_errors_scratch[index]
                            .take()
                            .map(|(_, failure)| failure);
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
                        .eval_at(voice.on_frame, frame)
                        .expect("validated gain expression")
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
                    let contribution = sample[channel] * amplitude * voice.velocity_at(frame);
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
                *expression = [0.0, 0.0, 0.0];
            }
            let (shared_frames, shared_error) = shared.render_block(
                compiled,
                &self.controls,
                self.controls_generation,
                start,
                limit,
                rate,
                &self.expressions[..limit],
                Some(&self.sums_scratch[..limit]),
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
            self.buffered.extend_from_slice(&self.sums_scratch[..limit]);
        }
        self.buffered_error = error;

        // Pruning would have removed these voices at their retirement frames.
        if self.buffered_error.is_none() {
            let mut index = 0;
            self.voices.retain(|_| {
                let keep = self.active_scratch[index] == frames;
                index += 1;
                keep
            });
        }
    }

    pub fn reset(&mut self, controls: &BTreeMap<String, f64>) -> Result<()> {
        let resolved = resolve_controls(
            &self.program.id,
            &self.program.controls,
            &self.program.control_indices,
            controls,
        )?;
        let shared = self
            .program
            .shared
            .as_ref()
            .map(|graph| GraphState::new_shared(graph, &resolved, self.rate))
            .transpose()?;
        let shared_reset = shared.clone();

        self.controls = resolved;
        self.controls_generation += 1;
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

/// Immutable executable code shared by every instance of one effect.
#[derive(Debug)]
pub struct CompiledEffect {
    id: String,
    graph: CompiledGraph,
    controls: Vec<CompiledControl>,
    control_indices: BTreeMap<String, usize>,
    channels: usize,
    input_channels: usize,
}

impl CompiledEffect {
    /// Compile a program already covered by the enclosing plan validation.
    pub(crate) fn compile_validated(program: &EffectProgram) -> Result<Self> {
        let mut graph = compile_graph(
            &program.graph,
            GraphStage::Shared,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )?;
        let (controls, control_indices) = compile_controls(
            "effect",
            &program.id,
            &program.controls,
            &|name| program.control_spec(name),
            None,
            Some(&mut graph),
        )?;
        Ok(Self {
            id: program.id.clone(),
            graph,
            controls,
            control_indices,
            channels: usize::from(program.channels()),
            input_channels: usize::from(program.input_channels),
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn input_channels(&self) -> usize {
        self.input_channels
    }
}

/// Mutable state for one effect node: its graph's state from the reset
/// origin, and its public control values.
#[derive(Debug, Clone)]
pub struct EffectRuntime {
    program: Arc<CompiledEffect>,
    rate: f64,
    controls: Vec<f64>,
    controls_generation: u64,
    state: GraphState,
    reset: GraphState,
    /// Node outputs of the frame being processed, one per node.
    scratch: Vec<[f64; 2]>,
}

impl EffectRuntime {
    pub fn new(
        program: Arc<CompiledEffect>,
        rate: f64,
        controls: &BTreeMap<String, f64>,
        tempo: &Arc<crate::delay::TempoCurve>,
    ) -> Result<Self> {
        validate_rate(rate)?;
        let resolved = resolve_controls(
            &program.id,
            &program.controls,
            &program.control_indices,
            controls,
        )?;
        let mut state = GraphState::new_shared(&program.graph, &resolved, rate)?;
        for node in &mut state.nodes {
            if let ProcessorState::Delay(line) = &mut node.state {
                line.attach_tempo(tempo);
            }
        }
        Ok(Self {
            scratch: vec![[0.0; 2]; program.graph.nodes.len()],
            reset: state.clone(),
            state,
            controls: resolved,
            controls_generation: 1,
            rate,
            program,
        })
    }

    /// Replace the instance's public control values for the current frame.
    pub fn update_controls(&mut self, controls: &BTreeMap<String, f64>) -> Result<()> {
        let changed = update_resolved_controls(
            &self.program.id,
            &self.program.controls,
            &self.program.control_indices,
            controls,
            &mut self.controls,
        )?;
        if changed {
            self.controls_generation += 1;
        }
        Ok(())
    }

    /// Process one frame of the audio at `in`. Channels beyond the input's
    /// width are ignored.
    pub fn process(&mut self, frame: u64, input: [f64; 2]) -> Result<[f64; 2]> {
        let program = Arc::clone(&self.program);
        let mut input = input;
        if program.input_channels < 2 {
            input[1] = 0.0;
        }
        let (rendered, error) = self.state.render_block(
            &program.graph,
            &self.controls,
            self.controls_generation,
            frame,
            1,
            self.rate,
            &[[0.0, 0.0, 0.0]],
            Some(&[input]),
            &mut self.scratch,
            1,
        );
        if let Some(error) = error {
            return Err(error);
        }
        if rendered != 1 {
            return Err(RenderError::RenderState(format!(
                "effect {} rendered no frame",
                program.id
            )));
        }
        let mut output = self.scratch[program.graph.output];
        if program.channels < 2 {
            output[1] = 0.0;
        }
        Ok(output)
    }

    pub(crate) fn reset_state(&mut self) {
        self.state.clone_from(&self.reset);
    }
}

/// The frames over which a joined note's velocity moves to its own.
const VELOCITY_GLIDE_FRAMES: u64 = 4_800;

/// `velocity`, or on its way there from a joined note's earlier one.
/// A `synth.random/1` value: uniform in [-1, 1) from the seed and the note's
/// address.
fn note_value(seed: u32, address: &str) -> f64 {
    let mut rng = crate::seeded::Rng::new(crate::seeded::mix(u64::from(seed), address.as_bytes()));
    2.0 * rng.unit() - 1.0
}

/// A per-note noise seed: the high half of the seed mixed with the address,
/// or the seed itself where that is zero, which xorshift cannot leave.
fn note_noise_seed(seed: u32, address: &str) -> u32 {
    match (crate::seeded::mix(u64::from(seed), address.as_bytes()) >> 32) as u32 {
        0 => seed,
        mixed => mixed,
    }
}

fn glided_velocity(velocity: f64, glide: Option<(f64, u64)>, frame: u64) -> f64 {
    match glide {
        Some((from, start)) if frame < start + VELOCITY_GLIDE_FRAMES => {
            let progress = frame.saturating_sub(start) as f64 / VELOCITY_GLIDE_FRAMES as f64;
            from + (velocity - from) * progress
        }
        _ => velocity,
    }
}

/// Render one voice's block: the frames before its retirement, and its
/// first error. Fills `output` with the voice's frames and returns how many
/// frames it is active for.
#[allow(clippy::too_many_arguments)]
fn render_voice(
    voice: &mut VoiceState,
    program: &CompiledInstrument,
    controls: &[f64],
    controls_generation: u64,
    start: u64,
    frames: usize,
    rate: f64,
    expressions: &mut Vec<[f64; 3]>,
    scratch: &mut Vec<[f64; 2]>,
    output_frames: &mut [[f64; 2]],
) -> (usize, Option<(usize, RenderError)>) {
    let graph = &program.voice;
    let stride = BLOCK_FRAMES;
    if expressions.len() < stride {
        expressions.resize(stride, [0.0, 0.0, 0.0]);
    }
    let scratch_len = stride
        * program.voice.nodes.len().max(
            program
                .shared
                .as_ref()
                .map_or(0, |shared| shared.nodes.len()),
        );
    if scratch.len() < scratch_len {
        scratch.resize(scratch_len, [0.0; 2]);
    }
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
    let mut rendered = length;
    for (offset, expression) in expressions[..length].iter_mut().enumerate() {
        match voice.expression_inputs(start + offset as u64, rate) {
            Ok((pitch_hz, timbre, pressure)) => {
                *expression = [pitch_hz, timbre, pressure];
            }
            Err(failure) => {
                rendered = offset;
                error = Some((offset, failure));
                break;
            }
        }
    }
    let (graph_frames, graph_error) = voice.graph.render_block(
        graph,
        controls,
        controls_generation,
        start,
        rendered,
        rate,
        &expressions[..rendered],
        None,
        scratch,
        stride,
    );
    if let Some(failure) = graph_error {
        error = Some((graph_frames, failure));
    }
    let output = graph.output * stride;
    output_frames[..graph_frames].copy_from_slice(&scratch[output..output + graph_frames]);
    (length, error)
}

impl VoiceState {
    fn velocity_at(&self, frame: u64) -> f64 {
        glided_velocity(self.velocity, self.velocity_glide, frame)
    }

    /// The pitch before expression: the note's, or on its way there from
    /// the note it joined, linear in cents.
    fn base_pitch_at(&self, frame: u64) -> f64 {
        match self.pitch_glide {
            Some((from, start, frames)) if (frame.saturating_sub(start) as f64) < frames => {
                let progress = frame.saturating_sub(start) as f64 / frames;
                from * (self.pitch_hz / from).powf(progress)
            }
            _ => self.pitch_hz,
        }
    }

    fn expression_inputs(&self, frame: u64, rate: f64) -> Result<(f64, f64, f64)> {
        let pitch_hz = if let Some(expression) = &self.pitch_expression {
            let cents = expression.eval_at(self.on_frame, frame).ok_or_else(|| {
                RenderError::Nonfinite("pitch expression cents cannot be represented".into())
            })?;
            let frequency = self.base_pitch_at(frame) * 2.0_f64.powf(cents / 1200.0);
            if !frequency.is_finite() || frequency <= 0.0 || frequency >= rate / 2.0 {
                return Err(RenderError::RenderState(format!(
                    "pitch expression at {} is outside (0, Nyquist)",
                    self.address
                )));
            }
            frequency
        } else {
            self.base_pitch_at(frame)
        };
        let timbre = self.timbre_expression.as_ref().map_or(0.0, |expression| {
            expression
                .eval_at(self.on_frame, frame)
                .expect("validated timbre expression")
        });
        if !timbre.is_finite() || !(0.0..=1.0).contains(&timbre) {
            return Err(RenderError::Nonfinite(format!(
                "timbre expression at {} produced an invalid value",
                self.address
            )));
        }
        let pressure = self.pressure_expression.as_ref().map_or(0.0, |expression| {
            expression
                .eval_at(self.on_frame, frame)
                .expect("validated pressure expression")
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
        address: &str,
    ) -> Result<Self> {
        Self::new(graph, controls, Some((frame, address)), velocity)
    }

    fn new_shared(graph: &CompiledGraph, controls: &[f64], rate: f64) -> Result<Self> {
        // Shared graphs contain no velocity-sensitive (voice-only) nodes.
        let mut state = Self::new(graph, controls, None, 0.0)?;
        if graph.has_reset_modulations {
            state.capture_reset(graph, controls, rate)?;
        }
        Ok(state)
    }

    /// `note` is the voice's note-on frame and address; shared graphs have
    /// none.
    fn new(
        graph: &CompiledGraph,
        controls: &[f64],
        note: Option<(u64, &str)>,
        velocity: f64,
    ) -> Result<Self> {
        let on_frame = note.map(|(frame, _)| frame);
        let address = note.map(|(_, address)| address);
        let mut nodes = Vec::with_capacity(graph.nodes.len());
        for node in &graph.nodes {
            let mut params = node.base_params.clone();
            apply_controls(node, controls, ParameterRate::NoteOn, &mut params);
            apply_controls(node, controls, ParameterRate::Reset, &mut params);
            let state = match &node.processor {
                ProcessorCode::Oscillator(waveform) => ProcessorState::Oscillator(Oscillator::new(
                    *waveform,
                    params[slot::OSCILLATOR_PHASE],
                )?),
                ProcessorCode::Noise(seed, per_note) => ProcessorState::Noise(match address {
                    Some(address) if *per_note => note_noise_seed(*seed, address),
                    _ => *seed,
                }),
                ProcessorCode::Random(seed) => {
                    ProcessorState::Random(address.map_or(0.0, |a| note_value(*seed, a)), None)
                }
                ProcessorCode::Pluck(seed) => {
                    ProcessorState::Pluck(crate::pluck::Pluck::new(*seed)?)
                }
                ProcessorCode::Modal(modes, strings) => {
                    ProcessorState::Modal(crate::modal::Modal::new(*modes, *strings)?)
                }
                ProcessorCode::Bow(players, body, channels, seed) => ProcessorState::Bow(Box::new(
                    crate::bow::Bow::new(*players, *body, *channels, *seed)?,
                )),
                ProcessorCode::Wavetable(_) => ProcessorState::Wavetable {
                    phase: params[slot::WAVETABLE_PHASE],
                },
                ProcessorCode::Sample(_) => ProcessorState::Sample {
                    velocity,
                    layers: None,
                },
                ProcessorCode::Adsr(_) => ProcessorState::Adsr(Adsr::new_curved(
                    on_frame.unwrap_or(0),
                    params[slot::ADSR_ATTACK],
                    params[slot::ADSR_DECAY],
                    params[slot::ADSR_SUSTAIN],
                    params[slot::ADSR_CURVE],
                )?),
                ProcessorCode::Lfo => {
                    ProcessorState::Lfo(Oscillator::new(Waveform::Sine, params[slot::LFO_PHASE])?)
                }
                ProcessorCode::OnePole(_) | ProcessorCode::HighPass(_) => {
                    ProcessorState::OnePole([0.0; 2])
                }
                ProcessorCode::Svf(..) => ProcessorState::Svf([[0.0; 2]; 2]),
                ProcessorCode::Drive(_) => ProcessorState::Drive([DriveState::default(); 2]),
                ProcessorCode::Velocity => ProcessorState::Velocity(velocity, None),
                ProcessorCode::Delay(_, _, frames) => {
                    ProcessorState::Delay(Box::new(crate::delay::DelayLine::new(*frames)))
                }
                ProcessorCode::Gain(_)
                | ProcessorCode::Key
                | ProcessorCode::Mix(_)
                | ProcessorCode::Pan
                | ProcessorCode::Balance
                | ProcessorCode::Width
                | ProcessorCode::Timbre
                | ProcessorCode::Pressure => ProcessorState::Stateless,
            };
            nodes.push(RuntimeNode { state, params });
        }
        Ok(Self {
            nodes,
            settled_generation: None,
        })
    }

    /// Give every bowed section of the voice the line it plays.
    fn play_line(&mut self, line: &Arc<crate::bow::Line>) -> Result<()> {
        for runtime in &mut self.nodes {
            if let ProcessorState::Bow(bow) = &mut runtime.state {
                bow.play(line.clone())?;
            }
        }
        Ok(())
    }

    /// A joined note: velocity sources glide to its velocity from `from`,
    /// and random sources to its value.
    fn glide_velocity(
        &mut self,
        graph: &CompiledGraph,
        from: f64,
        velocity: f64,
        address: &str,
        frame: u64,
    ) {
        for (compiled, runtime) in graph.nodes.iter().zip(&mut self.nodes) {
            match (&compiled.processor, &mut runtime.state) {
                (_, ProcessorState::Velocity(target, glide)) => {
                    *target = velocity;
                    *glide = Some((from, frame));
                }
                (ProcessorCode::Random(seed), ProcessorState::Random(target, glide)) => {
                    let from = glided_velocity(*target, *glide, frame);
                    *target = note_value(*seed, address);
                    *glide = Some((from, frame));
                }
                _ => {}
            }
        }
    }

    /// A re-articulated connected note: every envelope that restarts on a
    /// join enters its attack again from its present level.
    fn restart_envelopes(&mut self, graph: &CompiledGraph, frame: u64, rate: f64) -> Result<()> {
        for (compiled, runtime) in graph.nodes.iter().zip(&mut self.nodes) {
            if let (ProcessorCode::Adsr(true), ProcessorState::Adsr(envelope)) =
                (&compiled.processor, &mut runtime.state)
            {
                envelope.restart(frame, rate)?;
            }
        }
        Ok(())
    }

    /// The voice's note-off: every bowed section lifts its bow.
    fn lift_bows(&mut self, frame: u64) {
        for runtime in &mut self.nodes {
            if let ProcessorState::Bow(bow) = &mut runtime.state {
                bow.lift(frame);
            }
        }
    }

    /// Whether a plucked string without note-on modulation shapes its fill;
    /// a modulated one is shaped during the note-on capture.
    fn has_unmodulated_excitation(&self, graph: &CompiledGraph) -> bool {
        graph
            .nodes
            .iter()
            .zip(&self.nodes)
            .any(|(compiled, runtime)| {
                matches!(compiled.processor, ProcessorCode::Pluck(_))
                    && !compiled.has_note_on_modulations
                    && plucked_fill_is_shaped(&runtime.params)
            })
    }

    fn excite_unmodulated(
        &mut self,
        graph: &CompiledGraph,
        controls: &[f64],
        pitch_hz: f64,
    ) -> Result<()> {
        for (compiled, runtime) in graph.nodes.iter().zip(&mut self.nodes) {
            // Only a pluck has a fill; another node's slots 4 and 5 are not
            // `hardness` and `position`, and may not exist.
            if !matches!(compiled.processor, ProcessorCode::Pluck(_))
                || compiled.has_note_on_modulations
                || !plucked_fill_is_shaped(&runtime.params)
            {
                continue;
            }
            if let ProcessorState::Pluck(pluck) = &mut runtime.state {
                let ratio = note_on_ratio(compiled, controls, runtime.params[slot::PLUCK_RATIO]);
                pluck.excite(
                    pitch_hz * ratio,
                    runtime.params[slot::PLUCK_HARDNESS],
                    runtime.params[slot::PLUCK_POSITION],
                )?;
            }
        }
        Ok(())
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
                let mut release = compiled.base_params[slot::ADSR_RELEASE];
                for binding in &compiled.controls {
                    if binding.rate == ParameterRate::NoteOff
                        && binding.parameter == slot::ADSR_RELEASE
                    {
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
            if !matches!(compiled.processor, ProcessorCode::Adsr(_)) {
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
            validate_event_parameter(compiled, slot::ADSR_RELEASE, params[slot::ADSR_RELEASE])?;
            releases[index] = params[slot::ADSR_RELEASE];
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
                && (matches!(compiled.processor, ProcessorCode::Adsr(_))
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
                    (ProcessorCode::Adsr(_), state) => {
                        for parameter in [
                            slot::ADSR_ATTACK,
                            slot::ADSR_DECAY,
                            slot::ADSR_SUSTAIN,
                            slot::ADSR_CURVE,
                        ] {
                            validate_event_parameter(compiled, parameter, event_params[parameter])?;
                        }
                        *state = ProcessorState::Adsr(Adsr::new_curved(
                            frame,
                            event_params[slot::ADSR_ATTACK],
                            event_params[slot::ADSR_DECAY],
                            event_params[slot::ADSR_SUSTAIN],
                            event_params[slot::ADSR_CURVE],
                        )?);
                    }
                    (ProcessorCode::Oscillator(_), ProcessorState::Oscillator(oscillator)) => {
                        validate_event_parameter(
                            compiled,
                            slot::OSCILLATOR_PHASE,
                            event_params[slot::OSCILLATOR_PHASE],
                        )?;
                        oscillator.reset(event_params[slot::OSCILLATOR_PHASE])?;
                    }
                    (ProcessorCode::Wavetable(_), ProcessorState::Wavetable { phase }) => {
                        validate_event_parameter(
                            compiled,
                            slot::WAVETABLE_PHASE,
                            event_params[slot::WAVETABLE_PHASE],
                        )?;
                        *phase = canonical_event_phase(event_params[slot::WAVETABLE_PHASE]);
                    }
                    (ProcessorCode::Lfo, ProcessorState::Lfo(oscillator)) => {
                        validate_event_parameter(
                            compiled,
                            slot::LFO_PHASE,
                            event_params[slot::LFO_PHASE],
                        )?;
                        oscillator.reset(event_params[slot::LFO_PHASE])?;
                    }
                    (ProcessorCode::Pluck(_), ProcessorState::Pluck(pluck)) => {
                        for parameter in [slot::PLUCK_HARDNESS, slot::PLUCK_POSITION] {
                            validate_event_parameter(compiled, parameter, event_params[parameter])?;
                        }
                        let ratio = note_on_ratio(
                            compiled,
                            controls,
                            compiled.base_params[slot::PLUCK_RATIO],
                        );
                        pluck.excite(
                            pitch_hz * ratio,
                            event_params[slot::PLUCK_HARDNESS],
                            event_params[slot::PLUCK_POSITION],
                        )?;
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
                        validate_event_parameter(
                            compiled,
                            slot::LFO_PHASE,
                            event_params[slot::LFO_PHASE],
                        )?;
                        oscillator.reset(event_params[slot::LFO_PHASE])?;
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
        controls_generation: u64,
        start: u64,
        frames: usize,
        rate: f64,
        expressions: &[[f64; 3]],
        input: Option<&[[f64; 2]]>,
        scratch: &mut [[f64; 2]],
        stride: usize,
    ) -> (usize, Option<RenderError>) {
        // A node without sample-rate modulation computes the same parameters
        // until a control changes, so it keeps the values it already checked.
        let settled = self.settled_generation == Some(controls_generation);
        let input_at = |offset: usize| input.map_or([0.0; 2], |input| input[offset]);
        if !graph.delays.is_empty() && frames > 1 {
            // A line may feed back to its own input one frame later, so a
            // block cannot run node by node.
            return (
                0,
                Some(RenderError::RenderState(
                    "a graph with a delay line renders one frame at a time".into(),
                )),
            );
        }
        #[cfg(feature = "jit")]
        if let Some(jit) = graph.jit.as_ref() {
            if graph.delays.is_empty() {
                let (limit, error) = jit_bridge::execute_block(
                    jit,
                    graph,
                    &mut self.nodes,
                    controls,
                    settled,
                    start,
                    frames,
                    rate,
                    expressions,
                    input,
                    &mut scratch[graph.output * stride..graph.output * stride + frames],
                );
                if !settled && error.is_none() && frames > 0 {
                    self.settled_generation = Some(controls_generation);
                }
                return (limit, error);
            }
        }
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
                // A delay line's input is written after the graph has run.
                if !compiled.reads_input_later {
                    match sum_incoming(compiled, |source| match source {
                        Source::Input => input_at(offset),
                        Source::Node(index) => scratch[index * stride + offset],
                    }) {
                        Ok(sum) => audio_input = sum,
                        Err(failure) => {
                            stopped = Some((offset, failure));
                            break;
                        }
                    }
                }
                let [pitch_hz, timbre, pressure] = expressions[offset];
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
        for &node_index in &graph.delays {
            if limit == 0 {
                break;
            }
            let compiled = &graph.nodes[node_index];
            let written = sum_incoming(compiled, |source| match source {
                Source::Input => input_at(0),
                Source::Node(index) => scratch[index * stride],
            })
            .and_then(|sum| {
                let runtime = &mut self.nodes[node_index];
                let (ProcessorCode::Delay(channels, ..), ProcessorState::Delay(line)) =
                    (&compiled.processor, &mut runtime.state)
                else {
                    return Err(RenderError::RenderState(format!(
                        "instrument graph node {} has mismatched compiled state",
                        compiled.id
                    )));
                };
                line.write(
                    start,
                    sum,
                    scratch[node_index * stride],
                    *channels,
                    runtime.params[slot::DELAY_FEEDBACK],
                    runtime.params[slot::DELAY_DAMPING],
                )
            });
            if let Err(failure) = written {
                limit = 0;
                error = Some(failure);
            }
        }
        if !settled && error.is_none() && frames > 0 {
            self.settled_generation = Some(controls_generation);
        }
        (limit, error)
    }
}

/// The sum of a node's audio connections, in connection-ID order.
fn sum_incoming(compiled: &CompiledNode, value: impl Fn(&Source) -> [f64; 2]) -> Result<[f64; 2]> {
    let mut sum = [0.0; 2];
    for source in &compiled.incoming {
        let source = value(source);
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
        if matches!(node.processor, ProcessorCode::Adsr(_)) {
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

/// Whether a pluck's note-on `hardness` and `position` change its fill.
fn plucked_fill_is_shaped(params: &[f64]) -> bool {
    params[slot::PLUCK_HARDNESS] != 1.0 || params[slot::PLUCK_POSITION] != 0.0
}

/// A pluck's `ratio` at note-on: `base` with any control on it, before
/// modulation.
fn note_on_ratio(node: &CompiledNode, controls: &[f64], base: f64) -> f64 {
    node.controls
        .iter()
        .filter(|binding| binding.parameter == slot::PLUCK_RATIO)
        .fold(base, |_, binding| controls[binding.control])
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
        (ProcessorCode::Noise(..), ProcessorState::Noise(state)) => {
            // Version 1 fixes xorshift32's width, shifts and advance-before-output order.
            let mut next = *state;
            next ^= next << 13;
            next ^= next >> 17;
            next ^= next << 5;
            output[0] = (next as f64 / 2147483648.0 - 1.0) * params[slot::NOISE_LEVEL];
            if commit {
                *state = next;
            }
        }
        (ProcessorCode::Pluck(_), ProcessorState::Pluck(pluck)) => {
            output[0] = if commit {
                pluck.sample(
                    pitch_hz * params[slot::PLUCK_RATIO],
                    params[slot::PLUCK_DECAY],
                    params[slot::PLUCK_DAMPING],
                    params[slot::PLUCK_LEVEL],
                    params[slot::PLUCK_PICKUP],
                )?
            } else {
                pluck.preview(
                    pitch_hz * params[slot::PLUCK_RATIO],
                    params[slot::PLUCK_DECAY],
                    params[slot::PLUCK_DAMPING],
                    params[slot::PLUCK_LEVEL],
                    params[slot::PLUCK_PICKUP],
                )?
            };
        }
        (ProcessorCode::Bow(..), ProcessorState::Bow(bow)) => {
            let params = crate::bow::BowParams {
                ratio: params[slot::BOW_RATIO],
                dynamics: params[slot::BOW_DYNAMICS],
                position: params[slot::BOW_POSITION],
                vibrato: params[slot::BOW_VIBRATO],
                rate: params[slot::BOW_RATE],
                attack: params[slot::BOW_ATTACK],
                timing: params[slot::BOW_TIMING],
                spread: params[slot::BOW_SPREAD],
                width: params[slot::BOW_WIDTH],
                pan: params[slot::BOW_PAN],
                level: params[slot::BOW_LEVEL],
            };
            output = if commit {
                bow.sample(frame, &params, pitch_hz)?
            } else {
                bow.preview(frame, &params, pitch_hz)?
            };
        }
        (ProcessorCode::Modal(..), ProcessorState::Modal(bank)) => {
            let shape = crate::modal::ModalShape {
                frequency_hz: pitch_hz * params[slot::MODAL_RATIO],
                stretch: params[slot::MODAL_STRETCH],
                decay: params[slot::MODAL_DECAY],
                damping: params[slot::MODAL_DAMPING],
                position: params[slot::MODAL_POSITION],
                detune_cents: params[slot::MODAL_DETUNE],
            };
            output[0] = if commit {
                bank.sample(&shape, params[slot::MODAL_LEVEL], audio_input[0])?
            } else {
                bank.preview(&shape, params[slot::MODAL_LEVEL], audio_input[0])?
            };
        }
        (ProcessorCode::Oscillator(_), ProcessorState::Oscillator(oscillator)) => {
            let frequency =
                pitch_hz * params[slot::OSCILLATOR_RATIO] + params[slot::OSCILLATOR_FREQUENCY];
            validate_frequency(frequency)?;
            let level = params[slot::OSCILLATOR_LEVEL];
            if commit {
                output[0] = oscillator.sample(frequency, rate)? * level;
            } else {
                let mut preview = oscillator.clone();
                output[0] = preview.sample(frequency, rate)? * level;
            }
        }
        (ProcessorCode::Wavetable(table), ProcessorState::Wavetable { phase }) => {
            let frequency =
                pitch_hz * params[slot::WAVETABLE_RATIO] + params[slot::WAVETABLE_FREQUENCY];
            validate_frequency(frequency)?;
            output[0] = table.sample(*phase, params[slot::WAVETABLE_POSITION], frequency, rate)?
                * params[slot::WAVETABLE_LEVEL];
            if commit {
                *phase = (*phase + frequency / rate).rem_euclid(1.0);
            }
        }
        (ProcessorCode::Sample(node), ProcessorState::Sample { velocity, layers }) => {
            let frequency = pitch_hz * params[slot::SAMPLE_RATIO] + params[slot::SAMPLE_FREQUENCY];
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
                *output = sum * params[slot::SAMPLE_LEVEL];
            }
            if commit {
                if let Some(chosen) = chosen {
                    *layers = Some(chosen);
                }
            }
        }
        (ProcessorCode::Adsr(_), ProcessorState::Adsr(envelope)) => {
            output[0] = envelope.value(frame, rate)?;
        }
        (ProcessorCode::Lfo, ProcessorState::Lfo(oscillator)) => {
            if commit {
                output[0] =
                    oscillator.sample(params[slot::LFO_FREQUENCY], rate)? * params[slot::LFO_LEVEL];
            } else {
                let mut preview = oscillator.clone();
                output[0] =
                    preview.sample(params[slot::LFO_FREQUENCY], rate)? * params[slot::LFO_LEVEL];
            }
        }
        (ProcessorCode::Timbre, ProcessorState::Stateless) => output[0] = timbre,
        (ProcessorCode::Pressure, ProcessorState::Stateless) => output[0] = pressure,
        (ProcessorCode::Velocity, ProcessorState::Velocity(velocity, glide))
        | (ProcessorCode::Random(_), ProcessorState::Random(velocity, glide)) => {
            output[0] = glided_velocity(*velocity, *glide, frame)
        }
        // Octaves from C4 = 440 Hz * 2^(-9/12).
        (ProcessorCode::Key, ProcessorState::Stateless) => {
            output[0] = (pitch_hz / 440.0).log2() + 0.75;
        }
        (ProcessorCode::Gain(channels), ProcessorState::Stateless) => {
            for (output, input) in output.iter_mut().zip(audio_input).take(*channels) {
                *output = input * params[slot::GAIN_LEVEL];
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
                let lowpass = one_pole_step(input, previous, params[slot::ONEPOLE_CUTOFF], rate)?;
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
            let cutoff = pitch_hz * params[slot::SVF_RATIO] + params[slot::SVF_CUTOFF];
            let coefficients =
                SvfCoefficients::new(cutoff, params[slot::SVF_Q], rate).map_err(|_| {
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
            if params[slot::DRIVE_REFERENCE] > 0.0 && params[slot::DRIVE_DRIVE] == 0.0 {
                return Err(RenderError::Nonfinite(format!(
                    "drive node {} needs a positive drive when reference is set",
                    compiled.id
                )));
            }
            let mut next = *states;
            for ((output, state), input) in output
                .iter_mut()
                .zip(next.iter_mut())
                .zip(audio_input)
                .take(*channels)
            {
                let (value, after) = state.sample(
                    input,
                    params[slot::DRIVE_DRIVE],
                    params[slot::DRIVE_BIAS],
                    params[slot::DRIVE_LEVEL],
                    params[slot::DRIVE_REFERENCE],
                );
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
            output = pan_sample(audio_input[0], params[slot::PAN])?;
        }
        (ProcessorCode::Balance, ProcessorState::Stateless) => {
            // cos(π/2) is not exactly zero in binary64; a full balance
            // silences the far side exactly.
            let lowered = |amount: f64| {
                if amount >= 1.0 {
                    0.0
                } else {
                    (std::f64::consts::FRAC_PI_2 * amount.max(0.0)).cos()
                }
            };
            output = [
                audio_input[0] * lowered(params[slot::BALANCE]),
                audio_input[1] * lowered(-params[slot::BALANCE]),
            ];
        }
        (ProcessorCode::Width, ProcessorState::Stateless) => {
            let mid = (audio_input[0] + audio_input[1]) / 2.0;
            let side = (audio_input[0] - audio_input[1]) / 2.0;
            let width = params[slot::WIDTH];
            output = [mid + width * side, mid - width * side];
        }
        (ProcessorCode::Delay(channels, score, _), ProcessorState::Delay(line)) => {
            // The input is written after the rest of the graph has run; see
            // `GraphState::write_delays`.
            let _ = (audio_input, commit);
            output = line.read(frame, rate, *channels, *score, params[slot::DELAY_TIME])?;
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
        let declaration = node.processor.declaration();
        let processor = compile_processor(&node.processor, tables, samples)?;
        if declaration.params.len() > MAX_PARAMETER_SLOTS {
            return Err(RenderError::RenderState(format!(
                "graph node {} has {} parameter slots, more than {MAX_PARAMETER_SLOTS}",
                node.id,
                declaration.params.len()
            )));
        }
        let mut base_params = Vec::with_capacity(declaration.params.len());
        let mut specs = Vec::with_capacity(declaration.params.len());
        for param in declaration.params {
            let spec = param.spec(stage);
            let value = node.params.get(param.name).map_or_else(
                || rational_f64(&spec.default, "parameter default"),
                |value| rational_f64(value, "graph parameter"),
            )?;
            base_params.push(value);
            specs.push(compile_bounds(
                &spec,
                format!("node {} parameter {}", node.id, param.name),
            )?);
        }
        nodes.push(CompiledNode {
            id: node.id.clone(),
            declaration,
            reads_input_later: node.processor.reads_input_later(),
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
        let parameter =
            parameter_slot(&nodes[target], &modulation.to.parameter).ok_or_else(|| {
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
    let delays = order
        .iter()
        .copied()
        .filter(|&index| nodes[index].reads_input_later)
        .collect();
    Ok(CompiledGraph {
        order,
        delays,
        has_note_on_modulations,
        has_note_off_modulations,
        has_reset_modulations,
        note_on_required,
        note_off_required,
        reset_required,
        output: indices[graph.output.node.as_str()],
        amplitude: graph.amplitude.as_ref().map(|node| indices[node.as_str()]),
        channels: graph.channels as usize,
        #[cfg(feature = "jit")]
        jit: None,
        nodes,
    })
}

fn compile_processor(
    processor: &GraphProcessor,
    tables: &BTreeMap<String, Arc<TableBank>>,
    samples: &BTreeMap<String, Arc<SamplePlayback>>,
) -> Result<ProcessorCode> {
    Ok(match processor {
        GraphProcessor::Sine => ProcessorCode::Oscillator(Waveform::Sine),
        GraphProcessor::Saw => ProcessorCode::Oscillator(Waveform::Saw),
        GraphProcessor::Square => ProcessorCode::Oscillator(Waveform::Square),
        GraphProcessor::Triangle => ProcessorCode::Oscillator(Waveform::Triangle),
        GraphProcessor::Noise { seed, per_note } => ProcessorCode::Noise(*seed, *per_note),
        GraphProcessor::Random { seed } => ProcessorCode::Random(*seed),
        GraphProcessor::Pluck { seed } => ProcessorCode::Pluck(*seed),
        GraphProcessor::Modal { modes, strings } => ProcessorCode::Modal(*modes, *strings),
        GraphProcessor::Bow {
            players,
            body,
            channels,
            seed,
        } => ProcessorCode::Bow(*players, *body, *channels, *seed),
        GraphProcessor::Wavetable { table } => {
            ProcessorCode::Wavetable(tables.get(table).cloned().ok_or_else(|| {
                RenderError::RenderState(format!("wavetable {table} has no prebuilt table bank"))
            })?)
        }
        GraphProcessor::Sample {
            channels,
            zones,
            fade_shape,
        } => ProcessorCode::Sample(Arc::new(CompiledSampleNode {
            channels: usize::from(*channels),
            zones: zones.clone(),
            playbacks: zones
                .iter()
                .map(|zone| {
                    samples.get(&zone.sample).cloned().ok_or_else(|| {
                        RenderError::RenderState(format!("sample {} is not embedded", zone.sample))
                    })
                })
                .collect::<Result<_>>()?,
            fade_shape: *fade_shape,
        })),
        GraphProcessor::Adsr { on_join } => {
            ProcessorCode::Adsr(*on_join == crate::graph::AdsrJoin::Restart)
        }
        GraphProcessor::Timbre => ProcessorCode::Timbre,
        GraphProcessor::Pressure => ProcessorCode::Pressure,
        GraphProcessor::Velocity => ProcessorCode::Velocity,
        GraphProcessor::Key => ProcessorCode::Key,
        GraphProcessor::Lfo => ProcessorCode::Lfo,
        GraphProcessor::Gain { channels } => ProcessorCode::Gain(*channels as usize),
        GraphProcessor::OnePole { channels } => ProcessorCode::OnePole(*channels as usize),
        GraphProcessor::HighPass { channels } => ProcessorCode::HighPass(*channels as usize),
        GraphProcessor::Svf { channels, mode } => ProcessorCode::Svf(*channels as usize, *mode),
        GraphProcessor::Drive { channels } => ProcessorCode::Drive(*channels as usize),
        GraphProcessor::Mix { channels } => ProcessorCode::Mix(*channels as usize),
        GraphProcessor::Pan => ProcessorCode::Pan,
        GraphProcessor::Balance => ProcessorCode::Balance,
        GraphProcessor::Width => ProcessorCode::Width,
        GraphProcessor::Delay {
            channels,
            clock,
            max,
        } => ProcessorCode::Delay(
            *channels as usize,
            *clock == crate::graph::DelayClock::Score,
            crate::graph::delay_line_frames(max)
                .ok_or_else(|| RenderError::RenderState("delay line length overflowed".into()))?
                - 2,
        ),
    })
}

fn parameter_slot(node: &CompiledNode, name: &str) -> Option<usize> {
    node.declaration
        .params
        .iter()
        .position(|param| param.name == name)
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

/// Compile a program's public controls and bind each to the graph node
/// parameter it targets. `kind` names the program in errors.
#[allow(clippy::type_complexity)]
fn compile_controls(
    kind: &str,
    id: &str,
    declared: &BTreeMap<String, Control>,
    spec_of: &dyn Fn(&str) -> Option<ParameterSpec>,
    mut voice: Option<&mut CompiledGraph>,
    mut shared: Option<&mut CompiledGraph>,
) -> Result<(Vec<CompiledControl>, BTreeMap<String, usize>)> {
    let controls: Vec<CompiledControl> = declared
        .iter()
        .map(|(name, control)| {
            let spec = spec_of(name).ok_or_else(|| {
                RenderError::RenderState(format!(
                    "{kind} {id} control {name} has no parameter descriptor"
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

    for (name, control) in declared {
        let control_index = control_indices[name];
        let graph = match control.target.graph {
            GraphStage::Voice => voice.as_deref_mut(),
            GraphStage::Shared => shared.as_deref_mut(),
        }
        .ok_or_else(|| {
            RenderError::RenderState(format!(
                "{kind} {id} control {name} targets a missing shared graph"
            ))
        })?;
        let node = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == control.target.node)
            .ok_or_else(|| {
                RenderError::RenderState(format!(
                    "{kind} {id} control {name} targets a missing node"
                ))
            })?;
        let parameter = parameter_slot(node, &control.target.parameter).ok_or_else(|| {
            RenderError::RenderState(format!(
                "{kind} {id} control {name} targets a missing parameter"
            ))
        })?;
        node.controls.push(ControlBinding {
            control: control_index,
            parameter,
            rate: controls[control_index].rate,
        });
    }
    Ok((controls, control_indices))
}

fn resolve_controls(
    id: &str,
    controls: &[CompiledControl],
    indices: &BTreeMap<String, usize>,
    supplied: &BTreeMap<String, f64>,
) -> Result<Vec<f64>> {
    for name in supplied.keys() {
        if !indices.contains_key(name) {
            return Err(RenderError::RenderState(format!(
                "{id} has no public control {name}"
            )));
        }
    }
    controls
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
    id: &str,
    controls: &[CompiledControl],
    indices: &BTreeMap<String, usize>,
    supplied: &BTreeMap<String, f64>,
    resolved: &mut [f64],
) -> Result<bool> {
    if !supplied.is_empty() {
        for name in supplied.keys() {
            if !indices.contains_key(name) {
                return Err(RenderError::RenderState(format!(
                    "{id} has no public control {name}"
                )));
            }
        }
        for control in controls {
            let value = supplied
                .get(&control.name)
                .copied()
                .unwrap_or(control.default);
            validate_value(value, &control.bounds)?;
        }
    }
    let mut changed = false;
    for (index, control) in controls.iter().enumerate() {
        let value = if supplied.is_empty() {
            control.default
        } else {
            supplied
                .get(&control.name)
                .copied()
                .unwrap_or(control.default)
        };
        if resolved[index] != value {
            resolved[index] = value;
            changed = true;
        }
    }
    Ok(changed)
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

#[cfg(feature = "jit")]
mod jit_bridge {
    use super::*;
    use crate::jit::{
        compile_graph_jit, JitBlockContext, JitBoundSpec, JitCompiledGraph, JitControlBinding,
        JitErrorCallbacks, JitExpressionInput, JitGraphSpec, JitModSpec, JitNodeKind, JitNodeSpec,
        JitSource,
    };
    use std::ffi::c_void;

    fn map_source(source: &Source) -> JitSource {
        match source {
            Source::Input => JitSource::Input,
            Source::Node(index) => JitSource::Node(*index),
        }
    }

    fn node_kind(code: &ProcessorCode) -> JitNodeKind {
        match code {
            ProcessorCode::Mix(channels) => JitNodeKind::Mix {
                channels: *channels,
            },
            ProcessorCode::Gain(channels) => JitNodeKind::Gain {
                channels: *channels,
            },
            ProcessorCode::Width => JitNodeKind::Width,
            ProcessorCode::Timbre => JitNodeKind::Timbre,
            ProcessorCode::Pressure => JitNodeKind::Pressure,
            ProcessorCode::Key => JitNodeKind::Key,
            ProcessorCode::Noise(..) => JitNodeKind::Noise,
            ProcessorCode::Velocity | ProcessorCode::Random(_) => JitNodeKind::VelocityOrRandom,
            ProcessorCode::OnePole(channels) => JitNodeKind::OnePole {
                channels: *channels,
                highpass: false,
            },
            ProcessorCode::HighPass(channels) => JitNodeKind::OnePole {
                channels: *channels,
                highpass: true,
            },
            ProcessorCode::Svf(channels, mode) => JitNodeKind::Svf {
                channels: *channels,
                mode: match mode {
                    SvfMode::Lowpass => 0,
                    SvfMode::Bandpass => 1,
                    SvfMode::Highpass => 2,
                },
                slot_cutoff: slot::SVF_CUTOFF,
                slot_ratio: slot::SVF_RATIO,
                slot_q: slot::SVF_Q,
            },
            ProcessorCode::Pan => JitNodeKind::Pan,
            ProcessorCode::Balance => JitNodeKind::Balance,
            _ => JitNodeKind::Helper,
        }
    }

    fn inline_words_for(kind: JitNodeKind) -> usize {
        match kind {
            JitNodeKind::Noise => 1,
            JitNodeKind::VelocityOrRandom => 4,
            JitNodeKind::OnePole { .. } => 4,
            JitNodeKind::Svf { .. } => 10,
            _ => 0,
        }
    }

    fn eval_fn_for(code: &ProcessorCode) -> usize {
        match code {
            ProcessorCode::Pluck(_) => jit_eval_pluck as *const () as usize,
            ProcessorCode::Bow(..) => jit_eval_bow as *const () as usize,
            ProcessorCode::Modal(..) => jit_eval_modal as *const () as usize,
            ProcessorCode::Oscillator(_) => jit_eval_oscillator as *const () as usize,
            ProcessorCode::Wavetable(_) => jit_eval_wavetable as *const () as usize,
            ProcessorCode::Sample(_) => jit_eval_sample as *const () as usize,
            ProcessorCode::Adsr(_) => jit_eval_adsr as *const () as usize,
            ProcessorCode::Lfo => jit_eval_lfo as *const () as usize,
            ProcessorCode::Drive(_) => jit_eval_drive as *const () as usize,
            _ => jit_eval_fallback as *const () as usize,
        }
    }

    pub(super) fn compile_graph(graph: &CompiledGraph) -> Option<Arc<JitCompiledGraph>> {
        if !graph.delays.is_empty() || graph.nodes.len() > MAX_GRAPH_NODES {
            return None;
        }
        let mut inline_state_words = 0usize;
        let nodes = graph
            .nodes
            .iter()
            .map(|node| {
                let kind = node_kind(&node.processor);
                let inline_state_offset = inline_state_words;
                inline_state_words += inline_words_for(kind);
                let incoming = node.incoming.iter().map(map_source).collect();
                let mod_sources = node
                    .modulations
                    .iter()
                    .filter(|m| m.rate == ParameterRate::Sample)
                    .map(|m| map_source(&m.source))
                    .collect();
                let sample_controls = node
                    .controls
                    .iter()
                    .filter(|c| c.rate == ParameterRate::Sample)
                    .map(|c| JitControlBinding {
                        parameter: c.parameter,
                        control: c.control,
                    })
                    .collect();
                let sample_mods = node
                    .modulations
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.rate == ParameterRate::Sample)
                    .map(|(mod_index, m)| JitModSpec {
                        mod_index,
                        parameter: m.parameter,
                        source: map_source(&m.source),
                        depth: m.depth,
                    })
                    .collect();
                let bounds = node
                    .specs
                    .iter()
                    .enumerate()
                    .map(|(parameter, b)| JitBoundSpec {
                        parameter,
                        min: b.min,
                        max: b.max,
                        min_open: b.min_open,
                        max_open: b.max_open,
                    })
                    .collect();
                JitNodeSpec {
                    kind,
                    has_sample_modulations: node.has_sample_modulations,
                    reads_input_later: node.reads_input_later,
                    incoming,
                    mod_sources,
                    base_params: node.base_params.clone(),
                    sample_controls,
                    sample_mods,
                    bounds,
                    inline_state_offset,
                    eval_fn: eval_fn_for(&node.processor),
                    setup_static_fn: jit_setup_static_node as *const () as usize,
                }
            })
            .collect();
        let spec = JitGraphSpec {
            order: graph.order.clone(),
            nodes,
            output_node: graph.output,
            inline_state_words,
        };
        let errors = JitErrorCallbacks {
            nonfinite_incoming: jit_nonfinite_incoming,
            nonfinite_output: jit_nonfinite_output,
            mod_product_nonfinite: jit_mod_product_nonfinite,
            mod_sum_nonfinite: jit_mod_sum_nonfinite,
            param_out_of_range: jit_param_out_of_range,
            onepole_invalid: jit_onepole_invalid,
            svf_cutoff_invalid: jit_svf_cutoff_invalid,
            svf_q_invalid: jit_svf_q_invalid,
        };
        compile_graph_jit(&spec, errors)
    }

    fn pack_inline_state(graph: &CompiledGraph, nodes: &[RuntimeNode], buf: &mut [u64]) {
        let mut off = 0usize;
        for (comp, rt) in graph.nodes.iter().zip(nodes.iter()) {
            match (&comp.processor, &rt.state) {
                (ProcessorCode::Noise(..), ProcessorState::Noise(s)) => {
                    buf[off] = u64::from(*s);
                    off += 1;
                }
                (
                    ProcessorCode::Velocity | ProcessorCode::Random(_),
                    ProcessorState::Velocity(v, glide) | ProcessorState::Random(v, glide),
                ) => {
                    buf[off] = v.to_bits();
                    if let Some((from, start)) = glide {
                        buf[off + 1] = 1;
                        buf[off + 2] = from.to_bits();
                        buf[off + 3] = *start;
                    } else {
                        buf[off + 1] = 0;
                    }
                    off += 4;
                }
                (
                    ProcessorCode::OnePole(_) | ProcessorCode::HighPass(_),
                    ProcessorState::OnePole(prev),
                ) => {
                    buf[off] = prev[0].to_bits();
                    buf[off + 1] = prev[1].to_bits();
                    buf[off + 2] = 0; // uncached cutoff
                    buf[off + 3] = 0;
                    off += 4;
                }
                (ProcessorCode::Svf(..), ProcessorState::Svf(states)) => {
                    buf[off] = states[0][0].to_bits();
                    buf[off + 1] = states[0][1].to_bits();
                    buf[off + 2] = states[1][0].to_bits();
                    buf[off + 3] = states[1][1].to_bits();
                    buf[off + 4] = 0; // uncached cutoff
                    buf[off + 5] = 0; // uncached q
                    off += 10;
                }
                _ => {}
            }
        }
    }

    fn unpack_inline_state(graph: &CompiledGraph, nodes: &mut [RuntimeNode], buf: &[u64]) {
        let mut off = 0usize;
        for (comp, rt) in graph.nodes.iter().zip(nodes.iter_mut()) {
            match (&comp.processor, &mut rt.state) {
                (ProcessorCode::Noise(..), ProcessorState::Noise(s)) => {
                    *s = buf[off] as u32;
                    off += 1;
                }
                (
                    ProcessorCode::Velocity | ProcessorCode::Random(_),
                    ProcessorState::Velocity(..) | ProcessorState::Random(..),
                ) => {
                    off += 4;
                }
                (
                    ProcessorCode::OnePole(_) | ProcessorCode::HighPass(_),
                    ProcessorState::OnePole(prev),
                ) => {
                    prev[0] = f64::from_bits(buf[off]);
                    prev[1] = f64::from_bits(buf[off + 1]);
                    off += 4;
                }
                (ProcessorCode::Svf(..), ProcessorState::Svf(states)) => {
                    states[0][0] = f64::from_bits(buf[off]);
                    states[0][1] = f64::from_bits(buf[off + 1]);
                    states[1][0] = f64::from_bits(buf[off + 2]);
                    states[1][1] = f64::from_bits(buf[off + 3]);
                    off += 10;
                }
                _ => {}
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_block(
        jit: &JitCompiledGraph,
        graph: &CompiledGraph,
        nodes: &mut [RuntimeNode],
        controls: &[f64],
        settled: bool,
        start: u64,
        frames: usize,
        rate: f64,
        expressions: &[[f64; 3]],
        input: Option<&[[f64; 2]]>,
        output: &mut [[f64; 2]],
    ) -> (usize, Option<RenderError>) {
        let mut stack_inline = [0u64; 256];
        let mut heap_inline = Vec::new();
        let inline_buf = if jit.inline_state_words <= stack_inline.len() {
            &mut stack_inline[..jit.inline_state_words]
        } else {
            heap_inline.resize(jit.inline_state_words, 0);
            &mut heap_inline[..]
        };
        pack_inline_state(graph, nodes, inline_buf);

        let mut runtime_ptrs = [std::ptr::null_mut::<c_void>(); MAX_GRAPH_NODES];
        let mut params_ptrs = [std::ptr::null_mut::<f64>(); MAX_GRAPH_NODES];
        let mut compiled_ptrs = [std::ptr::null::<c_void>(); MAX_GRAPH_NODES];
        for (i, (rt, comp)) in nodes.iter_mut().zip(graph.nodes.iter()).enumerate() {
            params_ptrs[i] = rt.params.as_mut_ptr();
            runtime_ptrs[i] = (rt as *mut RuntimeNode).cast::<c_void>();
            compiled_ptrs[i] = (comp as *const CompiledNode).cast::<c_void>();
        }
        let mut error_out: Option<RenderError> = None;
        let mut ctx = JitBlockContext {
            runtime_nodes_ptr: runtime_ptrs.as_ptr(),
            runtime_params_ptr: params_ptrs.as_ptr(),
            inline_state_ptr: inline_buf.as_mut_ptr(),
            compiled_nodes_ptr: compiled_ptrs.as_ptr(),
            controls_ptr: controls.as_ptr(),
            controls_len: controls.len(),
            expressions_ptr: expressions.as_ptr().cast::<JitExpressionInput>(),
            input_ptr: input.map_or(std::ptr::null(), |s| s.as_ptr()),
            output_ptr: output.as_mut_ptr(),
            error_out: (&mut error_out as *mut Option<RenderError>).cast::<c_void>(),
            start_frame: start,
            frames,
            rate,
            settled: u8::from(settled),
        };
        let completed = unsafe { jit.call(&mut ctx) };
        unpack_inline_state(graph, nodes, inline_buf);
        (completed, error_out)
    }

    unsafe extern "C" fn jit_nonfinite_incoming(
        _compiled_node: *const c_void,
        error_out: *mut c_void,
    ) {
        let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
        *slot = Some(RenderError::Nonfinite(
            "instrument graph input sum is nonfinite".into(),
        ));
    }

    unsafe extern "C" fn jit_nonfinite_output(
        compiled_node: *const c_void,
        error_out: *mut c_void,
    ) {
        let compiled = unsafe { &*compiled_node.cast::<CompiledNode>() };
        let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
        *slot = Some(RenderError::Nonfinite(format!(
            "instrument graph node {} produced a nonfinite sample",
            compiled.id
        )));
    }

    unsafe extern "C" fn jit_mod_product_nonfinite(
        compiled_node: *const c_void,
        index: usize,
        _value: f64,
        error_out: *mut c_void,
    ) {
        let compiled = unsafe { &*compiled_node.cast::<CompiledNode>() };
        let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
        *slot = Some(RenderError::Nonfinite(format!(
            "modulation {} product is nonfinite",
            compiled.modulations[index].id
        )));
    }

    unsafe extern "C" fn jit_mod_sum_nonfinite(
        compiled_node: *const c_void,
        index: usize,
        _value: f64,
        error_out: *mut c_void,
    ) {
        let compiled = unsafe { &*compiled_node.cast::<CompiledNode>() };
        let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
        *slot = Some(RenderError::Nonfinite(format!(
            "modulation {} sum is nonfinite",
            compiled.modulations[index].id
        )));
    }

    unsafe extern "C" fn jit_param_out_of_range(
        compiled_node: *const c_void,
        index: usize,
        value: f64,
        error_out: *mut c_void,
    ) {
        let compiled = unsafe { &*compiled_node.cast::<CompiledNode>() };
        let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
        *slot = Some(RenderError::Nonfinite(format!(
            "{} value {value} is outside its finite range",
            compiled.specs[index].context
        )));
    }

    unsafe extern "C" fn jit_onepole_invalid(
        _compiled_node: *const c_void,
        error_out: *mut c_void,
    ) {
        let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
        *slot = Some(RenderError::Nonfinite(
            "one-pole input, state, or parameter is invalid".into(),
        ));
    }

    unsafe extern "C" fn jit_svf_cutoff_invalid(
        compiled_node: *const c_void,
        cutoff: f64,
        rate: f64,
        error_out: *mut c_void,
    ) {
        let compiled = unsafe { &*compiled_node.cast::<CompiledNode>() };
        let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
        *slot = Some(RenderError::Nonfinite(format!(
            "state-variable filter {} cutoff {cutoff} Hz must be strictly within 0..{} Hz",
            compiled.id,
            rate / 2.0
        )));
    }

    unsafe extern "C" fn jit_svf_q_invalid(
        _compiled_node: *const c_void,
        q: f64,
        _rate: f64,
        error_out: *mut c_void,
    ) {
        let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
        *slot = Some(RenderError::Nonfinite(format!(
            "state-variable filter q {q} must be finite and positive"
        )));
    }

    #[inline(always)]
    unsafe fn write_result(
        compiled: &CompiledNode,
        res: Result<[f64; 2]>,
        out_l: *mut f64,
        out_r: *mut f64,
        error_out: *mut c_void,
    ) -> u8 {
        match res {
            Ok(output) => {
                if !output[0].is_finite() || !output[1].is_finite() {
                    let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
                    *slot = Some(RenderError::Nonfinite(format!(
                        "instrument graph node {} produced a nonfinite sample",
                        compiled.id
                    )));
                    return 1;
                }
                unsafe {
                    *out_l = output[0];
                    *out_r = output[1];
                }
                0
            }
            Err(failure) => {
                let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
                *slot = Some(failure);
                1
            }
        }
    }

    unsafe extern "C" fn jit_setup_static_node(
        compiled_node: *const c_void,
        runtime_node: *mut c_void,
        controls_ptr: *const f64,
        controls_len: usize,
        error_out: *mut c_void,
    ) -> u8 {
        let compiled = unsafe { &*compiled_node.cast::<CompiledNode>() };
        let runtime = unsafe { &mut *runtime_node.cast::<RuntimeNode>() };
        let controls = if controls_len == 0 || controls_ptr.is_null() {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(controls_ptr, controls_len) }
        };
        runtime.params.clone_from_slice(&compiled.base_params);
        apply_controls(
            compiled,
            controls,
            ParameterRate::Sample,
            &mut runtime.params,
        );
        if let Err(failure) = validate_params(compiled, &runtime.params) {
            let slot = unsafe { &mut *error_out.cast::<Option<RenderError>>() };
            *slot = Some(failure);
            1
        } else {
            0
        }
    }

    macro_rules! def_eval_fn {
        ($name:ident, |$compiled:ident, $runtime:ident, $frame:ident, $rate:ident, $pitch:ident, $timbre:ident, $pressure:ident, $in_l:ident, $in_r:ident| $body:block) => {
            unsafe extern "C" fn $name(
                compiled_node: *const c_void,
                runtime_node: *mut c_void,
                $frame: u64,
                $rate: f64,
                $pitch: f64,
                $timbre: f64,
                $pressure: f64,
                $in_l: f64,
                $in_r: f64,
                out_l: *mut f64,
                out_r: *mut f64,
                error_out: *mut c_void,
            ) -> u8 {
                let $compiled = unsafe { &*compiled_node.cast::<CompiledNode>() };
                let $runtime = unsafe { &mut *runtime_node.cast::<RuntimeNode>() };
                #[allow(unused_mut)]
                let mut eval_inner = || -> Result<[f64; 2]> { $body };
                let res = eval_inner();
                unsafe { write_result($compiled, res, out_l, out_r, error_out) }
            }
        };
    }

    def_eval_fn!(
        jit_eval_oscillator,
        |compiled, runtime, _frame, rate, pitch_hz, _timbre, _pressure, _in_l, _in_r| {
            if let ProcessorState::Oscillator(osc) = &mut runtime.state {
                let frequency = pitch_hz * runtime.params[slot::OSCILLATOR_RATIO]
                    + runtime.params[slot::OSCILLATOR_FREQUENCY];
                validate_frequency(frequency)?;
                Ok([
                    osc.sample(frequency, rate)? * runtime.params[slot::OSCILLATOR_LEVEL],
                    0.0,
                ])
            } else {
                Err(RenderError::RenderState(format!(
                    "instrument graph node {} has mismatched compiled state",
                    compiled.id
                )))
            }
        }
    );

    def_eval_fn!(jit_eval_lfo, |compiled,
                                runtime,
                                _frame,
                                rate,
                                _pitch,
                                _timbre,
                                _pressure,
                                _in_l,
                                _in_r| {
        if let ProcessorState::Lfo(osc) = &mut runtime.state {
            Ok([
                osc.sample(runtime.params[slot::LFO_FREQUENCY], rate)?
                    * runtime.params[slot::LFO_LEVEL],
                0.0,
            ])
        } else {
            Err(RenderError::RenderState(format!(
                "instrument graph node {} has mismatched compiled state",
                compiled.id
            )))
        }
    });

    def_eval_fn!(jit_eval_adsr, |compiled,
                                 runtime,
                                 frame,
                                 rate,
                                 _pitch,
                                 _timbre,
                                 _pressure,
                                 _in_l,
                                 _in_r| {
        if let ProcessorState::Adsr(env) = &runtime.state {
            Ok([env.value(frame, rate)?, 0.0])
        } else {
            Err(RenderError::RenderState(format!(
                "instrument graph node {} has mismatched compiled state",
                compiled.id
            )))
        }
    });

    def_eval_fn!(jit_eval_drive, |compiled,
                                  runtime,
                                  _frame,
                                  _rate,
                                  _pitch,
                                  _timbre,
                                  _pressure,
                                  in_l,
                                  in_r| {
        if let (ProcessorCode::Drive(channels), ProcessorState::Drive(states)) =
            (&compiled.processor, &mut runtime.state)
        {
            if runtime.params[slot::DRIVE_REFERENCE] > 0.0
                && runtime.params[slot::DRIVE_DRIVE] == 0.0
            {
                return Err(RenderError::Nonfinite(format!(
                    "drive node {} needs a positive drive when reference is set",
                    compiled.id
                )));
            }
            let mut out = [0.0; 2];
            let audio_input = [in_l, in_r];
            for ch in 0..*channels {
                let (value, after) = states[ch].sample(
                    audio_input[ch],
                    runtime.params[slot::DRIVE_DRIVE],
                    runtime.params[slot::DRIVE_BIAS],
                    runtime.params[slot::DRIVE_LEVEL],
                    runtime.params[slot::DRIVE_REFERENCE],
                );
                out[ch] = value;
                states[ch] = after;
            }
            Ok(out)
        } else {
            Err(RenderError::RenderState(format!(
                "instrument graph node {} has mismatched compiled state",
                compiled.id
            )))
        }
    });

    def_eval_fn!(
        jit_eval_wavetable,
        |compiled, runtime, _frame, rate, pitch_hz, _timbre, _pressure, _in_l, _in_r| {
            if let (ProcessorCode::Wavetable(table), ProcessorState::Wavetable { phase }) =
                (&compiled.processor, &mut runtime.state)
            {
                let frequency = pitch_hz * runtime.params[slot::WAVETABLE_RATIO]
                    + runtime.params[slot::WAVETABLE_FREQUENCY];
                validate_frequency(frequency)?;
                let out = table.sample(
                    *phase,
                    runtime.params[slot::WAVETABLE_POSITION],
                    frequency,
                    rate,
                )? * runtime.params[slot::WAVETABLE_LEVEL];
                *phase = (*phase + frequency / rate).rem_euclid(1.0);
                Ok([out, 0.0])
            } else {
                Err(RenderError::RenderState(format!(
                    "instrument graph node {} has mismatched compiled state",
                    compiled.id
                )))
            }
        }
    );

    def_eval_fn!(jit_eval_pluck, |compiled,
                                  runtime,
                                  _frame,
                                  _rate,
                                  pitch_hz,
                                  _timbre,
                                  _pressure,
                                  _in_l,
                                  _in_r| {
        if let ProcessorState::Pluck(pluck) = &mut runtime.state {
            Ok([
                pluck.sample(
                    pitch_hz * runtime.params[slot::PLUCK_RATIO],
                    runtime.params[slot::PLUCK_DECAY],
                    runtime.params[slot::PLUCK_DAMPING],
                    runtime.params[slot::PLUCK_LEVEL],
                    runtime.params[slot::PLUCK_PICKUP],
                )?,
                0.0,
            ])
        } else {
            Err(RenderError::RenderState(format!(
                "instrument graph node {} has mismatched compiled state",
                compiled.id
            )))
        }
    });

    def_eval_fn!(jit_eval_modal, |compiled,
                                  runtime,
                                  _frame,
                                  _rate,
                                  pitch_hz,
                                  _timbre,
                                  _pressure,
                                  in_l,
                                  _in_r| {
        if let ProcessorState::Modal(bank) = &mut runtime.state {
            let shape = crate::modal::ModalShape {
                frequency_hz: pitch_hz * runtime.params[slot::MODAL_RATIO],
                stretch: runtime.params[slot::MODAL_STRETCH],
                decay: runtime.params[slot::MODAL_DECAY],
                damping: runtime.params[slot::MODAL_DAMPING],
                position: runtime.params[slot::MODAL_POSITION],
                detune_cents: runtime.params[slot::MODAL_DETUNE],
            };
            Ok([
                bank.sample(&shape, runtime.params[slot::MODAL_LEVEL], in_l)?,
                0.0,
            ])
        } else {
            Err(RenderError::RenderState(format!(
                "instrument graph node {} has mismatched compiled state",
                compiled.id
            )))
        }
    });

    def_eval_fn!(jit_eval_bow, |compiled,
                                runtime,
                                frame,
                                _rate,
                                pitch_hz,
                                _timbre,
                                _pressure,
                                _in_l,
                                _in_r| {
        if let ProcessorState::Bow(bow) = &mut runtime.state {
            let params = crate::bow::BowParams {
                ratio: runtime.params[slot::BOW_RATIO],
                dynamics: runtime.params[slot::BOW_DYNAMICS],
                position: runtime.params[slot::BOW_POSITION],
                vibrato: runtime.params[slot::BOW_VIBRATO],
                rate: runtime.params[slot::BOW_RATE],
                attack: runtime.params[slot::BOW_ATTACK],
                timing: runtime.params[slot::BOW_TIMING],
                spread: runtime.params[slot::BOW_SPREAD],
                width: runtime.params[slot::BOW_WIDTH],
                pan: runtime.params[slot::BOW_PAN],
                level: runtime.params[slot::BOW_LEVEL],
            };
            bow.sample(frame, &params, pitch_hz)
        } else {
            Err(RenderError::RenderState(format!(
                "instrument graph node {} has mismatched compiled state",
                compiled.id
            )))
        }
    });

    def_eval_fn!(jit_eval_sample, |compiled,
                                   runtime,
                                   _frame,
                                   rate,
                                   pitch_hz,
                                   _timbre,
                                   _pressure,
                                   _in_l,
                                   _in_r| {
        process_graph_node(
            compiled,
            &mut runtime.state,
            &runtime.params,
            0,
            rate,
            pitch_hz,
            0.0,
            0.0,
            [0.0, 0.0],
            true,
        )
    });

    def_eval_fn!(
        jit_eval_fallback,
        |compiled, runtime, frame, rate, pitch_hz, timbre, pressure, in_l, in_r| {
            process_graph_node(
                compiled,
                &mut runtime.state,
                &runtime.params,
                frame,
                rate,
                pitch_hz,
                timbre,
                pressure,
                [in_l, in_r],
                true,
            )
        }
    );
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
                        processor: GraphProcessor::ADSR,
                        params: BTreeMap::new(),
                    },
                    GraphNode {
                        id: "noise".into(),
                        processor: GraphProcessor::Noise {
                            seed: 1,
                            per_note: false,
                        },
                        params: BTreeMap::new(),
                    },
                ],
                connections: Vec::new(),
                modulations: Vec::new(),
                output: crate::plan::PortRef::new("noise", "out").unwrap(),
                amplitude: Some("amp".into()),
                connected: false,
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
            processor: GraphProcessor::ADSR,
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
                connected: false,
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
                        processor: GraphProcessor::ADSR,
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
                connected: false,
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
            processor: GraphProcessor::ADSR,
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
                connected: false,
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
                        processor: GraphProcessor::ADSR,
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
                connected: false,
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
            connected: false,
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
            connected: false,
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
                expected.sample(480.0, 3.0, 0.5, 1.0, 0.0).unwrap();
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
                actual.sample(480.0, 3.0, 0.5, 1.0, 0.0).unwrap(),
                expected.sample(480.0, 3.0, 0.5, 1.0, 0.0).unwrap()
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
        play_with(compiled, frames, ahead, false)
    }

    /// As `play`; `expressive` gives every note a timbre and a pressure ramp
    /// from 0 to 1 over 4800 frames, which `play` leaves at zero.
    fn play_with(
        compiled: &Arc<CompiledInstrument>,
        frames: u64,
        ahead: bool,
        expressive: bool,
    ) -> (Vec<Vec<f64>>, Option<(u64, RenderError)>, usize) {
        use crate::plan::{
            ExpressionClock, Interpolation, PressureExpressionPoint, TimbreExpressionPoint,
        };
        let ramp = |a: i64, b: i64| -> Vec<(crate::Rational, crate::Rational, Interpolation)> {
            vec![
                (
                    crate::Rational::from_integer(0.into()),
                    crate::Rational::from_integer(a.into()),
                    Interpolation::Linear,
                ),
                (
                    crate::Rational::from_integer(1.into()),
                    crate::Rational::from_integer(b.into()),
                    Interpolation::Step,
                ),
            ]
        };
        let expressions = |gate: u64| {
            let timbre = TimbreExpression {
                clock: ExpressionClock::Normalized,
                points: ramp(0, 1)
                    .into_iter()
                    .map(|(position, value, shape)| TimbreExpressionPoint {
                        position,
                        value,
                        shape,
                    })
                    .collect(),
            };
            let pressure = PressureExpression {
                clock: ExpressionClock::Normalized,
                points: ramp(1, 0)
                    .into_iter()
                    .map(|(position, value, shape)| PressureExpressionPoint {
                        position,
                        value,
                        shape,
                    })
                    .collect(),
            };
            let per_frame = crate::Rational::new(1.into(), 4800.into());
            (
                ExpressionRuntime::new(Arc::new(timbre), per_frame.clone(), gate),
                ExpressionRuntime::new(Arc::new(pressure), per_frame, gate),
            )
        };
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
                    if expressive {
                        let (timbre, pressure) = expressions(4800);
                        runtime
                            .note_on_with_expressions(
                                address,
                                pitch,
                                velocity,
                                frame,
                                None,
                                None,
                                Some(timbre),
                                Some(pressure),
                                None,
                                0,
                            )
                            .unwrap();
                    } else {
                        runtime.note_on(address, pitch, velocity, frame).unwrap();
                    }
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

    /// The JIT kernel must reproduce the interpreter bit for bit, including the
    /// frame and message of the first error.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_output_is_bit_identical_to_the_interpreter() {
        let builtin = |name: &str| {
            format!(
                r#"maac 1;
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &keys:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import studio {{ builtin = "std/studio/1.2.0"; }}
node keys {{ instrument = &studio.{name}; }}
"#
            )
        };
        let mut compiled_with_jit = 0;
        for name in [
            "electric_piano",
            "piano",
            "bass",
            "pad",
            "electric_guitar",
            "kick",
            "snare",
            "closed_hat",
            "open_hat",
            "crash",
            "clap",
            "low_tom",
            "high_tom",
        ] {
            let source = builtin(name);
            let jit = program(&source, name);
            JIT_DISABLED.with(|flag| flag.set(true));
            let interpreter = program(&source, name);
            JIT_DISABLED.with(|flag| flag.set(false));
            assert!(interpreter.voice.jit.is_none());
            // A graph that stops compiling falls back silently, so require it.
            // Graphs with delay lines are never compiled and would be listed here.
            assert!(
                jit.voice.jit.is_some(),
                "{name}: voice graph not JIT-compiled"
            );
            if let Some(shared) = &jit.shared {
                assert!(
                    shared.jit.is_some(),
                    "{name}: shared graph not JIT-compiled"
                );
            }
            compiled_with_jit += 1;
            for ahead in [false, true] {
                let (expected, expected_error, _) = play(&interpreter, 12_000, ahead);
                let (actual, actual_error, _) = play(&jit, 12_000, ahead);
                assert!(expected_error.is_none(), "{name}: {expected_error:?}");
                assert_eq!(expected_error, actual_error, "{name} ahead={ahead}");
                assert_eq!(expected.len(), actual.len(), "{name} ahead={ahead}");
                for (frame, (e, a)) in expected.iter().zip(&actual).enumerate() {
                    let bits = |f: &Vec<f64>| f.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
                    assert_eq!(bits(e), bits(a), "{name} ahead={ahead} frame {frame}");
                }
            }
        }
        assert_eq!(compiled_with_jit, 13);
    }

    #[cfg(feature = "jit")]
    fn bits_of(frames: &[Vec<f64>]) -> Vec<Vec<u64>> {
        frames
            .iter()
            .map(|frame| frame.iter().map(|x| x.to_bits()).collect())
            .collect()
    }

    /// Inline node kinds that no studio patch reaches: Random, Timbre,
    /// Pressure, Balance and Width, each driving a sample-rate parameter.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_matches_the_interpreter_on_the_remaining_inline_nodes() {
        let source = r#"maac 1;
project p { score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
instrument wide { channels = 2;
  voice v { channels = 2; amplitude = &amp; output = &spread:out;
    node amp { type = "synth.adsr/1"; params = { sustain = 1; release = 50ms; }; }
    node osc { type = "synth.saw/1"; }
    node dice { type = "synth.random/1"; }
    node bright { type = "synth.timbre/1"; }
    node press { type = "synth.pressure/1"; }
    node place { type = "synth.pan/1"; }
    node lean { type = "synth.balance/1"; }
    node spread { type = "synth.width/1"; }
    connect osc_place { from = &osc:out; to = &place:in; }
    connect place_lean { from = &place:out; to = &lean:in; }
    connect lean_spread { from = &lean:out; to = &spread:in; }
    modulate m1 { from = &dice:out; to = &lean.params.balance; depth = 0.5; }
    modulate m2 { from = &bright:out; to = &spread.params.width; depth = 1; }
    modulate m3 { from = &press:out; to = &place.params.pan; depth = 0.5; }
  }
}
node sound { instrument = &wide; }
"#;
        let jit = program(source, "wide");
        JIT_DISABLED.with(|flag| flag.set(true));
        let interpreter = program(source, "wide");
        JIT_DISABLED.with(|flag| flag.set(false));
        assert!(
            jit.voice.jit.is_some(),
            "the wide graph was not JIT-compiled"
        );
        for ahead in [false, true] {
            let (expected, expected_error, _) = play_with(&interpreter, 12_000, ahead, true);
            let (actual, actual_error, _) = play_with(&jit, 12_000, ahead, true);
            assert!(expected_error.is_none(), "{expected_error:?}");
            assert_eq!(expected_error, actual_error);
            assert_eq!(bits_of(&expected), bits_of(&actual), "ahead={ahead}");
        }
    }

    /// Splitmix64, so the fuzz below is the same on every run and platform.
    #[cfg(feature = "jit")]
    struct Rng(u64);

    #[cfg(feature = "jit")]
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// A value for a control, biased to the edges the kernels branch on:
    /// both bounds, just inside them, zero of either sign, the smallest
    /// subnormal and the default. Only values the control accepts are returned.
    #[cfg(feature = "jit")]
    fn extreme_control(control: &CompiledControl, rng: &mut Rng) -> f64 {
        let bounds = &control.bounds;
        let span = bounds.max - bounds.min;
        let candidates = [
            control.default,
            bounds.min,
            bounds.max,
            next_up(bounds.min),
            next_down(bounds.max),
            0.0,
            -0.0,
            f64::from_bits(1),
            -f64::from_bits(1),
            f64::MIN_POSITIVE / 2.0,
            bounds.min + span * rng.unit(),
            bounds.min + span * rng.unit() * 1e-9,
        ];
        let value = candidates[rng.below(candidates.len())];
        if validate_value(value, bounds).is_ok() {
            value
        } else {
            control.default
        }
    }

    #[cfg(feature = "jit")]
    fn next_up(x: f64) -> f64 {
        if x.is_finite() {
            f64::from_bits(if x >= 0.0 {
                x.to_bits() + 1
            } else {
                x.to_bits() - 1
            })
        } else {
            x
        }
    }

    #[cfg(feature = "jit")]
    fn next_down(x: f64) -> f64 {
        -next_up(-x)
    }

    /// Plays notes at extreme pitches and velocities under extreme controls,
    /// switching controls at note boundaries. Returns every frame or the first
    /// error with its frame.
    #[cfg(feature = "jit")]
    fn play_fuzz(
        compiled: &Arc<CompiledInstrument>,
        seed: u64,
        frames: u64,
        ahead: bool,
    ) -> (Vec<Vec<f64>>, Option<(u64, RenderError)>) {
        let mut rng = Rng(seed);
        let pitches = [
            1e-300,
            f64::MIN_POSITIVE,
            0.5,
            27.5,
            220.0,
            4186.0,
            11_999.0,
            23_999.9,
            24_000.0,
            30_000.0,
        ];
        let velocities = [0.0, -0.0, f64::from_bits(1), 0.5, 1.0];
        // Event frames, sorted and distinct, so blocks end where they do.
        let mut at: Vec<u64> = (0..6).map(|_| rng.next() % (frames / 2)).collect();
        at.sort_unstable();
        at.dedup();
        struct Event {
            frame: u64,
            address: String,
            on: Option<(f64, f64)>,
            controls: BTreeMap<String, f64>,
        }
        let mut events = Vec::new();
        for (i, &frame) in at.iter().enumerate() {
            let controls = compiled
                .controls
                .iter()
                .map(|control| (control.name.clone(), extreme_control(control, &mut rng)))
                .collect();
            events.push(Event {
                frame,
                address: format!("n{i}"),
                on: Some((
                    pitches[rng.below(pitches.len())],
                    velocities[rng.below(velocities.len())],
                )),
                controls,
            });
            events.push(Event {
                frame: frame + 1 + rng.next() % (frames / 3),
                address: format!("n{i}"),
                on: None,
                controls: BTreeMap::new(),
            });
        }
        events.sort_by_key(|event| (event.frame, event.on.is_none()));
        let event_frames: Vec<u64> = {
            let mut frames: Vec<u64> = events.iter().map(|event| event.frame).collect();
            frames.dedup();
            frames
        };
        let mut runtime =
            InstrumentRuntime::new(Arc::clone(compiled), 8, 48000.0, &BTreeMap::new()).unwrap();
        let mut rendered = Vec::new();
        for frame in 0..frames {
            for event in events.iter().filter(|event| event.frame == frame) {
                let outcome = match event.on {
                    None => runtime.note_off(&event.address, frame),
                    Some((pitch, velocity)) => {
                        let controls = runtime.update_controls(&event.controls);
                        controls.and_then(|()| {
                            runtime.note_on(event.address.clone(), pitch, velocity, frame)
                        })
                    }
                };
                if let Err(error) = outcome {
                    return (rendered, Some((frame, error)));
                }
            }
            if let Err(error) = runtime.prune_finished(frame) {
                return (rendered, Some((frame, error)));
            }
            let horizon = if ahead {
                event_frames
                    .iter()
                    .copied()
                    .find(|&event| event > frame)
                    .unwrap_or(frames)
            } else {
                frame + 1
            };
            match runtime.render_until(frame, horizon) {
                Ok(output) => rendered.push(output.to_vec()),
                Err(error) => return (rendered, Some((frame, error))),
            }
        }
        (rendered, None)
    }

    /// Seeded fuzz of extreme pitches, velocities and control values: the JIT
    /// must match the interpreter on every bit and on the first error.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_matches_the_interpreter_on_extreme_inputs() {
        let studio = |name: &str| {
            format!(
                r#"maac 1;
project p {{ score = [0q, 1q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &keys:out; }}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import studio {{ builtin = "std/studio/1.2.0"; }}
node keys {{ instrument = &studio.{name}; }}
"#
            )
        };
        let mut errors = 0;
        let mut runs = 0;
        for name in [
            "electric_piano",
            "piano",
            "bass",
            "pad",
            "electric_guitar",
            "kick",
            "snare",
            "closed_hat",
            "open_hat",
            "crash",
            "clap",
            "low_tom",
            "high_tom",
        ] {
            let source = studio(name);
            let jit = program(&source, name);
            JIT_DISABLED.with(|flag| flag.set(true));
            let interpreter = program(&source, name);
            JIT_DISABLED.with(|flag| flag.set(false));
            for seed in 0..10u64 {
                for ahead in [false, true] {
                    let (expected, expected_error) = play_fuzz(&interpreter, seed, 9_000, ahead);
                    let (actual, actual_error) = play_fuzz(&jit, seed, 9_000, ahead);
                    runs += 1;
                    errors += usize::from(expected_error.is_some());
                    assert_eq!(
                        expected_error, actual_error,
                        "{name} seed {seed} ahead={ahead}"
                    );
                    assert_eq!(
                        bits_of(&expected),
                        bits_of(&actual),
                        "{name} seed {seed} ahead={ahead}"
                    );
                }
            }
        }
        // Both outcomes must be exercised, or the fuzz proves little.
        assert!(
            errors > 0 && errors < runs,
            "{errors} of {runs} runs failed"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn jit_reports_the_interpreters_first_error() {
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
        let jit = program(source, "swept");
        JIT_DISABLED.with(|flag| flag.set(true));
        let interpreter = program(source, "swept");
        JIT_DISABLED.with(|flag| flag.set(false));
        assert!(
            jit.voice.jit.is_some(),
            "the sweep graph was not JIT-compiled"
        );
        let (expected, expected_error, _) = play(&interpreter, 20_000, false);
        let (actual, actual_error, _) = play(&jit, 20_000, false);
        assert!(expected_error.is_some());
        assert_eq!(expected_error, actual_error);
        assert_eq!(bits_of(&expected), bits_of(&actual));
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

#[cfg(all(feature = "jit", test))]
thread_local! {
    /// Lets a test compile the same instrument for the interpreter alone.
    static JIT_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(feature = "jit")]
fn jit_disabled_for_test() -> bool {
    #[cfg(test)]
    {
        JIT_DISABLED.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Each kernel's parameter slots, by name, from the declarations.
mod slot {
    use crate::processor::{
        ADSR, BALANCE as BALANCE_DECLARATION, BOW, DELAY_SCORE, DELAY_SECONDS, DRIVE, GAIN,
        HIGHPASS, LFO, MODAL, NOISE, ONEPOLE, PAN as PAN_DECLARATION, PLUCK, SAMPLE, SINE, SVF,
        WAVETABLE, WIDTH as WIDTH_DECLARATION,
    };

    // The fixed waveforms share their parameters.
    pub(super) const OSCILLATOR_RATIO: usize = SINE.slot("ratio");
    pub(super) const OSCILLATOR_FREQUENCY: usize = SINE.slot("frequency");
    pub(super) const OSCILLATOR_PHASE: usize = SINE.slot("phase");
    pub(super) const OSCILLATOR_LEVEL: usize = SINE.slot("level");
    pub(super) const WAVETABLE_RATIO: usize = WAVETABLE.slot("ratio");
    pub(super) const WAVETABLE_FREQUENCY: usize = WAVETABLE.slot("frequency");
    pub(super) const WAVETABLE_PHASE: usize = WAVETABLE.slot("phase");
    pub(super) const WAVETABLE_LEVEL: usize = WAVETABLE.slot("level");
    pub(super) const WAVETABLE_POSITION: usize = WAVETABLE.slot("position");
    pub(super) const SAMPLE_RATIO: usize = SAMPLE.slot("ratio");
    pub(super) const SAMPLE_FREQUENCY: usize = SAMPLE.slot("frequency");
    pub(super) const SAMPLE_LEVEL: usize = SAMPLE.slot("level");
    pub(super) const NOISE_LEVEL: usize = NOISE.slot("level");
    pub(super) const PLUCK_RATIO: usize = PLUCK.slot("ratio");
    pub(super) const PLUCK_DECAY: usize = PLUCK.slot("decay");
    pub(super) const PLUCK_DAMPING: usize = PLUCK.slot("damping");
    pub(super) const PLUCK_LEVEL: usize = PLUCK.slot("level");
    pub(super) const PLUCK_HARDNESS: usize = PLUCK.slot("hardness");
    pub(super) const PLUCK_POSITION: usize = PLUCK.slot("position");
    pub(super) const PLUCK_PICKUP: usize = PLUCK.slot("pickup");
    pub(super) const MODAL_RATIO: usize = MODAL.slot("ratio");
    pub(super) const MODAL_STRETCH: usize = MODAL.slot("stretch");
    pub(super) const MODAL_DECAY: usize = MODAL.slot("decay");
    pub(super) const MODAL_DAMPING: usize = MODAL.slot("damping");
    pub(super) const MODAL_POSITION: usize = MODAL.slot("position");
    pub(super) const MODAL_DETUNE: usize = MODAL.slot("detune");
    pub(super) const MODAL_LEVEL: usize = MODAL.slot("level");
    pub(super) const BOW_RATIO: usize = BOW.slot("ratio");
    pub(super) const BOW_DYNAMICS: usize = BOW.slot("dynamics");
    pub(super) const BOW_POSITION: usize = BOW.slot("position");
    pub(super) const BOW_VIBRATO: usize = BOW.slot("vibrato");
    pub(super) const BOW_RATE: usize = BOW.slot("rate");
    pub(super) const BOW_ATTACK: usize = BOW.slot("attack");
    pub(super) const BOW_TIMING: usize = BOW.slot("timing");
    pub(super) const BOW_SPREAD: usize = BOW.slot("spread");
    pub(super) const BOW_WIDTH: usize = BOW.slot("width");
    pub(super) const BOW_PAN: usize = BOW.slot("pan");
    pub(super) const BOW_LEVEL: usize = BOW.slot("level");
    pub(super) const ADSR_ATTACK: usize = ADSR.slot("attack");
    pub(super) const ADSR_DECAY: usize = ADSR.slot("decay");
    pub(super) const ADSR_SUSTAIN: usize = ADSR.slot("sustain");
    pub(super) const ADSR_RELEASE: usize = ADSR.slot("release");
    pub(super) const ADSR_CURVE: usize = ADSR.slot("curve");
    pub(super) const LFO_FREQUENCY: usize = LFO.slot("frequency");
    pub(super) const LFO_PHASE: usize = LFO.slot("phase");
    pub(super) const LFO_LEVEL: usize = LFO.slot("level");
    pub(super) const GAIN_LEVEL: usize = GAIN.slot("level");
    // A high-pass is the input less the one-pole low-pass of the same cutoff.
    pub(super) const ONEPOLE_CUTOFF: usize = ONEPOLE.slot("cutoff");
    const _: () = assert!(HIGHPASS.slot("cutoff") == ONEPOLE_CUTOFF);
    pub(super) const SVF_CUTOFF: usize = SVF.slot("cutoff");
    pub(super) const SVF_RATIO: usize = SVF.slot("ratio");
    pub(super) const SVF_Q: usize = SVF.slot("q");
    pub(super) const DRIVE_DRIVE: usize = DRIVE.slot("drive");
    pub(super) const DRIVE_BIAS: usize = DRIVE.slot("bias");
    pub(super) const DRIVE_LEVEL: usize = DRIVE.slot("level");
    pub(super) const DRIVE_REFERENCE: usize = DRIVE.slot("reference");
    pub(super) const PAN: usize = PAN_DECLARATION.slot("pan");
    pub(super) const BALANCE: usize = BALANCE_DECLARATION.slot("balance");
    pub(super) const WIDTH: usize = WIDTH_DECLARATION.slot("width");
    // A delay line reads the same slots on either clock.
    pub(super) const DELAY_TIME: usize = DELAY_SECONDS.slot("time");
    pub(super) const DELAY_FEEDBACK: usize = DELAY_SECONDS.slot("feedback");
    pub(super) const DELAY_DAMPING: usize = DELAY_SECONDS.slot("damping");
    const _: () = assert!(
        DELAY_SCORE.slot("time") == DELAY_TIME
            && DELAY_SCORE.slot("feedback") == DELAY_FEEDBACK
            && DELAY_SCORE.slot("damping") == DELAY_DAMPING
    );
}
