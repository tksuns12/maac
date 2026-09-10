//! Sample based execution for MaaC foundation processors and reusable instruments.
//!
//! The plan is the execution boundary.  [`DspEngine::new`] validates it before
//! allocating render state, and [`DspEngine::render`] resets that state before
//! every render.  A callback receives one interleaved output frame at a time;
//! the slice is valid until the callback returns.

use crate::audio_buffer::OwnedAudioSlice;
use crate::audio_clip::{prepare_clip, PreparedAudioClip};
use crate::expression::ExpressionRuntime;
use crate::graph::ParameterRate;
use crate::kit::{KitRuntime, KitSample};
use crate::plan::{
    AutomationAnchorView, AutomationClock, EventKind, EventView, GainExpression, Interpolation,
    PitchExpression, Plan, PlanError, PlanLimits, PlanView, PortRef, PressureExpression, Processor,
    ProcessorView, Rational, TimbreExpression,
};
use crate::plan_artifact::PlanArtifact;
use crate::plan_v3::{TimingContext, VersionedPlan};
use crate::production_compressor::{Compressor, CompressorParams};
use crate::production_eq::Eq;
use crate::production_reverb::Reverb;
use crate::tempo::TimeValue;
use crate::voice::{CompiledInstrument, InstrumentRuntime};
use crate::wavetable::TableBank;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;

/// Result type used by the streaming renderer and its callback.
pub type Result<T> = std::result::Result<T, RenderError>;

/// Errors raised before or during a render.
#[derive(Clone, Debug, PartialEq)]
pub enum RenderError {
    /// The plan was rejected at the execution boundary.
    Plan(PlanError),
    /// A graph or port was not executable despite passing the plan validator.
    RenderState(String),
    /// A processor parameter or computed value was not finite or was outside
    /// the processor's declared range.
    Nonfinite(String),
    /// A polyphonic node has no free voice at a note-on. Release tails count as
    /// allocated voices until their envelope reaches zero.
    VoiceLimit { node: String, address: String },
    /// The caller stopped a render through its callback.
    Callback(String),
}

impl RenderError {
    /// Stable wire-style code useful to a CLI or test harness.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Plan(error) => match error.code.as_str() {
                "E_VERSION" => "E_VERSION",
                "E_ASSET" => "E_ASSET",
                "E_HASH" => "E_HASH",
                "E_UNKNOWN_FIELD" => "E_UNKNOWN_FIELD",
                "E_UNKNOWN_KIND" => "E_UNKNOWN_KIND",
                "E_REFERENCE" => "E_REFERENCE",
                "E_RANGE" => "E_RANGE",
                "E_SCHEDULE" => "E_SCHEDULE",
                "E_TEMPO" => "E_TEMPO",
                "E_INTERVAL" => "E_INTERVAL",
                "E_TIME_PRECISION" => "E_TIME_PRECISION",
                "E_SUBSAMPLE_NOTE" => "E_SUBSAMPLE_NOTE",
                "E_AUTOMATION_WRITER" => "E_AUTOMATION_WRITER",
                "E_CAPABILITY" => "E_CAPABILITY",
                "E_PORT_TYPE" | "E_PORT_CARDINALITY" => "E_PORT_TYPE",
                "E_ALGEBRAIC_LOOP" => "E_ALGEBRAIC_LOOP",
                "E_VOICE_LIMIT" => "E_VOICE_LIMIT",
                "E_NONFINITE" => "E_NONFINITE",
                "E_RESOURCE_LIMIT" | "E_RATIONAL_LIMIT" => "E_RESOURCE_LIMIT",
                _ => "E_RENDER_STATE",
            },
            Self::RenderState(_) => "E_RENDER_STATE",
            Self::Nonfinite(_) => "E_NONFINITE",
            Self::VoiceLimit { .. } => "E_VOICE_LIMIT",
            Self::Callback(_) => "E_RENDER_CALLBACK",
        }
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(error) => error.fmt(f),
            Self::RenderState(message) => write!(f, "E_RENDER_STATE: {message}"),
            Self::Nonfinite(message) => write!(f, "E_NONFINITE: {message}"),
            Self::VoiceLimit { node, address } => {
                write!(
                    f,
                    "E_VOICE_LIMIT at {node}: voice capacity exceeded for {address}"
                )
            }
            Self::Callback(message) => write!(f, "E_RENDER_CALLBACK: {message}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<PlanError> for RenderError {
    fn from(value: PlanError) -> Self {
        Self::Plan(value)
    }
}

/// Render a validated plan to a frame callback.
pub fn render<F>(plan: &Plan, callback: F) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    render_with_limits(plan, &PlanLimits::default(), callback)
}

/// Validate and stream a plan under an explicit caller resource allowance.
pub fn render_with_limits<F>(plan: &Plan, limits: &PlanLimits, callback: F) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    DspEngine::new_with_limits(plan, limits)?.render(callback)
}

/// Alias with an explicit name for callers that prefer not to shadow a local
/// `render` function.
pub fn render_plan<F>(plan: &Plan, callback: F) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    render(plan, callback)
}

/// Explicit-name alias for streaming with caller-selected limits.
pub fn render_plan_with_limits<F>(plan: &Plan, limits: &PlanLimits, callback: F) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    render_with_limits(plan, limits, callback)
}

/// Resettable sample based renderer for one immutable performance plan.
pub struct DspEngine<'a> {
    plan: PlanView<'a>,
    limits: PlanLimits,
    rate: f64,
    channels: usize,
    nodes: Vec<NodeState>,
    node_indices: HashMap<String, usize>,
    topo_order: Vec<usize>,
    control_order: Vec<usize>,
    modulations: Vec<ModulationRuntime>,
    incoming_modulations: Vec<Vec<usize>>,
    incoming: Vec<Vec<usize>>,
    connections: Vec<ConnectionRuntime>,
    events: Vec<EventRuntime>,
    on_events: BTreeMap<u64, Vec<usize>>,
    off_events: BTreeMap<u64, Vec<usize>>,
    tempo: TempoRuntime,
    frame: Vec<f64>,
}

impl<'a> DspEngine<'a> {
    /// Validate and prepare a plan.  No render state is accepted from a
    /// previous invocation.
    pub fn new(plan: &'a Plan) -> Result<Self> {
        Self::new_with_limits(plan, &PlanLimits::default())
    }

    /// Validate under caller limits before preparing any render state.
    pub fn new_with_limits(plan: &'a Plan, limits: &PlanLimits) -> Result<Self> {
        Self::new_for_view(plan.view(), limits)
    }

    /// Prepare either supported plan representation with default limits.
    pub fn new_versioned(plan: &'a VersionedPlan) -> Result<Self> {
        Self::new_versioned_with_limits(plan, &PlanLimits::default())
    }

    /// Prepare either supported plan representation under caller limits.
    pub fn new_versioned_with_limits(plan: &'a VersionedPlan, limits: &PlanLimits) -> Result<Self> {
        Self::new_for_view(
            match plan {
                VersionedPlan::Legacy(plan) => plan.view(),
                VersionedPlan::V3(plan) => plan.view(),
            },
            limits,
        )
    }

    /// Prepare an opaque artifact, independently validating before allocation.
    pub fn new_artifact(plan: &'a PlanArtifact) -> Result<Self> {
        Self::new_artifact_with_limits(plan, &PlanLimits::default())
    }
    /// Prepare an opaque artifact under caller resource limits.
    pub fn new_artifact_with_limits(plan: &'a PlanArtifact, limits: &PlanLimits) -> Result<Self> {
        Self::new_for_view(plan.view(), limits)
    }

