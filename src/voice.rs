//! Compiled reusable instrument graphs and their per-instance DSP state.

use crate::dsp::{one_pole_step, pan_sample, RenderError, Result};
use crate::expression::ExpressionRuntime;
use crate::graph::{
    parameter_descriptor_for_stage, topological_order, GraphProcessor, GraphProgram, GraphStage,
    InstrumentProgram, ParameterRate, ParameterSpec,
};
use crate::plan::{GainExpression, PitchExpression};
use crate::synth::{Adsr, Oscillator, Waveform};
use crate::wavetable::TableBank;
use num_traits::ToPrimitive;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

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
}

#[derive(Debug)]
struct CompiledGraph {
    nodes: Vec<CompiledNode>,
    order: Vec<usize>,
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
    controls: Vec<ControlBinding>,
}

#[derive(Debug)]
enum ProcessorCode {
    Oscillator(Waveform),
    Noise(u32),
    Pluck(u32),
    Wavetable(Arc<TableBank>),
    Adsr,
    Lfo,
    Gain(usize),
    OnePole(usize),
    HighPass(usize),
    Mix(usize),
    Pan,
}

#[derive(Clone, Copy, Debug)]
enum Source {
    Input,
    Node(usize),
}

#[derive(Debug)]
struct ModulationBinding {
    source: Source,
    parameter: usize,
    depth: f64,
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
    on_frame: u64,
    released: bool,
    graph: GraphState,
}

#[derive(Clone, Debug)]
struct GraphState {
    nodes: Vec<RuntimeNode>,
}

#[derive(Clone, Debug)]
struct RuntimeNode {
    state: ProcessorState,
    params: Vec<f64>,
    output: [f64; 2],
}

#[derive(Clone, Debug)]
enum ProcessorState {
    Oscillator(Oscillator),
    Noise(u32),
    Pluck(crate::pluck::Pluck),
    Wavetable { phase: f64 },
    Adsr(Adsr),
    Lfo(Oscillator),
    OnePole([f64; 2]),
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
        let mut voice = compile_graph(&program.voice, GraphStage::Voice, tables)?;
        let mut shared = program
            .shared
            .as_ref()
            .map(|graph| compile_graph(graph, GraphStage::Shared, tables))
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
            .map(|graph| GraphState::new_shared(graph, &resolved))
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
        })
    }

    /// Replace the instance's public control cache for the current frame.
    pub fn update_controls(&mut self, controls: &BTreeMap<String, f64>) -> Result<()> {
        update_resolved_controls(&self.program, controls, &mut self.controls)
    }

    pub fn note_on(
        &mut self,
        address: impl Into<String>,
        pitch_hz: f64,
        velocity: f64,
        frame: u64,
    ) -> Result<()> {
        self.note_on_with_expressions(address, pitch_hz, velocity, frame, None, None)
    }

    // Keep the public note-on contract unchanged while carrying both optional curves.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn note_on_with_expressions(
        &mut self,
        address: impl Into<String>,
        pitch_hz: f64,
        velocity: f64,
        frame: u64,
        pitch_expression: Option<ExpressionRuntime<PitchExpression>>,
        gain_expression: Option<ExpressionRuntime<GainExpression>>,
    ) -> Result<()> {
        if !pitch_hz.is_finite() || !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
            return Err(RenderError::Nonfinite(
                "instrument note pitch must be finite and velocity must be within 0..=1".into(),
            ));
        }
        if self.voices.len() >= self.capacity {
            return Err(RenderError::VoiceLimit {
                node: self.program.id.clone(),
                address: address.into(),
            });
        }
        let voice = VoiceState {
            address: address.into(),
            pitch_hz,
            velocity,
            pitch_expression,
            gain_expression,
            on_frame: frame,
            released: false,
            graph: GraphState::new_voice(&self.program.voice, &self.controls, frame)?,
        };
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
        voice
            .graph
            .release(&self.program.voice, &self.controls, frame, self.rate)?;
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
        let voice_channels = self.program.voice.channels;
        let mut voice_sum = [0.0; 2];
        for voice in &mut self.voices {
            let pitch_hz = if let Some(expression) = &voice.pitch_expression {
                let cents = expression
                    .curve
                    .cents_at(&expression.coordinate_at(voice.on_frame, frame))
                    .to_f64()
                    .ok_or_else(|| {
                        RenderError::Nonfinite(
                            "pitch expression cents cannot be represented".into(),
                        )
                    })?;
                let frequency = voice.pitch_hz * 2.0_f64.powf(cents / 1200.0);
                if !frequency.is_finite() || frequency <= 0.0 || frequency >= self.rate / 2.0 {
                    return Err(RenderError::RenderState(format!(
                        "pitch expression at {} is outside (0, Nyquist)",
                        voice.address
                    )));
                }
                frequency
            } else {
                voice.pitch_hz
            };
            let sample = voice.graph.render(
                &self.program.voice,
                &self.controls,
                frame,
                self.rate,
                pitch_hz,
                [0.0; 2],
            )?;
            let amplitude = voice
                .graph
                .amplitude(&self.program.voice, frame, self.rate)?;
            let gain = voice.gain_expression.as_ref().map(|expression| {
                expression
                    .curve
                    .gain_at(&expression.coordinate_at(voice.on_frame, frame))
            });
            if gain.is_some_and(|value| !value.is_finite() || value < 0.0) {
                return Err(RenderError::Nonfinite(format!(
                    "gain expression at {} produced an invalid value",
                    voice.address
                )));
            }
            for channel in 0..voice_channels {
                let contribution = sample[channel] * amplitude * voice.velocity;
                voice_sum[channel] += match gain {
                    Some(gain) => contribution * gain,
                    None => contribution,
                };
                if !voice_sum[channel].is_finite() {
                    return Err(RenderError::Nonfinite(format!(
                        "instrument {} voice sum is nonfinite",
                        self.program.id
                    )));
                }
            }
        }

        self.output = if let (Some(compiled), Some(shared)) =
            (self.program.shared.as_ref(), self.shared.as_mut())
        {
            shared.render(compiled, &self.controls, frame, self.rate, 0.0, voice_sum)?
        } else {
            voice_sum
        };
        Ok(&self.output[..self.program.channels])
    }

    pub fn reset(&mut self, controls: &BTreeMap<String, f64>) -> Result<()> {
        self.controls = resolve_controls(&self.program, controls)?;
        self.voices.clear();
        self.output = [0.0; 2];
        self.shared = self
            .program
            .shared
            .as_ref()
            .map(|graph| GraphState::new_shared(graph, &self.controls))
            .transpose()?;
        self.shared_reset = self.shared.clone();
        Ok(())
    }

    pub(crate) fn reset_state(&mut self) {
        self.voices.clear();
        self.output = [0.0; 2];
        self.shared.clone_from(&self.shared_reset);
    }

    pub fn active_voice_count(&self) -> usize {
        self.voices.len()
    }
}

impl GraphState {
    fn new_voice(graph: &CompiledGraph, controls: &[f64], frame: u64) -> Result<Self> {
        Self::new(graph, controls, Some(frame))
    }

    fn new_shared(graph: &CompiledGraph, controls: &[f64]) -> Result<Self> {
        Self::new(graph, controls, None)
    }