    pub(crate) fn new_for_view(plan: PlanView<'a>, limits: &PlanLimits) -> Result<Self> {
        let timing = plan
            .validate_and_timing(limits)
            .map_err(RenderError::Plan)?;

        let rate = f64::from(plan.output.sample_rate_hz);
        let channels = usize::from(plan.output.channels);
        let tempo = TempoRuntime::new(&plan, timing.as_ref())?;

        let mut table_banks = BTreeMap::new();
        let mut instrument_programs = BTreeMap::new();
        if let Some(resources) = &plan.instruments {
            for wavetable in &resources.wavetables {
                table_banks.insert(
                    wavetable.id.clone(),
                    Arc::new(TableBank::new(wavetable).map_err(RenderError::Plan)?),
                );
            }
            for program in &resources.programs {
                instrument_programs.insert(
                    program.id.clone(),
                    Arc::new(CompiledInstrument::compile_validated(
                        program,
                        &table_banks,
                    )?),
                );
            }
        }

        let mut kit_samples = BTreeMap::new();
        for asset in plan.audio_assets.unwrap_or(&[]) {
            kit_samples.insert(asset.id.clone(), Arc::new(KitSample::from_asset(asset)?));
        }
        let node_indices: HashMap<String, usize> = plan
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.clone(), index))
            .collect();
        let mut reset_instrument_nodes = vec![false; plan.nodes.len()];
        for edge in plan.modulations {
            let target_index = node_indices[&edge.target.node];
            let target = plan
                .nodes
                .get(target_index)
                .expect("validated modulation target exists");
            if plan
                .instrument_control_spec(target, &edge.target.port)
                .is_some_and(|spec| spec.rate == ParameterRate::Reset)
            {
                reset_instrument_nodes[target_index] = true;
            }
        }
        let mut nodes = Vec::with_capacity(plan.nodes.len());
        for (index, node) in plan.nodes.iter().enumerate() {
            let (mut state, compiled) = match node.processor {
                ProcessorView::Lfo(config) => {
                    let context = timing
                        .as_ref()
                        .ok_or_else(|| RenderError::RenderState("LFO timing missing".into()))?;
                    let lfo =
                        crate::core_control::prepare_lfo(config, plan.output, context, limits)?;
                    (
                        NodeState::new_control(node.id.clone(), ControlRuntime::Lfo(lfo)),
                        None,
                    )
                }
                ProcessorView::Constant => (
                    NodeState::new_control(node.id.clone(), ControlRuntime::Constant),
                    None,
                ),
                ProcessorView::WarpRate(clip) => {
                    let sample = kit_samples
                        .get(&clip.asset)
                        .ok_or_else(|| RenderError::RenderState("clip asset missing".into()))?;
                    let timing = timing
                        .as_ref()
                        .ok_or_else(|| RenderError::RenderState("clip timing missing".into()))?;
                    let prepared = PreparedTransport::Warp(crate::warp_clip::prepare_clip(
                        clip,
                        timing,
                        plan.output,
                        sample.frames(),
                    )?);
                    let slice = sample.owned_slice(
                        clip.source_start_frame,
                        clip.source_end_frame,
                        false,
                    )?;
                    (
                        NodeState::new_audio(node.id.clone(), AudioRuntime { slice, prepared }),
                        None,
                    )
                }
                ProcessorView::Audio(clip) => {
                    let sample = kit_samples
                        .get(&clip.asset)
                        .ok_or_else(|| RenderError::RenderState("clip asset missing".into()))?;
                    let timing = timing
                        .as_ref()
                        .ok_or_else(|| RenderError::RenderState("clip timing missing".into()))?;
                    let prepared = PreparedTransport::Rate(prepare_clip(
                        clip,
                        timing,
                        plan.output,
                        sample.rate_hz(),
                        sample.frames(),
                    )?);
                    let slice = sample.owned_slice(
                        clip.source_start_frame,
                        clip.source_end_frame,
                        clip.reverse,
                    )?;
                    (
                        NodeState::new_audio(node.id.clone(), AudioRuntime { slice, prepared }),
                        None,
                    )
                }
                ProcessorView::Core(processor) => {
                    let compiled = match processor {
                        Processor::Instrument { program, .. } => {
                            Some(instrument_programs.get(program).cloned().ok_or_else(|| {
                                RenderError::RenderState(format!(
                                    "instrument node {} refers to missing program {program}",
                                    node.id
                                ))
                            })?)
                        }
                        _ => None,
                    };
                    (
                        NodeState::new(
                            node.id.clone(),
                            processor.clone(),
                            rate,
                            compiled.as_ref(),
                        )?,
                        compiled,
                    )
                }
                ProcessorView::Kit {
                    channels,
                    voices,
                    samples,
                } => {
                    let mapping = samples
                        .iter()
                        .map(|sample| {
                            let asset =
                                kit_samples.get(&sample.asset).cloned().ok_or_else(|| {
                                    RenderError::RenderState(format!(
                                        "kit asset {} missing",
                                        sample.asset
                                    ))
                                })?;
                            Ok((sample.key.clone(), asset))
                        })
                        .collect::<Result<BTreeMap<_, _>>>()?;
                    (
                        NodeState::new_kit(
                            node.id.clone(),
                            channels,
                            KitRuntime::new(channels, voices, mapping)?,
                        )?,
                        None,
                    )
                }
            };
            let resolved_params = plan.resolved_node_params(node).map_err(RenderError::Plan)?;
            for (parameter, value) in &resolved_params {
                let value = rational_f64(value, "node parameter")?;
                state.base_params.insert(parameter.clone(), value);
                state.current_params.insert(parameter.clone(), value);
            }
            state.validate_current_parameters(rate)?;
            if !reset_instrument_nodes[index] {
                state.initialize_instrument(compiled, rate)?;
            }
            nodes.push(state);
        }

        let mut connections: Vec<ConnectionRuntime> = plan
            .connections
            .iter()
            .map(|connection| {
                let from = *node_indices.get(&connection.from.node).ok_or_else(|| {
                    RenderError::RenderState(format!(
                        "connection {} has no source node",
                        connection.id
                    ))
                })?;
                let to = *node_indices.get(&connection.to.node).ok_or_else(|| {
                    RenderError::RenderState(format!(
                        "connection {} has no destination node",
                        connection.id
                    ))
                })?;
                Ok(ConnectionRuntime {
                    id: connection.id.clone(),
                    from,
                    to,
                    to_port: connection.to.port.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        // The graph's reduction order is part of the MaaC contract.  A
        // destination's list is sorted independently from declaration order.
        connections.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
        let mut incoming = vec![Vec::new(); nodes.len()];
        for (index, connection) in connections.iter().enumerate() {
            incoming[connection.to].push(index);
        }

        let mut modulations = plan
            .modulations
            .iter()
            .map(|edge| {
                let to = node_indices[&edge.target.node];
                let rate = plan
                    .nodes
                    .get(to)
                    .and_then(|target| plan.instrument_control_spec(target, &edge.target.port))
                    .map_or(ParameterRate::Sample, |spec| spec.rate);
                Ok(ModulationRuntime {
                    id: edge.id.clone(),
                    from: node_indices[&edge.from.node],
                    to,
                    parameter: edge.target.port.clone(),
                    amount: rational_f64(&edge.amount, "modulation amount")?,
                    rate,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        modulations.sort_by(|a, b| a.id.as_bytes().cmp(b.id.as_bytes()));
        let mut incoming_modulations = vec![Vec::new(); nodes.len()];
        for (i, edge) in modulations.iter().enumerate() {
            incoming_modulations[edge.to].push(i);
        }
        let order = stable_topological_order(&nodes, &connections, &modulations)?;
        let control_order: Vec<usize> = order
            .iter()
            .copied()
            .filter(|i| nodes[*i].control.is_some())
            .collect();
        let topo_order = order
            .into_iter()
            .filter(|i| nodes[*i].control.is_none() && nodes[*i].delay.is_none())
            .collect();
        let automation = build_automations(&plan, &node_indices, &tempo, rate, timing.as_ref())?;
        for lane in automation {
            let parameter = lane.parameter.clone();
            nodes[lane.node].automations.insert(parameter, lane);
        }

        if reset_instrument_nodes.iter().any(|targeted| *targeted) {
            bootstrap_reset_instruments(
                &plan,
                &mut nodes,
                &control_order,
                &modulations,
                &incoming_modulations,
                &instrument_programs,
                &tempo,
                rate,
            )?;
        }

        let mut events = Vec::with_capacity(plan.events().len());
        let mut on_events: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        let mut off_events: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        for event in plan.events() {
            let node = *node_indices.get(&event.target.node).ok_or_else(|| {
                RenderError::RenderState(format!("event {} has no target node", event.address))
            })?;
            let runtime = EventRuntime::from_event(&event, node, plan.output.sample_rate_hz)?;
            let index = events.len();
            on_events.entry(runtime.on_frame).or_default().push(index);
            if let Some(frame) = runtime.off_frame {
                off_events.entry(frame).or_default().push(index);
            }
            events.push(runtime);
        }
        sort_event_indices(&mut on_events, &events);
        sort_event_indices(&mut off_events, &events);

        let output_node = *node_indices
            .get(&plan.output.output.node)
            .ok_or_else(|| RenderError::RenderState("output node does not exist".into()))?;
        if plan.output.output.port != "out" || nodes[output_node].output.len() != channels {
            return Err(RenderError::RenderState(
                "output port or channel count does not match the plan".into(),
            ));
        }

        Ok(Self {
            plan,
            limits: *limits,
            rate,
            channels,
            nodes,
            node_indices,
            topo_order,
            control_order,
            modulations,
            incoming_modulations,
            incoming,
            connections,
            events,
            on_events,
            off_events,
            tempo,
            frame: vec![0.0; channels],
        })
    }

    /// Render all frames, resetting processors and voices first.
    pub fn render<F>(&mut self, mut callback: F) -> Result<()>
    where
        F: FnMut(&[f64]) -> Result<()>,
    {
        let output = self.plan.output.output.clone();
        self.render_ports_inner(&[output], false, |outputs| callback(&outputs[0]))
    }

    /// Capture selected ports from one complete graph execution. Buffers are reused each frame.
    pub fn render_ports<F>(&mut self, ports: &[PortRef], callback: F) -> Result<()>
    where
        F: FnMut(&[Vec<f64>]) -> Result<()>,
    {
        self.render_ports_inner(ports, true, callback)
    }

    fn render_ports_inner<F>(
        &mut self,
        ports: &[PortRef],
        charge_capture: bool,
        mut callback: F,
    ) -> Result<()>
    where
        F: FnMut(&[Vec<f64>]) -> Result<()>,
    {
        let resource_error = || {
            RenderError::Plan(PlanError {
                code: "E_RESOURCE_LIMIT".into(),
                path: "capture".into(),
                message: "capture count, allocation, or aggregate work exceeds its bound".into(),
                span: None,
            })
        };
        if ports.len() > crate::production_data::MAX_TARGETS {
            return Err(resource_error());
        }
        let channels = ports.iter().try_fold(0u64, |sum, port| {
            sum.checked_add(u64::from(
                self.plan
                    .audio_output_channels(port)
                    .map_err(RenderError::Plan)?,
            ))
            .ok_or_else(resource_error)
        })?;
        if charge_capture {
            let work = self
                .plan
                .execution_work(&self.limits)
                .map_err(RenderError::Plan)?;
            let copy_work = self
                .plan
                .output
                .total_frames
                .checked_mul(channels)
                .ok_or_else(resource_error)?;
            if work.checked_add(copy_work).ok_or_else(resource_error)?
                > self
                    .limits
                    .max_execution_work
                    .min(PlanLimits::MAX_SONG_EXECUTION_WORK)
            {
                return Err(resource_error());
            }
        }
        if ports.is_empty() {
            return Err(RenderError::RenderState(
                "at least one capture port is required".into(),
            ));
        }
        let mut selections = Vec::new();
        let mut outputs = Vec::new();
        selections
            .try_reserve_exact(ports.len())
            .map_err(|_| resource_error())?;
        outputs
            .try_reserve_exact(ports.len())
            .map_err(|_| resource_error())?;
        for port in ports {
            let channels = self
                .plan
                .audio_output_channels(port)
                .map_err(RenderError::Plan)?;
            selections.push(
                *self
                    .node_indices
                    .get(&port.node)
                    .ok_or_else(|| RenderError::RenderState("capture node missing".into()))?,
            );
            let mut samples = Vec::new();
            samples
                .try_reserve_exact(usize::from(channels))
                .map_err(|_| resource_error())?;
            samples.resize(usize::from(channels), 0.0);
            outputs.push(samples);
        }
        self.reset();
        let output_node = *self
            .node_indices
            .get(&self.plan.output.output.node)
            .ok_or_else(|| RenderError::RenderState("output node does not exist".into()))?;

        for frame_index in 0..self.plan.output.total_frames {
            self.read_delays();
            self.evaluate_parameters(frame_index)?;

            // The score schedule requires every note-off to be applied before
            // any note-on at a shared frame.  This is also what makes a just
            // released tail available for a same-frame new voice.
            if let Some(events) = self.off_events.get(&frame_index).cloned() {
                for event_index in events {
                    self.release_event(event_index, frame_index)?;
                }
            }
            // A released tail remains allocated through the last sample with
            // nonzero envelope.  It must be retired before same-frame note-ons
            // so a voice whose envelope reaches zero at this frame is free for
            // the new event.
            for node in &mut self.nodes {
                node.prune_finished_voices(frame_index, self.rate)?;
            }
            if let Some(events) = self.on_events.get(&frame_index).cloned() {
                for event_index in events {
                    self.start_event(event_index, frame_index)?;
                }
            }

            let topo_order = self.topo_order.clone();
            for node_index in topo_order {
                self.process_node(node_index, frame_index)?;
            }
            self.commit_delays()?;

            self.frame
                .copy_from_slice(&self.nodes[output_node].output[..self.channels]);
            if self.frame.iter().any(|sample| !sample.is_finite()) {
                return Err(RenderError::Nonfinite(format!(
                    "nonfinite output at frame {frame_index}"
                )));
            }
            for (output, index) in outputs.iter_mut().zip(&selections) {
                output.copy_from_slice(&self.nodes[*index].output);
            }
            callback(&outputs)?;
        }
        Ok(())
    }

    /// Clear every processor, voice, phase accumulator, and output sample.
    pub fn reset(&mut self) {
        for node in &mut self.nodes {
            node.reset();
        }
        self.frame.fill(0.0);
    }

    fn read_delays(&mut self) {
        for node in &mut self.nodes {
            if let Some(delay) = &node.delay {
                let output = delay.read();
                node.output.copy_from_slice(&output[..delay.channels]);
            }
        }
    }

    fn commit_delays(&mut self) -> Result<()> {
        for index in 0..self.nodes.len() {
            let Some(channels) = self.nodes[index].delay.as_ref().map(|delay| delay.channels)
            else {
                continue;
            };
            let connection = &self.connections[self.incoming[index][0]];
            let mut input = [0.0; 2];
            input[..channels].copy_from_slice(&self.nodes[connection.from].output[..channels]);
            self.nodes[index]
                .delay
                .as_mut()
                .expect("delay state exists")
                .stage(&input[..channels])?;
        }
        for node in &mut self.nodes {
            if let Some(delay) = &mut node.delay {
                delay.commit();
            }
        }
        Ok(())
    }

    fn evaluate_parameters(&mut self, frame: u64) -> Result<()> {
        if self.plan.version == 7 {
            return self.evaluate_modulated_parameters(frame);
        }
        for node in &mut self.nodes {
            node.current_params.clone_from(&node.base_params);
            for (name, lane) in &node.automations {
                let value = lane.lane.value_at(frame, &self.tempo)?;
                if !value.is_finite() {
                    return Err(RenderError::Nonfinite(format!(
                        "automation {} on {} produced a nonfinite value",
                        lane.id, node.id
                    )));
                }
                node.current_params.insert(name.clone(), value);
            }
            node.validate_current_parameters(self.rate)?;
            if let Some(instrument) = &mut node.instrument {
                instrument.update_controls(&node.current_params)?;
            }
        }
        Ok(())
    }

    /// V7 controls have only control parents. Evaluate their DAG before audio,
    /// then apply audio targets before the existing note-off/note-on sequence.
    fn evaluate_modulated_parameters(&mut self, frame: u64) -> Result<()> {
        for node in &mut self.nodes {
            for (name, value) in &node.base_params {
                *node
                    .current_params
                    .get_mut(name)
                    .expect("prepared base parameter exists") = *value;
            }
            for (name, lane) in &node.automations {
                let value = lane.lane.value_at(frame, &self.tempo)?;
                if !value.is_finite() {
                    return Err(RenderError::Nonfinite(
                        "control automation is nonfinite".into(),
                    ));
                }
                *node.current_params.get_mut(name).ok_or_else(|| {
                    RenderError::RenderState("automation parameter missing".into())
                })? = value;
            }
        }
        for order_index in 0..self.control_order.len() {
            let index = self.control_order[order_index];
            self.apply_modulations(index)?;
            let node = &mut self.nodes[index];
            node.validate_modulated_parameters(self.rate)?;
            node.control_value = match node.control.as_ref() {
                Some(ControlRuntime::Lfo(lfo)) => lfo.value_at(frame)?,
                Some(ControlRuntime::Constant) => node.current_param("value"),
                None => return Err(RenderError::RenderState("control state missing".into())),
            };
            if !node.control_value.is_finite() {
                return Err(RenderError::Nonfinite("control output is nonfinite".into()));
            }
        }
        for index in 0..self.nodes.len() {
            if self.nodes[index].control.is_some() {
                continue;
            }
            self.apply_modulations(index)?;
            let node = &mut self.nodes[index];
            node.validate_modulated_parameters(self.rate)?;
            if let Some(instrument) = &mut node.instrument {
                instrument
                    .update_controls(&node.current_params)
                    .map_err(|_| {
                        crate::plan::err(
                            "E_RANGE",
                            "modulations.target",
                            "instrument control is outside its declared range",
                        )
                    })?;
            }
        }
        Ok(())
    }

    fn apply_modulations(&mut self, node: usize) -> Result<()> {
        for &index in &self.incoming_modulations[node] {
            apply_modulation_to_parameter(&mut self.nodes, &self.modulations[index])?;
        }
        Ok(())
    }

    fn start_event(&mut self, event_index: usize, frame: u64) -> Result<()> {
        let event = self
            .events
            .get(event_index)
            .ok_or_else(|| RenderError::RenderState("on-event index is invalid".into()))?
            .clone();
        let node = self
            .nodes
            .get_mut(event.node)
            .ok_or_else(|| RenderError::RenderState("event node index is invalid".into()))?;
        let (
            pitch_hz,
            velocity,
            pitch_expression,
            gain_expression,
            timbre_expression,
            pressure_expression,
        ) = match event.data {
            EventData::Hit { key, velocity } => {
                let kit = node
                    .kit
                    .as_mut()
                    .ok_or_else(|| RenderError::RenderState("hit target lacks kit state".into()))?;
                return kit
                    .onset(frame, &key, velocity, &event.address)
                    .map_err(RenderError::Plan);
            }
            EventData::Note {
                pitch_hz,
                velocity,
                pitch_expression,
                gain_expression,
                timbre_expression,
                pressure_expression,
            } => (
                pitch_hz,
                velocity,
                pitch_expression,
                gain_expression,
                timbre_expression,
                pressure_expression,
            ),
        };
        if let Some(instrument) = &mut node.instrument {
            return instrument
                .note_on_with_expressions(
                    event.address.clone(),
                    pitch_hz,
                    velocity,
                    frame,
                    pitch_expression,
                    gain_expression,
                    timbre_expression,
                    pressure_expression,
                )
                .map_err(|error| match error {
                    RenderError::VoiceLimit { address, .. } => RenderError::VoiceLimit {
                        node: node.id.clone(),
                        address,
                    },
                    other => other,
                });
        }
        let attack = node.current_param("attack");
        let capacity = match node.processor {
            RuntimeProcessor::Core(Processor::Sine { voices }) => voices as usize,
            _ => {
                return Err(RenderError::RenderState(
                    "native note event targets a non-sine processor".into(),
                ))
            }
        };
        if node.voices.len() >= capacity {
            return Err(RenderError::VoiceLimit {
                node: node.id.clone(),
                address: event.address,
            });
        }
        let voice = Voice {
            address: event.address,
            pitch_hz,
            pitch_expression,
            gain_expression,
            velocity,
            on_frame: frame,
            attack,
            // Release is an event-rate parameter sampled at note-off. It is
            // deliberately left unset until `release_event` captures it.
            release: 0.0,
            phase: 0.0,
            released: false,
            release_frame: frame,
            release_amplitude: 0.0,
        };
        let insertion = node
            .voices
            .binary_search_by(|existing| existing.address.as_bytes().cmp(voice.address.as_bytes()))
            .unwrap_or_else(|position| position);
        node.voices.insert(insertion, voice);
        Ok(())
    }

    fn release_event(&mut self, event_index: usize, frame: u64) -> Result<()> {
        let event = self
            .events
            .get(event_index)
            .ok_or_else(|| RenderError::RenderState("off-event index is invalid".into()))?
            .clone();
        let node = self
            .nodes
            .get_mut(event.node)
            .ok_or_else(|| RenderError::RenderState("event node index is invalid".into()))?;
        if !matches!(event.data, EventData::Note { .. }) {
            return Err(RenderError::RenderState(
                "hit cannot receive note-off".into(),
            ));
        }
        if let Some(instrument) = &mut node.instrument {
            return instrument.note_off(&event.address, frame);
        }
        let position = node
            .voices
            .binary_search_by(|voice| voice.address.as_bytes().cmp(event.address.as_bytes()))
            .map_err(|_| {
                RenderError::RenderState(format!(
                    "note-off for {} has no allocated voice",
                    event.address
                ))
            })?;
        // Attack is sampled at note-on; release is sampled at note-off. This
        // matters when automation changes the release parameter while a note
        // is held.
        let release = node.current_param("release");
        if node.voices[position].released {
            return Err(RenderError::RenderState(format!(
                "note-off for {} was delivered twice",
                event.address
            )));
        }
        let attack_amplitude = node.voices[position].attack_amplitude(frame, self.rate);
        if release <= 0.0 {
            node.voices.remove(position);
        } else {
            let voice = &mut node.voices[position];
            voice.release = release;
            voice.release_frame = frame;
            voice.release_amplitude = attack_amplitude;
            voice.released = true;
        }
        Ok(())
    }

    fn process_node(&mut self, node_index: usize, frame: u64) -> Result<()> {
        if let Some(matrix) = self.nodes[node_index].matrix {
            let connection = &self.connections[self.incoming[node_index][0]];
            let mut input = [0.0; 2];
            input[..matrix.inputs]
                .copy_from_slice(&self.nodes[connection.from].output[..matrix.inputs]);
            let mut output = [0.0; 2];
            for (output_channel, sample) in output.iter_mut().enumerate().take(matrix.outputs) {
                let mut sum = 0.0;
                for (input_channel, input) in input.iter().copied().enumerate().take(matrix.inputs)
                {
                    let product = matrix.coefficients[output_channel][input_channel] * input;
                    if !product.is_finite() {
                        return Err(RenderError::Nonfinite(format!(
                            "matrix node {} produced a nonfinite product",
                            self.nodes[node_index].id
                        )));
                    }
                    sum += product;
                    if !sum.is_finite() {
                        return Err(RenderError::Nonfinite(format!(
                            "matrix node {} produced a nonfinite sum",
                            self.nodes[node_index].id
                        )));
                    }
                }
                *sample = sum;
            }
            self.nodes[node_index]
                .output
                .copy_from_slice(&output[..matrix.outputs]);
            return Ok(());
        }
        let processor = match self.nodes[node_index].processor.clone() {
            RuntimeProcessor::Control => {
                return Err(RenderError::RenderState(
                    "control entered audio evaluation".into(),
                ))
            }
            RuntimeProcessor::Audio => {
                let node = &mut self.nodes[node_index];
                let output = node
                    .audio
                    .as_ref()
                    .ok_or_else(|| RenderError::RenderState("audio state missing".into()))?
                    .render_frame(frame)?;
                let channels = node.output.len();
                node.output.copy_from_slice(&output[..channels]);
                return Ok(());
            }
            RuntimeProcessor::Core(processor) => processor,
            RuntimeProcessor::Kit => {
                let node = &mut self.nodes[node_index];
                let level = node.current_param("level");
                let output = node
                    .kit
                    .as_mut()
                    .ok_or_else(|| RenderError::RenderState("kit state missing".into()))?
                    .render_frame(frame, level)?;
                let channels = node.output.len();
                node.output.copy_from_slice(&output[..channels]);
                return Ok(());
            }
        };
        let incoming = self.incoming[node_index].clone();
        match processor {
            Processor::Sine { .. } => {
                let level = self.nodes[node_index].current_param("level");
                let output = self.nodes[node_index].sine_sample(frame, self.rate, level)?;
                self.nodes[node_index].output[0] = output;
            }
            Processor::OnePole { channels } => {
                let cutoff = self.nodes[node_index].current_param("cutoff");
                let mut input = [0.0; 2];
                for &connection_index in &incoming {
                    let connection = &self.connections[connection_index];
                    for (channel, sample) in
                        input.iter_mut().enumerate().take(usize::from(channels))
                    {
                        *sample += self.nodes[connection.from].output[channel];
                    }
                }
                let node = &mut self.nodes[node_index];
                for (channel, input) in input
                    .iter()
                    .copied()
                    .enumerate()
                    .take(usize::from(channels))
                {
                    let output = one_pole_step(
                        input,
                        &mut node.onepole_previous[channel],
                        cutoff,
                        self.rate,
                    )?;
                    node.output[channel] = output;
                }
            }
            Processor::Gain { channels } => {
                let gain = self.nodes[node_index].current_param("gain");
                let mut output = [0.0; 2];
                for (channel, sample) in output.iter_mut().enumerate().take(usize::from(channels)) {
                    // Plan validation guarantees exactly one audio input.
                    let connection = &self.connections[incoming[0]];
                    *sample = gain * self.nodes[connection.from].output[channel];
                    if !sample.is_finite() {
                        return Err(RenderError::Nonfinite(format!(
                            "gain node {} produced a nonfinite value",
                            self.nodes[node_index].id
                        )));
                    }
                }
                self.nodes[node_index]
                    .output
                    .copy_from_slice(&output[..usize::from(channels)]);
            }
            Processor::Fader { channels } => {
                let level = self.nodes[node_index].current_param("level");
                let factor = 10.0_f64.powf(level / 20.0);
                if !factor.is_finite() {
                    return Err(RenderError::Nonfinite(format!(
                        "fader node {} produced a nonfinite factor",
                        self.nodes[node_index].id
                    )));
                }
                let mut output = [0.0; 2];
                for (channel, sample) in output.iter_mut().enumerate().take(usize::from(channels)) {
                    // Plan validation guarantees exactly one audio input.
                    let connection = &self.connections[incoming[0]];
                    *sample = factor * self.nodes[connection.from].output[channel];
                    if !sample.is_finite() {
                        return Err(RenderError::Nonfinite(format!(
                            "fader node {} produced a nonfinite value",
                            self.nodes[node_index].id
                        )));
                    }
                }
                self.nodes[node_index]
                    .output
                    .copy_from_slice(&output[..usize::from(channels)]);
            }
            Processor::Eq { channels, .. }
            | Processor::Compressor { channels, .. }
            | Processor::Reverb { channels, .. } => {
                let channels = usize::from(channels);
                let main = incoming
                    .iter()
                    .map(|i| &self.connections[*i])
                    .find(|c| c.to_port == "in")
                    .ok_or_else(|| RenderError::RenderState("native main input missing".into()))?;
                let mut input = [0.0; 2];
                input[..channels].copy_from_slice(&self.nodes[main.from].output[..channels]);
                let side = incoming
                    .iter()
                    .map(|i| &self.connections[*i])
                    .find(|c| c.to_port == "sidechain");
                let mut side_input = [0.0; 2];
                let side_channels = side.map(|c| {
                    let output = &self.nodes[c.from].output;
                    side_input[..output.len()].copy_from_slice(output);
                    output.len()
                });
                let node = &mut self.nodes[node_index];
                let output = match &processor {
                    Processor::Eq { .. } => {
                        let frequency = node.current_param("frequency");
                        let q = node.current_params.get("q").copied().unwrap_or(1.0);
                        let gain = node.current_params.get("gain").copied().unwrap_or(0.0);
                        node.eq
                            .as_mut()
                            .ok_or_else(|| RenderError::RenderState("EQ state missing".into()))?
                            .process(&input[..channels], frequency, q, gain)?
                    }
                    Processor::Compressor { .. } => {
                        let params = CompressorParams {
                            threshold: node.current_param("threshold"),
                            ratio: node.current_param("ratio"),
                            knee: node.current_param("knee"),
                            attack: node.current_param("attack"),
                            release: node.current_param("release"),
                            makeup: node.current_param("makeup"),
                        };
                        node.compressor
                            .as_mut()
                            .ok_or_else(|| {
                                RenderError::RenderState("compressor state missing".into())
                            })?
                            .process(
                                &input[..channels],
                                side_channels.map(|n| &side_input[..n]),
                                params,
                            )?
                    }
                    Processor::Reverb { .. } => {
                        let decay = node.current_param("decay");
                        let mix = node.current_param("mix");
                        node.reverb
                            .as_mut()
                            .ok_or_else(|| RenderError::RenderState("reverb state missing".into()))?
                            .process(&input[..channels], decay, mix)?
                    }
                    _ => unreachable!(),
                };
                node.output.copy_from_slice(&output[..channels]);
            }
            Processor::Pan => {
                let mut input = 0.0;
                for &connection_index in &incoming {
                    let connection = &self.connections[connection_index];
                    input += self.nodes[connection.from].output[0];
                }
                let pan = self.nodes[node_index].current_param("pan");
                let pair = pan_sample(input, pan)?;
                self.nodes[node_index].output[0] = pair[0];
                self.nodes[node_index].output[1] = pair[1];
            }
            Processor::Sum { channels } => {
                for channel in 0..usize::from(channels) {
                    let mut sum = 0.0;
                    for &connection_index in &incoming {
                        let connection = &self.connections[connection_index];
                        sum += self.nodes[connection.from].output[channel];
                    }
                    if !sum.is_finite() {
                        return Err(RenderError::Nonfinite(format!(
                            "sum node {} produced a nonfinite value",
                            self.nodes[node_index].id
                        )));
                    }
                    self.nodes[node_index].output[channel] = sum;
                }
            }
            Processor::Noise { channels, .. } => {
                let node = &mut self.nodes[node_index];
                let noise = node
                    .noise
                    .as_ref()
                    .ok_or_else(|| RenderError::RenderState("noise state missing".into()))?;
                for channel in 0..usize::from(channels) {
                    node.output[channel] = noise.sample(frame, channel as u32);
                }
            }
            Processor::Matrix { .. } => unreachable!("matrix uses its prepared runtime"),
            Processor::Delay { .. } => unreachable!("delay uses the frame scheduler"),
            Processor::Instrument { .. } => {
                let node = &mut self.nodes[node_index];
                let node_id = node.id.clone();
                let channels = node.output.len();
                let rendered = node
                    .instrument
                    .as_mut()
                    .ok_or_else(|| {
                        RenderError::RenderState(format!(
                            "instrument node {} has no runtime state",
                            node_id
                        ))
                    })?
                    .render(frame)?;
                let output = [rendered[0], rendered.get(1).copied().unwrap_or(0.0)];
                node.output.copy_from_slice(&output[..channels]);
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn bootstrap_reset_instruments(
    plan: &PlanView<'_>,
    nodes: &mut [NodeState],
    control_order: &[usize],
    modulations: &[ModulationRuntime],
    incoming_modulations: &[Vec<usize>],
    instrument_programs: &BTreeMap<String, Arc<CompiledInstrument>>,
    tempo: &TempoRuntime,
    rate: f64,
) -> Result<()> {
    let mut required_controls = vec![false; nodes.len()];
    let mut target_nodes = vec![false; nodes.len()];
    let mut target_bounds: BTreeMap<usize, BTreeMap<String, ResetTargetBounds>> = BTreeMap::new();
    for edge in modulations {
        if edge.rate != ParameterRate::Reset {
            continue;
        }
        required_controls[edge.from] = true;
        target_nodes[edge.to] = true;
        let target = plan
            .nodes
            .get(edge.to)
            .ok_or_else(|| RenderError::RenderState("reset target node missing".into()))?;
        let spec = plan
            .instrument_control_spec(target, &edge.parameter)
            .filter(|spec| spec.rate == ParameterRate::Reset)
            .ok_or_else(|| RenderError::RenderState("reset target control missing".into()))?;
        target_bounds
            .entry(edge.to)
            .or_default()
            .entry(edge.parameter.clone())
            .or_insert(ResetTargetBounds {
                parameter: edge.parameter.clone(),
                min: rational_f64(&spec.min, "reset control minimum")?,
                max: rational_f64(&spec.max, "reset control maximum")?,
                min_open: spec.min_open,
                max_open: spec.max_open,
            });
    }

    for &index in control_order.iter().rev() {
        if !required_controls[index] {
            continue;
        }
        for &edge_index in &incoming_modulations[index] {
            required_controls[modulations[edge_index].from] = true;
        }
    }

    for &index in control_order {
        if !required_controls[index] {
            continue;
        }
        nodes[index]
            .current_params
            .clone_from(&nodes[index].base_params);
        for (name, lane) in &nodes[index].automations {
            let value = lane.lane.value_at(0, tempo)?;
            if !value.is_finite() {
                return Err(RenderError::Nonfinite(
                    "control automation is nonfinite".into(),
                ));
            }
            *nodes[index]
                .current_params
                .get_mut(name)
                .ok_or_else(|| RenderError::RenderState("automation parameter missing".into()))? =
                value;
        }
        for &edge_index in &incoming_modulations[index] {
            apply_modulation_to_parameter(nodes, &modulations[edge_index])?;
        }
        nodes[index].validate_modulated_parameters(rate)?;
        nodes[index].control_value = match nodes[index].control.as_ref() {
            Some(ControlRuntime::Lfo(lfo)) => lfo.value_at(0)?,
            Some(ControlRuntime::Constant) => nodes[index].current_param("value"),
            None => return Err(RenderError::RenderState("control state missing".into())),
        };
        if !nodes[index].control_value.is_finite() {
            return Err(RenderError::Nonfinite("control output is nonfinite".into()));
        }
    }

    for (index, targeted) in target_nodes.into_iter().enumerate() {
        if !targeted {
            continue;
        }
        nodes[index]
            .current_params
            .clone_from(&nodes[index].base_params);
        for &edge_index in &incoming_modulations[index] {
            let edge = &modulations[edge_index];
            if edge.rate != ParameterRate::Reset {
                continue;
            }
            apply_modulation_to_parameter(nodes, edge)?;
        }
        if let Some(bounds_by_parameter) = target_bounds.get(&index) {
            for bounds in bounds_by_parameter.values() {
                let value = nodes[index].current_param(&bounds.parameter);
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
                    return Err(crate::plan::err(
                        "E_RANGE",
                        "modulations.target",
                        "instrument control is outside its declared range",
                    )
                    .into());
                }
            }
        }
        let program = match &nodes[index].processor {
            RuntimeProcessor::Core(Processor::Instrument { program, .. }) => program,
            _ => {
                return Err(RenderError::RenderState(
                    "reset modulation target is not an instrument".into(),
                ))
            }
        };
        let compiled = instrument_programs.get(program).cloned().ok_or_else(|| {
            RenderError::RenderState(format!(
                "instrument node {} refers to missing program {program}",
                nodes[index].id
            ))
        })?;
        nodes[index].initialize_instrument(Some(compiled), rate)?;
    }
    Ok(())
}

fn apply_modulation_to_parameter(nodes: &mut [NodeState], edge: &ModulationRuntime) -> Result<()> {
    let product = edge.amount * nodes[edge.from].control_value;
    if !product.is_finite() {
        return Err(RenderError::Nonfinite(format!(
            "modulation {} product is nonfinite",
            edge.id
        )));
    }
    let target = nodes[edge.to]
        .current_params
        .get_mut(&edge.parameter)
        .ok_or_else(|| RenderError::RenderState("modulation parameter missing".into()))?;
    let sum = *target + product;
    if !sum.is_finite() {
        return Err(RenderError::Nonfinite(format!(
            "modulation {} sum is nonfinite",
            edge.id
        )));
    }
    *target = sum;
    Ok(())
}

#[derive(Debug)]
enum PreparedTransport {
    Rate(PreparedAudioClip),
    Warp(crate::warp_clip::PreparedWarpClip),
}
impl PreparedTransport {
    fn coordinate(&self, frame: u64) -> Option<(u64, f64)> {
        match self {
            Self::Rate(prepared) => prepared.coordinate(frame),
            Self::Warp(prepared) => prepared
                .coordinate(frame)
                .map(|(index, fraction)| (index as u64, fraction)),
        }
    }
    fn gain_at(&self, frame: u64) -> f64 {
        match self {
            Self::Rate(prepared) => prepared.gain_at(frame),
            Self::Warp(prepared) => prepared.gain_at(frame),
        }
    }
}

/// Stateless transport evaluated from the absolute output frame after preparation.
#[derive(Debug)]
struct AudioRuntime {
    slice: OwnedAudioSlice,
    prepared: PreparedTransport,
}
impl AudioRuntime {
    fn render_frame(&self, frame: u64) -> Result<[f64; 2]> {
        let mut output = [0.; 2];
        if let Some((index, fraction)) = self.prepared.coordinate(frame) {
            let gain = self.prepared.gain_at(frame);
            for (channel, sample) in output
                .iter_mut()
                .enumerate()
                .take(usize::from(self.slice.channels()))
            {
                *sample = self.slice.interpolate(index, fraction, channel) * gain;
                if !sample.is_finite() {
                    return Err(crate::plan::err(
                        "E_NONFINITE",
                        "nodes.audio",
                        "audio clip produced a nonfinite sample",
                    )
                    .into());
                }
            }
        }
        Ok(output)
    }
}

#[derive(Clone, Debug)]
enum RuntimeProcessor {
    Control,
    Core(Processor),
    Kit,
    Audio,
}

#[derive(Debug)]
enum ControlRuntime {
    Lfo(crate::core_control::PreparedLfo),
    Constant,
}
struct ModulationRuntime {
    id: String,
    from: usize,
    to: usize,
    parameter: String,
    amount: f64,
    rate: ParameterRate,
}

#[derive(Clone, Debug)]
struct ResetTargetBounds {
    parameter: String,
    min: f64,
    max: f64,
    min_open: bool,
    max_open: bool,
}

#[derive(Clone, Copy, Debug)]
struct MatrixRuntime {
    inputs: usize,
    outputs: usize,
    coefficients: [[f64; 2]; 2],
}

impl MatrixRuntime {
    fn new(inputs: u8, outputs: u8, coefficients: &[Vec<Rational>]) -> Result<Self> {
        let inputs = usize::from(inputs);
        let outputs = usize::from(outputs);
        let mut prepared = [[0.0; 2]; 2];
        for (output, row) in prepared.iter_mut().enumerate().take(outputs) {
            let source = coefficients.get(output).ok_or_else(|| {
                RenderError::RenderState("matrix coefficient row is missing".into())
            })?;
            for (input, coefficient) in row.iter_mut().enumerate().take(inputs) {
                *coefficient = rational_f64(
                    source.get(input).ok_or_else(|| {
                        RenderError::RenderState("matrix coefficient is missing".into())
                    })?,
                    "matrix coefficient",
                )?;
            }
        }
        Ok(Self {
            inputs,
            outputs,
            coefficients: prepared,
        })
    }
}

#[derive(Debug)]
struct DelayRuntime {
    channels: usize,
    frames: usize,
    cursor: usize,
    history: Vec<f64>,
    pending: [f64; 2],
}

impl DelayRuntime {
    fn new(channels: u8, frames: u64) -> Result<Self> {
        let channels = usize::from(channels);
        let frames = usize::try_from(frames).map_err(|_| {
            delay_resource_error("delay frame count cannot be represented by this runtime")
        })?;
        let cells = frames.checked_mul(channels).ok_or_else(|| {
            delay_resource_error("delay storage arithmetic overflowed this runtime")
        })?;
        let mut history = Vec::new();
        history
            .try_reserve_exact(cells)
            .map_err(|_| delay_resource_error("delay storage allocation failed"))?;
        history.resize(cells, 0.0);
        Ok(Self {
            channels,
            frames,
            cursor: 0,
            history,
            pending: [0.0; 2],
        })
    }

    fn read(&self) -> [f64; 2] {
        let mut output = [0.0; 2];
        let offset = self.cursor * self.channels;
        output[..self.channels].copy_from_slice(&self.history[offset..offset + self.channels]);
        output
    }

    fn stage(&mut self, input: &[f64]) -> Result<()> {
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err(RenderError::Nonfinite(
                "delay input contains a nonfinite sample".into(),
            ));
        }
        self.pending[..self.channels].copy_from_slice(input);
        Ok(())
    }

    fn commit(&mut self) {
        let offset = self.cursor * self.channels;
        self.history[offset..offset + self.channels]
            .copy_from_slice(&self.pending[..self.channels]);
        self.cursor = (self.cursor + 1) % self.frames;
    }

    fn reset(&mut self) {
        self.cursor = 0;
        self.history.fill(0.0);
        self.pending.fill(0.0);
    }
}

fn delay_resource_error(message: &str) -> RenderError {
    crate::plan::err("E_RESOURCE_LIMIT", "nodes.delay", message).into()
}

#[derive(Clone, Debug)]
struct NoiseRuntime {
    prefix: Sha256,
}

impl NoiseRuntime {
    fn new(node: &str, seed: u64) -> Self {
        let mut prefix = Sha256::new();
        prefix.update(b"maac-noise-1\0");
        prefix.update(seed.to_le_bytes());
        prefix.update(node.as_bytes());
        prefix.update([0]);
        Self { prefix }
    }

    fn sample(&self, frame: u64, channel: u32) -> f64 {
        let mut hasher = self.prefix.clone();
        hasher.update(frame.to_le_bytes());
        hasher.update(channel.to_le_bytes());
        let digest = hasher.finalize();
        let mut first = [0; 8];
        first.copy_from_slice(&digest[..8]);
        let random = u64::from_le_bytes(first) >> 11;
        2.0 * (random as f64 / 9_007_199_254_740_992.0) - 1.0
    }
}

#[derive(Debug)]
struct NodeState {
    id: String,
    processor: RuntimeProcessor,
    control: Option<ControlRuntime>,
    control_value: f64,
    kit: Option<KitRuntime>,
    audio: Option<AudioRuntime>,
    base_params: BTreeMap<String, f64>,
    current_params: BTreeMap<String, f64>,
    automations: BTreeMap<String, AutomationBinding>,
    output: Vec<f64>,
    onepole_previous: Vec<f64>,
    voices: Vec<Voice>,
    instrument: Option<InstrumentRuntime>,
    eq: Option<Eq>,
    compressor: Option<Compressor>,
    reverb: Option<Reverb>,
    noise: Option<NoiseRuntime>,
    matrix: Option<MatrixRuntime>,
    delay: Option<DelayRuntime>,
    native_bounds: BTreeMap<String, (f64, f64, bool)>,
}

impl NodeState {
    fn new(
        id: String,
        processor: Processor,
        rate: f64,
        compiled: Option<&Arc<CompiledInstrument>>,
    ) -> Result<Self> {
        let channels = match processor {
            Processor::Sine { .. } => 1,
            Processor::OnePole { channels }
            | Processor::Gain { channels }
            | Processor::Fader { channels }
            | Processor::Eq { channels, .. }
            | Processor::Compressor { channels, .. }
            | Processor::Reverb { channels, .. }
            | Processor::Sum { channels }
            | Processor::Noise { channels, .. } => usize::from(channels),
            Processor::Delay { channels, .. } => usize::from(channels),
            Processor::Matrix { outputs, .. } => usize::from(outputs),
            Processor::Pan => 2,
            Processor::Instrument { channels, .. } => usize::from(channels),
        };
        let mut base_params = BTreeMap::new();
        match processor {
            Processor::Sine { .. } => {
                base_params.insert("attack".into(), 0.005);
                base_params.insert("release".into(), 0.080);
                base_params.insert("level".into(), 0.2);
            }
            Processor::OnePole { .. } => {
                base_params.insert("cutoff".into(), 1000.0);
            }
            Processor::Gain { .. } => {
                base_params.insert("gain".into(), 1.0);
            }
            Processor::Fader { .. } => {
                base_params.insert("level".into(), 0.0);
            }
            Processor::Pan => {
                base_params.insert("pan".into(), 0.0);
            }
            Processor::Sum { .. }
            | Processor::Noise { .. }
            | Processor::Matrix { .. }
            | Processor::Delay { .. } => {}
            Processor::Eq { .. } | Processor::Compressor { .. } | Processor::Reverb { .. } => {
                for (name, value) in crate::plan::production_defaults(&processor) {
                    base_params.insert(name, rational_f64(&value, "native default")?);
                }
            }
            Processor::Instrument { .. } => {
                base_params = compiled
                    .ok_or_else(|| {
                        RenderError::RenderState("instrument program was not compiled".into())
                    })?
                    .default_controls();
            }
        }
        let eq = if let Processor::Eq { channels, mode } = processor {
            Some(Eq::new(channels, mode)?)
        } else {
            None
        };
        let compressor = if let Processor::Compressor {
            channels,
            sidechain_channels,
        } = processor
        {
            Some(Compressor::new(channels, sidechain_channels)?)
        } else {
            None
        };
        let reverb = if let Processor::Reverb {
            channels,
            predelay_frames,
            ref damping,
        } = processor
        {
            Some(Reverb::new(
                channels,
                predelay_frames as usize,
                rational_f64(damping, "damping")?,
            )?)
        } else {
            None
        };
        let matrix = if let Processor::Matrix {
            inputs,
            outputs,
            ref coefficients,
        } = processor
        {
            Some(MatrixRuntime::new(inputs, outputs, coefficients)?)
        } else {
            None
        };
        let noise = if let Processor::Noise { seed, .. } = processor {
            Some(NoiseRuntime::new(&id, seed))
        } else {
            None
        };
        let delay = if let Processor::Delay { channels, frames } = processor {
            Some(DelayRuntime::new(channels, frames)?)
        } else {
            None
        };
        let mut native_bounds = BTreeMap::new();
        for name in base_params.keys() {
            if let Some((min, max, open)) =
                crate::plan::production_parameter_bounds(&processor, name)
            {
                native_bounds.insert(
                    name.clone(),
                    (
                        rational_f64(&min, "native lower bound")?,
                        rational_f64(&max, "native upper bound")?,
                        open,
                    ),
                );
            }
        }
        let state = Self {
            id,
            processor: RuntimeProcessor::Core(processor),
            control: None,
            control_value: 0.,
            kit: None,
            audio: None,
            current_params: base_params.clone(),
            base_params,
            automations: BTreeMap::new(),
            output: vec![0.0; channels],
            onepole_previous: vec![0.0; channels],
            voices: Vec::new(),
            instrument: None,
            eq,
            compressor,
            reverb,
            noise,
            matrix,
            delay,
            native_bounds,
        };
        state.validate_current_parameters(rate)?;
        Ok(state)
    }

    fn new_kit(id: String, channels: u8, kit: KitRuntime) -> Result<Self> {
        let base_params = BTreeMap::from([("level".into(), 1.0)]);
        Ok(Self {
            id,
            processor: RuntimeProcessor::Kit,
            control: None,
            control_value: 0.,
            kit: Some(kit),
            audio: None,
            current_params: base_params.clone(),
            base_params,
            automations: BTreeMap::new(),
            output: vec![0.0; usize::from(channels)],
            onepole_previous: Vec::new(),
            voices: Vec::new(),
            instrument: None,
            eq: None,
            compressor: None,
            reverb: None,
            noise: None,
            matrix: None,
            delay: None,
            native_bounds: BTreeMap::new(),
        })
    }

    fn new_audio(id: String, audio: AudioRuntime) -> Self {
        let channels = usize::from(audio.slice.channels());
        Self {
            id,
            processor: RuntimeProcessor::Audio,
            control: None,
            control_value: 0.,
            kit: None,
            audio: Some(audio),
            base_params: BTreeMap::new(),
            current_params: BTreeMap::new(),
            automations: BTreeMap::new(),
            output: vec![0.; channels],
            onepole_previous: Vec::new(),
            voices: Vec::new(),
            instrument: None,
            eq: None,
            compressor: None,
            reverb: None,
            noise: None,
            matrix: None,
            delay: None,
            native_bounds: BTreeMap::new(),
        }
    }

    fn new_control(id: String, control: ControlRuntime) -> Self {
        let base_params = if matches!(control, ControlRuntime::Constant) {
            BTreeMap::from([("value".into(), 0.)])
        } else {
            BTreeMap::new()
        };
        Self {
            id,
            processor: RuntimeProcessor::Control,
            control: Some(control),
            control_value: 0.,
            kit: None,
            audio: None,
            current_params: base_params.clone(),
            base_params,
            automations: BTreeMap::new(),
            output: Vec::new(),
            onepole_previous: Vec::new(),
            voices: Vec::new(),
            instrument: None,
            eq: None,
            compressor: None,
            reverb: None,
            noise: None,
            matrix: None,
            delay: None,
            native_bounds: BTreeMap::new(),
        }
    }

    fn validate_modulated_parameters(&mut self, rate: f64) -> Result<()> {
        if self.current_params.values().any(|v| !v.is_finite()) {
            return Err(RenderError::Nonfinite(
                "combined parameter is nonfinite".into(),
            ));
        }
        // Pan is the core clamp-policy parameter; clamp only the final sum.
        if matches!(self.processor, RuntimeProcessor::Core(Processor::Pan)) {
            if let Some(pan) = self.current_params.get_mut("pan") {
                *pan = pan.clamp(-1., 1.);
            }
        }
        self.validate_current_parameters(rate).map_err(|_| {
            crate::plan::err(
                "E_RANGE",
                "modulations.target",
                "combined parameter is outside its declared range",
            )
            .into()
        })
    }

    fn initialize_instrument(
        &mut self,
        compiled: Option<Arc<CompiledInstrument>>,
        rate: f64,
    ) -> Result<()> {
        if let Some(compiled) = compiled {
            let capacity = match self.processor {
                RuntimeProcessor::Core(Processor::Instrument { voices, .. }) => voices,
                _ => {
                    return Err(RenderError::RenderState(
                        "compiled instrument attached to a legacy processor".into(),
                    ))
                }
            };
            self.instrument = Some(InstrumentRuntime::new(
                compiled,
                capacity,
                rate,
                &self.current_params,
            )?);
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.control_value = 0.;
        self.output.fill(0.0);
        self.onepole_previous.fill(0.0);
        self.voices.clear();
        if let Some(kit) = &mut self.kit {
            kit.reset();
        }
        if let Some(eq) = &mut self.eq {
            eq.reset();
        }
        if let Some(compressor) = &mut self.compressor {
            compressor.reset();
        }
        if let Some(reverb) = &mut self.reverb {
            reverb.reset();
        }
        if let Some(delay) = &mut self.delay {
            delay.reset();
        }
        self.current_params.clone_from(&self.base_params);
        if let Some(instrument) = &mut self.instrument {
            instrument.reset_state();
        }
    }

    fn current_param(&self, name: &str) -> f64 {
        self.current_params.get(name).copied().unwrap_or(0.0)
    }

    fn validate_current_parameters(&self, rate: f64) -> Result<()> {
        for (name, value) in &self.current_params {
            if !value.is_finite() {
                return Err(RenderError::Nonfinite(format!(
                    "parameter {name} on {} is nonfinite",
                    self.id
                )));
            }
            if let Some(&(min, max, open)) = self.native_bounds.get(name) {
                if if open {
                    *value <= min || *value >= max
                } else {
                    *value < min || *value > max
                } {
                    return Err(RenderError::Nonfinite(format!(
                        "native parameter {name} is outside its range"
                    )));
                }
            }
            match (&self.processor, name.as_str()) {
                (RuntimeProcessor::Kit, "level") if *value < 0.0 => {
                    return Err(RenderError::Nonfinite(format!(
                        "kit level on {} is negative",
                        self.id
                    )));
                }
                (RuntimeProcessor::Core(Processor::Gain { .. }), "gain") if *value < 0.0 => {
                    return Err(RenderError::Nonfinite(format!(
                        "gain on {} must be nonnegative",
                        self.id
                    )))
                }
                (
                    RuntimeProcessor::Core(Processor::Sine { .. }),
                    "attack" | "release" | "level",
                ) if *value < 0.0 => {
                    return Err(RenderError::RenderState(format!(
                        "parameter {name} on {} is negative",
                        self.id
                    )))
                }
                (RuntimeProcessor::Core(Processor::OnePole { .. }), "cutoff")
                    if *value <= 0.0 || *value >= rate / 2.0 =>
                {
                    return Err(RenderError::RenderState(format!(
                        "cutoff on {} is outside (0, Nyquist)",
                        self.id
                    )))
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn prune_finished_voices(&mut self, frame: u64, rate: f64) -> Result<()> {
        self.voices
            .retain(|voice| !voice.released || voice.envelope(frame, rate) > 0.0);
        if let Some(instrument) = &mut self.instrument {
            instrument.prune_finished(frame)?;
        }
        Ok(())
    }

    fn sine_sample(&mut self, frame: u64, rate: f64, level: f64) -> Result<f64> {
        let mut sum = 0.0;
        for voice in &mut self.voices {
            let envelope = voice.envelope(frame, rate);
            let gain = if let Some(expression) = &voice.gain_expression {
                let gain = expression
                    .curve
                    .gain_at(&expression.coordinate_at(voice.on_frame, frame));
                if !gain.is_finite() || gain < 0.0 {
                    return Err(RenderError::Nonfinite(format!(
                        "gain expression at {} produced an invalid value",
                        voice.address
                    )));
                }
                gain
            } else {
                1.0
            };
            let value = voice.velocity * gain * envelope * level * voice.phase.sin();
            if !value.is_finite() {
                return Err(RenderError::Nonfinite(format!(
                    "sine node {} produced a nonfinite value",
                    self.id
                )));
            }
            sum += value;
            let pitch_hz = if let Some(expression) = &voice.pitch_expression {
                let coordinate = expression.coordinate_at(voice.on_frame, frame);
                let cents = expression
                    .curve
                    .cents_at(&coordinate)
                    .to_f64()
                    .ok_or_else(|| {
                        RenderError::Nonfinite(
                            "pitch expression cents cannot be represented".into(),
                        )
                    })?;
                let frequency = voice.pitch_hz * 2.0_f64.powf(cents / 1200.0);
                if !frequency.is_finite() || frequency <= 0.0 || frequency >= rate / 2.0 {
                    return Err(RenderError::RenderState(format!(
                        "pitch expression at {} is outside (0, Nyquist)",
                        voice.address
                    )));
                }
                frequency
            } else {
                voice.pitch_hz
            };
            let phase_increment = 2.0 * std::f64::consts::PI * pitch_hz / rate;
            voice.phase = (voice.phase + phase_increment).rem_euclid(2.0 * std::f64::consts::PI);
        }
        // A release voice is retained through the frame at which its envelope
        // reaches zero, then freed.  This preserves the specified allocation
        // lifecycle while avoiding another frame of stale state.
        self.voices.retain(|voice| {
            if !voice.released {
                return true;
            }
            voice.envelope(frame, rate) > 0.0
        });
        if !sum.is_finite() {
            return Err(RenderError::Nonfinite(format!(
                "sine node {} produced a nonfinite sum",
                self.id
            )));
        }
        Ok(sum)
    }
}

#[derive(Clone, Debug)]
struct Voice {
    address: String,
    pitch_hz: f64,
    pitch_expression: Option<ExpressionRuntime<PitchExpression>>,
    gain_expression: Option<ExpressionRuntime<GainExpression>>,
    velocity: f64,
    on_frame: u64,
    attack: f64,
    release: f64,
    phase: f64,
    released: bool,
    release_frame: u64,
    release_amplitude: f64,
}

impl Voice {
    fn attack_amplitude(&self, frame: u64, rate: f64) -> f64 {
        if self.attack <= 0.0 {
            1.0
        } else {
            ((frame.saturating_sub(self.on_frame) as f64) / (self.attack * rate)).min(1.0)
        }
    }

    fn envelope(&self, frame: u64, rate: f64) -> f64 {
        if !self.released {
            return self.attack_amplitude(frame, rate);
        }
        let elapsed = frame.saturating_sub(self.release_frame) as f64;
        if self.release <= 0.0 {
            0.0
        } else {
            self.release_amplitude * (1.0 - elapsed / (self.release * rate)).max(0.0)
        }
    }
}

#[derive(Clone, Debug)]
struct EventRuntime {
    address: String,
    node: usize,
    data: EventData,
    on_frame: u64,
    off_frame: Option<u64>,
    order: i32,
}

#[derive(Clone, Debug)]
// Retain inline preallocated expression state without a new per-note heap allocation.
// Event count remains bounded by plan limits.
#[allow(clippy::large_enum_variant)]
enum EventData {
    Hit {
        key: String,
        velocity: f64,
    },
    Note {
        pitch_hz: f64,
        velocity: f64,
        pitch_expression: Option<ExpressionRuntime<PitchExpression>>,
        gain_expression: Option<ExpressionRuntime<GainExpression>>,
        timbre_expression: Option<ExpressionRuntime<TimbreExpression>>,
        pressure_expression: Option<ExpressionRuntime<PressureExpression>>,
    },
}

impl EventRuntime {
    fn from_event(event: &EventView<'_>, node: usize, rate: u32) -> Result<Self> {
        if let EventKind::Hit { key, velocity } = event.kind {
            return Ok(Self {
                address: event.address.clone(),
                node,
                data: EventData::Hit {
                    key: key.clone(),
                    velocity: rational_f64(velocity, "hit velocity")?,
                },
                on_frame: event.on_frame,
                off_frame: None,
                order: event.order,
            });
        }
        let EventKind::Note {
            pressure_expression,
            timbre_expression,
            pitch_hz,
            velocity,
            pitch_expression,
            gain_expression,
        } = &event.kind
        else {
            return Err(RenderError::RenderState(format!(
                "unsupported event kind at {}",
                event.address
            )));
        };
        let velocity = velocity.to_f64().ok_or_else(|| {
            RenderError::Nonfinite(format!(
                "velocity at {} cannot be represented",
                event.address
            ))
        })?;
        if !velocity.is_finite() || !pitch_hz.is_finite() {
            return Err(RenderError::Nonfinite(format!(
                "event {} has a nonfinite value",
                event.address
            )));
        }
        Ok(Self {
            address: event.address.clone(),
            node,
            data: EventData::Note {
                pitch_hz: *pitch_hz,
                velocity,
                timbre_expression: timbre_expression.as_ref().map(|curve| ExpressionRuntime {
                    coordinate_per_frame: curve.clock.coordinate_for_view(
                        event,
                        event.on_frame + 1,
                        rate,
                    ),
                    gate: event.off_frame.expect("validated note off") - event.on_frame,
                    curve: Arc::new(curve.clone()),
                }),
                pressure_expression: pressure_expression.as_ref().map(|curve| ExpressionRuntime {
                    coordinate_per_frame: curve.clock.coordinate_for_view(
                        event,
                        event.on_frame + 1,
                        rate,
                    ),
                    gate: event.off_frame.expect("validated note off") - event.on_frame,
                    curve: Arc::new(curve.clone()),
                }),
                gain_expression: gain_expression.as_ref().map(|curve| ExpressionRuntime {
                    coordinate_per_frame: curve.clock.coordinate_for_view(
                        event,
                        event.on_frame + 1,
                        rate,
                    ),
                    gate: event.off_frame.expect("validated note off") - event.on_frame,
                    curve: Arc::new(curve.clone()),
                }),
                pitch_expression: pitch_expression.as_ref().map(|curve| ExpressionRuntime {
                    coordinate_per_frame: curve.clock.coordinate_for_view(
                        event,
                        event.on_frame + 1,
                        rate,
                    ),
                    gate: event.off_frame.expect("validated note off") - event.on_frame,
                    curve: Arc::new(curve.clone()),
                }),
            },
            on_frame: event.on_frame,
            off_frame: event.off_frame,
            order: event.order,
        })
    }
}

#[derive(Clone, Debug)]
struct ConnectionRuntime {
    id: String,
    from: usize,
    to: usize,
    to_port: String,
}

fn sort_event_indices(events: &mut BTreeMap<u64, Vec<usize>>, all: &[EventRuntime]) {
    for indices in events.values_mut() {
        indices.sort_by(|left, right| {
            all[*left].order.cmp(&all[*right].order).then_with(|| {
                all[*left]
                    .address
                    .as_bytes()
                    .cmp(all[*right].address.as_bytes())
            })
        });
    }
}

fn stable_topological_order(
    nodes: &[NodeState],
    connections: &[ConnectionRuntime],
    modulations: &[ModulationRuntime],
) -> Result<Vec<usize>> {
    let mut indegree = vec![0usize; nodes.len()];
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for connection in connections {
        if nodes[connection.from].delay.is_some() || nodes[connection.to].delay.is_some() {
            continue;
        }
        indegree[connection.to] = indegree[connection.to].saturating_add(1);
        outgoing[connection.from].push(connection.to);
    }
    for edge in modulations {
        indegree[edge.to] = indegree[edge.to].saturating_add(1);
        outgoing[edge.from].push(edge.to);
    }
    let mut ready: BTreeSet<(Vec<u8>, usize)> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            (indegree[index] == 0).then_some((node.id.as_bytes().to_vec(), index))
        })
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some((_, node)) = ready.pop_first() {
        order.push(node);
        for destination in &outgoing[node] {
            indegree[*destination] -= 1;
            if indegree[*destination] == 0 {
                ready.insert((nodes[*destination].id.as_bytes().to_vec(), *destination));
            }
        }
    }
    if order.len() != nodes.len() {
        return Err(RenderError::RenderState(
            "same-sample graph contains a cycle".into(),
        ));
    }
    Ok(order)
}

struct TempoRuntime {
    /// Absolute `T(score_start)` retained exactly. Seconds-clock automation
    /// anchors are authored in this global coordinate, while frame numbers
    /// are always relative to this reset origin.
    origin_seconds: Option<Rational>,
    points: Vec<TempoRuntimePoint>,
    rate: f64,
    score_end_relative_q: f64,
    score_end_seconds: Option<Rational>,
    score_end_frame: u64,
    rate_hz: u64,
}

#[derive(Clone, Debug)]
struct TempoRuntimePoint {
    /// These floating values are local coordinates. Exact rational
    /// differences are taken before conversion so a large absolute origin
    /// cannot erase a sample or automation boundary.
    q: f64,
    seconds: f64,
    bpm: f64,
    slope: f64,
    frame: i64,
}

impl TempoRuntime {
    fn new(plan: &PlanView<'_>, timing: Option<&TimingContext>) -> Result<Self> {
        if matches!(plan.version, 3..=7) {
            let timing = timing.ok_or_else(|| {
                RenderError::RenderState("certified plan validation omitted timing".into())
            })?;
            return Self::new_v3(plan, timing);
        }
        let score_start_q = plan.output.score_start_q.clone();
        let score_end_q = plan.output.score_end_q.clone();
        let origin_seconds = plan
            .tempo
            .seconds_at(&score_start_q)
            .map_err(RenderError::Plan)?;
        let score_end_seconds = plan
            .tempo
            .seconds_at(&score_end_q)
            .map_err(RenderError::Plan)?;
        let rate_hz = u64::from(plan.output.sample_rate_hz);
        let score_end_relative_seconds = score_end_seconds.clone() - origin_seconds.clone();
        let score_end_frame_i64 = ceil_rational_to_i64(
            &(score_end_relative_seconds * BigInt::from(rate_hz)),
            "score-end frame",
        )?;
        if score_end_frame_i64 < 0 {
            return Err(RenderError::RenderState(
                "score end precedes the reset origin".into(),
            ));
        }
        // Anchor the runtime at the reset origin. A tempo point far before a
        // huge score start can have an unrepresentable absolute frame even
        // though every rendered frame is ordinary; retaining its absolute
        // coordinate would also lose local score precision in binary64.
        let active_bpm = plan
            .tempo
            .points
            .iter()
            .take_while(|point| point.q <= score_start_q)
            .last()
            .or_else(|| plan.tempo.points.first())
            .expect("validated tempo map is nonempty")
            .bpm
            .clone();
        let mut points = Vec::with_capacity(plan.tempo.points.len());
        points.push(TempoRuntimePoint {
            q: 0.0,
            seconds: 0.0,
            bpm: rational_f64(&active_bpm, "tempo BPM")?,
            slope: 0.0,
            frame: 0,
        });
        for point in &plan.tempo.points {
            if point.q <= score_start_q || point.q >= score_end_q {
                continue;
            }
            let point_seconds = plan.tempo.seconds_at(&point.q).map_err(RenderError::Plan)?;
            let relative_seconds = point_seconds - origin_seconds.clone();
            let point_frame = ceil_rational_to_i64(
                &(relative_seconds.clone() * BigInt::from(rate_hz)),
                "tempo frame boundary",
            )?;
            let relative_q = point.q.clone() - score_start_q.clone();
            points.push(TempoRuntimePoint {
                q: rational_f64(&relative_q, "tempo position")?,
                seconds: rational_f64(&relative_seconds, "tempo time")?,
                bpm: rational_f64(&point.bpm, "tempo BPM")?,
                slope: 0.0,
                frame: point_frame,
            });
        }
        let score_end_relative_q = score_end_q - score_start_q;
        Ok(Self {
            origin_seconds: Some(origin_seconds),
            points,
            rate: f64::from(plan.output.sample_rate_hz),
            score_end_relative_q: rational_f64(&score_end_relative_q, "score end")?,
            score_end_seconds: Some(score_end_seconds),
            score_end_frame: u64::try_from(score_end_frame_i64)
                .map_err(|_| RenderError::RenderState("score-end frame exceeds u64".into()))?,
            rate_hz,
        })
    }

    fn new_v3(plan: &PlanView<'_>, timing: &TimingContext) -> Result<Self> {
        let origin = &plan.output.score_start_q;
        let end = &plan.output.score_end_q;
        let rate_hz = u64::from(plan.output.sample_rate_hz);
        let mut points = Vec::new();
        // Start at the exact reset coordinate, including a reset inside a ramp.
        // All score differences are rational before conversion to local doubles.
        for q in std::iter::once(origin).chain(
            plan.tempo
                .points
                .iter()
                .map(|p| &p.q)
                .filter(|q| *q > origin && *q < end),
        ) {
            let index = plan
                .tempo
                .points
                .partition_point(|p| &p.q <= q)
                .saturating_sub(1);
            let point = &plan.tempo.points[index];
            let slope = if q >= &point.q && point.shape == Interpolation::Linear {
                let next = &plan.tempo.points[index + 1];
                (&next.bpm - &point.bpm) / (&next.q - &point.q)
            } else {
                Rational::zero()
            };
            let bpm = &point.bpm + &slope * (q - &point.q);
            let relative = timing.relative_at_score(q, &Rational::zero())?;
            points.push(TempoRuntimePoint {
                q: rational_f64(&(q - origin), "tempo position")?,
                seconds: timing.approximate_time(&relative)?,
                bpm: rational_f64(&bpm, "tempo BPM")?,
                slope: rational_f64(&slope, "tempo slope")?,
                frame: saturating_frame(timing.ceil_time(&relative, rate_hz)?),
            });
        }
        let score_end_frame = timing
            .ceil_time(&timing.end, rate_hz)?
            .to_u64()
            .ok_or_else(|| RenderError::RenderState("score-end frame exceeds u64".into()))?;
        Ok(Self {
            origin_seconds: None,
            score_end_seconds: None,
            points,
            rate: f64::from(plan.output.sample_rate_hz),
            score_end_relative_q: rational_f64(&(end - origin), "score end")?,
            score_end_frame,
            rate_hz,
        })
    }

    fn score_at_frame(&self, frame: u64) -> Result<f64> {
        if self.points.is_empty() {
            return Ok(0.0);
        }
        if frame >= self.score_end_frame {
            return Ok(self.score_end_relative_q);
        }
        let frame_i64 = i64::try_from(frame).unwrap_or(i64::MAX);
        let point_index = self
            .points
            .partition_point(|point| point.frame <= frame_i64);
        let current_index = point_index.saturating_sub(1);
        let current = &self.points[current_index];
        let seconds = frame as f64 / self.rate;
        let elapsed = seconds - current.seconds;
        // Keep the legacy constant-segment operation order unchanged. For a
        // ramp, expm1(x)/x avoids division by a tiny slope and cancellation.
        let delta = elapsed * current.bpm / 60.0;
        let x = current.slope * elapsed / 60.0;
        let score = current.q
            + if x == 0.0 {
                delta
            } else {
                delta * (x.exp_m1() / x)
            };
        if !score.is_finite() {
            return Err(RenderError::Nonfinite("inverse tempo is nonfinite".into()));
        }
        // The local coordinate should already be before score end because the
        // exact frame boundary above handled the physical tail. The clamp is
        // only a finite guard for the final binary64 conversion.
        Ok(score.min(self.score_end_relative_q))
    }

    fn frame_for_score(&self, plan: &PlanView<'_>, q: &Rational) -> Result<i64> {
        let seconds = plan.tempo.seconds_at(q).map_err(RenderError::Plan)?
            - self.origin_seconds.as_ref().expect("legacy clock").clone();
        Ok(ceil_rational_saturating_to_i64(
            &(seconds * BigInt::from(self.rate_hz)),
        ))
    }

    fn frame_for_seconds(&self, absolute_seconds: &Rational) -> Result<i64> {
        let relative =
            absolute_seconds.clone() - self.origin_seconds.as_ref().expect("legacy clock").clone();
        Ok(ceil_rational_saturating_to_i64(
            &(relative * BigInt::from(self.rate_hz)),
        ))
    }
}

#[derive(Clone, Debug)]
struct AutomationBinding {
    id: String,
    node: usize,
    parameter: String,
    lane: AutomationRuntime,
}

#[derive(Clone, Debug)]
struct AutomationRuntime {
    clock: AutomationClock,
    /// Local floating anchor used only after exact frame selection. The
    /// authored seconds anchor itself remains absolute in the plan.
    at: f64,
    at_exact: Option<Rational>,
    points: Vec<AutomationPointRuntime>,
    point_positions_exact: Vec<Rational>,
    knot_frames: Vec<i64>,
    base: f64,
    tail_value: f64,
}

#[derive(Clone, Debug)]
struct AutomationPointRuntime {
    position: f64,
    value: f64,
    shape: Interpolation,
}

impl AutomationRuntime {
    fn value_at(&self, frame: u64, tempo: &TempoRuntime) -> Result<f64> {
        // Both score- and seconds-clock automation stop advancing when the
        // score reaches its physical end. Evaluate the lane at that exact end
        // coordinate so a later knot cannot become active during the tail.
        if frame >= tempo.score_end_frame {
            return Ok(self.tail_value);
        }
        let frame_i64 = i64::try_from(frame).unwrap_or(i64::MAX);
        let Some(mut index) = self.knot_frames.iter().rposition(|knot| *knot <= frame_i64) else {
            return Ok(self.base);
        };
        if index >= self.points.len() {
            index = self.points.len() - 1;
        }
        if index + 1 >= self.points.len() {
            return Ok(self.points[index].value);
        }
        let coordinate = match self.clock {
            AutomationClock::Score => tempo.score_at_frame(frame)?,
            AutomationClock::Seconds => frame as f64 / tempo.rate,
        };
        let left = &self.points[index];
        let right = &self.points[index + 1];
        let x = coordinate - self.at;
        let denominator = right.position - left.position;
        let u = if denominator == 0.0 {
            1.0
        } else {
            ((x - left.position) / denominator).clamp(0.0, 1.0)
        };
        let value = match left.shape {
            Interpolation::Step => left.value,
            Interpolation::Linear => left.value + (right.value - left.value) * u,
            Interpolation::Exponential => {
                if left.value <= 0.0 || right.value <= 0.0 {
                    return Err(RenderError::RenderState(
                        "exponential automation requires positive endpoints".into(),
                    ));
                }
                left.value * (right.value / left.value).powf(u)
            }
        };
        Ok(value)
    }

    fn value_at_exact(&self, coordinate: &Rational) -> Result<f64> {
        let relative = coordinate.clone() - self.at_exact.as_ref().expect("legacy anchor").clone();
        if relative.is_negative() {
            return Ok(self.base);
        }
        let mut index = self
            .point_positions_exact
            .partition_point(|position| position <= &relative)
            .saturating_sub(1);
        if index >= self.points.len() {
            index = self.points.len() - 1;
        }
        if index + 1 >= self.points.len() {
            return Ok(self.points[index].value);
        }
        let left_position = &self.point_positions_exact[index];
        let right_position = &self.point_positions_exact[index + 1];
        let denominator = right_position.clone() - left_position.clone();
        let u = rational_f64(
            &((relative - left_position.clone()) / denominator),
            "automation interpolation",
        )?
        .clamp(0.0, 1.0);
        let left = &self.points[index];
        let right = &self.points[index + 1];
        let value = match left.shape {
            Interpolation::Step => left.value,
            Interpolation::Linear => left.value + (right.value - left.value) * u,
            Interpolation::Exponential => {
                if left.value <= 0.0 || right.value <= 0.0 {
                    return Err(RenderError::RenderState(
                        "exponential automation requires positive endpoints".into(),
                    ));
                }
                left.value * (right.value / left.value).powf(u)
            }
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(RenderError::Nonfinite(
                "automation interpolation is nonfinite".into(),
            ))
        }
    }
}

fn build_automations(
    plan: &PlanView<'_>,
    node_indices: &HashMap<String, usize>,
    tempo: &TempoRuntime,
    rate: f64,
    timing: Option<&TimingContext>,
) -> Result<Vec<AutomationBinding>> {
    let mut result = Vec::with_capacity(plan.automations().len());
    for automation in plan.automations() {
        let node = *node_indices.get(&automation.target.node).ok_or_else(|| {
            RenderError::RenderState(format!("automation {} has no target node", automation.id))
        })?;
        // An authored node parameter replaces the processor default before an
        // automation lane begins. The default applies only when the node did
        // not author that parameter at all.
        let resolved_params = plan
            .resolved_node_params(plan.nodes.get(node).expect("indexed node exists"))
            .map_err(RenderError::Plan)?;
        let base = match resolved_params.get(&automation.target.port) {
            Some(value) => rational_f64(value, "automation base parameter")?,
            None => match plan.nodes.get(node).expect("indexed node exists").processor {
                ProcessorView::Core(processor) => {
                    default_parameter(processor, &automation.target.port)
                }
                ProcessorView::Kit { .. } => 1.0,
                ProcessorView::Constant => 0.0,
                ProcessorView::Lfo(_) | ProcessorView::Audio(_) | ProcessorView::WarpRate(_) => {
                    return Err(crate::plan::err(
                        "E_CAPABILITY",
                        "nodes.audio",
                        "audio rendering is not supported at this boundary yet",
                    )
                    .into())
                }
            },
        };
        if let Some(timing) = timing {
            result.push(build_automation_v3(
                &automation,
                node,
                base,
                timing,
                tempo,
                &plan.output.score_end_q,
            )?);
            continue;
        }
        let anchor = match automation.anchor {
            AutomationAnchorView::Score(q) | AutomationAnchorView::Seconds(q) => q,
        };
        let at_exact = match automation.clock {
            AutomationClock::Score => anchor.clone() - plan.output.score_start_q.clone(),
            AutomationClock::Seconds => {
                anchor.clone() - tempo.origin_seconds.as_ref().expect("legacy clock").clone()
            }
        };
        let at = rational_f64(&at_exact, "automation anchor")?;
        let mut points = Vec::with_capacity(automation.points.len());
        let mut point_positions_exact = Vec::with_capacity(automation.points.len());
        let mut knot_frames = Vec::with_capacity(automation.points.len());
        for point in automation.points {
            let position = rational_f64(&point.position, "automation position")?;
            let value = rational_f64(&point.value, "automation value")?;
            let absolute = match automation.clock {
                AutomationClock::Score => anchor.clone() + point.position.clone(),
                AutomationClock::Seconds => anchor.clone() + point.position.clone(),
            };
            let frame = match automation.clock {
                AutomationClock::Score => tempo.frame_for_score(plan, &absolute)?,
                // `automation.at` is absolute T, while this frame is relative
                // to the reset origin T(score.start).
                AutomationClock::Seconds => tempo.frame_for_seconds(&absolute)?,
            };
            points.push(AutomationPointRuntime {
                position,
                value,
                shape: point.shape,
            });
            point_positions_exact.push(point.position.clone());
            knot_frames.push(frame);
        }
        let tail_coordinate = match automation.clock {
            AutomationClock::Score => {
                plan.output.score_end_q.clone() - plan.output.score_start_q.clone()
            }
            AutomationClock::Seconds => {
                tempo
                    .score_end_seconds
                    .as_ref()
                    .expect("legacy clock")
                    .clone()
                    - tempo.origin_seconds.as_ref().expect("legacy clock").clone()
            }
        };
        let lane = AutomationRuntime {
            clock: automation.clock,
            at,
            at_exact: Some(at_exact),
            points,
            point_positions_exact,
            knot_frames,
            base,
            tail_value: 0.0,
        };
        let tail_value = lane.value_at_exact(&tail_coordinate)?;
        result.push(AutomationBinding {
            id: automation.id.clone(),
            node,
            parameter: automation.target.port.clone(),
            lane: AutomationRuntime { tail_value, ..lane },
        });
    }
    let _ = rate;
    Ok(result)
}

fn default_parameter(processor: &Processor, parameter: &str) -> f64 {
    if let Some(value) = crate::plan::production_defaults(processor).get(parameter) {
        return value.to_f64().expect("bounded native default");
    }
    match (processor, parameter) {
        (Processor::Sine { .. }, "attack") => 0.005,
        (Processor::Sine { .. }, "release") => 0.080,
        (Processor::Sine { .. }, "level") => 0.2,
        (Processor::OnePole { .. }, "cutoff") => 1000.0,
        (Processor::Gain { .. }, "gain") => 1.0,
        (Processor::Fader { .. }, "level") => 0.0,
        (Processor::Pan, "pan") => 0.0,
        _ => 0.0,
    }
}

fn rational_f64(value: &Rational, what: &str) -> Result<f64> {
    // Convert the rational as a value. Converting numerator and denominator
    // independently can turn both into infinities even when their ratio is a
    // finite binary64 value.
    let output = value.to_f64().ok_or_else(|| {
        RenderError::Nonfinite(format!("{what} cannot be represented as binary64"))
    })?;
    if output.is_finite() {
        Ok(output)
    } else {
        Err(RenderError::Nonfinite(format!("{what} is nonfinite")))
    }
}

fn ceil_rational_to_i64(value: &Rational, what: &str) -> Result<i64> {
    let (quotient, remainder) = value.numer().div_rem(value.denom());
    let rounded = if remainder.is_zero() {
        quotient
    } else if value.is_positive() {
        quotient + BigInt::from(1u8)
    } else {
        quotient
    };
    rounded
        .to_i64()
        .ok_or_else(|| RenderError::RenderState(format!("{what} frame boundary overflows i64")))
}

/// Automation knots outside the finite render horizon only need an ordering
/// sentinel for selecting the active interval. Their exact rational position
/// is still used for local interpolation and tail freezing. Saturating here
/// avoids rejecting a valid plan merely because an irrelevant global seconds
/// anchor is beyond signed-64 frame range.
fn ceil_rational_saturating_to_i64(value: &Rational) -> i64 {
    let (quotient, remainder) = value.numer().div_rem(value.denom());
    let rounded = if remainder.is_zero() {
        quotient
    } else if value.is_positive() {
        quotient + BigInt::from(1u8)
    } else {
        quotient
    };
    rounded.to_i64().unwrap_or_else(|| {
        if rounded.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}

/// One sample of the reference one-pole filter.  The state is updated only
/// after the output has been calculated, matching section 18.6 exactly.
pub fn one_pole_step(
    input: f64,
    previous: &mut f64,
    cutoff_hz: f64,
    sample_rate_hz: f64,
) -> Result<f64> {
    if !input.is_finite()
        || !previous.is_finite()
        || !cutoff_hz.is_finite()
        || !sample_rate_hz.is_finite()
        || sample_rate_hz <= 0.0
        || cutoff_hz <= 0.0
        || cutoff_hz >= sample_rate_hz / 2.0
    {
        return Err(RenderError::Nonfinite(
            "one-pole input, state, or parameter is invalid".into(),
        ));
    }
    let a = (-2.0 * std::f64::consts::PI * cutoff_hz / sample_rate_hz).exp();
    let output = (1.0 - a) * input + a * *previous;
    if !output.is_finite() {
        return Err(RenderError::Nonfinite(
            "one-pole output is nonfinite".into(),
        ));
    }
    *previous = output;
    Ok(output)
}

/// One sample of the equal-power mono-to-stereo pan reference processor.
pub fn pan_sample(input: f64, pan: f64) -> Result<[f64; 2]> {
    if !input.is_finite() || !pan.is_finite() {
        return Err(RenderError::Nonfinite(
            "pan input or parameter is nonfinite".into(),
        ));
    }
    let clamped = pan.clamp(-1.0, 1.0);
    let theta = (clamped + 1.0) * std::f64::consts::PI / 4.0;
    let output = [input * theta.cos(), input * theta.sin()];
    if output.iter().any(|sample| !sample.is_finite()) {
        return Err(RenderError::Nonfinite("pan output is nonfinite".into()));
    }
    Ok(output)
}

/// Add connection inputs left-to-right.  The caller should provide them in
/// unsigned UTF-8 connection-ID order; this helper intentionally does not
/// reorder an already resolved stream.
pub fn sum_samples(inputs: &[f64]) -> Result<f64> {
    let mut sum = 0.0;
    for input in inputs {
        if !input.is_finite() {
            return Err(RenderError::Nonfinite("sum input is nonfinite".into()));
        }
        sum += input;
    }
    if sum.is_finite() {
        Ok(sum)
    } else {
        Err(RenderError::Nonfinite("sum output is nonfinite".into()))
    }
}

/// Render selected ports in requested order while preserving the entire graph and sidechains.
pub fn render_ports_with_limits<F>(
    plan: &Plan,
    limits: &PlanLimits,
    ports: &[PortRef],
    callback: F,
) -> Result<()>
where
    F: FnMut(&[Vec<f64>]) -> Result<()>,
{
    DspEngine::new_with_limits(plan, limits)?.render_ports(ports, callback)
}

/// Render a legacy or version 3 plan using the shared DSP engine.
pub fn render_versioned<F>(plan: &VersionedPlan, callback: F) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    render_versioned_with_limits(plan, &PlanLimits::default(), callback)
}
/// Render a versioned plan under explicit caller resource limits.
pub fn render_versioned_with_limits<F>(
    plan: &VersionedPlan,
    limits: &PlanLimits,
    callback: F,
) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    DspEngine::new_versioned_with_limits(plan, limits)?.render(callback)
}
/// Render an opaque plan artifact using the shared engine.
pub fn render_artifact<F>(plan: &PlanArtifact, callback: F) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    render_artifact_with_limits(plan, &PlanLimits::default(), callback)
}
/// Render an artifact under explicit caller limits.
pub fn render_artifact_with_limits<F>(
    plan: &PlanArtifact,
    limits: &PlanLimits,
    callback: F,
) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    DspEngine::new_artifact_with_limits(plan, limits)?.render(callback)
}
/// Capture selected artifact ports while retaining complete graph context.
pub fn render_ports_artifact_with_limits<F>(
    plan: &PlanArtifact,
    limits: &PlanLimits,
    ports: &[PortRef],
    callback: F,
) -> Result<()>
where
    F: FnMut(&[Vec<f64>]) -> Result<()>,
{
    DspEngine::new_artifact_with_limits(plan, limits)?.render_ports(ports, callback)
}

#[allow(dead_code)] // Retained as the shared crate-internal IO boundary.
pub(crate) fn render_view_with_limits<F>(
    plan: PlanView<'_>,
    limits: &PlanLimits,
    callback: F,
) -> Result<()>
where
    F: FnMut(&[f64]) -> Result<()>,
{
    DspEngine::new_for_view(plan, limits)?.render(callback)
}

/// Capture selected ports from a versioned plan using caller limits.
pub fn render_ports_versioned_with_limits<F>(
    plan: &VersionedPlan,
    limits: &PlanLimits,
    ports: &[PortRef],
    callback: F,
) -> Result<()>
where
    F: FnMut(&[Vec<f64>]) -> Result<()>,
{
    DspEngine::new_versioned_with_limits(plan, limits)?.render_ports(ports, callback)
}

fn timing_error(error: crate::music::MusicError) -> RenderError {
    RenderError::Plan(PlanError {
        code: error.code.as_str().into(),
        path: "timing".into(),
        message: error.message,
        span: None,
    })
}
fn saturating_frame(frame: BigInt) -> i64 {
    frame.to_i64().unwrap_or_else(|| {
        if frame.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}

fn build_automation_v3(
    lane: &crate::plan::AutomationView<'_>,
    node: usize,
    base: f64,
    timing: &TimingContext,
    tempo: &TempoRuntime,
    score_end: &Rational,
) -> Result<AutomationBinding> {
    let anchor = match (lane.clock, lane.anchor) {
        (AutomationClock::Score, AutomationAnchorView::Score(q)) => {
            TimeValue::from_rational(q - &timing.origin).map_err(timing_error)?
        }
        (AutomationClock::Seconds, AutomationAnchorView::Score(q)) => {
            timing.relative_at_score(q, &Rational::zero())?
        }
        (AutomationClock::Seconds, AutomationAnchorView::Seconds(s)) => {
            timing.relative_at_seconds(s)?
        }
        _ => {
            return Err(RenderError::RenderState(
                "score clock requires score anchor".into(),
            ))
        }
    };
    let mut points = Vec::with_capacity(lane.points.len());
    let mut knot_frames = Vec::with_capacity(lane.points.len());
    for point in lane.points {
        // Form each local knot coordinate exactly before binary64 conversion.
        // A distant anchor and compensating point offset must not erase the
        // small rendered coordinate used for continuous interpolation.
        let (frame, position) = match (lane.clock, lane.anchor) {
            (AutomationClock::Score, AutomationAnchorView::Score(q)) => {
                let absolute_q = q + &point.position;
                (
                    timing.frame_at_score(&absolute_q, &Rational::zero(), tempo.rate_hz)?,
                    rational_f64(&(absolute_q - &timing.origin), "automation score position")?,
                )
            }
            (AutomationClock::Seconds, AutomationAnchorView::Score(q)) => {
                let coordinate = timing.relative_at_score(q, &point.position)?;
                (
                    timing.ceil_time(&coordinate, tempo.rate_hz)?,
                    timing.approximate_time(&coordinate)?,
                )
            }
            (AutomationClock::Seconds, AutomationAnchorView::Seconds(s)) => {
                let coordinate = timing.relative_at_seconds(&(s + &point.position))?;
                (
                    timing.ceil_time(&coordinate, tempo.rate_hz)?,
                    timing.approximate_time(&coordinate)?,
                )
            }
            _ => unreachable!("validated anchor"),
        };
        knot_frames.push(saturating_frame(frame));
        points.push(AutomationPointRuntime {
            position,
            value: rational_f64(&point.value, "automation value")?,
            shape: point.shape,
        });
    }
    let end = match lane.clock {
        AutomationClock::Score => {
            TimeValue::from_rational(score_end - &timing.origin).map_err(timing_error)?
        }
        AutomationClock::Seconds => timing.end.clone(),
    };
    let relative_end = end.difference(&anchor).map_err(timing_error)?;
    // Select the held tail interval by certified comparisons, not rounded
    // doubles or a ceil-frame proxy (which cannot distinguish subframe knots).
    let mut active = None;
    for (index, point) in lane.points.iter().enumerate() {
        let position = TimeValue::from_rational(point.position.clone()).map_err(timing_error)?;
        if timing.compare_times(&relative_end, &position)? != std::cmp::Ordering::Less {
            active = Some(index);
        } else {
            break;
        }
    }
    let tail_value = if let Some(index) = active {
        let left = &points[index];
        if index + 1 == points.len() || left.shape == Interpolation::Step {
            left.value
        } else {
            let delta = relative_end
                .add_offset(&(-lane.points[index].position.clone()))
                .map_err(timing_error)?;
            let width = &lane.points[index + 1].position - &lane.points[index].position;
            let u = (timing.approximate_time(&delta)?
                / rational_f64(&width, "automation interval")?)
            .clamp(0.0, 1.0);
            let right = &points[index + 1];
            match left.shape {
                Interpolation::Linear => left.value + (right.value - left.value) * u,
                Interpolation::Exponential => left.value * (right.value / left.value).powf(u),
                Interpolation::Step => unreachable!(),
            }
        }
    } else {
        base
    };
    if !tail_value.is_finite() {
        return Err(RenderError::Nonfinite(
            "automation tail is nonfinite".into(),
        ));
    }
    Ok(AutomationBinding {
        id: lane.id.clone(),
        node,
        parameter: lane.target.port.clone(),
        lane: AutomationRuntime {
            clock: lane.clock,
            at: 0.0,
            at_exact: None,
            points,
            point_positions_exact: Vec::new(),
            knot_frames,
            base,
            tail_value,
        },
    })
}

#[cfg(test)]
mod reset_bootstrap_tests {
    use super::*;
    use crate::bundle::SourceBundle;

    fn source(outer_reset: bool) -> SourceBundle {
        let outer = if outer_reset {
            r#"
node cure { type = "core.constant/1"; params = { value = 1; }; }
modulate cure_phase { from = &cure:out; target = &sound.params.phase; amount = -1/2; }
"#
        } else {
            ""
        };
        SourceBundle::new(
            "reset-bootstrap.maac",
            format!(
                r#"maac 1;
project bootstrap {{ score = [0q, 1/48000q]; tail = 0s; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sound:out; }}
tempo clock {{ points = [(0q, 60bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
instrument local {{
 channels = 1;
 voice v {{
  channels = 1; amplitude = &amp; output = &osc:out;
  node amp {{ type = "synth.adsr/1"; params = {{ attack = 0s; decay = 0s; sustain = 1; release = 0s; }}; }}
  node osc {{ type = "synth.sine/1"; params = {{ frequency = 0Hz; }}; }}
 }}
 shared fx {{
  channels = 1; output = &target:out;
  node source {{ type = "synth.lfo/1"; params = {{ frequency = 0Hz; phase = 1/4; level = 1; }}; }}
  node target {{ type = "synth.lfo/1"; params = {{ frequency = 0Hz; phase = 0; level = 1; }}; }}
  modulate internal_phase {{ from = &source:out; to = &target.params.phase; depth = 1/4; }}
 }}
 control phase {{ target = &fx.target.params.phase; default = 1; }}
 control source_level {{ target = &fx.source.params.level; default = 1; }}
}}
node sound {{ instrument = &local; params = {{ phase = 1; source_level = 1; }}; config = {{ voices = 1; }}; }}
curve source_level_values {{ clock = score; points = [(0q, 0, step), (1/48000q, 0, step)]; }}
automation source_level_at_origin {{ target = &sound.params.source_level; curve = &source_level_values; at = 0q; }}
{outer}"#
            ),
        )
    }

    #[test]
    fn outer_reset_capture_precedes_internal_reset_construction() {
        let plan = crate::compiler::compile_bundle_artifact(&source(true)).unwrap();
        let mut engine = DspEngine::new_artifact(&plan).unwrap();
        let mut samples = Vec::new();
        engine
            .render(|frame| {
                samples.push(frame[0]);
                Ok(())
            })
            .unwrap();
        assert_eq!(samples.len(), 1);
        assert!((samples[0] + 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn instrument_without_outer_reset_keeps_eager_error_timing() {
        let plan = crate::compiler::compile_bundle_artifact(&source(false)).unwrap();
        let error = match DspEngine::new_artifact(&plan) {
            Ok(_) => panic!("invalid internal reset capture unexpectedly prepared"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "E_RANGE");
    }
}

#[cfg(test)]
mod delay_scheduler_tests {
    use super::*;

    #[test]
    fn published_delay_outputs_do_not_constrain_zero_delay_node_order() {
        let nodes = vec![
            NodeState::new("z_delay".into(), Processor::delay(1, 1), 48_000.0, None).unwrap(),
            NodeState::new("a_consumer".into(), Processor::gain(1), 48_000.0, None).unwrap(),
            NodeState::new("b_independent".into(), Processor::gain(1), 48_000.0, None).unwrap(),
        ];
        let connections = vec![ConnectionRuntime {
            id: "delay_consumer".into(),
            from: 0,
            to: 1,
            to_port: "in".into(),
        }];
        let order = stable_topological_order(&nodes, &connections, &[]).unwrap();
        let processing = order
            .into_iter()
            .filter(|index| nodes[*index].delay.is_none())
            .map(|index| nodes[index].id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(processing, ["a_consumer", "b_independent"]);
    }
}