    fn new(graph: &CompiledGraph, controls: &[f64], on_frame: Option<u64>) -> Result<Self> {
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
                ProcessorCode::Adsr => ProcessorState::Adsr(Adsr::new(
                    on_frame.unwrap_or(0),
                    params[0],
                    params[1],
                    params[2],
                )?),
                ProcessorCode::Lfo => {
                    ProcessorState::Lfo(Oscillator::new(Waveform::Sine, params[1])?)
                }
                ProcessorCode::OnePole(_) | ProcessorCode::HighPass(_) => {
                    ProcessorState::OnePole([0.0; 2])
                }
                ProcessorCode::Gain(_) | ProcessorCode::Mix(_) | ProcessorCode::Pan => {
                    ProcessorState::Stateless
                }
            };
            nodes.push(RuntimeNode {
                state,
                params,
                output: [0.0; 2],
            });
        }
        Ok(Self { nodes })
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

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        graph: &CompiledGraph,
        controls: &[f64],
        frame: u64,
        rate: f64,
        pitch_hz: f64,
        input: [f64; 2],
    ) -> Result<[f64; 2]> {
        for &node_index in &graph.order {
            let compiled = &graph.nodes[node_index];
            self.nodes[node_index]
                .params
                .clone_from_slice(&compiled.base_params);
            apply_controls(
                compiled,
                controls,
                ParameterRate::Sample,
                &mut self.nodes[node_index].params,
            );
            for modulation in &compiled.modulations {
                let source = source_sample(&self.nodes, modulation.source, input)[0];
                self.nodes[node_index].params[modulation.parameter] += source * modulation.depth;
            }
            validate_params(compiled, &self.nodes[node_index].params)?;

            let audio_input = sum_inputs(&self.nodes, &compiled.incoming, input)?;
            let runtime = &mut self.nodes[node_index];
            runtime.output = [0.0; 2];
            match (&compiled.processor, &mut runtime.state) {
                (ProcessorCode::Noise(_), ProcessorState::Noise(state)) => {
                    // Version 1 fixes xorshift32's width, shifts and advance-before-output order.
                    *state ^= *state << 13;
                    *state ^= *state >> 17;
                    *state ^= *state << 5;
                    runtime.output[0] = (*state as f64 / 2147483648.0 - 1.0) * runtime.params[0];
                }
                (ProcessorCode::Pluck(_), ProcessorState::Pluck(pluck)) => {
                    runtime.output[0] = pluck.sample(
                        pitch_hz * runtime.params[0],
                        runtime.params[1],
                        runtime.params[2],
                        runtime.params[3],
                    )?;
                }
                (ProcessorCode::Oscillator(_), ProcessorState::Oscillator(oscillator)) => {
                    let frequency = pitch_hz * runtime.params[0] + runtime.params[1];
                    validate_frequency(frequency)?;
                    runtime.output[0] = oscillator.sample(frequency, rate)? * runtime.params[3];
                }
                (ProcessorCode::Wavetable(table), ProcessorState::Wavetable { phase }) => {
                    let frequency = pitch_hz * runtime.params[0] + runtime.params[1];
                    validate_frequency(frequency)?;
                    runtime.output[0] = table.sample(*phase, runtime.params[4], frequency, rate)?
                        * runtime.params[3];
                    *phase = (*phase + frequency / rate).rem_euclid(1.0);
                }
                (ProcessorCode::Adsr, ProcessorState::Adsr(envelope)) => {
                    runtime.output[0] = envelope.value(frame, rate)?;
                }
                (ProcessorCode::Lfo, ProcessorState::Lfo(oscillator)) => {
                    runtime.output[0] =
                        oscillator.sample(runtime.params[0], rate)? * runtime.params[2];
                }
                (ProcessorCode::Gain(channels), ProcessorState::Stateless) => {
                    for (output, input) in
                        runtime.output.iter_mut().zip(audio_input).take(*channels)
                    {
                        *output = input * runtime.params[0];
                    }
                }
                (
                    ProcessorCode::OnePole(channels) | ProcessorCode::HighPass(channels),
                    ProcessorState::OnePole(previous),
                ) => {
                    for ((output, previous), input) in runtime
                        .output
                        .iter_mut()
                        .zip(previous.iter_mut())
                        .zip(audio_input)
                        .take(*channels)
                    {
                        let lowpass = one_pole_step(input, previous, runtime.params[0], rate)?;
                        *output = if matches!(compiled.processor, ProcessorCode::HighPass(_)) {
                            input - lowpass
                        } else {
                            lowpass
                        };
                    }
                }
                (ProcessorCode::Mix(channels), ProcessorState::Stateless) => {
                    runtime.output[..*channels].copy_from_slice(&audio_input[..*channels]);
                }
                (ProcessorCode::Pan, ProcessorState::Stateless) => {
                    runtime.output = pan_sample(audio_input[0], runtime.params[0])?;
                }
                _ => {
                    return Err(RenderError::RenderState(format!(
                        "instrument graph node {} has mismatched compiled state",
                        compiled.id
                    )))
                }
            }
            if runtime.output.iter().any(|sample| !sample.is_finite()) {
                return Err(RenderError::Nonfinite(format!(
                    "instrument graph node {} produced a nonfinite sample",
                    compiled.id
                )));
            }
        }
        Ok(self.nodes[graph.output].output)
    }
}

fn compile_graph(
    graph: &GraphProgram,
    stage: GraphStage,
    tables: &BTreeMap<String, Arc<TableBank>>,
) -> Result<CompiledGraph> {
    let indices: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    let mut nodes = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        let (names, processor) = compile_processor(&node.processor, tables)?;
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
        nodes[target].modulations.push(ModulationBinding {
            source: compile_source(&modulation.from.node, &indices)?,
            parameter,
            depth: rational_f64(&modulation.depth, "modulation depth")?,
        });
    }

    Ok(CompiledGraph {
        order: topological_order(graph).map_err(RenderError::Plan)?,
        output: indices[graph.output.node.as_str()],
        amplitude: graph.amplitude.as_ref().map(|node| indices[node.as_str()]),
        channels: graph.channels as usize,
        nodes,
    })
}

fn compile_processor(
    processor: &GraphProcessor,
    tables: &BTreeMap<String, Arc<TableBank>>,
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
        GraphProcessor::Adsr => (
            &["attack", "decay", "sustain", "release"],
            ProcessorCode::Adsr,
        ),
        GraphProcessor::Lfo => (&["frequency", "phase", "level"], ProcessorCode::Lfo),
        GraphProcessor::Gain { channels } => (&["level"], ProcessorCode::Gain(*channels as usize)),
        GraphProcessor::OnePole { channels } => {
            (&["cutoff"], ProcessorCode::OnePole(*channels as usize))
        }
        GraphProcessor::HighPass { channels } => {
            (&["cutoff"], ProcessorCode::HighPass(*channels as usize))
        }
        GraphProcessor::Mix { channels } => (&[], ProcessorCode::Mix(*channels as usize)),
        GraphProcessor::Pan => (&["pan"], ProcessorCode::Pan),
    })
}

fn parameter_slot(processor: &ProcessorCode, name: &str) -> Option<usize> {
    let names: &[&str] = match processor {
        ProcessorCode::Oscillator(_) => &["ratio", "frequency", "phase", "level"],
        ProcessorCode::Pluck(_) => &["ratio", "decay", "damping", "level"],
        ProcessorCode::Wavetable(_) => &["ratio", "frequency", "phase", "level", "position"],
        ProcessorCode::Adsr => &["attack", "decay", "sustain", "release"],
        ProcessorCode::Lfo => &["frequency", "phase", "level"],
        ProcessorCode::Gain(_) | ProcessorCode::Noise(_) => &["level"],
        ProcessorCode::OnePole(_) | ProcessorCode::HighPass(_) => &["cutoff"],
        ProcessorCode::Mix(_) => &[],
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

fn source_sample(nodes: &[RuntimeNode], source: Source, input: [f64; 2]) -> [f64; 2] {
    match source {
        Source::Input => input,
        Source::Node(index) => nodes[index].output,
    }
}

fn sum_inputs(nodes: &[RuntimeNode], incoming: &[Source], input: [f64; 2]) -> Result<[f64; 2]> {
    let mut sum = [0.0; 2];
    for source in incoming {
        let source = source_sample(nodes, *source, input);
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
