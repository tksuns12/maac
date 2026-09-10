//! MaaC source-to-performance-plan resolution.
//!
//! The compiler is deliberately a finite resolver.  It does not render audio,
//! execute modules, or infer a transport.  Its output is the standalone
//! [`crate::plan::Plan`] consumed by the renderer.  All source arithmetic stays
//! exact until the pitch boundary and all expansion paths are bounded.

use crate::plan_v3::{
    AutomationAnchor, AutomationV3, PlanV3, ResolvedEventV3, TimingContext, VersionedPlan,
};
use crate::plan_v7::{ModulationV7, NodeV7, PlanV7, ProcessorV7};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::TryFrom;
use std::sync::Arc;

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::audio_asset::AudioAsset;
use crate::bundle::{sha256_digest, ResolvedBundle, SourceBundle, SourceIdentity};
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
use crate::exact::{Rational, MAX_RATIONAL_BITS};
use crate::instrument_plan::InstrumentResources;
use crate::library::{LibrarySet, ResolvedInstance};
use crate::music::{
    MeterMap, MeterPoint, MusicError, MusicErrorCode, Pitch, TempoMap as ExactTempoMap,
    TempoPoint as ExactTempoPoint, TempoShape, Tuning,
};
use crate::plan::{
    Automation, AutomationClock, AutomationPoint, Connection, EventKind, EventTarget,
    ExpressionClock, GainExpression, GainExpressionPoint, Interpolation, Node, OutputSettings,
    PitchExpression, PitchExpressionPoint, Plan, PlanLimits, PortRef, PressureExpression,
    PressureExpressionPoint, Processor, Region, ResolvedEvent, SourceMapping, SourceSpan, TempoMap,
    TempoPoint, TimbreExpression, TimbreExpressionPoint,
};
use crate::plan_artifact::PlanArtifact;
use crate::plan_v4::{NodeV4, PlanV4, ProcessorV4};
use crate::plan_v5::{AudioClip, AudioFadeShape, NodeV5, PlanV5, ProcessorV5};
use crate::plan_v6::{NodeV6, PlanV6, ProcessorV6, WarpAnchor, WarpClip};
use crate::semantic::InstrumentNodeDescriptor;
use crate::syntax::{Document, Field, Object, Reference, Unit, Value, ValueKind};

/// Maximum number of emitted note events in a resolved plan.
pub const MAX_EXPANDED_NOTES: usize = 100_000;
/// Maximum nesting depth of pattern uses during resolution.
pub const MAX_PATTERN_DEPTH: usize = 64;
/// Maximum amount of finite expansion work.  This also bounds repeated empty
/// patterns, which otherwise have no emitted-note count with which to stop.
pub const MAX_EXPANSION_WORK: u64 = 20_000_000;

type CResult<T> = Result<T, Diagnostics>;

fn diagnostics(
    code: DiagnosticCode,
    message: impl Into<String>,
    span: Option<Span>,
) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, span));
    diagnostics
}

fn path_diagnostic(
    code: DiagnosticCode,
    message: impl Into<String>,
    object: &Object,
    field: Option<&Field>,
) -> Diagnostics {
    let diagnostic = Diagnostic::error(code, message, field.map(|f| f.span).or(Some(object.span)))
        .object_path([object.id.clone()]);
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(diagnostic);
    diagnostics
}

fn plan_error(error: crate::plan::PlanError) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(error.diagnostic());
    diagnostics
}

fn event_diagnostic(
    code: DiagnosticCode,
    message: impl Into<String>,
    event: &ExpandedEvent,
    fields: &[&str],
) -> Diagnostics {
    let mut diagnostic = Diagnostic::error(code, message, Some(event.source_span))
        .object_path(event.source.path.clone());
    if !fields.is_empty() {
        diagnostic = diagnostic.field_path(fields.iter().copied());
    }
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(diagnostic);
    diagnostics
}

fn event_music_error(error: MusicError, event: &ExpandedEvent, fields: &[&str]) -> Diagnostics {
    let diagnostics = map_music_error(error, Some(event.source_span));
    let Some(diagnostic) = diagnostics.into_vec().into_iter().next() else {
        return Diagnostics::new();
    };
    let mut diagnostic = diagnostic.object_path(event.source.path.clone());
    if !fields.is_empty() {
        diagnostic = diagnostic.field_path(fields.iter().copied());
    }
    let mut result = Diagnostics::new();
    result.push(diagnostic);
    result
}

fn map_music_error(error: MusicError, span: Option<Span>) -> Diagnostics {
    let code = match error.code {
        MusicErrorCode::Range => DiagnosticCode::Range,
        MusicErrorCode::Tempo => DiagnosticCode::Tempo,
        MusicErrorCode::MeterBoundary => DiagnosticCode::MeterBoundary,
        MusicErrorCode::Capability => DiagnosticCode::Capability,
        MusicErrorCode::Pitch => DiagnosticCode::Range,
        MusicErrorCode::Nonfinite => DiagnosticCode::Nonfinite,
        MusicErrorCode::ResourceLimit => DiagnosticCode::ResourceLimit,
        MusicErrorCode::TimePrecision => DiagnosticCode::TimePrecision,
    };
    diagnostics(code, error.message, span)
}

fn ensure_rational(value: &Rational, span: Option<Span>) -> CResult<Rational> {
    if value.numer().magnitude().bits() > MAX_RATIONAL_BITS
        || value.denom().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(diagnostics(
            DiagnosticCode::ResourceLimit,
            format!("rational exceeds {MAX_RATIONAL_BITS}-bit bound"),
            span,
        ));
    }
    Ok(value.clone())
}

fn checked_add(a: &Rational, b: &Rational, span: Option<Span>) -> CResult<Rational> {
    ensure_rational(a, span)?;
    ensure_rational(b, span)?;
    if a.numer().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || b.numer().magnitude().bits() + a.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || a.denom().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(diagnostics(
            DiagnosticCode::ResourceLimit,
            "rational addition intermediate exceeds the bit bound",
            span,
        ));
    }
    let value = a + b;
    ensure_rational(&value, span)
}

fn checked_sub(a: &Rational, b: &Rational, span: Option<Span>) -> CResult<Rational> {
    ensure_rational(a, span)?;
    ensure_rational(b, span)?;
    if a.numer().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || b.numer().magnitude().bits() + a.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || a.denom().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(diagnostics(
            DiagnosticCode::ResourceLimit,
            "rational subtraction intermediate exceeds the bit bound",
            span,
        ));
    }
    let value = a - b;
    ensure_rational(&value, span)
}

fn checked_mul(a: &Rational, b: &Rational, span: Option<Span>) -> CResult<Rational> {
    ensure_rational(a, span)?;
    ensure_rational(b, span)?;
    if a.numer().magnitude().bits() + b.numer().magnitude().bits() > MAX_RATIONAL_BITS
        || a.denom().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(diagnostics(
            DiagnosticCode::ResourceLimit,
            "rational multiplication intermediate exceeds the bit bound",
            span,
        ));
    }
    let value = a * b;
    ensure_rational(&value, span)
}

fn checked_div(a: &Rational, b: &Rational, span: Option<Span>) -> CResult<Rational> {
    ensure_rational(a, span)?;
    ensure_rational(b, span)?;
    if b.is_zero() {
        return Err(diagnostics(DiagnosticCode::Range, "division by zero", span));
    }
    if a.numer().magnitude().bits() + b.denom().magnitude().bits() > MAX_RATIONAL_BITS
        || a.denom().magnitude().bits() + b.numer().magnitude().bits() > MAX_RATIONAL_BITS
    {
        return Err(diagnostics(
            DiagnosticCode::ResourceLimit,
            "rational division intermediate exceeds the bit bound",
            span,
        ));
    }
    let value = a / b;
    ensure_rational(&value, span)
}

fn ceil_u64(value: &Rational, span: Option<Span>) -> CResult<u64> {
    ensure_rational(value, span)?;
    if value.is_negative() {
        return Err(diagnostics(
            DiagnosticCode::Interval,
            "scheduled time precedes the score reset origin",
            span,
        ));
    }
    let (q, r) = value.numer().div_rem(value.denom());
    let q = if r.is_zero() { q } else { q + BigInt::one() };
    q.to_u64().ok_or_else(|| {
        diagnostics(
            DiagnosticCode::TimePrecision,
            "frame ceiling exceeds the host frame range",
            span,
        )
    })
}

fn bounded_u64_mul(a: u64, b: u64, span: Span) -> CResult<u64> {
    a.checked_mul(b).ok_or_else(|| {
        diagnostics(
            DiagnosticCode::ResourceLimit,
            "bounded expansion count overflows the host range",
            Some(span),
        )
    })
}

fn bigint_i64(value: &Rational, span: Option<Span>) -> CResult<i64> {
    ensure_rational(value, span)?;
    if !value.denom().is_one() {
        return Err(diagnostics(
            DiagnosticCode::Range,
            "expected an integer",
            span,
        ));
    }
    value.to_integer().to_i64().ok_or_else(|| {
        diagnostics(
            DiagnosticCode::ResourceLimit,
            "integer exceeds the signed 64-bit bound",
            span,
        )
    })
}

fn bigint_u64(value: &Rational, span: Option<Span>) -> CResult<u64> {
    let integer = bigint_i64(value, span)?;
    u64::try_from(integer).map_err(|_| {
        diagnostics(
            DiagnosticCode::Range,
            "expected a nonnegative integer",
            span,
        )
    })
}

fn bigint_u64_full(value: &Rational, span: Option<Span>) -> CResult<u64> {
    ensure_rational(value, span)?;
    if !value.denom().is_one() {
        return Err(diagnostics(
            DiagnosticCode::Range,
            "expected an integer",
            span,
        ));
    }
    if value.is_negative() {
        return Err(diagnostics(
            DiagnosticCode::Range,
            "expected a nonnegative integer",
            span,
        ));
    }
    value.to_integer().to_u64().ok_or_else(|| {
        diagnostics(
            DiagnosticCode::ResourceLimit,
            "integer exceeds the unsigned 64-bit bound",
            span,
        )
    })
}

#[derive(Clone, Debug)]
struct LeafDef {
    id: String,
    span: Span,
    at: Rational,
    velocity: Rational,
    onset_offset: Rational,
    order: i32,
    kind: LeafKind,
}
#[derive(Clone, Debug)]
enum LeafKind {
    Note(Box<NoteDef>),
    Hit { key: String },
}
#[derive(Clone, Debug)]
struct NoteDef {
    pitch_expression: Option<String>,
    gain_expression: Option<String>,
    timbre_expression: Option<String>,
    pressure_expression: Option<String>,
    dur: Rational,
    pitch: Pitch,
    release_velocity: f64,
    release_offset: Rational,
}

#[derive(Clone, Debug)]
struct UseDef {
    id: String,
    span: Span,
    pattern: String,
    at: Rational,
    count: u64,
    stretch: Rational,
    transpose_cents: Rational,
    cut: bool,
}

#[derive(Clone, Debug)]
enum PatternChild {
    Leaf(Box<LeafDef>),
    Use(Box<UseDef>),
}

#[derive(Clone, Debug)]
struct PatternDef {
    id: String,
    span: Span,
    length: Rational,
    children: Vec<PatternChild>,
}

#[derive(Clone, Debug)]
struct OverrideDef {
    span: Span,
    event: String,
    delete: bool,
    set: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
struct InsertDef {
    id: String,
    span: Span,
    leaf: LeafDef,
}

#[derive(Clone, Debug)]
struct PlaceDef {
    id: String,
    span: Span,
    pattern: String,
    track: String,
    at: Rational,
    count: u64,
    stretch: Rational,
    transpose_cents: Rational,
    cut: bool,
    overrides: Vec<OverrideDef>,
    inserts: Vec<InsertDef>,
}

#[derive(Clone, Debug)]
struct ExpandedEvent {
    address: String,
    source: SourceMapping,
    source_span: Span,
    target: EventTarget,
    score_on_q: Rational,
    velocity: Rational,
    onset_offset_seconds: Rational,
    order: i32,
    kind: ExpandedKind,
}
#[derive(Clone, Debug)]
enum ExpandedKind {
    Note(Box<ExpandedNote>),
    Hit { key: String },
}
#[derive(Clone, Debug)]
struct ExpandedNote {
    pitch_expression: Option<String>,
    gain_expression: Option<String>,
    timbre_expression: Option<String>,
    pressure_expression: Option<String>,
    expression_scale: Rational,
    pitch: Pitch,
    transpose_cents: Rational,
    dur_q: Rational,
    release_velocity: f64,
    release_offset_seconds: Rational,
}
impl ExpandedEvent {
    fn note(&self) -> CResult<&ExpandedNote> {
        match &self.kind {
            ExpandedKind::Note(note) => Ok(note),
            ExpandedKind::Hit { .. } => Err(event_diagnostic(
                DiagnosticCode::Capability,
                "native hits require the sample-kit execution profile",
                self,
                &[],
            )),
        }
    }
}

#[derive(Clone, Debug)]
struct CurveDef {
    clock: String,
    points: Vec<(Rational, Rational, Interpolation)>,
}

struct ExpansionState<'a> {
    origin: Rational,
    scale: Rational,
    transpose: Rational,
    address: &'a mut Vec<String>,
    inherited_cut_end: Option<Rational>,
    target: &'a EventTarget,
    depth: usize,
}

struct Compiler<'a> {
    document: &'a Document,
    score_start: Rational,
    score_end: Rational,
    sample_rate: u32,
    project_seed: u64,
    tail_seconds: Rational,
    output: PortRef,
    exact_tempo: ExactTempoMap,
    plan_tempo: TempoMap,
    meter: MeterMap,
    tunings: HashMap<String, Arc<Tuning>>,
    patterns: BTreeMap<String, PatternDef>,
    tracks: BTreeMap<String, EventTarget>,
    places: Vec<PlaceDef>,
    nodes: Vec<Node>,
    kit_nodes: BTreeMap<String, NodeV4>,
    audio_assets: Vec<AudioAsset>,
    control_nodes: BTreeMap<String, NodeV7>,
    modulations: Vec<ModulationV7>,
    audio_clips: BTreeMap<String, crate::semantic::AudioClipSource>,
    connections: Vec<Connection>,
    curves: BTreeMap<String, CurveDef>,
    automations: Vec<AutomationV3>,
    allow_ramps: bool,
    allow_hits: bool,
    regions: Vec<Region>,
    events: Vec<ExpandedEvent>,
    expansion_work: u64,
    instrument_instances: BTreeMap<String, ResolvedInstance>,
    instrument_descriptors: BTreeMap<String, InstrumentNodeDescriptor>,
    instrument_resources: Option<InstrumentResources>,
}

impl<'a> Compiler<'a> {
    fn new(document: &'a Document) -> Self {
        // These placeholders are replaced while reading the required project
        // maps.  Keeping initialization infallible makes each phase report a
        // source diagnostic instead of panicking on malformed input.
        let zero = Rational::zero();
        let default_tempo = ExactTempoMap::new(vec![ExactTempoPoint::new(
            zero.clone(),
            Rational::from_integer(120.into()),
            TempoShape::Step,
        )])
        .expect("constant default tempo is valid");
        let default_meter = MeterMap::new(vec![MeterPoint::new(zero.clone(), 4, 4)])
            .expect("constant default meter is valid");
        Self {
            document,
            score_start: zero.clone(),
            score_end: Rational::one(),
            sample_rate: 48_000,
            project_seed: 0,
            tail_seconds: zero,
            output: PortRef {
                node: "_missing".to_owned(),
                port: "out".to_owned(),
            },
            exact_tempo: default_tempo,
            plan_tempo: TempoMap { points: Vec::new() },
            meter: default_meter,
            tunings: HashMap::new(),
            patterns: BTreeMap::new(),
            tracks: BTreeMap::new(),
            places: Vec::new(),
            nodes: Vec::new(),
            kit_nodes: BTreeMap::new(),
            audio_assets: Vec::new(),
            control_nodes: BTreeMap::new(),
            modulations: Vec::new(),
            audio_clips: BTreeMap::new(),
            connections: Vec::new(),
            curves: BTreeMap::new(),
            automations: Vec::new(),
            allow_ramps: false,
            allow_hits: false,
            regions: Vec::new(),
            events: Vec::new(),
            expansion_work: 0,
            instrument_instances: BTreeMap::new(),
            instrument_descriptors: BTreeMap::new(),
            instrument_resources: None,
        }
    }

    fn with_instruments(
        document: &'a Document,
        instances: BTreeMap<String, ResolvedInstance>,
        descriptors: BTreeMap<String, InstrumentNodeDescriptor>,
        resources: InstrumentResources,
    ) -> Self {
        Self {
            instrument_instances: instances,
            instrument_descriptors: descriptors,
            instrument_resources: Some(resources),
            ..Self::new(document)
        }
    }

    fn run(self, limits: &PlanLimits) -> CResult<Plan> {
        let document = self.document;
        match self.run_versioned(limits, None, document)? {
            VersionedPlan::Legacy(plan) => Ok(plan),
            VersionedPlan::V3(_) => unreachable!("legacy compiler disables ramps"),
        }
    }

    fn run_versioned(
        mut self,
        limits: &PlanLimits,
        production: Option<crate::production_data::ProductionSettings>,
        original: &Document,
    ) -> CResult<VersionedPlan> {
        // SourceGraph owns the source schema and declaration/reference checks.
        // Keep this call ahead of lowering so malformed unused declarations
        // cannot disappear during expansion.
        crate::semantic::validate_source_with_tempo_profile(
            self.document,
            &self.instrument_descriptors,
            self.allow_ramps,
        )?;
        self.read_composition(limits)?;
        if self
            .plan_tempo
            .points
            .iter()
            .any(|p| p.shape == Interpolation::Linear)
        {
            let plan = self.finish_v3(limits, production, original)?;
            return Ok(VersionedPlan::V3(plan));
        }
        let plan = self.finish_plan(limits)?;
        plan.validate_with_limits(limits).map_err(plan_error)?;
        attach_production(plan, production, original, limits).map(VersionedPlan::Legacy)
    }

    fn object(&self, id: &str) -> CResult<&Object> {
        self.document.object(id).ok_or_else(|| {
            diagnostics(
                DiagnosticCode::Reference,
                format!("unknown object reference `&{id}`"),
                None,
            )
        })
    }

    fn field<'b>(&self, object: &'b Object, name: &str) -> CResult<&'b Field> {
        object.field(name).ok_or_else(|| {
            path_diagnostic(
                DiagnosticCode::Range,
                format!("required field `{name}` is missing"),
                object,
                None,
            )
        })
    }

    fn read_composition(&mut self, limits: &PlanLimits) -> CResult<()> {
        self.validate_document_shape()?;
        self.read_tunings()?;
        self.read_project_and_maps()?;
        self.read_nodes_and_connections()?;
        self.read_patterns()?;
        self.validate_pattern_graph()?;
        self.read_tracks_and_places()?;
        self.read_curves_and_automation(limits)?;
        self.read_regions()?;
        self.expand_places()?;
        Ok(())
    }

    fn run_artifact(
        mut self,
        resolved: &ResolvedBundle,
        limits: &PlanLimits,
        production: Option<crate::production_data::ProductionSettings>,
        original: &Document,
    ) -> CResult<PlanArtifact> {
        self.allow_ramps = true;
        self.allow_hits = true;
        let has_controls = uses_control_profile(self.document);
        if has_controls {
            let nodes = self
                .document
                .objects
                .values()
                .filter(|o| matches!(o.kind.as_str(), "node" | "audio"))
                .count();
            let edges = self
                .document
                .objects
                .values()
                .filter(|o| matches!(o.kind.as_str(), "connect" | "modulate"))
                .count();
            if nodes > limits.max_nodes || edges > limits.max_connections {
                return Err(diagnostics(
                    DiagnosticCode::ResourceLimit,
                    "aggregate node or connection/modulation budget exceeded",
                    None,
                ));
            }
        }
        let has_audio = uses_audio_profile(self.document);
        let has_warp = self.document.objects.values().any(|object| {
            object.kind == "audio"
                && object
                    .field("mode")
                    .and_then(|field| field.value.as_symbol())
                    == Some("warp_rate")
        });
        let graph = if has_controls {
            crate::semantic::validate_source_with_control_profile(
                self.document,
                &self.instrument_descriptors,
                true,
            )?
        } else if has_warp {
            crate::semantic::validate_source_with_warp_profile(
                self.document,
                &self.instrument_descriptors,
                true,
            )?
        } else if has_audio {
            crate::semantic::validate_source_with_audio_profile(
                self.document,
                &self.instrument_descriptors,
                true,
            )?
        } else {
            crate::semantic::validate_source_with_kit_profile(
                self.document,
                &self.instrument_descriptors,
                true,
            )?
        };
        self.control_nodes = graph.control_nodes().clone();
        self.modulations = graph.modulations().to_vec();
        self.audio_clips = graph.audio_clips().clone();
        self.kit_nodes = graph.kit_nodes().clone();
        for (id, source) in graph.audio_sources() {
            let path = crate::bundle::normalize_file_reference("package.maac", &source.path)?;
            let bytes = resolved.assets.get(&path).ok_or_else(|| {
                diagnostics(
                    DiagnosticCode::Asset,
                    format!("resolved audio asset `{id}` is missing `{path}`"),
                    None,
                )
            })?;
            self.audio_assets.push(AudioAsset {
                id: id.clone(),
                format: source.format.clone(),
                rate_hz: source.rate_hz,
                channels: source.channels,
                frames: source.frames,
                hash: source.hash.clone(),
                bytes: bytes.clone(),
            });
        }
        self.read_composition(limits)?;
        if has_controls {
            self.finish_v7(limits, production, original)
                .map(PlanArtifact::from_v7)
        } else if has_warp {
            self.finish_v6(limits, production, original)
                .map(PlanArtifact::from_v6)
        } else if has_audio {
            self.finish_v5(limits, production, original)
                .map(PlanArtifact::from_v5)
        } else {
            self.finish_v4(limits, production, original)
                .map(PlanArtifact::from_v4)
        }
    }

    fn optional<'b>(&self, object: &'b Object, name: &str) -> Option<&'b Value> {
        object.field(name).map(|field| &field.value)
    }

    fn check_fields(&self, object: &Object, allowed: &[&str]) -> CResult<()> {
        for (name, field) in &object.fields {
            if name != "label" && !allowed.contains(&name.as_str()) {
                return Err(path_diagnostic(
                    DiagnosticCode::UnknownField,
                    format!("unknown field `{name}` on {}", object.kind),
                    object,
                    Some(field),
                ));
            }
        }
        Ok(())
    }

    fn validate_document_shape(&self) -> CResult<()> {
        if self.document.version != 1 {
            return Err(diagnostics(
                DiagnosticCode::Version,
                format!("unsupported MaaC version {}", self.document.version),
                None,
            ));
        }
        let mut project_count = 0usize;
        for object in self.document.objects.values() {
            let allowed = match object.kind.as_str() {
                "project" => {
                    project_count += 1;
                    &[
                        "score", "rate", "tempo", "meter", "output", "tail", "seed", "requires",
                    ][..]
                }
                "tempo" => &["points"][..],
                "meter" => &["points"][..],
                "tuning" => &["period", "steps", "reference_index", "reference_frequency"][..],
                "pattern" => &["length"][..],
                "track" => &["target"][..],
                "place" => &[
                    "pattern",
                    "track",
                    "at",
                    "count",
                    "stretch",
                    "transpose",
                    "boundary",
                ][..],
                "curve" => &["clock", "points"][..],
                "automation" => &["target", "curve", "at"][..],
                "node" if self.instrument_instances.contains_key(&object.id) => {
                    &["instrument", "preset", "config", "params"][..]
                }
                "node" => &["type", "config", "params"][..],
                "connect" => &["from", "to"][..],
                "region" => &["span", "label"][..],
                "asset" if self.allow_hits => &[
                    "kind", "path", "hash", "format", "rate", "channels", "frames",
                ][..],
                "audio" if self.audio_clips.contains_key(&object.id) => &[
                    "asset",
                    "at",
                    "source",
                    "mode",
                    "warp",
                    "speed",
                    "reverse",
                    "gain",
                    "fade_in",
                    "fade_out",
                    "fade_shape",
                    "track",
                ][..],
                "modulate" if uses_control_profile(self.document) => {
                    &["from", "target", "amount", "label"][..]
                }
                "modulate" | "asset" | "audio" | "extension" => {
                    return Err(path_diagnostic(
                        DiagnosticCode::Capability,
                        format!(
                            "{} is recognized but outside the standalone performance compiler",
                            object.kind
                        ),
                        object,
                        None,
                    ));
                }
                other => {
                    return Err(path_diagnostic(
                        DiagnosticCode::UnknownKind,
                        format!("unknown MaaC object kind `{other}`"),
                        object,
                        None,
                    ));
                }
            };
            self.check_fields(object, allowed)?;
            if let Some(label) = object.field("label") {
                self.string_value(&label.value, object, Some(label), "label")?;
            }
        }
        if project_count != 1 {
            return Err(diagnostics(
                DiagnosticCode::Range,
                format!("document must contain exactly one project (found {project_count})"),
                None,
            ));
        }
        Ok(())
    }

    fn string_value(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
        name: &str,
    ) -> CResult<String> {
        match &value.kind {
            ValueKind::String(value) | ValueKind::Symbol(value) => Ok(value.clone()),
            _ => Err(path_diagnostic(
                DiagnosticCode::Unit,
                format!("{name} must be a string or symbol"),
                object,
                field,
            )),
        }
    }

    fn reference(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<Reference> {
        match &value.kind {
            ValueKind::Reference(reference) => Ok(reference.clone()),
            _ => Err(path_diagnostic(
                DiagnosticCode::Reference,
                "expected a reference",
                object,
                field,
            )),
        }
    }

    fn one_reference(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<String> {
        let reference = self.reference(value, object, field)?;
        if reference.path.len() != 1 || reference.port.is_some() {
            return Err(path_diagnostic(
                DiagnosticCode::Reference,
                "expected an object reference",
                object,
                field,
            ));
        }
        Ok(reference.path[0].clone())
    }

    fn port_reference(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<PortRef> {
        let reference = self.reference(value, object, field)?;
        if reference.path.len() != 1 || reference.port.is_none() {
            return Err(path_diagnostic(
                DiagnosticCode::Reference,
                "expected an object port reference",
                object,
                field,
            ));
        }
        PortRef::new(reference.path[0].clone(), reference.port.unwrap()).map_err(plan_error)
    }

    fn rational_value(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<Rational> {
        let value = match &value.kind {
            ValueKind::Number(value) => value,
            _ => {
                return Err(path_diagnostic(
                    DiagnosticCode::Unit,
                    "expected a dimensionless exact number",
                    object,
                    field,
                ))
            }
        };
        ensure_rational(value, Some(value_span(value, field)))
    }

    fn quantity(
        &self,
        value: &Value,
        unit: Unit,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<Rational> {
        let (number, actual) = match &value.kind {
            ValueKind::Quantity { value, unit } => (value, *unit),
            _ => {
                return Err(path_diagnostic(
                    DiagnosticCode::Unit,
                    format!("expected quantity in {unit}"),
                    object,
                    field,
                ))
            }
        };
        let converted = match (actual, unit) {
            (Unit::Q, Unit::Q)
            | (Unit::S, Unit::S)
            | (Unit::Ms, Unit::Ms)
            | (Unit::Hz, Unit::Hz)
            | (Unit::KHz, Unit::KHz)
            | (Unit::Bpm, Unit::Bpm)
            | (Unit::Ct, Unit::Ct)
            | (Unit::Db, Unit::Db) => number.clone(),
            (Unit::Ms, Unit::S) => checked_div(
                number,
                &Rational::from_integer(1000.into()),
                Some(value.span),
            )?,
            (Unit::KHz, Unit::Hz) => checked_mul(
                number,
                &Rational::from_integer(1000.into()),
                Some(value.span),
            )?,
            _ => {
                return Err(path_diagnostic(
                    DiagnosticCode::Unit,
                    format!("incompatible unit: expected {unit}, found {actual}"),
                    object,
                    field,
                ))
            }
        };
        ensure_rational(&converted, Some(value.span))
    }

    fn q_value(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
        global: bool,
    ) -> CResult<Rational> {
        if let ValueKind::Call { function, args } = &value.kind {
            if !global || function != "bar" || args.len() != 2 {
                return Err(path_diagnostic(
                    DiagnosticCode::Unit,
                    "only bar(b,u) is allowed as a global score position",
                    object,
                    field,
                ));
            }
            let bar = bigint_i64(
                &self.rational_value(&args[0], object, field)?,
                Some(args[0].span),
            )?;
            let unit = self.rational_value(&args[1], object, field)?;
            return self
                .meter
                .bar_to_q(bar, &unit)
                .map_err(|error| map_music_error(error, Some(value.span)));
        }
        self.quantity(value, Unit::Q, object, field)
    }

    fn seconds_value(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<Rational> {
        match value.kind {
            ValueKind::Quantity { unit: Unit::S, .. }
            | ValueKind::Quantity { unit: Unit::Ms, .. } => {
                self.quantity(value, Unit::S, object, field)
            }
            _ => Err(path_diagnostic(
                DiagnosticCode::Unit,
                "expected a physical seconds quantity",
                object,
                field,
            )),
        }
    }

    fn cents_value(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<Rational> {
        self.quantity(value, Unit::Ct, object, field)
    }

    fn bool_value(&self, value: &Value, object: &Object, field: Option<&Field>) -> CResult<bool> {
        match value.kind {
            ValueKind::Boolean(value) => Ok(value),
            _ => Err(path_diagnostic(
                DiagnosticCode::Range,
                "expected a boolean",
                object,
                field,
            )),
        }
    }

    fn symbol_value(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<String> {
        self.string_value(value, object, field, "symbol")
    }

    fn record<'b>(
        &self,
        value: &'b Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<&'b BTreeMap<String, Field>> {
        match &value.kind {
            ValueKind::Record(fields) => Ok(fields),
            _ => Err(path_diagnostic(
                DiagnosticCode::Unit,
                "expected a record",
                object,
                field,
            )),
        }
    }

    fn list<'b>(
        &self,
        value: &'b Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<&'b [Value]> {
        match &value.kind {
            ValueKind::List(items) => Ok(items),
            _ => Err(path_diagnostic(
                DiagnosticCode::Unit,
                "expected a list",
                object,
                field,
            )),
        }
    }

    fn tuple<'b>(
        &self,
        value: &'b Value,
        object: &Object,
        field: Option<&Field>,
        len: usize,
    ) -> CResult<&'b [Value]> {
        match &value.kind {
            ValueKind::Tuple(items) if items.len() == len => Ok(items),
            ValueKind::Tuple(_) => Err(path_diagnostic(
                DiagnosticCode::Range,
                "tuple has the wrong arity",
                object,
                field,
            )),
            _ => Err(path_diagnostic(
                DiagnosticCode::Unit,
                "expected a tuple",
                object,
                field,
            )),
        }
    }

    fn read_tunings(&mut self) -> CResult<()> {
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "tuning")
        {
            self.check_fields(
                object,
                &["period", "steps", "reference_index", "reference_frequency"],
            )?;
            let period = self.cents_value(
                self.field(object, "period")?.value_ref(),
                object,
                object.field("period"),
            )?;
            let steps_value = self.field(object, "steps")?;
            let steps = self.list(&steps_value.value, object, Some(steps_value))?;
            let mut steps_cents = Vec::with_capacity(steps.len());
            for step in steps {
                steps_cents.push(self.cents_value(step, object, Some(steps_value))?);
            }
            let reference_index = self
                .optional(object, "reference_index")
                .map(|value| {
                    bigint_i64(
                        &self.rational_value(value, object, object.field("reference_index"))?,
                        Some(value.span),
                    )
                })
                .transpose()?
                .unwrap_or(0);
            let reference_frequency = self
                .optional(object, "reference_frequency")
                .map(|value| {
                    self.quantity(value, Unit::Hz, object, object.field("reference_frequency"))
                })
                .transpose()?
                .unwrap_or_else(|| Rational::from_integer(440.into()));
            let tuning = Tuning::new(period, steps_cents, reference_index, reference_frequency)
                .map_err(|error| map_music_error(error, Some(object.span)))?;
            self.tunings.insert(object.id.clone(), Arc::new(tuning));
        }
        Ok(())
    }

    fn read_project_and_maps(&mut self) -> CResult<()> {
        let project = self
            .document
            .objects
            .values()
            .find(|object| object.kind == "project")
            .expect("validate_document_shape checked project count");
        // Global bar coordinates use the authored meter, including project crop bounds.
        // Meter knots themselves are explicit q positions, so this has no circular dependency.
        let tempo_id = self.one_reference(
            &self.field(project, "tempo")?.value,
            project,
            project.field("tempo"),
        )?;
        let meter_id = self.one_reference(
            &self.field(project, "meter")?.value,
            project,
            project.field("meter"),
        )?;
        let tempo_object = self.object(&tempo_id)?.clone();
        let meter_object = self.object(&meter_id)?.clone();
        if tempo_object.kind != "tempo" || meter_object.kind != "meter" {
            return Err(path_diagnostic(
                DiagnosticCode::Reference,
                "project tempo/meter references have the wrong kind",
                project,
                None,
            ));
        }
        self.read_meter(&meter_object)?;
        self.read_tempo(&tempo_object)?;
        let score_field = self.field(project, "score")?;
        let score = self.list(&score_field.value, project, Some(score_field))?;
        if score.len() != 2 {
            return Err(path_diagnostic(
                DiagnosticCode::Interval,
                "score must contain [start,end]",
                project,
                Some(score_field),
            ));
        }
        self.score_start = self.q_value(&score[0], project, Some(score_field), true)?;
        self.score_end = self.q_value(&score[1], project, Some(score_field), true)?;
        if self.score_start >= self.score_end {
            return Err(path_diagnostic(
                DiagnosticCode::Interval,
                "score end must be after score start",
                project,
                Some(score_field),
            ));
        }
        let rate = self.quantity(
            &self.field(project, "rate")?.value,
            Unit::Hz,
            project,
            project.field("rate"),
        )?;
        if rate.denom() != &BigInt::one() || rate.is_negative() || rate.is_zero() {
            return Err(path_diagnostic(
                DiagnosticCode::Range,
                "sample rate must be a positive integer",
                project,
                project.field("rate"),
            ));
        }
        self.sample_rate = rate.to_u32().ok_or_else(|| {
            path_diagnostic(
                DiagnosticCode::Range,
                "sample rate is out of range",
                project,
                project.field("rate"),
            )
        })?;
        self.project_seed = self
            .optional(project, "seed")
            .map(|value| {
                bigint_u64_full(
                    &self.rational_value(value, project, project.field("seed"))?,
                    Some(value.span),
                )
            })
            .transpose()?
            .unwrap_or(0);
        self.tail_seconds = self
            .optional(project, "tail")
            .map(|value| self.seconds_value(value, project, project.field("tail")))
            .transpose()?
            .unwrap_or_else(Rational::zero);
        if self.tail_seconds.is_negative() {
            return Err(path_diagnostic(
                DiagnosticCode::Range,
                "tail must be nonnegative",
                project,
                project.field("tail"),
            ));
        }
        let output_field = project.field("output");
        if let Some(output_field) = output_field {
            self.output = self.port_reference(&output_field.value, project, Some(output_field))?;
        } else {
            self.output = PortRef::new("_document_only", "out").map_err(plan_error)?;
        }
        Ok(())
    }

    fn read_tempo(&mut self, object: &Object) -> CResult<()> {
        let field = self.field(object, "points")?;
        let points = self.list(&field.value, object, Some(field))?;
        let mut exact = Vec::with_capacity(points.len());
        let mut plan = Vec::with_capacity(points.len());
        for point in points {
            let values = self.tuple(point, object, Some(field), 3)?;
            let q = self.q_value(&values[0], object, Some(field), true)?;
            let bpm = self.quantity(&values[1], Unit::Bpm, object, Some(field))?;
            let shape = self.symbol_value(&values[2], object, Some(field))?;
            let shape =
                match shape.as_str() {
                    "step" => {
                        exact.push(ExactTempoPoint::new(
                            q.clone(),
                            bpm.clone(),
                            TempoShape::Step,
                        ));
                        Interpolation::Step
                    }
                    "linear" if self.allow_ramps => Interpolation::Linear,
                    "linear" => return Err(path_diagnostic(
                        DiagnosticCode::Capability,
                        "linear tempo is recognized but unsupported by the exact standalone plan",
                        object,
                        Some(field),
                    )),
                    _ => {
                        return Err(path_diagnostic(
                            DiagnosticCode::Tempo,
                            "tempo shape must be step or linear",
                            object,
                            Some(field),
                        ))
                    }
                };
            plan.push(TempoPoint { q, bpm, shape });
        }
        if !plan.iter().any(|p| p.shape == Interpolation::Linear) {
            self.exact_tempo = ExactTempoMap::new(exact)
                .map_err(|error| map_music_error(error, Some(object.span)))?;
        }
        self.plan_tempo = TempoMap { points: plan };
        Ok(())
    }

    fn read_meter(&mut self, object: &Object) -> CResult<()> {
        let field = self.field(object, "points")?;
        let points = self.list(&field.value, object, Some(field))?;
        let mut meter_points = Vec::with_capacity(points.len());
        for point in points {
            let values = self.tuple(point, object, Some(field), 3)?;
            let q = self.q_value(&values[0], object, Some(field), true)?;
            let numerator = bigint_u64(
                &self.rational_value(&values[1], object, Some(field))?,
                Some(values[1].span),
            )?;
            let denominator = bigint_u64(
                &self.rational_value(&values[2], object, Some(field))?,
                Some(values[2].span),
            )?;
            let numerator = u32::try_from(numerator).map_err(|_| {
                diagnostics(
                    DiagnosticCode::Range,
                    "meter numerator is out of range",
                    Some(values[1].span),
                )
            })?;
            let denominator = u32::try_from(denominator).map_err(|_| {
                diagnostics(
                    DiagnosticCode::Range,
                    "meter denominator is out of range",
                    Some(values[2].span),
                )
            })?;
            meter_points.push(MeterPoint::new(q, numerator, denominator));
        }
        self.meter = MeterMap::new(meter_points)
            .map_err(|error| map_music_error(error, Some(object.span)))?;
        Ok(())
    }

    fn read_nodes_and_connections(&mut self) -> CResult<()> {
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "node")
        {
            if self.kit_nodes.contains_key(&object.id)
                || self.control_nodes.contains_key(&object.id)
            {
                continue;
            }
            if let Some(instance) = self.instrument_instances.get(&object.id).cloned() {
                let mut node = Node::new(
                    object.id.clone(),
                    Processor::Instrument {
                        program: instance.program_id,
                        voices: instance.voices,
                        channels: instance.channels,
                    },
                )
                .map_err(plan_error)?;
                node.params = instance.params;
                self.nodes.push(node);
                continue;
            }
            let type_field = self.field(object, "type")?;
            let node_type =
                self.string_value(&type_field.value, object, Some(type_field), "type")?;
            if matches!(
                node_type.as_str(),
                "fx.eq/1" | "fx.compressor/1" | "fx.reverb/1"
            ) {
                self.nodes
                    .push(crate::semantic::production_node(object).map_err(|error| {
                        let mut errors = Diagnostics::new();
                        errors.push(error);
                        errors
                    })?);
                continue;
            }
            let config = self
                .optional(object, "config")
                .map(|value| self.record(value, object, object.field("config")))
                .transpose()?;
            let processor = match node_type.as_str() {
                "core.sine/1" => {
                    let voices = config
                        .and_then(|fields| fields.get("voices"))
                        .map(|field| {
                            bigint_u64(
                                &self.rational_value(&field.value, object, Some(field))?,
                                Some(field.value.span),
                            )
                        })
                        .transpose()?
                        .unwrap_or(64);
                    if voices == 0 {
                        return Err(diagnostics(
                            DiagnosticCode::Range,
                            "voices must be positive",
                            Some(type_field.value.span),
                        ));
                    }
                    if voices > u64::from(PlanLimits::MAX_VOICES) {
                        return Err(diagnostics(
                            DiagnosticCode::ResourceLimit,
                            "voices exceeds the bounded voice limit",
                            Some(type_field.value.span),
                        ));
                    }
                    Processor::sine(u32::try_from(voices).map_err(|_| {
                        diagnostics(
                            DiagnosticCode::ResourceLimit,
                            "voices is out of range",
                            Some(type_field.value.span),
                        )
                    })?)
                }
                "core.noise/1" => {
                    let channels_field = config
                        .and_then(|fields| fields.get("channels"))
                        .ok_or_else(|| {
                            path_diagnostic(
                                DiagnosticCode::Range,
                                "core.noise/1 requires config.channels",
                                object,
                                object.field("config"),
                            )
                        })?;
                    let channels = bigint_u64(
                        &self.rational_value(
                            &channels_field.value,
                            object,
                            Some(channels_field),
                        )?,
                        Some(channels_field.value.span),
                    )?;
                    let channels = u8::try_from(channels).map_err(|_| {
                        diagnostics(
                            DiagnosticCode::Range,
                            "noise channels is out of range",
                            Some(type_field.value.span),
                        )
                    })?;
                    let seed = config
                        .and_then(|fields| fields.get("seed"))
                        .map(|field| {
                            bigint_u64_full(
                                &self.rational_value(&field.value, object, Some(field))?,
                                Some(field.value.span),
                            )
                        })
                        .transpose()?
                        .unwrap_or(self.project_seed);
                    Processor::noise(channels, seed)
                }
                "core.delay/1" => {
                    let field_value = |name: &str| -> CResult<u64> {
                        let field =
                            config.and_then(|fields| fields.get(name)).ok_or_else(|| {
                                path_diagnostic(
                                    DiagnosticCode::Range,
                                    format!("core.delay/1 requires config.{name}"),
                                    object,
                                    object.field("config"),
                                )
                            })?;
                        bigint_u64(
                            &self.rational_value(&field.value, object, Some(field))?,
                            Some(field.value.span),
                        )
                    };
                    let channels = u8::try_from(field_value("channels")?).map_err(|_| {
                        diagnostics(
                            DiagnosticCode::Range,
                            "delay channels is out of range",
                            Some(type_field.value.span),
                        )
                    })?;
                    let frames = field_value("frames")?;
                    Processor::delay(channels, frames)
                }
                "core.matrix/1" => {
                    let dimension = |name: &str| -> CResult<u8> {
                        let field =
                            config.and_then(|fields| fields.get(name)).ok_or_else(|| {
                                path_diagnostic(
                                    DiagnosticCode::Range,
                                    format!("core.matrix/1 requires config.{name}"),
                                    object,
                                    object.field("config"),
                                )
                            })?;
                        let value = bigint_u64(
                            &self.rational_value(&field.value, object, Some(field))?,
                            Some(field.value.span),
                        )?;
                        u8::try_from(value).map_err(|_| {
                            diagnostics(
                                DiagnosticCode::Range,
                                format!("matrix {name} is out of range"),
                                Some(field.value.span),
                            )
                        })
                    };
                    let coefficients_field = config
                        .and_then(|fields| fields.get("coefficients"))
                        .ok_or_else(|| {
                        path_diagnostic(
                            DiagnosticCode::Range,
                            "core.matrix/1 requires config.coefficients",
                            object,
                            object.field("config"),
                        )
                    })?;
                    let rows =
                        self.list(&coefficients_field.value, object, Some(coefficients_field))?;
                    let coefficients = rows
                        .iter()
                        .map(|row| {
                            self.list(row, object, Some(coefficients_field))?
                                .iter()
                                .map(|coefficient| {
                                    self.rational_value(
                                        coefficient,
                                        object,
                                        Some(coefficients_field),
                                    )
                                })
                                .collect::<CResult<Vec<_>>>()
                        })
                        .collect::<CResult<Vec<_>>>()?;
                    Processor::matrix(dimension("inputs")?, dimension("outputs")?, coefficients)
                }
                "core.onepole/1" | "core.gain/1" | "core.fader/1" => {
                    let channels_field = config
                        .and_then(|fields| fields.get("channels"))
                        .ok_or_else(|| {
                            path_diagnostic(
                                DiagnosticCode::Range,
                                format!("{node_type} requires config.channels"),
                                object,
                                object.field("config"),
                            )
                        })?;
                    let channels = bigint_u64(
                        &self.rational_value(
                            &channels_field.value,
                            object,
                            Some(channels_field),
                        )?,
                        Some(channels_field.value.span),
                    )?;
                    let channels = u8::try_from(channels).map_err(|_| {
                        diagnostics(
                            DiagnosticCode::Range,
                            "channels is out of range",
                            Some(type_field.value.span),
                        )
                    })?;
                    if node_type == "core.gain/1" {
                        Processor::gain(channels)
                    } else if node_type == "core.fader/1" {
                        Processor::fader(channels)
                    } else {
                        Processor::one_pole(channels)
                    }
                }
                "core.pan/1" => Processor::pan(),
                "core.sum/1" => {
                    let channels_field = config
                        .and_then(|fields| fields.get("channels"))
                        .ok_or_else(|| {
                            path_diagnostic(
                                DiagnosticCode::Range,
                                "core.sum/1 requires config.channels",
                                object,
                                object.field("config"),
                            )
                        })?;
                    let channels = bigint_u64(
                        &self.rational_value(
                            &channels_field.value,
                            object,
                            Some(channels_field),
                        )?,
                        Some(channels_field.value.span),
                    )?;
                    Processor::sum(u8::try_from(channels).map_err(|_| {
                        diagnostics(
                            DiagnosticCode::Range,
                            "channels is out of range",
                            Some(type_field.value.span),
                        )
                    })?)
                }
                _ => {
                    return Err(path_diagnostic(
                        DiagnosticCode::Capability,
                        format!("unsupported processor `{node_type}`"),
                        object,
                        Some(type_field),
                    ))
                }
            };
            let mut node = Node::new(object.id.clone(), processor).map_err(plan_error)?;
            match &node.processor {
                Processor::Sine { .. } => {
                    node.params
                        .insert("attack".into(), Rational::new(1.into(), 200.into()));
                    node.params
                        .insert("release".into(), Rational::new(2.into(), 25.into()));
                    node.params
                        .insert("level".into(), Rational::new(1.into(), 5.into()));
                }
                Processor::OnePole { .. } => {
                    node.params
                        .insert("cutoff".into(), Rational::from_integer(1000.into()));
                }
                Processor::Gain { .. } => {
                    node.params.insert("gain".into(), Rational::one());
                }
                Processor::Fader { .. } => {
                    node.params.insert("level".into(), Rational::zero());
                }
                Processor::Noise { .. } => {}
                Processor::Delay { .. } => {}
                Processor::Matrix { .. } => {}
                Processor::Pan => {
                    node.params.insert("pan".into(), Rational::zero());
                }
                Processor::Sum { .. }
                | Processor::Instrument { .. }
                | Processor::Eq { .. }
                | Processor::Compressor { .. }
                | Processor::Reverb { .. } => {}
            }
            if let Some(params_value) = self.optional(object, "params") {
                let params = self.record(params_value, object, object.field("params"))?;
                for (name, field) in params {
                    let value = match &node.processor {
                        Processor::Sine { .. } => match name.as_str() {
                            "attack" | "release" => {
                                self.seconds_value(&field.value, object, Some(field))?
                            }
                            "level" => self.rational_value(&field.value, object, Some(field))?,
                            _ => {
                                return Err(path_diagnostic(
                                    DiagnosticCode::UnknownField,
                                    format!("unknown sine parameter `{name}`"),
                                    object,
                                    Some(field),
                                ))
                            }
                        },
                        Processor::OnePole { .. } if name == "cutoff" => {
                            self.quantity(&field.value, Unit::Hz, object, Some(field))?
                        }
                        Processor::Gain { .. } if name == "gain" => {
                            self.rational_value(&field.value, object, Some(field))?
                        }
                        Processor::Fader { .. } if name == "level" => {
                            self.quantity(&field.value, Unit::Db, object, Some(field))?
                        }
                        Processor::Noise { .. } => {
                            return Err(path_diagnostic(
                                DiagnosticCode::UnknownField,
                                format!("noise has no parameter `{name}`"),
                                object,
                                Some(field),
                            ))
                        }
                        Processor::Delay { .. } => {
                            return Err(path_diagnostic(
                                DiagnosticCode::UnknownField,
                                format!("delay has no parameter `{name}`"),
                                object,
                                Some(field),
                            ))
                        }
                        Processor::Matrix { .. } => {
                            return Err(path_diagnostic(
                                DiagnosticCode::UnknownField,
                                format!("matrix has no parameter `{name}`"),
                                object,
                                Some(field),
                            ))
                        }
                        Processor::Pan if name == "pan" => {
                            self.rational_value(&field.value, object, Some(field))?
                        }
                        Processor::Sum { .. } => {
                            return Err(path_diagnostic(
                                DiagnosticCode::UnknownField,
                                format!("sum has no parameter `{name}`"),
                                object,
                                Some(field),
                            ))
                        }
                        Processor::Instrument { .. } => {
                            self.rational_value(&field.value, object, Some(field))?
                        }
                        _ => {
                            return Err(path_diagnostic(
                                DiagnosticCode::UnknownField,
                                format!("unknown processor parameter `{name}`"),
                                object,
                                Some(field),
                            ))
                        }
                    };
                    node.params.insert(name.clone(), value);
                }
            }
            self.nodes.push(node);
        }
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "connect")
        {
            let from = self.port_reference(
                &self.field(object, "from")?.value,
                object,
                object.field("from"),
            )?;
            let to =
                self.port_reference(&self.field(object, "to")?.value, object, object.field("to"))?;
            self.connections
                .push(Connection::new(object.id.clone(), from, to).map_err(plan_error)?);
        }
        Ok(())
    }

    fn parse_pitch(&self, value: &Value, object: &Object, field: Option<&Field>) -> CResult<Pitch> {
        match &value.kind {
            ValueKind::Symbol(symbol) | ValueKind::String(symbol) => Pitch::parse_spelled(symbol)
                .map_err(|error| map_music_error(error, Some(value.span))),
            ValueKind::Quantity { unit: Unit::Hz, .. }
            | ValueKind::Quantity {
                unit: Unit::KHz, ..
            } => Pitch::hz(self.quantity(value, Unit::Hz, object, field)?)
                .map_err(|error| map_music_error(error, Some(value.span))),
            ValueKind::Call { function, args } if function == "key" && args.len() == 1 => {
                let key = bigint_i64(
                    &self.rational_value(&args[0], object, field)?,
                    Some(args[0].span),
                )?;
                Ok(Pitch::key(key))
            }
            ValueKind::Call { function, args } if function == "degree" && args.len() == 2 => {
                let index = bigint_i64(
                    &self.rational_value(&args[0], object, field)?,
                    Some(args[0].span),
                )?;
                let tuning_id = self.one_reference(&args[1], object, field)?;
                let tuning = self.tunings.get(&tuning_id).ok_or_else(|| {
                    path_diagnostic(
                        DiagnosticCode::Reference,
                        "unknown tuning reference",
                        object,
                        field,
                    )
                })?;
                Ok(Pitch::degree(index, tuning.clone()))
            }
            ValueKind::Call { function, args } if function == "ratio" && args.len() == 2 => {
                let ratio = self.rational_value(&args[0], object, field)?;
                let frequency = self.quantity(&args[1], Unit::Hz, object, field)?;
                Pitch::ratio(ratio, frequency)
                    .map_err(|error| map_music_error(error, Some(value.span)))
            }
            _ => Err(path_diagnostic(
                DiagnosticCode::Unit,
                "invalid pitch value",
                object,
                field,
            )),
        }
    }

    fn parse_note(&self, object: &Object) -> CResult<LeafDef> {
        self.check_fields(
            object,
            &[
                "at",
                "dur",
                "pitch",
                "velocity",
                "release_velocity",
                "onset_offset",
                "release_offset",
                "order",
            ],
        )?;
        let at_field = self.field(object, "at")?;
        let dur_field = self.field(object, "dur")?;
        let pitch_field = self.field(object, "pitch")?;
        let at = self.q_value(&at_field.value, object, Some(at_field), false)?;
        let dur = self.q_value(&dur_field.value, object, Some(dur_field), false)?;
        if at.is_negative() || dur <= Rational::zero() {
            return Err(path_diagnostic(
                DiagnosticCode::Interval,
                "note onset must be nonnegative and duration positive",
                object,
                None,
            ));
        }
        let pitch = self.parse_pitch(&pitch_field.value, object, Some(pitch_field))?;
        let velocity = self
            .optional(object, "velocity")
            .map(|value| self.rational_value(value, object, object.field("velocity")))
            .transpose()?
            .unwrap_or_else(Rational::one);
        let release_velocity = self
            .optional(object, "release_velocity")
            .map(|value| self.rational_value(value, object, object.field("release_velocity")))
            .transpose()?
            .unwrap_or_else(|| Rational::new(1.into(), 2.into()));
        if velocity.is_negative()
            || velocity > Rational::one()
            || release_velocity.is_negative()
            || release_velocity > Rational::one()
        {
            return Err(path_diagnostic(
                DiagnosticCode::Range,
                "velocity values must be in [0,1]",
                object,
                None,
            ));
        }
        let release_velocity = release_velocity.to_f64().ok_or_else(|| {
            diagnostics(
                DiagnosticCode::Nonfinite,
                "release velocity is not finite",
                Some(object.span),
            )
        })?;
        let onset_offset = self
            .optional(object, "onset_offset")
            .map(|value| self.seconds_value(value, object, object.field("onset_offset")))
            .transpose()?
            .unwrap_or_else(Rational::zero);
        let release_offset = self
            .optional(object, "release_offset")
            .map(|value| self.seconds_value(value, object, object.field("release_offset")))
            .transpose()?
            .unwrap_or_else(Rational::zero);
        let order = self
            .optional(object, "order")
            .map(|value| {
                bigint_i64(
                    &self.rational_value(value, object, object.field("order"))?,
                    Some(value.span),
                )
            })
            .transpose()?
            .unwrap_or(0);
        let order = i32::try_from(order).map_err(|_| {
            diagnostics(
                DiagnosticCode::ResourceLimit,
                "event order exceeds signed 32-bit range",
                Some(object.span),
            )
        })?;
        let expression_curve = |kind: &str| {
            object
                .children
                .values()
                .find(|child| {
                    child.kind == "expression"
                        && child.field("kind").and_then(|f| f.value.as_symbol()) == Some(kind)
                })
                .map(|child| {
                    self.one_reference(
                        &self.field(child, "curve")?.value,
                        child,
                        child.field("curve"),
                    )
                })
                .transpose()
        };
        let pitch_expression = expression_curve("pitch")?;
        let gain_expression = expression_curve("gain")?;
        let timbre_expression = expression_curve("timbre")?;
        let pressure_expression = expression_curve("pressure")?;
        Ok(LeafDef {
            id: object.id.clone(),
            span: object.span,
            at,
            velocity,
            onset_offset,
            order,
            kind: LeafKind::Note(Box::new(NoteDef {
                pitch_expression,
                gain_expression,
                timbre_expression,
                pressure_expression,
                dur,
                pitch,
                release_velocity,
                release_offset,
            })),
        })
    }

    fn parse_hit(&self, object: &Object) -> CResult<LeafDef> {
        self.check_fields(object, &["at", "key", "velocity", "onset_offset", "order"])?;
        if !object.children.is_empty() {
            return Err(path_diagnostic(
                DiagnosticCode::UnknownKind,
                "hit cannot have children",
                object,
                None,
            ));
        }
        let at_field = self.field(object, "at")?;
        let at = self.q_value(&at_field.value, object, Some(at_field), false)?;
        if at.is_negative() {
            return Err(path_diagnostic(
                DiagnosticCode::Range,
                "hit onset must be nonnegative",
                object,
                Some(at_field),
            ));
        }
        let key_field = self.field(object, "key")?;
        let key = self.string_value(&key_field.value, object, Some(key_field), "key")?;
        let velocity = self
            .optional(object, "velocity")
            .map(|value| self.rational_value(value, object, object.field("velocity")))
            .transpose()?
            .unwrap_or_else(Rational::one);
        if velocity.is_negative() || velocity > Rational::one() {
            return Err(path_diagnostic(
                DiagnosticCode::Range,
                "hit velocity must be in [0,1]",
                object,
                object.field("velocity"),
            ));
        }
        let onset_offset = self
            .optional(object, "onset_offset")
            .map(|value| self.seconds_value(value, object, object.field("onset_offset")))
            .transpose()?
            .unwrap_or_else(Rational::zero);
        let order = self
            .optional(object, "order")
            .map(|value| {
                bigint_i64(
                    &self.rational_value(value, object, object.field("order"))?,
                    Some(value.span),
                )
            })
            .transpose()?
            .unwrap_or(0);
        let order = i32::try_from(order).map_err(|_| {
            diagnostics(
                DiagnosticCode::ResourceLimit,
                "event order exceeds signed 32-bit range",
                Some(object.span),
            )
        })?;
        Ok(LeafDef {
            id: object.id.clone(),
            span: object.span,
            at,
            velocity,
            onset_offset,
            order,
            kind: LeafKind::Hit { key },
        })
    }

    fn parse_use(&self, object: &Object) -> CResult<UseDef> {
        self.check_fields(
            object,
            &["pattern", "at", "count", "stretch", "transpose", "boundary"],
        )?;
        let pattern = self.one_reference(
            &self.field(object, "pattern")?.value,
            object,
            object.field("pattern"),
        )?;
        let at = self.q_value(
            &self.field(object, "at")?.value,
            object,
            object.field("at"),
            false,
        )?;
        let count = self
            .optional(object, "count")
            .map(|value| {
                bigint_u64(
                    &self.rational_value(value, object, object.field("count"))?,
                    Some(value.span),
                )
            })
            .transpose()?
            .unwrap_or(1);
        let stretch = self
            .optional(object, "stretch")
            .map(|value| self.rational_value(value, object, object.field("stretch")))
            .transpose()?
            .unwrap_or_else(Rational::one);
        let transpose = self
            .optional(object, "transpose")
            .map(|value| self.cents_value(value, object, object.field("transpose")))
            .transpose()?
            .unwrap_or_else(Rational::zero);
        let boundary = self
            .optional(object, "boundary")
            .map(|value| self.symbol_value(value, object, object.field("boundary")))
            .transpose()?
            .unwrap_or_else(|| "spill".to_owned());
        if at.is_negative()
            || count == 0
            || stretch <= Rational::zero()
            || !matches!(boundary.as_str(), "cut" | "spill")
        {
            return Err(path_diagnostic(
                DiagnosticCode::Range,
                "invalid use repetition parameters",
                object,
                None,
            ));
        }
        if count > MAX_EXPANDED_NOTES as u64 {
            return Err(path_diagnostic(
                DiagnosticCode::ResourceLimit,
                "use repetition count exceeds bounded expansion",
                object,
                None,
            ));
        }
        Ok(UseDef {
            id: object.id.clone(),
            span: object.span,
            pattern,
            at,
            count,
            stretch,
            transpose_cents: transpose,
            cut: boundary == "cut",
        })
    }

    fn read_patterns(&mut self) -> CResult<()> {
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "pattern")
        {
            let length_field = self.field(object, "length")?;
            let length = self.q_value(&length_field.value, object, Some(length_field), false)?;
            if length <= Rational::zero() {
                return Err(path_diagnostic(
                    DiagnosticCode::Interval,
                    "pattern length must be positive",
                    object,
                    Some(length_field),
                ));
            }
            let mut children = Vec::new();
            for child in object.children.values() {
                let parsed = match child.kind.as_str() {
                    "note" => PatternChild::Leaf(Box::new(self.parse_note(child)?)),
                    "use" => PatternChild::Use(Box::new(self.parse_use(child)?)),
                    "hit" if self.allow_hits => {
                        PatternChild::Leaf(Box::new(self.parse_hit(child)?))
                    }
                    "hit" | "message" => {
                        return Err(path_diagnostic(
                            DiagnosticCode::Capability,
                            format!(
                                "pattern child kind `{}` is outside the standalone sine plan",
                                child.kind
                            ),
                            child,
                            None,
                        ))
                    }
                    "expression" => {
                        return Err(path_diagnostic(
                            DiagnosticCode::Capability,
                            "expression is not a direct pattern child",
                            child,
                            None,
                        ))
                    }
                    _ => {
                        return Err(path_diagnostic(
                            DiagnosticCode::UnknownKind,
                            format!("unknown pattern child kind `{}`", child.kind),
                            child,
                            None,
                        ))
                    }
                };
                match &parsed {
                    PatternChild::Leaf(note) if note.at >= length => {
                        return Err(path_diagnostic(
                            DiagnosticCode::Interval,
                            "pattern child onset must be before pattern length",
                            child,
                            None,
                        ))
                    }
                    PatternChild::Use(use_def) if use_def.at >= length => {
                        return Err(path_diagnostic(
                            DiagnosticCode::Interval,
                            "use onset must be before pattern length",
                            child,
                            None,
                        ))
                    }
                    _ => {}
                }
                children.push(parsed);
            }
            self.patterns.insert(
                object.id.clone(),
                PatternDef {
                    id: object.id.clone(),
                    span: object.span,
                    length,
                    children,
                },
            );
        }
        Ok(())
    }

    fn validate_pattern_graph(&self) -> CResult<()> {
        let mut states: HashMap<&str, u8> = HashMap::new();
        for id in self.patterns.keys() {
            self.visit_pattern(id, &mut states, 0)?;
        }
        Ok(())
    }

    fn visit_pattern<'b>(
        &'b self,
        id: &'b str,
        states: &mut HashMap<&'b str, u8>,
        depth: usize,
    ) -> CResult<()> {
        if depth >= MAX_PATTERN_DEPTH {
            return Err(diagnostics(
                DiagnosticCode::ResourceLimit,
                format!("pattern nesting exceeds {MAX_PATTERN_DEPTH} levels"),
                self.patterns.get(id).map(|pattern| pattern.span),
            ));
        }
        match states.get(id).copied() {
            Some(1) => {
                return Err(diagnostics(
                    DiagnosticCode::PatternCycle,
                    format!("pattern reference cycle includes `{id}`"),
                    self.patterns.get(id).map(|p| p.span),
                ))
            }
            Some(2) => return Ok(()),
            _ => {}
        }
        let pattern = self.patterns.get(id).ok_or_else(|| {
            diagnostics(
                DiagnosticCode::Reference,
                format!("unknown pattern reference `&{id}`"),
                None,
            )
        })?;
        states.insert(id, 1);
        for child in &pattern.children {
            if let PatternChild::Use(use_def) = child {
                let source = self.patterns.get(&use_def.pattern).ok_or_else(|| {
                    diagnostics(
                        DiagnosticCode::Reference,
                        format!("unknown pattern reference `&{}'", use_def.pattern),
                        Some(use_def.span),
                    )
                })?;
                let span = use_def.span;
                let occupied = checked_mul(
                    &checked_mul(
                        &use_def.stretch,
                        &Rational::from_integer(BigInt::from(use_def.count)),
                        Some(span),
                    )?,
                    &source.length,
                    Some(span),
                )?;
                let end = checked_add(&use_def.at, &occupied, Some(span))?;
                if end > pattern.length {
                    return Err(diagnostics(
                        DiagnosticCode::Interval,
                        "repeated use does not fit in its parent pattern",
                        Some(span),
                    ));
                }
                self.visit_pattern(&use_def.pattern, states, depth + 1)?;
            }
        }
        states.insert(id, 2);
        Ok(())
    }

    fn read_tracks_and_places(&mut self) -> CResult<()> {
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "track")
        {
            self.check_fields(object, &["target"])?;
            if let Some(target) = object.field("target") {
                self.tracks.insert(
                    object.id.clone(),
                    self.port_reference(&target.value, object, Some(target))?,
                );
            }
        }
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "place")
        {
            let pattern = self.one_reference(
                &self.field(object, "pattern")?.value,
                object,
                object.field("pattern"),
            )?;
            let track = self.one_reference(
                &self.field(object, "track")?.value,
                object,
                object.field("track"),
            )?;
            let at = self.q_value(
                &self.field(object, "at")?.value,
                object,
                object.field("at"),
                true,
            )?;
            let count = self
                .optional(object, "count")
                .map(|value| {
                    bigint_u64(
                        &self.rational_value(value, object, object.field("count"))?,
                        Some(value.span),
                    )
                })
                .transpose()?
                .unwrap_or(1);
            let stretch = self
                .optional(object, "stretch")
                .map(|value| self.rational_value(value, object, object.field("stretch")))
                .transpose()?
                .unwrap_or_else(Rational::one);
            let transpose = self
                .optional(object, "transpose")
                .map(|value| self.cents_value(value, object, object.field("transpose")))
                .transpose()?
                .unwrap_or_else(Rational::zero);
            let boundary = self
                .optional(object, "boundary")
                .map(|value| self.symbol_value(value, object, object.field("boundary")))
                .transpose()?
                .unwrap_or_else(|| "spill".to_owned());
            if count > MAX_EXPANDED_NOTES as u64 {
                return Err(path_diagnostic(
                    DiagnosticCode::ResourceLimit,
                    "placement repetition count exceeds bounded expansion",
                    object,
                    object.field("count"),
                ));
            }
            if at < self.score_start
                || count == 0
                || stretch <= Rational::zero()
                || !matches!(boundary.as_str(), "cut" | "spill")
            {
                return Err(path_diagnostic(
                    DiagnosticCode::Range,
                    "invalid placement parameters",
                    object,
                    None,
                ));
            }
            let mut overrides = Vec::new();
            let mut inserts = Vec::new();
            for child in object.children.values() {
                match child.kind.as_str() {
                    "override" => {
                        let event_field = self.field(child, "event")?;
                        let event = self.string_value(
                            &event_field.value,
                            child,
                            Some(event_field),
                            "event",
                        )?;
                        let set_present = child.field("set").is_some();
                        let delete_present = child.field("delete").is_some();
                        if set_present == delete_present {
                            return Err(path_diagnostic(
                                DiagnosticCode::Interval,
                                "override requires exactly one of set or delete",
                                child,
                                None,
                            ));
                        }
                        let set: BTreeMap<String, Value> = child
                            .field("set")
                            .map(|field| self.record(&field.value, child, Some(field)))
                            .transpose()?
                            .map(|fields| {
                                fields
                                    .iter()
                                    .map(|(name, field)| (name.clone(), field.value.clone()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let delete = child
                            .field("delete")
                            .map(|field| self.bool_value(&field.value, child, Some(field)))
                            .transpose()?
                            .unwrap_or(false);
                        if delete_present && !delete {
                            return Err(path_diagnostic(
                                DiagnosticCode::Range,
                                "override delete must be true",
                                child,
                                child.field("delete"),
                            ));
                        }
                        overrides.push(OverrideDef {
                            span: child.span,
                            event,
                            delete,
                            set,
                        });
                    }
                    "insert" => {
                        if child.children.len() != 1 {
                            return Err(path_diagnostic(
                                DiagnosticCode::Interval,
                                "insert must contain exactly one leaf",
                                child,
                                None,
                            ));
                        }
                        let leaf = child.children.values().next().unwrap();
                        if leaf.kind != "note" && !(self.allow_hits && leaf.kind == "hit") {
                            return Err(path_diagnostic(
                                DiagnosticCode::Capability,
                                "only note inserts are supported by the standalone sine plan",
                                leaf,
                                None,
                            ));
                        }
                        inserts.push(InsertDef {
                            id: child.id.clone(),
                            span: child.span,
                            leaf: if leaf.kind == "hit" {
                                self.parse_hit(leaf)?
                            } else {
                                self.parse_note(leaf)?
                            },
                        });
                    }
                    _ => {
                        return Err(path_diagnostic(
                            DiagnosticCode::UnknownKind,
                            format!("unknown place child kind `{}`", child.kind),
                            child,
                            None,
                        ))
                    }
                }
            }
            self.places.push(PlaceDef {
                id: object.id.clone(),
                span: object.span,
                pattern,
                track,
                at,
                count,
                stretch,
                transpose_cents: transpose,
                cut: boundary == "cut",
                overrides,
                inserts,
            });
        }
        Ok(())
    }

    fn read_curves_and_automation(&mut self, limits: &PlanLimits) -> CResult<()> {
        let mut automation_points = 0usize;
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "curve")
        {
            let clock = self.symbol_value(
                &self.field(object, "clock")?.value,
                object,
                object.field("clock"),
            )?;
            if !matches!(clock.as_str(), "score" | "seconds" | "normalized") {
                return Err(path_diagnostic(
                    DiagnosticCode::Capability,
                    "unsupported curve clock",
                    object,
                    None,
                ));
            }
            let field = self.field(object, "points")?;
            let values = self.list(&field.value, object, Some(field))?;
            if values.is_empty() {
                return Err(path_diagnostic(
                    DiagnosticCode::Range,
                    "curve requires points",
                    object,
                    Some(field),
                ));
            }
            let mut points = Vec::new();
            for value in values {
                let tuple = self.tuple(value, object, Some(field), 3)?;
                let position = match clock.as_str() {
                    "score" => self.q_value(&tuple[0], object, Some(field), false)?,
                    "normalized" => self.rational_value(&tuple[0], object, Some(field))?,
                    _ => self.seconds_value(&tuple[0], object, Some(field))?,
                };
                let value = if matches!(tuple[1].kind, ValueKind::Quantity { unit: Unit::Ct, .. }) {
                    self.cents_value(&tuple[1], object, Some(field))?
                } else {
                    self.numeric_parameter_value(&tuple[1], object, Some(field))?
                };
                let shape = match self.symbol_value(&tuple[2], object, Some(field))?.as_str() {
                    "step" => Interpolation::Step,
                    "linear" => Interpolation::Linear,
                    "exponential" => Interpolation::Exponential,
                    _ => {
                        return Err(path_diagnostic(
                            DiagnosticCode::Range,
                            "unknown curve interpolation",
                            object,
                            Some(field),
                        ))
                    }
                };
                points.push((position, value, shape));
            }
            self.curves
                .insert(object.id.clone(), CurveDef { clock, points });
        }
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "automation")
        {
            let target_ref = self.reference(
                &self.field(object, "target")?.value,
                object,
                object.field("target"),
            )?;
            if target_ref.path.len() != 3
                || target_ref.path[1] != "params"
                || target_ref.port.is_some()
            {
                return Err(path_diagnostic(
                    DiagnosticCode::Reference,
                    "automation target must be a node parameter reference",
                    object,
                    object.field("target"),
                ));
            }
            let curve_id = self.one_reference(
                &self.field(object, "curve")?.value,
                object,
                object.field("curve"),
            )?;
            let curve = self.curves.get(&curve_id).ok_or_else(|| {
                path_diagnostic(
                    DiagnosticCode::Reference,
                    "automation curve does not exist",
                    object,
                    object.field("curve"),
                )
            })?;
            if curve.clock == "normalized" {
                return Err(path_diagnostic(
                    DiagnosticCode::Capability,
                    "normalized curves cannot be global automation",
                    object,
                    object.field("curve"),
                ));
            }
            automation_points = automation_points
                .checked_add(curve.points.len())
                .ok_or_else(|| {
                    diagnostics(
                        DiagnosticCode::ResourceLimit,
                        "automation point budget overflow",
                        Some(object.span),
                    )
                })?;
            if automation_points > limits.max_automation_points {
                return Err(path_diagnostic(
                    DiagnosticCode::ResourceLimit,
                    "automation point budget exceeded",
                    object,
                    None,
                ));
            }
            let at_value = self.field(object, "at")?;
            let score_anchor = curve.clock == "score"
                || match &at_value.value.kind {
                    ValueKind::Quantity { unit: Unit::Q, .. } => true,
                    ValueKind::Call { function, .. } => function == "bar",
                    _ => false,
                };
            let at = if score_anchor {
                AutomationAnchor::Score {
                    q: self.q_value(&at_value.value, object, Some(at_value), true)?,
                }
            } else {
                AutomationAnchor::Seconds {
                    seconds: self.seconds_value(&at_value.value, object, Some(at_value))?,
                }
            };
            let target = PortRef::new(target_ref.path[0].clone(), target_ref.path[2].clone())
                .map_err(plan_error)?;
            let points = curve
                .points
                .iter()
                .map(|(position, value, shape)| AutomationPoint {
                    position: position.clone(),
                    value: value.clone(),
                    shape: *shape,
                })
                .collect();
            self.automations.push(AutomationV3 {
                id: object.id.clone(),
                target,
                clock: if curve.clock == "score" {
                    AutomationClock::Score
                } else {
                    AutomationClock::Seconds
                },
                at,
                points,
            });
        }
        Ok(())
    }

    fn numeric_parameter_value(
        &self,
        value: &Value,
        object: &Object,
        field: Option<&Field>,
    ) -> CResult<Rational> {
        match value.kind {
            ValueKind::Number(_) => self.rational_value(value, object, field),
            ValueKind::Quantity { unit: Unit::Db, .. } => {
                self.quantity(value, Unit::Db, object, field)
            }
            ValueKind::Quantity { unit: Unit::Hz, .. }
            | ValueKind::Quantity {
                unit: Unit::KHz, ..
            } => self.quantity(value, Unit::Hz, object, field),
            ValueKind::Quantity { unit: Unit::S, .. }
            | ValueKind::Quantity { unit: Unit::Ms, .. } => {
                self.seconds_value(value, object, field)
            }
            _ => Err(path_diagnostic(
                DiagnosticCode::Unit,
                "curve value must be numeric",
                object,
                field,
            )),
        }
    }

    fn read_regions(&mut self) -> CResult<()> {
        for object in self
            .document
            .objects
            .values()
            .filter(|object| object.kind == "region")
        {
            let field = self.field(object, "span")?;
            let values = self.list(&field.value, object, Some(field))?;
            if values.len() != 2 {
                return Err(path_diagnostic(
                    DiagnosticCode::Interval,
                    "region span must contain [start,end]",
                    object,
                    Some(field),
                ));
            }
            let start = self.q_value(&values[0], object, Some(field), true)?;
            let end = self.q_value(&values[1], object, Some(field), true)?;
            let label = self
                .optional(object, "label")
                .map(|value| self.string_value(value, object, object.field("label"), "label"))
                .transpose()?;
            if start < self.score_start || end > self.score_end || start >= end {
                return Err(path_diagnostic(
                    DiagnosticCode::Interval,
                    "region must be a positive interval inside the score",
                    object,
                    Some(field),
                ));
            }
            self.regions.push(Region {
                id: object.id.clone(),
                start_q: start,
                end_q: end,
                label,
            });
        }
        Ok(())
    }

    fn expand_places(&mut self) -> CResult<()> {
        let places = self.places.clone();
        for place in &places {
            let (pattern_work, pattern_events) = self.pattern_bounds(&place.pattern)?;
            let place_work = bounded_u64_mul(place.count, pattern_work.max(1), place.span)?;
            let projected_work = self.expansion_work.saturating_add(place_work);
            if projected_work > MAX_EXPANSION_WORK {
                return Err(diagnostics(
                    DiagnosticCode::ResourceLimit,
                    "pattern expansion work exceeds the bounded limit",
                    Some(place.span),
                ));
            }
            let place_events = bounded_u64_mul(place.count, pattern_events, place.span)?;
            if place_events > MAX_EXPANDED_NOTES as u64
                || self.events.len() as u64 + place_events > MAX_EXPANDED_NOTES as u64
            {
                return Err(diagnostics(
                    DiagnosticCode::ResourceLimit,
                    "expanded note count exceeds 100000",
                    Some(place.span),
                ));
            }
            let pattern_length = self
                .patterns
                .get(&place.pattern)
                .map(|pattern| pattern.length.clone())
                .ok_or_else(|| {
                    diagnostics(
                        DiagnosticCode::Reference,
                        format!("unknown pattern reference `&{}'", place.pattern),
                        Some(place.span),
                    )
                })?;
            let occupied = checked_mul(
                &checked_mul(
                    &place.stretch,
                    &Rational::from_integer(BigInt::from(place.count)),
                    Some(place.span),
                )?,
                &pattern_length,
                Some(place.span),
            )?;
            let place_end = checked_add(&place.at, &occupied, Some(place.span))?;
            if place_end > self.score_end {
                return Err(diagnostics(
                    DiagnosticCode::Interval,
                    "placement repetition span must fit in the project score",
                    Some(place.span),
                ));
            }
            let target = self.tracks.get(&place.track).cloned().ok_or_else(|| {
                diagnostics(
                    DiagnosticCode::Reference,
                    format!("unknown track reference `&{}'", place.track),
                    Some(place.span),
                )
            })?;
            if place.count > MAX_EXPANDED_NOTES as u64 {
                return Err(diagnostics(
                    DiagnosticCode::ResourceLimit,
                    "placement repetition count exceeds bounded expansion",
                    Some(place.span),
                ));
            }
            for repetition in 0..place.count {
                let repetition_offset = checked_mul(
                    &checked_mul(&place.stretch, &pattern_length, Some(place.span))?,
                    &Rational::from_integer(BigInt::from(repetition)),
                    Some(place.span),
                )?;
                let origin = checked_add(&place.at, &repetition_offset, Some(place.span))?;
                let mut address = vec![place.id.clone(), repetition.to_string()];
                let cut_end = if place.cut {
                    Some(checked_add(
                        &origin,
                        &checked_mul(&place.stretch, &pattern_length, Some(place.span))?,
                        Some(place.span),
                    )?)
                } else {
                    None
                };
                let mut state = ExpansionState {
                    origin,
                    scale: place.stretch.clone(),
                    transpose: place.transpose_cents.clone(),
                    address: &mut address,
                    inherited_cut_end: cut_end,
                    target: &target,
                    depth: 0,
                };
                self.expand_pattern(&place.pattern, &mut state)?;
            }
            self.apply_place_edits(place, &target)?;
        }
        Ok(())
    }

    /// Compute bounded expansion work and emitted-note count without walking
    /// every repetition.  This is a preflight guard for empty patterns and
    /// nested repetitions whose emitted count alone may be zero or small.
    fn pattern_bounds(&self, id: &str) -> CResult<(u64, u64)> {
        fn visit(
            compiler: &Compiler<'_>,
            id: &str,
            memo: &mut HashMap<String, (u64, u64)>,
            depth: usize,
        ) -> CResult<(u64, u64)> {
            if depth >= MAX_PATTERN_DEPTH {
                return Err(diagnostics(
                    DiagnosticCode::ResourceLimit,
                    format!("pattern nesting exceeds {MAX_PATTERN_DEPTH} levels"),
                    compiler.patterns.get(id).map(|pattern| pattern.span),
                ));
            }
            if let Some(value) = memo.get(id) {
                return Ok(*value);
            }
            let pattern = compiler.patterns.get(id).ok_or_else(|| {
                diagnostics(
                    DiagnosticCode::Reference,
                    format!("unknown pattern reference `&{id}`"),
                    None,
                )
            })?;
            let mut work = pattern.children.len() as u64;
            let mut events = pattern
                .children
                .iter()
                .filter(|child| matches!(child, PatternChild::Leaf(_)))
                .count() as u64;
            for child in &pattern.children {
                if let PatternChild::Use(use_def) = child {
                    let (child_work, child_events) =
                        visit(compiler, &use_def.pattern, memo, depth + 1)?;
                    let invocation_work = child_work.saturating_add(1);
                    work = work.saturating_add(use_def.count.saturating_mul(invocation_work));
                    events = events.saturating_add(use_def.count.saturating_mul(child_events));
                    if work > MAX_EXPANSION_WORK || events > MAX_EXPANDED_NOTES as u64 {
                        return Err(diagnostics(
                            DiagnosticCode::ResourceLimit,
                            "bounded pattern expansion exceeds host limits",
                            Some(use_def.span),
                        ));
                    }
                }
            }
            memo.insert(id.to_owned(), (work, events));
            Ok((work, events))
        }

        visit(self, id, &mut HashMap::new(), 0)
    }

    fn expand_leaf(
        leaf: &LeafDef,
        state: &ExpansionState<'_>,
        source_path: Vec<String>,
    ) -> CResult<ExpandedEvent> {
        let score_on_q = checked_add(
            &state.origin,
            &checked_mul(&state.scale, &leaf.at, Some(leaf.span))?,
            Some(leaf.span),
        )?;
        let kind = match &leaf.kind {
            LeafKind::Hit { key } => ExpandedKind::Hit { key: key.clone() },
            LeafKind::Note(note) => {
                let dur = checked_mul(&state.scale, &note.dur, Some(leaf.span))?;
                let end = checked_add(&score_on_q, &dur, Some(leaf.span))?;
                let dur_q = if let Some(cut_end) = &state.inherited_cut_end {
                    if end > *cut_end {
                        checked_sub(cut_end, &score_on_q, Some(leaf.span))?
                    } else {
                        dur
                    }
                } else {
                    dur
                };
                if dur_q <= Rational::zero() {
                    return Err(diagnostics(
                        DiagnosticCode::Interval,
                        "cut boundary removes a note's entire positive gate",
                        Some(leaf.span),
                    ));
                }
                ExpandedKind::Note(Box::new(ExpandedNote {
                    pitch_expression: note.pitch_expression.clone(),
                    gain_expression: note.gain_expression.clone(),
                    timbre_expression: note.timbre_expression.clone(),
                    pressure_expression: note.pressure_expression.clone(),
                    expression_scale: state.scale.clone(),
                    pitch: note.pitch.clone(),
                    transpose_cents: state.transpose.clone(),
                    dur_q,
                    release_velocity: note.release_velocity,
                    release_offset_seconds: note.release_offset.clone(),
                }))
            }
        };
        Ok(ExpandedEvent {
            address: state.address.join("/"),
            source: SourceMapping {
                object: leaf.id.clone(),
                path: source_path,
                span: Some(SourceSpan {
                    start: leaf.span.start,
                    end: leaf.span.end,
                }),
            },
            source_span: leaf.span,
            target: state.target.clone(),
            score_on_q,
            velocity: leaf.velocity.clone(),
            onset_offset_seconds: leaf.onset_offset.clone(),
            order: leaf.order,
            kind,
        })
    }

    fn expand_pattern(&mut self, pattern_id: &str, state: &mut ExpansionState<'_>) -> CResult<()> {
        if state.depth >= MAX_PATTERN_DEPTH {
            return Err(diagnostics(
                DiagnosticCode::ResourceLimit,
                format!("pattern nesting exceeds {MAX_PATTERN_DEPTH} levels"),
                None,
            ));
        }
        let pattern = self.patterns.get(pattern_id).cloned().ok_or_else(|| {
            diagnostics(
                DiagnosticCode::Reference,
                format!("unknown pattern reference `&{pattern_id}`"),
                None,
            )
        })?;
        for child in pattern.children {
            self.expansion_work = self.expansion_work.saturating_add(1);
            if self.expansion_work > MAX_EXPANSION_WORK {
                return Err(diagnostics(
                    DiagnosticCode::ResourceLimit,
                    "pattern expansion work exceeds the bounded limit",
                    Some(pattern.span),
                ));
            }
            match child {
                PatternChild::Leaf(leaf) => {
                    state.address.push(leaf.id.clone());
                    let source_path = vec![pattern.id.clone(), leaf.id.clone()];
                    let event = Self::expand_leaf(&leaf, state, source_path)?;
                    if self.events.len() >= MAX_EXPANDED_NOTES {
                        return Err(diagnostics(
                            DiagnosticCode::ResourceLimit,
                            "expanded note count exceeds 100000",
                            Some(leaf.span),
                        ));
                    }
                    self.events.push(event);
                    state.address.pop();
                }
                PatternChild::Use(use_def) => {
                    let source = self
                        .patterns
                        .get(&use_def.pattern)
                        .cloned()
                        .ok_or_else(|| {
                            diagnostics(
                                DiagnosticCode::Reference,
                                format!("unknown pattern reference `&{}'", use_def.pattern),
                                Some(use_def.span),
                            )
                        })?;
                    let use_origin = checked_add(
                        &state.origin,
                        &checked_mul(&state.scale, &use_def.at, Some(use_def.span))?,
                        Some(use_def.span),
                    )?;
                    let child_scale =
                        checked_mul(&state.scale, &use_def.stretch, Some(use_def.span))?;
                    let child_transpose = checked_add(
                        &state.transpose,
                        &use_def.transpose_cents,
                        Some(use_def.span),
                    )?;
                    let child_span = checked_mul(&child_scale, &source.length, Some(use_def.span))?;
                    for repetition in 0..use_def.count {
                        self.expansion_work = self.expansion_work.saturating_add(1);
                        if self.expansion_work > MAX_EXPANSION_WORK {
                            return Err(diagnostics(
                                DiagnosticCode::ResourceLimit,
                                "pattern expansion work exceeds the bounded limit",
                                Some(use_def.span),
                            ));
                        }
                        let repetition_offset = checked_mul(
                            &child_span,
                            &Rational::from_integer(BigInt::from(repetition)),
                            Some(use_def.span),
                        )?;
                        let child_origin =
                            checked_add(&use_origin, &repetition_offset, Some(use_def.span))?;
                        let boundary = checked_add(&child_origin, &child_span, Some(use_def.span))?;
                        let cut_end = match (&state.inherited_cut_end, use_def.cut) {
                            (Some(parent), true) => Some(parent.clone().min(boundary)),
                            (Some(parent), false) => Some(parent.clone()),
                            (None, true) => Some(boundary),
                            (None, false) => None,
                        };
                        state.address.push(use_def.id.clone());
                        state.address.push(repetition.to_string());
                        let mut child_state = ExpansionState {
                            origin: child_origin,
                            scale: child_scale.clone(),
                            transpose: child_transpose.clone(),
                            address: &mut *state.address,
                            inherited_cut_end: cut_end,
                            target: state.target,
                            depth: state.depth + 1,
                        };
                        self.expand_pattern(&use_def.pattern, &mut child_state)?;
                        state.address.pop();
                        state.address.pop();
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_place_edits(&mut self, place: &PlaceDef, target: &EventTarget) -> CResult<()> {
        let place_prefix = format!("{}/", place.id);
        let indexes: HashMap<String, usize> = self
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.address.starts_with(&place_prefix))
            .map(|(index, event)| (event.address.clone(), index))
            .collect();
        let mut deleted = HashSet::new();
        let mut override_targets = HashSet::new();
        for override_def in &place.overrides {
            let address = format!("{}/{}", place.id, override_def.event);
            if !override_targets.insert(address.clone()) {
                return Err(diagnostics(
                    DiagnosticCode::InstanceTarget,
                    "an occurrence may have at most one override",
                    Some(override_def.span),
                ));
            }
            let index = indexes.get(&address).copied().ok_or_else(|| {
                diagnostics(
                    DiagnosticCode::InstanceTarget,
                    format!("override target `{}` does not exist", override_def.event),
                    Some(override_def.span),
                )
            })?;
            if deleted.contains(&address) {
                return Err(diagnostics(
                    DiagnosticCode::InstanceTarget,
                    "override target was already deleted",
                    Some(override_def.span),
                ));
            }
            if override_def.delete {
                deleted.insert(address.clone());
                continue;
            }
            self.apply_override(index, &override_def.set, place)?;
        }
        if !deleted.is_empty() {
            self.events
                .retain(|event| !deleted.contains(&event.address));
        }
        for insert in &place.inserts {
            let leaf = &insert.leaf;
            let mut address = vec![place.id.clone(), insert.id.clone(), leaf.id.clone()];
            let state = ExpansionState {
                origin: place.at.clone(),
                scale: Rational::one(),
                transpose: Rational::zero(),
                address: &mut address,
                inherited_cut_end: None,
                target,
                depth: 0,
            };
            let event = Self::expand_leaf(
                leaf,
                &state,
                vec![place.id.clone(), insert.id.clone(), leaf.id.clone()],
            )?;
            if self
                .events
                .iter()
                .any(|existing| existing.address == event.address)
            {
                return Err(diagnostics(
                    DiagnosticCode::DuplicateId,
                    "inserted event address is duplicated",
                    Some(insert.span),
                ));
            }
            self.events.push(event);
        }
        Ok(())
    }

    fn apply_override(
        &mut self,
        index: usize,
        set: &BTreeMap<String, Value>,
        place: &PlaceDef,
    ) -> CResult<()> {
        let object = self
            .document
            .object(&place.id)
            .unwrap_or_else(|| self.document.objects.values().next().unwrap());
        for (name, value) in set {
            if matches!(self.events[index].kind, ExpandedKind::Hit { .. })
                && !matches!(
                    name.as_str(),
                    "at" | "key" | "velocity" | "onset_offset" | "order" | "label"
                )
            {
                return Err(diagnostics(
                    DiagnosticCode::UnknownField,
                    format!("field `{name}` is not valid for a hit override"),
                    Some(value.span),
                ));
            }
            match name.as_str() {
                "at" => {
                    let at = self.q_value(value, object, None, false)?;
                    self.events[index].score_on_q = checked_add(&place.at, &at, Some(value.span))?;
                }
                "dur" => {
                    let dur = self.q_value(value, object, None, false)?;
                    if let ExpandedKind::Note(note) = &mut self.events[index].kind {
                        note.dur_q = dur;
                    }
                }
                "key" => {
                    let key = self.string_value(value, object, None, "key")?;
                    if let ExpandedKind::Hit { key: existing } = &mut self.events[index].kind {
                        *existing = key;
                    } else {
                        return Err(diagnostics(
                            DiagnosticCode::UnknownField,
                            "key is not valid for a note override",
                            Some(value.span),
                        ));
                    }
                }
                "pitch" => {
                    let pitch = self.parse_pitch(value, object, None)?;
                    if let ExpandedKind::Note(note) = &mut self.events[index].kind {
                        note.pitch = pitch;
                        note.transpose_cents = Rational::zero();
                    }
                }
                "velocity" => {
                    self.events[index].velocity = self.rational_value(value, object, None)?
                }
                "release_velocity" => {
                    let velocity = self.rational_value(value, object, None)?;
                    let release_velocity = velocity.to_f64().ok_or_else(|| {
                        diagnostics(
                            DiagnosticCode::Nonfinite,
                            "release velocity is not finite",
                            Some(value.span),
                        )
                    })?;
                    if let ExpandedKind::Note(note) = &mut self.events[index].kind {
                        note.release_velocity = release_velocity;
                    }
                }
                "onset_offset" => {
                    self.events[index].onset_offset_seconds =
                        self.seconds_value(value, object, None)?
                }
                "release_offset" => {
                    let offset = self.seconds_value(value, object, None)?;
                    if let ExpandedKind::Note(note) = &mut self.events[index].kind {
                        note.release_offset_seconds = offset;
                    }
                }
                "order" => {
                    let order =
                        bigint_i64(&self.rational_value(value, object, None)?, Some(value.span))?;
                    self.events[index].order = i32::try_from(order).map_err(|_| {
                        diagnostics(
                            DiagnosticCode::ResourceLimit,
                            "event order exceeds signed 32-bit range",
                            Some(value.span),
                        )
                    })?;
                }
                "label" => {
                    self.string_value(value, object, None, "label")?;
                }
                _ => {
                    return Err(diagnostics(
                        DiagnosticCode::UnknownField,
                        format!("unknown occurrence override field `{name}`"),
                        Some(value.span),
                    ))
                }
            }
        }
        let invalid_note = match &self.events[index].kind {
            ExpandedKind::Note(note) => {
                note.dur_q <= Rational::zero() || !(0.0..=1.0).contains(&note.release_velocity)
            }
            ExpandedKind::Hit { .. } => false,
        };
        if invalid_note
            || self.events[index].velocity.is_negative()
            || self.events[index].velocity > Rational::one()
        {
            return Err(diagnostics(
                DiagnosticCode::Range,
                if matches!(self.events[index].kind, ExpandedKind::Hit { .. }) {
                    "occurrence override produces an invalid hit"
                } else {
                    "occurrence override produces an invalid note"
                },
                Some(self.events[index].source_span),
            ));
        }
        Ok(())
    }

    fn lower_event_kind(&self, expanded: &ExpandedEvent) -> CResult<EventKind> {
        let note = match &expanded.kind {
            ExpandedKind::Note(note) => note,
            ExpandedKind::Hit { key } => {
                return Ok(EventKind::Hit {
                    key: key.clone(),
                    velocity: expanded.velocity.clone(),
                })
            }
        };
        // A source pitch can be above Nyquist when an occurrence
        // transposition brings it back into range.  Apply all final
        // transforms before the receiver-specific Nyquist check.
        let mut frequency = note
            .pitch
            .resolve_hz(None)
            .map_err(|error| event_music_error(error, expanded, &["pitch"]))?;
        let transpose = note.transpose_cents.to_f64().ok_or_else(|| {
            diagnostics(
                DiagnosticCode::Nonfinite,
                "pitch transposition is not finite",
                Some(expanded.source_span),
            )
        })?;
        frequency *= 2.0_f64.powf(transpose / 1200.0);
        if !frequency.is_finite()
            || frequency <= 0.0
            || frequency >= f64::from(self.sample_rate) / 2.0
        {
            return Err(event_diagnostic(
                DiagnosticCode::Range,
                "resolved pitch is outside the finite Nyquist range",
                expanded,
                &["pitch"],
            ));
        }
        Ok(EventKind::Note {
            pressure_expression: note
                .pressure_expression
                .as_ref()
                .map(|id| {
                    let curve = &self.curves[id];
                    let clock = match curve.clock.as_str() {
                        "score" => ExpressionClock::Score,
                        "seconds" => ExpressionClock::Seconds,
                        _ => ExpressionClock::Normalized,
                    };
                    let points = curve
                        .points
                        .iter()
                        .map(|(position, value, shape)| {
                            Ok(PressureExpressionPoint {
                                position: if clock == ExpressionClock::Score {
                                    checked_mul(
                                        position,
                                        &note.expression_scale,
                                        Some(expanded.source_span),
                                    )?
                                } else {
                                    position.clone()
                                },
                                value: value.clone(),
                                shape: *shape,
                            })
                        })
                        .collect::<CResult<Vec<_>>>()?;
                    Ok(PressureExpression { clock, points })
                })
                .transpose()?,
            timbre_expression: note
                .timbre_expression
                .as_ref()
                .map(|id| {
                    let curve = &self.curves[id];
                    let clock = match curve.clock.as_str() {
                        "score" => ExpressionClock::Score,
                        "seconds" => ExpressionClock::Seconds,
                        _ => ExpressionClock::Normalized,
                    };
                    let points = curve
                        .points
                        .iter()
                        .map(|(position, value, shape)| {
                            Ok(TimbreExpressionPoint {
                                position: if clock == ExpressionClock::Score {
                                    checked_mul(
                                        position,
                                        &note.expression_scale,
                                        Some(expanded.source_span),
                                    )?
                                } else {
                                    position.clone()
                                },
                                value: value.clone(),
                                shape: *shape,
                            })
                        })
                        .collect::<CResult<Vec<_>>>()?;
                    Ok(TimbreExpression { clock, points })
                })
                .transpose()?,
            gain_expression: note
                .gain_expression
                .as_ref()
                .map(|id| {
                    let curve = &self.curves[id];
                    let clock = match curve.clock.as_str() {
                        "score" => ExpressionClock::Score,
                        "seconds" => ExpressionClock::Seconds,
                        _ => ExpressionClock::Normalized,
                    };
                    let points = curve
                        .points
                        .iter()
                        .map(|(position, gain, shape)| {
                            Ok(GainExpressionPoint {
                                position: if clock == ExpressionClock::Score {
                                    checked_mul(
                                        position,
                                        &note.expression_scale,
                                        Some(expanded.source_span),
                                    )?
                                } else {
                                    position.clone()
                                },
                                gain: gain.clone(),
                                shape: *shape,
                            })
                        })
                        .collect::<CResult<Vec<_>>>()?;
                    Ok(GainExpression { clock, points })
                })
                .transpose()?,
            pitch_expression: note
                .pitch_expression
                .as_ref()
                .map(|id| {
                    let curve = &self.curves[id];
                    let clock = match curve.clock.as_str() {
                        "score" => ExpressionClock::Score,
                        "seconds" => ExpressionClock::Seconds,
                        _ => ExpressionClock::Normalized,
                    };
                    let points = curve
                        .points
                        .iter()
                        .map(|(position, cents, shape)| {
                            Ok(PitchExpressionPoint {
                                position: if clock == ExpressionClock::Score {
                                    checked_mul(
                                        position,
                                        &note.expression_scale,
                                        Some(expanded.source_span),
                                    )?
                                } else {
                                    position.clone()
                                },
                                cents: cents.clone(),
                                shape: *shape,
                            })
                        })
                        .collect::<CResult<Vec<_>>>()?;
                    Ok(PitchExpression { clock, points })
                })
                .transpose()?,
            pitch_hz: frequency,
            velocity: expanded.velocity.clone(),
        })
    }

    fn check_point_budget(&self, limits: &PlanLimits) -> CResult<()> {
        // Expansion keeps only curve identities; bound the aggregate before cloning points.
        let mut point_count = 0usize;
        let warp_counts = self.audio_clips.values().filter_map(|source| {
            if source
                .object
                .field("mode")
                .and_then(|field| field.value.as_symbol())
                != Some("warp_rate")
            {
                return None;
            }
            match &source.object.field("warp")?.value.kind {
                ValueKind::List(points) => Some(points.len()),
                _ => None,
            }
        });
        let has_warp_points = warp_counts.clone().any(|count| count != 0);
        for count in warp_counts
            .chain(self.automations.iter().map(|a| a.points.len()))
            .chain(
                self.events
                    .iter()
                    .filter_map(|e| match &e.kind {
                        ExpandedKind::Note(note) => Some(note),
                        ExpandedKind::Hit { .. } => None,
                    })
                    .flat_map(|e| {
                        [
                            e.pitch_expression.as_ref(),
                            e.gain_expression.as_ref(),
                            e.timbre_expression.as_ref(),
                            e.pressure_expression.as_ref(),
                        ]
                        .into_iter()
                        .flatten()
                    })
                    .map(|id| self.curves[id].points.len()),
            )
        {
            point_count = point_count.checked_add(count).ok_or_else(|| {
                diagnostics(
                    DiagnosticCode::ResourceLimit,
                    "expression point budget overflow",
                    None,
                )
            })?;
            if point_count > limits.max_automation_points {
                return Err(diagnostics(
                    DiagnosticCode::ResourceLimit,
                    if has_warp_points {
                        "aggregate automation, expression and warp point budget exceeded"
                    } else {
                        "aggregate automation and expression point budget exceeded"
                    },
                    None,
                ));
            }
        }

        Ok(())
    }

    fn lower_score_events(&self) -> CResult<Vec<ResolvedEventV3>> {
        self.events
            .iter()
            .map(|expanded| {
                if !self.allow_hits {
                    expanded.note()?;
                }
                let (score_off_q, release_offset_seconds, release_velocity) = match &expanded.kind {
                    ExpandedKind::Note(note) => (
                        Some(checked_add(
                            &expanded.score_on_q,
                            &note.dur_q,
                            Some(expanded.source_span),
                        )?),
                        note.release_offset_seconds.clone(),
                        note.release_velocity,
                    ),
                    ExpandedKind::Hit { .. } => (None, Rational::zero(), 0.0),
                };
                Ok(ResolvedEventV3 {
                    address: expanded.address.clone(),
                    source: expanded.source.clone(),
                    target: expanded.target.clone(),
                    kind: self.lower_event_kind(expanded)?,
                    score_on_q: expanded.score_on_q.clone(),
                    score_off_q,
                    onset_offset_seconds: expanded.onset_offset_seconds.clone(),
                    release_offset_seconds,
                    release_velocity,
                    on_frame: 0,
                    off_frame: None,
                    order: expanded.order,
                })
            })
            .collect::<CResult<Vec<_>>>()
    }

    fn schedule_score_events(
        output: &mut OutputSettings,
        events: &mut [ResolvedEventV3],
        timing: &TimingContext,
        limits: &PlanLimits,
    ) -> CResult<()> {
        output.total_frames = timing
            .duration_frames(output, &limits.bounded())
            .map_err(plan_error)?;
        for event in events.iter_mut() {
            (event.on_frame, event.off_frame) = timing
                .schedule(event, u64::from(output.sample_rate_hz))
                .map_err(plan_error)?;
        }
        events.sort_by(|a, b| {
            a.on_frame
                .cmp(&b.on_frame)
                .then(a.order.cmp(&b.order))
                .then(a.address.cmp(&b.address))
        });
        Ok(())
    }

    fn finish_v3(
        &self,
        limits: &PlanLimits,
        mut production: Option<crate::production_data::ProductionSettings>,
        original: &Document,
    ) -> CResult<PlanV3> {
        self.check_point_budget(limits)?;
        let events = self.lower_score_events()?;
        let mut plan = PlanV3 {
            version: 3,
            output: OutputSettings {
                score_start_q: self.score_start.clone(),
                score_end_q: self.score_end.clone(),
                tail_seconds: self.tail_seconds.clone(),
                sample_rate_hz: self.sample_rate,
                channels: self.output_channels()?,
                total_frames: 0,
                output: self.output.clone(),
            },
            tempo: self.plan_tempo.clone(),
            events,
            nodes: self.nodes.clone(),
            connections: self.connections.clone(),
            automation: self.automations.clone(),
            regions: self.regions.clone(),
            source_mappings: Vec::new(),
            instruments: self.instrument_resources.clone(),
            production: None,
        };
        if let Some(settings) = &mut production {
            settings.execution_identity = Some(
                crate::production_identity::execution_identity_for_view(original, &plan.view())
                    .map_err(|error| {
                        diagnostics(
                            DiagnosticCode::Range,
                            format!("execution identity: {error}"),
                            None,
                        )
                    })?,
            );
        }
        plan.production = production;
        plan.preflight_with_limits(limits).map_err(plan_error)?;
        let timing = TimingContext::new_with_limits(&plan.tempo, &plan.output, limits)
            .map_err(plan_error)?;
        Self::schedule_score_events(&mut plan.output, &mut plan.events, &timing, limits)?;
        plan.view()
            .validate_with_timing(limits, &timing)
            .map_err(plan_error)?;
        Ok(plan)
    }

    fn lower_audio_clips(&self) -> CResult<Vec<NodeV5>> {
        self.audio_clips
            .values()
            .filter(|source| {
                source
                    .object
                    .field("mode")
                    .and_then(|field| field.value.as_symbol())
                    == Some("rate")
            })
            .map(|source| {
                let object = &source.object;
                let asset_field = self.field(object, "asset")?;
                let asset = self.one_reference(&asset_field.value, object, Some(asset_field))?;
                let at_field = self.field(object, "at")?;
                let at = match at_field.value.kind {
                    ValueKind::Quantity {
                        unit: Unit::S | Unit::Ms,
                        ..
                    } => AutomationAnchor::Seconds {
                        seconds: self.seconds_value(&at_field.value, object, Some(at_field))?,
                    },
                    _ => AutomationAnchor::Score {
                        q: self.q_value(&at_field.value, object, Some(at_field), true)?,
                    },
                };
                let source_field = self.field(object, "source")?;
                let frames = match &source_field.value.kind {
                    ValueKind::List(frames) if frames.len() == 2 => frames,
                    _ => {
                        return Err(path_diagnostic(
                            DiagnosticCode::Range,
                            "source must contain two frame endpoints",
                            object,
                            Some(source_field),
                        ))
                    }
                };
                let frame = |value: &Value| -> CResult<u64> {
                    match &value.kind {
                        ValueKind::Quantity {
                            value: number,
                            unit: Unit::Frame,
                        } => bigint_u64(number, Some(value.span)),
                        _ => Err(path_diagnostic(
                            DiagnosticCode::Unit,
                            "source endpoints must be integer frames",
                            object,
                            Some(source_field),
                        )),
                    }
                };
                let number = |name: &str, default: Rational| -> CResult<Rational> {
                    object
                        .field(name)
                        .map(|field| self.rational_value(&field.value, object, Some(field)))
                        .unwrap_or(Ok(default))
                };
                let seconds = |name: &str| -> CResult<Rational> {
                    object
                        .field(name)
                        .map(|field| self.seconds_value(&field.value, object, Some(field)))
                        .unwrap_or(Ok(Rational::zero()))
                };
                let reverse = object
                    .field("reverse")
                    .map(|field| self.bool_value(&field.value, object, Some(field)))
                    .unwrap_or(Ok(false))?;
                let fade_shape = match object.field("fade_shape") {
                    None => AudioFadeShape::Linear,
                    Some(field) => match self
                        .symbol_value(&field.value, object, Some(field))?
                        .as_str()
                    {
                        "linear" => AudioFadeShape::Linear,
                        "equal_power" => AudioFadeShape::EqualPower,
                        _ => {
                            return Err(path_diagnostic(
                                DiagnosticCode::Range,
                                "unsupported fade shape",
                                object,
                                Some(field),
                            ))
                        }
                    },
                };
                let track = object
                    .field("track")
                    .map(|field| self.one_reference(&field.value, object, Some(field)))
                    .transpose()?;
                Ok(NodeV5 {
                    id: object.id.clone(),
                    params: BTreeMap::new(),
                    processor: ProcessorV5::Audio {
                        clip: Box::new(AudioClip {
                            asset,
                            channels: source.channels,
                            at,
                            source_start_frame: frame(&frames[0])?,
                            source_end_frame: frame(&frames[1])?,
                            speed: number("speed", Rational::one())?,
                            reverse,
                            gain: number("gain", Rational::one())?,
                            fade_in_seconds: seconds("fade_in")?,
                            fade_out_seconds: seconds("fade_out")?,
                            fade_shape,
                            source: SourceMapping {
                                object: object.id.clone(),
                                path: vec![object.id.clone()],
                                span: Some(SourceSpan {
                                    start: object.span.start,
                                    end: object.span.end,
                                }),
                            },
                            track,
                            start_frame: 0,
                            end_frame: 0,
                        }),
                    },
                })
            })
            .collect()
    }

    fn lower_warp_clips(&self) -> CResult<Vec<NodeV6>> {
        self.audio_clips
            .values()
            .filter(|source| {
                source
                    .object
                    .field("mode")
                    .and_then(|field| field.value.as_symbol())
                    == Some("warp_rate")
            })
            .map(|source| {
                let object = &source.object;
                let asset = self.field(object, "asset")?;
                let at = self.field(object, "at")?;
                let source_field = self.field(object, "source")?;
                let ValueKind::List(frames) = &source_field.value.kind else {
                    return Err(path_diagnostic(
                        DiagnosticCode::Range,
                        "source requires frame endpoints",
                        object,
                        Some(source_field),
                    ));
                };
                let frame = |value: &Value| -> CResult<u64> {
                    match &value.kind {
                        ValueKind::Quantity {
                            value,
                            unit: Unit::Frame,
                        } => bigint_u64(value, Some(source_field.span)),
                        _ => Err(path_diagnostic(
                            DiagnosticCode::Unit,
                            "source endpoint requires integer frames",
                            object,
                            Some(source_field),
                        )),
                    }
                };
                let warp_field = self.field(object, "warp")?;
                let ValueKind::List(points) = &warp_field.value.kind else {
                    return Err(path_diagnostic(
                        DiagnosticCode::Range,
                        "warp requires anchors",
                        object,
                        Some(warp_field),
                    ));
                };
                let warp = points
                    .iter()
                    .map(|point| {
                        let ValueKind::Tuple(pair) = &point.kind else {
                            return Err(path_diagnostic(
                                DiagnosticCode::Range,
                                "warp requires tuples",
                                object,
                                Some(warp_field),
                            ));
                        };
                        let [q, f] = pair.as_slice() else {
                            return Err(path_diagnostic(
                                DiagnosticCode::Range,
                                "warp requires pairs",
                                object,
                                Some(warp_field),
                            ));
                        };
                        Ok(WarpAnchor {
                            q: self.q_value(q, object, Some(warp_field), false)?,
                            source_frame: frame(f)?,
                        })
                    })
                    .collect::<CResult<Vec<_>>>()?;
                let seconds = |name: &str| -> CResult<Rational> {
                    object
                        .field(name)
                        .map(|field| self.seconds_value(&field.value, object, Some(field)))
                        .unwrap_or(Ok(Rational::zero()))
                };
                let gain = object
                    .field("gain")
                    .map(|field| self.rational_value(&field.value, object, Some(field)))
                    .unwrap_or(Ok(Rational::one()))?;
                let fade_shape = match object
                    .field("fade_shape")
                    .and_then(|field| field.value.as_symbol())
                {
                    None | Some("linear") => AudioFadeShape::Linear,
                    Some("equal_power") => AudioFadeShape::EqualPower,
                    _ => {
                        return Err(path_diagnostic(
                            DiagnosticCode::Range,
                            "unsupported fade shape",
                            object,
                            object.field("fade_shape"),
                        ))
                    }
                };
                let [start, end] = frames.as_slice() else {
                    return Err(path_diagnostic(
                        DiagnosticCode::Range,
                        "source requires two endpoints",
                        object,
                        Some(source_field),
                    ));
                };
                Ok(NodeV6 {
                    id: object.id.clone(),
                    params: BTreeMap::new(),
                    processor: ProcessorV6::WarpRate {
                        clip: Box::new(WarpClip {
                            asset: self.one_reference(&asset.value, object, Some(asset))?,
                            channels: source.channels,
                            at_q: self.q_value(&at.value, object, Some(at), true)?,
                            source_start_frame: frame(start)?,
                            source_end_frame: frame(end)?,
                            warp,
                            gain,
                            fade_in_seconds: seconds("fade_in")?,
                            fade_out_seconds: seconds("fade_out")?,
                            fade_shape,
                            source: SourceMapping {
                                object: object.id.clone(),
                                path: vec![object.id.clone()],
                                span: Some(SourceSpan {
                                    start: object.span.start,
                                    end: object.span.end,
                                }),
                            },
                            track: object
                                .field("track")
                                .map(|field| self.one_reference(&field.value, object, Some(field)))
                                .transpose()?,
                            start_frame: 0,
                            end_frame: 0,
                        }),
                    },
                })
            })
            .collect()
    }

    fn finish_v7(
        &self,
        limits: &PlanLimits,
        mut production: Option<crate::production_data::ProductionSettings>,
        original: &Document,
    ) -> CResult<PlanV7> {
        self.check_point_budget(limits)?;
        let mut nodes: Vec<NodeV6> = self
            .nodes
            .iter()
            .map(|node| NodeV6 {
                id: node.id.clone(),
                processor: ProcessorV6::Core {
                    processor: node.processor.clone(),
                },
                params: node.params.clone(),
            })
            .collect();
        nodes.extend(
            self.kit_nodes
                .values()
                .cloned()
                .map(NodeV5::from)
                .map(NodeV6::from),
        );
        nodes.extend(self.lower_audio_clips()?.into_iter().map(NodeV6::from));
        nodes.extend(self.lower_warp_clips()?);
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut nodes: Vec<NodeV7> = nodes.into_iter().map(NodeV7::from).collect();
        nodes.extend(self.control_nodes.values().cloned());
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut plan = PlanV7 {
            version: 7,
            output: OutputSettings {
                score_start_q: self.score_start.clone(),
                score_end_q: self.score_end.clone(),
                tail_seconds: self.tail_seconds.clone(),
                sample_rate_hz: self.sample_rate,
                channels: self.output_channels()?,
                total_frames: 0,
                output: self.output.clone(),
            },
            tempo: self.plan_tempo.clone(),
            events: self.lower_score_events()?,
            nodes,
            audio_assets: self.audio_assets.clone(),
            connections: self.connections.clone(),
            modulations: self.modulations.clone(),
            automation: self.automations.clone(),
            regions: self.regions.clone(),
            source_mappings: self
                .document
                .objects
                .values()
                .filter(|o| self.control_nodes.contains_key(&o.id) || o.kind == "modulate")
                .map(|o| SourceMapping {
                    object: o.id.clone(),
                    path: vec![o.id.clone()],
                    span: Some(SourceSpan {
                        start: o.span.start,
                        end: o.span.end,
                    }),
                })
                .collect(),
            instruments: self.instrument_resources.clone(),
            production: None,
        };
        if let Some(settings) = &mut production {
            settings.execution_identity = Some(
                crate::production_identity::execution_identity_for_view(original, &plan.view())
                    .map_err(|error| {
                        diagnostics(
                            DiagnosticCode::Range,
                            format!("execution identity: {error}"),
                            None,
                        )
                    })?,
            );
        }
        plan.production = production;
        let limits = limits.bounded();
        plan.view().preflight_timing(&limits).map_err(plan_error)?;
        let timing = TimingContext::new_with_limits(&plan.tempo, &plan.output, &limits)
            .map_err(plan_error)?;
        Self::schedule_score_events(&mut plan.output, &mut plan.events, &timing, &limits)?;
        for node in &mut plan.nodes {
            if let ProcessorV7::Audio { clip } = &mut node.processor {
                let asset = plan
                    .audio_assets
                    .iter()
                    .find(|asset| asset.id == clip.asset)
                    .ok_or_else(|| {
                        diagnostics(DiagnosticCode::Reference, "clip asset does not exist", None)
                    })?;
                (clip.start_frame, clip.end_frame) = crate::audio_clip::schedule_clip(
                    clip,
                    &timing,
                    &plan.output,
                    asset.rate_hz,
                    asset.frames,
                )
                .map_err(plan_error)?;
            }
        }
        for node in &mut plan.nodes {
            if let ProcessorV7::WarpRate { clip } = &mut node.processor {
                let asset = plan
                    .audio_assets
                    .iter()
                    .find(|asset| asset.id == clip.asset)
                    .ok_or_else(|| {
                        diagnostics(DiagnosticCode::Reference, "clip asset does not exist", None)
                    })?;
                (clip.start_frame, clip.end_frame) =
                    crate::warp_clip::schedule_clip(clip, &timing, &plan.output, asset.frames)
                        .map_err(plan_error)?;
            }
        }
        plan.view()
            .validate_with_timing(&limits, &timing)
            .map_err(plan_error)?;
        struct ByteBudget(usize);
        impl std::io::Write for ByteBudget {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_sub(bytes.len())
                    .ok_or_else(|| std::io::Error::other("artifact JSON exceeds byte limit"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(ByteBudget(limits.max_json_bytes), &plan)
            .map_err(|error| diagnostics(DiagnosticCode::ResourceLimit, error.to_string(), None))?;
        Ok(plan)
    }

    fn finish_v6(
        &self,
        limits: &PlanLimits,
        mut production: Option<crate::production_data::ProductionSettings>,
        original: &Document,
    ) -> CResult<PlanV6> {
        self.check_point_budget(limits)?;
        let mut nodes: Vec<NodeV6> = self
            .nodes
            .iter()
            .map(|node| NodeV6 {
                id: node.id.clone(),
                processor: ProcessorV6::Core {
                    processor: node.processor.clone(),
                },
                params: node.params.clone(),
            })
            .collect();
        nodes.extend(
            self.kit_nodes
                .values()
                .cloned()
                .map(NodeV5::from)
                .map(NodeV6::from),
        );
        nodes.extend(self.lower_audio_clips()?.into_iter().map(NodeV6::from));
        nodes.extend(self.lower_warp_clips()?);
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut plan = PlanV6 {
            version: 6,
            output: OutputSettings {
                score_start_q: self.score_start.clone(),
                score_end_q: self.score_end.clone(),
                tail_seconds: self.tail_seconds.clone(),
                sample_rate_hz: self.sample_rate,
                channels: self.output_channels()?,
                total_frames: 0,
                output: self.output.clone(),
            },
            tempo: self.plan_tempo.clone(),
            events: self.lower_score_events()?,
            nodes,
            audio_assets: self.audio_assets.clone(),
            connections: self.connections.clone(),
            automation: self.automations.clone(),
            regions: self.regions.clone(),
            source_mappings: Vec::new(),
            instruments: self.instrument_resources.clone(),
            production: None,
        };
        if let Some(settings) = &mut production {
            settings.execution_identity = Some(
                crate::production_identity::execution_identity_for_view(original, &plan.view())
                    .map_err(|error| {
                        diagnostics(
                            DiagnosticCode::Range,
                            format!("execution identity: {error}"),
                            None,
                        )
                    })?,
            );
        }
        plan.production = production;
        let limits = limits.bounded();
        plan.view().preflight_timing(&limits).map_err(plan_error)?;
        let timing = TimingContext::new_with_limits(&plan.tempo, &plan.output, &limits)
            .map_err(plan_error)?;
        Self::schedule_score_events(&mut plan.output, &mut plan.events, &timing, &limits)?;
        for node in &mut plan.nodes {
            if let ProcessorV6::Audio { clip } = &mut node.processor {
                let asset = plan
                    .audio_assets
                    .iter()
                    .find(|asset| asset.id == clip.asset)
                    .ok_or_else(|| {
                        diagnostics(DiagnosticCode::Reference, "clip asset does not exist", None)
                    })?;
                (clip.start_frame, clip.end_frame) = crate::audio_clip::schedule_clip(
                    clip,
                    &timing,
                    &plan.output,
                    asset.rate_hz,
                    asset.frames,
                )
                .map_err(plan_error)?;
            }
        }
        for node in &mut plan.nodes {
            if let ProcessorV6::WarpRate { clip } = &mut node.processor {
                let asset = plan
                    .audio_assets
                    .iter()
                    .find(|asset| asset.id == clip.asset)
                    .ok_or_else(|| {
                        diagnostics(DiagnosticCode::Reference, "clip asset does not exist", None)
                    })?;
                (clip.start_frame, clip.end_frame) =
                    crate::warp_clip::schedule_clip(clip, &timing, &plan.output, asset.frames)
                        .map_err(plan_error)?;
            }
        }
        plan.view()
            .validate_with_timing(&limits, &timing)
            .map_err(plan_error)?;
        struct ByteBudget(usize);
        impl std::io::Write for ByteBudget {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_sub(bytes.len())
                    .ok_or_else(|| std::io::Error::other("artifact JSON exceeds byte limit"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(ByteBudget(limits.max_json_bytes), &plan)
            .map_err(|error| diagnostics(DiagnosticCode::ResourceLimit, error.to_string(), None))?;
        Ok(plan)
    }

    fn finish_v5(
        &self,
        limits: &PlanLimits,
        mut production: Option<crate::production_data::ProductionSettings>,
        original: &Document,
    ) -> CResult<PlanV5> {
        self.check_point_budget(limits)?;
        let mut nodes: Vec<NodeV5> = self
            .nodes
            .iter()
            .map(|node| NodeV5 {
                id: node.id.clone(),
                processor: ProcessorV5::Core {
                    processor: node.processor.clone(),
                },
                params: node.params.clone(),
            })
            .collect();
        nodes.extend(self.kit_nodes.values().cloned().map(NodeV5::from));
        nodes.extend(self.lower_audio_clips()?);
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut plan = PlanV5 {
            version: 5,
            output: OutputSettings {
                score_start_q: self.score_start.clone(),
                score_end_q: self.score_end.clone(),
                tail_seconds: self.tail_seconds.clone(),
                sample_rate_hz: self.sample_rate,
                channels: self.output_channels()?,
                total_frames: 0,
                output: self.output.clone(),
            },
            tempo: self.plan_tempo.clone(),
            events: self.lower_score_events()?,
            nodes,
            audio_assets: self.audio_assets.clone(),
            connections: self.connections.clone(),
            automation: self.automations.clone(),
            regions: self.regions.clone(),
            source_mappings: Vec::new(),
            instruments: self.instrument_resources.clone(),
            production: None,
        };
        if let Some(settings) = &mut production {
            settings.execution_identity = Some(
                crate::production_identity::execution_identity_for_view(original, &plan.view())
                    .map_err(|error| {
                        diagnostics(
                            DiagnosticCode::Range,
                            format!("execution identity: {error}"),
                            None,
                        )
                    })?,
            );
        }
        plan.production = production;
        let limits = limits.bounded();
        plan.view().preflight_timing(&limits).map_err(plan_error)?;
        let timing = TimingContext::new_with_limits(&plan.tempo, &plan.output, &limits)
            .map_err(plan_error)?;
        Self::schedule_score_events(&mut plan.output, &mut plan.events, &timing, &limits)?;
        for node in &mut plan.nodes {
            if let ProcessorV5::Audio { clip } = &mut node.processor {
                let asset = plan
                    .audio_assets
                    .iter()
                    .find(|asset| asset.id == clip.asset)
                    .ok_or_else(|| {
                        diagnostics(DiagnosticCode::Reference, "clip asset does not exist", None)
                    })?;
                (clip.start_frame, clip.end_frame) = crate::audio_clip::schedule_clip(
                    clip,
                    &timing,
                    &plan.output,
                    asset.rate_hz,
                    asset.frames,
                )
                .map_err(plan_error)?;
            }
        }
        plan.view()
            .validate_with_timing(&limits, &timing)
            .map_err(plan_error)?;
        struct ByteBudget(usize);
        impl std::io::Write for ByteBudget {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_sub(bytes.len())
                    .ok_or_else(|| std::io::Error::other("artifact JSON exceeds byte limit"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(ByteBudget(limits.max_json_bytes), &plan)
            .map_err(|error| diagnostics(DiagnosticCode::ResourceLimit, error.to_string(), None))?;
        Ok(plan)
    }

    fn finish_v4(
        &self,
        limits: &PlanLimits,
        mut production: Option<crate::production_data::ProductionSettings>,
        original: &Document,
    ) -> CResult<PlanV4> {
        self.check_point_budget(limits)?;
        let mut nodes: Vec<NodeV4> = self
            .nodes
            .iter()
            .map(|node| NodeV4 {
                id: node.id.clone(),
                processor: ProcessorV4::Core {
                    processor: node.processor.clone(),
                },
                params: node.params.clone(),
            })
            .collect();
        nodes.extend(self.kit_nodes.values().cloned());
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut plan = PlanV4 {
            version: 4,
            output: OutputSettings {
                score_start_q: self.score_start.clone(),
                score_end_q: self.score_end.clone(),
                tail_seconds: self.tail_seconds.clone(),
                sample_rate_hz: self.sample_rate,
                channels: self.output_channels()?,
                total_frames: 0,
                output: self.output.clone(),
            },
            tempo: self.plan_tempo.clone(),
            events: self.lower_score_events()?,
            nodes,
            audio_assets: self.audio_assets.clone(),
            connections: self.connections.clone(),
            automation: self.automations.clone(),
            regions: self.regions.clone(),
            source_mappings: Vec::new(),
            instruments: self.instrument_resources.clone(),
            production: None,
        };
        if let Some(settings) = &mut production {
            settings.execution_identity = Some(
                crate::production_identity::execution_identity_for_view(original, &plan.view())
                    .map_err(|error| {
                        diagnostics(
                            DiagnosticCode::Range,
                            format!("execution identity: {error}"),
                            None,
                        )
                    })?,
            );
        }
        plan.production = production;
        let limits = limits.bounded();
        plan.view().preflight_timing(&limits).map_err(plan_error)?;
        let timing = TimingContext::new_with_limits(&plan.tempo, &plan.output, &limits)
            .map_err(plan_error)?;
        Self::schedule_score_events(&mut plan.output, &mut plan.events, &timing, &limits)?;
        plan.view()
            .validate_with_timing(&limits, &timing)
            .map_err(plan_error)?;
        // Bound the encoded artifact without constructing another timing context or retaining JSON.
        struct ByteBudget(usize);
        impl std::io::Write for ByteBudget {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_sub(bytes.len())
                    .ok_or_else(|| std::io::Error::other("artifact JSON exceeds byte limit"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(ByteBudget(limits.max_json_bytes), &plan)
            .map_err(|error| diagnostics(DiagnosticCode::ResourceLimit, error.to_string(), None))?;
        Ok(plan)
    }

    fn finish_plan(&self, limits: &PlanLimits) -> CResult<Plan> {
        self.check_point_budget(limits)?;
        let score_start_seconds = self
            .exact_tempo
            .seconds_at(&self.score_start)
            .map_err(|error| map_music_error(error, None))?;
        let score_end_seconds = self
            .exact_tempo
            .seconds_at(&self.score_end)
            .map_err(|error| map_music_error(error, None))?;
        let render_duration = checked_add(
            &checked_sub(&score_end_seconds, &score_start_seconds, None)?,
            &self.tail_seconds,
            None,
        )?;
        if render_duration > Rational::from_integer(BigInt::from(PlanLimits::MAX_DURATION_SECONDS))
        {
            return Err(diagnostics(
                DiagnosticCode::ResourceLimit,
                "render duration exceeds the 30 minute limit",
                None,
            ));
        }
        let total_frames = ceil_u64(
            &checked_mul(
                &render_duration,
                &Rational::from_integer(BigInt::from(self.sample_rate)),
                None,
            )?,
            None,
        )?;
        let mut events = Vec::new();
        for expanded in &self.events {
            let note = expanded.note()?;
            if expanded.score_on_q < self.score_start || expanded.score_on_q >= self.score_end {
                return Err(event_diagnostic(
                    DiagnosticCode::Interval,
                    "effective note onset lies outside the score",
                    expanded,
                    &["at"],
                ));
            }
            let score_off = checked_add(
                &expanded.score_on_q,
                &note.dur_q,
                Some(expanded.source_span),
            )?;
            let on_seconds = checked_add(
                &self
                    .exact_tempo
                    .seconds_at(&expanded.score_on_q)
                    .map_err(|error| map_music_error(error, Some(expanded.source_span)))?,
                &expanded.onset_offset_seconds,
                Some(expanded.source_span),
            )?;
            if on_seconds < score_start_seconds || on_seconds >= score_end_seconds {
                return Err(event_diagnostic(
                    DiagnosticCode::Interval,
                    "effective note onset lies outside the score",
                    expanded,
                    &["onset_offset"],
                ));
            }
            let unclamped_off = checked_add(
                &self
                    .exact_tempo
                    .seconds_at(&score_off)
                    .map_err(|error| map_music_error(error, Some(expanded.source_span)))?,
                &note.release_offset_seconds,
                Some(expanded.source_span),
            )?;
            let off_seconds = unclamped_off.min(score_end_seconds.clone());
            if off_seconds <= on_seconds {
                return Err(event_diagnostic(
                    DiagnosticCode::Interval,
                    "physical note gate must be positive",
                    expanded,
                    &["release_offset"],
                ));
            }
            let on_delta = checked_sub(
                &on_seconds,
                &score_start_seconds,
                Some(expanded.source_span),
            )?;
            let off_delta = checked_sub(
                &off_seconds,
                &score_start_seconds,
                Some(expanded.source_span),
            )?;
            let on_frame = ceil_u64(
                &checked_mul(
                    &on_delta,
                    &Rational::from_integer(BigInt::from(self.sample_rate)),
                    Some(expanded.source_span),
                )?,
                Some(expanded.source_span),
            )?;
            let off_frame = ceil_u64(
                &checked_mul(
                    &off_delta,
                    &Rational::from_integer(BigInt::from(self.sample_rate)),
                    Some(expanded.source_span),
                )?,
                Some(expanded.source_span),
            )?;
            if off_frame == on_frame {
                return Err(event_diagnostic(
                    DiagnosticCode::SubsampleNote,
                    "positive note gate collapses to zero engine frames",
                    expanded,
                    &["dur"],
                ));
            }
            let kind = self.lower_event_kind(expanded)?;
            events.push(ResolvedEvent {
                address: expanded.address.clone(),
                source: expanded.source.clone(),
                target: expanded.target.clone(),
                kind,
                score_on_q: expanded.score_on_q.clone(),
                score_off_q: Some(score_off),
                onset_offset_seconds: expanded.onset_offset_seconds.clone(),
                release_offset_seconds: note.release_offset_seconds.clone(),
                on_seconds,
                off_seconds: Some(off_seconds),
                release_velocity: note.release_velocity,
                on_frame,
                off_frame: Some(off_frame),
                order: expanded.order,
            });
        }
        events.sort_by(|a, b| {
            a.on_frame
                .cmp(&b.on_frame)
                .then(a.order.cmp(&b.order))
                .then(a.address.cmp(&b.address))
        });
        let plan = Plan {
            version: if self.instrument_resources.is_some() {
                2
            } else {
                1
            },
            output: OutputSettings {
                score_start_q: self.score_start.clone(),
                score_end_q: self.score_end.clone(),
                tail_seconds: self.tail_seconds.clone(),
                sample_rate_hz: self.sample_rate,
                channels: self.output_channels()?,
                total_frames,
                output: self.output.clone(),
            },
            tempo: self.plan_tempo.clone(),
            events,
            nodes: self.nodes.clone(),
            connections: self.connections.clone(),
            automation: self
                .automations
                .iter()
                .map(|lane| {
                    let at = match &lane.at {
                        AutomationAnchor::Score { q } if lane.clock == AutomationClock::Seconds => {
                            self.exact_tempo
                                .seconds_at(q)
                                .map_err(|e| map_music_error(e, None))?
                        }
                        AutomationAnchor::Score { q } => q.clone(),
                        AutomationAnchor::Seconds { seconds } => seconds.clone(),
                    };
                    Ok(Automation {
                        id: lane.id.clone(),
                        target: lane.target.clone(),
                        clock: lane.clock,
                        at,
                        points: lane.points.clone(),
                    })
                })
                .collect::<CResult<Vec<_>>>()?,
            regions: self.regions.clone(),
            source_mappings: Vec::new(),
            instruments: self.instrument_resources.clone(),
            production: None,
        };
        Ok(plan)
    }

    fn output_channels(&self) -> CResult<u8> {
        if let Some(clip) = self.audio_clips.get(&self.output.node) {
            return Ok(clip.channels);
        }
        if let Some(node) = self.kit_nodes.get(&self.output.node) {
            if let ProcessorV4::Kit { channels, .. } = node.processor {
                return Ok(channels);
            }
        }
        let node = self
            .nodes
            .iter()
            .find(|node| node.id == self.output.node)
            .ok_or_else(|| {
                diagnostics(
                    DiagnosticCode::Reference,
                    "project output node does not exist",
                    None,
                )
            })?;
        match node.processor {
            Processor::Sum { channels } => Ok(channels),
            Processor::Pan => Ok(2),
            Processor::OnePole { channels }
            | Processor::Gain { channels }
            | Processor::Fader { channels }
            | Processor::Noise { channels, .. }
            | Processor::Delay { channels, .. }
            | Processor::Matrix {
                outputs: channels, ..
            }
            | Processor::Eq { channels, .. }
            | Processor::Compressor { channels, .. }
            | Processor::Reverb { channels, .. } => Ok(channels),
            Processor::Sine { .. } => Ok(1),
            Processor::Instrument { channels, .. } => Ok(channels),
        }
    }
}

fn value_span(value: &Rational, field: Option<&Field>) -> Span {
    field.map(|field| field.value.span).unwrap_or_else(|| {
        let _ = value;
        Span::default()
    })
}

fn prepare_instrument_compiler<'a>(
    resolved: &ResolvedBundle,
    libraries: LibrarySet,
    document: &'a Document,
) -> CResult<Compiler<'a>> {
    let mut instances = BTreeMap::new();
    let mut descriptors = BTreeMap::new();
    for node in document
        .objects
        .values()
        .filter(|object| object.kind == "node" && object.field("instrument").is_some())
    {
        let instance = libraries.resolve_instance(&resolved.entry, node)?;
        let program = libraries
            .programs
            .iter()
            .find(|program| program.id == instance.program_id)
            .ok_or_else(|| {
                path_diagnostic(
                    DiagnosticCode::Reference,
                    "resolved instrument program is missing",
                    node,
                    node.field("instrument"),
                )
            })?;
        let mut controls = BTreeMap::new();
        for name in program.controls.keys() {
            let spec = program.control_spec(name).ok_or_else(|| {
                path_diagnostic(
                    DiagnosticCode::Reference,
                    format!("instrument control `{name}` has no parameter metadata"),
                    node,
                    node.field("instrument"),
                )
            })?;
            controls.insert(name.clone(), spec);
        }
        descriptors.insert(
            node.id.clone(),
            InstrumentNodeDescriptor {
                channels: instance.channels,
                controls,
            },
        );
        instances.insert(node.id.clone(), instance);
    }
    let resources = InstrumentResources {
        entry_source: resolved.entry.clone(),
        programs: libraries.programs,
        wavetables: libraries.wavetables,
        wavetable_sources: libraries.wavetable_sources,
        source_files: resolved.source_files.clone(),
        dependencies: resolved.dependencies.clone(),
        libraries: libraries.metadata,
    };
    Ok(Compiler::with_instruments(
        document,
        instances,
        descriptors,
        resources,
    ))
}

fn compile_resolved(
    resolved: &ResolvedBundle,
    libraries: LibrarySet,
    limits: &PlanLimits,
) -> CResult<Plan> {
    let document = libraries.entry_document();
    if !document
        .objects
        .values()
        .any(|object| object.kind == "project")
    {
        return Err(diagnostics(
            DiagnosticCode::Conflict,
            "library sources can be checked but cannot be compiled as compositions",
            None,
        ));
    }
    prepare_instrument_compiler(resolved, libraries, &document)?.run(limits)
}

/// Resolve and compile a source bundle into a self-contained version 2 plan.
pub fn compile_bundle(bundle: &SourceBundle) -> Result<Plan, Diagnostics> {
    compile_bundle_with_limits(bundle, &PlanLimits::default())
}

/// Resolve and compile a bundle under an explicit caller resource allowance.
pub fn compile_bundle_with_limits(
    bundle: &SourceBundle,
    limits: &PlanLimits,
) -> Result<Plan, Diagnostics> {
    let mut resolved = bundle.resolve()?;
    let original = resolved
        .documents
        .get(&resolved.entry)
        .expect("resolved entry exists")
        .clone();
    let (document, production) =
        crate::production_data::prepare_document(&original, &bundle.assets)?;
    resolved.documents.insert(resolved.entry.clone(), document);
    let libraries = LibrarySet::resolve(&resolved)?;
    attach_production(
        compile_resolved(&resolved, libraries, limits)?,
        production,
        &original,
        limits,
    )
}

/// Resolve and validate either a composition bundle or all exports of a
/// library bundle.
pub fn check_bundle(bundle: &SourceBundle) -> Result<(), Diagnostics> {
    check_bundle_with_limits(bundle, &PlanLimits::default())
}

/// Validate a bundle, applying caller limits when the entry is a composition.
pub fn check_bundle_with_limits(
    bundle: &SourceBundle,
    limits: &PlanLimits,
) -> Result<(), Diagnostics> {
    let mut resolved = bundle.resolve()?;
    let original = resolved
        .documents
        .get(&resolved.entry)
        .expect("resolved entry exists")
        .clone();
    let (document, production) =
        crate::production_data::prepare_document(&original, &bundle.assets)?;
    resolved.documents.insert(resolved.entry.clone(), document);
    let libraries = LibrarySet::resolve(&resolved)?;
    if libraries
        .entry_document()
        .objects
        .values()
        .any(|object| object.kind == "project")
    {
        attach_production(
            compile_resolved(&resolved, libraries, limits)?,
            production,
            &original,
            limits,
        )
        .map(|_| ())
    } else {
        Ok(())
    }
}

fn local_resolved(document: &Document) -> ResolvedBundle {
    let entry = "document.maac".to_owned();
    ResolvedBundle {
        entry: entry.clone(),
        documents: BTreeMap::from([(entry.clone(), document.clone())]),
        imports: BTreeMap::from([(entry.clone(), BTreeMap::new())]),
        assets: BTreeMap::new(),
        dependencies: Vec::new(),
        source_files: vec![SourceIdentity {
            path: entry,
            hash: sha256_digest(document.source().as_bytes()),
        }],
    }
}

fn has_library_syntax(document: &Document) -> bool {
    document.objects.values().any(|object| {
        matches!(
            object.kind.as_str(),
            "library" | "instrument" | "preset" | "wavetable"
        ) || (object.kind == "node" && object.field("instrument").is_some())
    })
}

fn reject_unresolved_imports(document: &Document) -> CResult<()> {
    if let Some(import) = document
        .objects
        .values()
        .find(|object| object.kind == "import")
    {
        Err(path_diagnostic(
            DiagnosticCode::Reference,
            "imports require SourceBundle resolution",
            import,
            import.field("path"),
        ))
    } else {
        Ok(())
    }
}

/// Compile a parsed MaaC document into a validated standalone performance plan.
pub fn compile(document: &Document) -> Result<Plan, Diagnostics> {
    compile_with_limits(document, &PlanLimits::default())
}

/// Compile a parsed document under an explicit caller resource allowance.
pub fn compile_with_limits(document: &Document, limits: &PlanLimits) -> Result<Plan, Diagnostics> {
    reject_unresolved_imports(document)?;
    let (prepared, production) =
        crate::production_data::prepare_document(document, &BTreeMap::new())?;
    let plan = if has_library_syntax(&prepared) {
        let resolved = local_resolved(&prepared);
        let libraries = LibrarySet::resolve(&resolved)?;
        compile_resolved(&resolved, libraries, limits)?
    } else {
        Compiler::new(&prepared).run(limits)?
    };
    attach_production(plan, production, document, limits)
}

/// Validate a document through the same resolution path used by [`compile`].
pub fn check(document: &Document) -> Result<(), Diagnostics> {
    check_with_limits(document, &PlanLimits::default())
}

/// Validate a parsed document through compilation under caller limits.
pub fn check_with_limits(document: &Document, limits: &PlanLimits) -> Result<(), Diagnostics> {
    reject_unresolved_imports(document)?;
    if has_library_syntax(document) {
        let resolved = local_resolved(document);
        let libraries = LibrarySet::resolve(&resolved)?;
        if libraries
            .entry_document()
            .objects
            .values()
            .any(|object| object.kind == "project")
        {
            compile_resolved(&resolved, libraries, limits).map(|_| ())
        } else {
            Ok(())
        }
    } else {
        Compiler::new(document).run(limits).map(|_| ())
    }
}

trait FieldValueRef {
    fn value_ref(&self) -> &Value;
}

impl FieldValueRef for Field {
    fn value_ref(&self) -> &Value {
        &self.value
    }
}

fn attach_production(
    mut plan: Plan,
    mut production: Option<crate::production_data::ProductionSettings>,
    original: &Document,
    limits: &PlanLimits,
) -> CResult<Plan> {
    if let Some(settings) = &mut production {
        settings.execution_identity = Some(
            crate::production_identity::execution_identity(original, &plan).map_err(|error| {
                diagnostics(
                    DiagnosticCode::Range,
                    format!("execution identity: {error}"),
                    None,
                )
            })?,
        );
    }
    plan.production = production;
    if plan.production.is_some() {
        plan.validate_with_limits(limits).map_err(plan_error)?;
    }
    Ok(plan)
}

fn compile_resolved_versioned(
    resolved: &ResolvedBundle,
    libraries: LibrarySet,
    limits: &PlanLimits,
    production: Option<crate::production_data::ProductionSettings>,
    original: &Document,
) -> CResult<VersionedPlan> {
    let document = libraries.entry_document();
    if !document
        .objects
        .values()
        .any(|object| object.kind == "project")
    {
        return Err(diagnostics(
            DiagnosticCode::Conflict,
            "library sources can be checked but cannot be compiled as compositions",
            None,
        ));
    }
    {
        let mut compiler = prepare_instrument_compiler(resolved, libraries, &document)?;
        compiler.allow_ramps = true;
        compiler.run_versioned(limits, production, original)
    }
}

fn uses_audio_profile(document: &Document) -> bool {
    document
        .objects
        .values()
        .any(|object| object.kind == "audio")
}

fn uses_control_profile(document: &Document) -> bool {
    document.objects.values().any(|o| {
        o.kind == "modulate"
            || (o.kind == "node"
                && o.field("type")
                    .and_then(|f| f.value.as_string())
                    .is_some_and(|kind| matches!(kind, "core.lfo/1" | "core.constant/1")))
    })
}

fn uses_kit_profile(document: &Document) -> bool {
    fn visit(object: &Object) -> bool {
        object.kind == "hit"
            || (object.kind == "asset"
                && object.field("kind").and_then(|f| f.value.as_symbol()) == Some("audio"))
            || (object.kind == "node"
                && object.field("type").and_then(|f| f.value.as_string()) == Some("core.kit/1"))
            || object.children.values().any(visit)
    }
    document.objects.values().any(visit)
}

fn prepare_artifact_bundle(
    bundle: &SourceBundle,
) -> CResult<(
    ResolvedBundle,
    LibrarySet,
    Option<crate::production_data::ProductionSettings>,
    Document,
)> {
    let mut resolved = bundle.resolve()?;
    let original = resolved
        .documents
        .get(&resolved.entry)
        .expect("resolved entry exists")
        .clone();
    let (document, production) =
        crate::production_data::prepare_document(&original, &bundle.assets)?;
    resolved.documents.insert(resolved.entry.clone(), document);
    let libraries = LibrarySet::resolve(&resolved)?;
    Ok((resolved, libraries, production, original))
}

fn compile_resolved_artifact(
    resolved: &ResolvedBundle,
    libraries: LibrarySet,
    limits: &PlanLimits,
    production: Option<crate::production_data::ProductionSettings>,
    original: &Document,
) -> CResult<PlanArtifact> {
    let document = libraries.entry_document();
    if !uses_kit_profile(&document)
        && !uses_audio_profile(&document)
        && !uses_control_profile(&document)
    {
        return compile_resolved_versioned(resolved, libraries, limits, production, original)
            .map(PlanArtifact::from);
    }
    if !document
        .objects
        .values()
        .any(|object| object.kind == "project")
    {
        return Err(diagnostics(
            DiagnosticCode::Conflict,
            "library sources can be checked but cannot be compiled as compositions",
            None,
        ));
    }
    prepare_instrument_compiler(resolved, libraries, &document)?
        .run_artifact(resolved, limits, production, original)
}

/// Compile a bundle to a self-contained artifact, including native sample kits.
pub fn compile_bundle_artifact(bundle: &SourceBundle) -> Result<PlanArtifact, Diagnostics> {
    compile_bundle_artifact_with_limits(bundle, &PlanLimits::default())
}
/// Compile an artifact under an explicit caller resource allowance.
pub fn compile_bundle_artifact_with_limits(
    bundle: &SourceBundle,
    limits: &PlanLimits,
) -> Result<PlanArtifact, Diagnostics> {
    let (resolved, libraries, production, original) = prepare_artifact_bundle(bundle)?;
    compile_resolved_artifact(&resolved, libraries, limits, production, &original)
}
/// Check a composition through artifact compilation or validate all library exports.
pub fn check_bundle_artifact(bundle: &SourceBundle) -> Result<(), Diagnostics> {
    check_bundle_artifact_with_limits(bundle, &PlanLimits::default())
}
/// Check a bundle under an explicit caller resource allowance.
pub fn check_bundle_artifact_with_limits(
    bundle: &SourceBundle,
    limits: &PlanLimits,
) -> Result<(), Diagnostics> {
    let (resolved, libraries, production, original) = prepare_artifact_bundle(bundle)?;
    if libraries
        .entry_document()
        .objects
        .values()
        .any(|object| object.kind == "project")
    {
        compile_resolved_artifact(&resolved, libraries, limits, production, &original).map(|_| ())
    } else {
        Ok(())
    }
}

/// Compile a bundle into its legacy step plan or a certified version 3 ramp plan.
pub fn compile_bundle_versioned(bundle: &SourceBundle) -> Result<VersionedPlan, Diagnostics> {
    compile_bundle_versioned_with_limits(bundle, &PlanLimits::default())
}

/// Resolve and compile a bundle under an explicit caller resource allowance.
pub fn compile_bundle_versioned_with_limits(
    bundle: &SourceBundle,
    limits: &PlanLimits,
) -> Result<VersionedPlan, Diagnostics> {
    let mut resolved = bundle.resolve()?;
    let original = resolved
        .documents
        .get(&resolved.entry)
        .expect("resolved entry exists")
        .clone();
    let (document, production) =
        crate::production_data::prepare_document(&original, &bundle.assets)?;
    resolved.documents.insert(resolved.entry.clone(), document);
    let libraries = LibrarySet::resolve(&resolved)?;
    compile_resolved_versioned(&resolved, libraries, limits, production, &original)
}

/// Resolve and validate either a composition bundle or all exports of a
/// library bundle.
pub fn check_bundle_versioned(bundle: &SourceBundle) -> Result<(), Diagnostics> {
    check_bundle_versioned_with_limits(bundle, &PlanLimits::default())
}

/// Validate a bundle, applying caller limits when the entry is a composition.
pub fn check_bundle_versioned_with_limits(
    bundle: &SourceBundle,
    limits: &PlanLimits,
) -> Result<(), Diagnostics> {
    let mut resolved = bundle.resolve()?;
    let original = resolved
        .documents
        .get(&resolved.entry)
        .expect("resolved entry exists")
        .clone();
    let (document, production) =
        crate::production_data::prepare_document(&original, &bundle.assets)?;
    resolved.documents.insert(resolved.entry.clone(), document);
    let libraries = LibrarySet::resolve(&resolved)?;
    if libraries
        .entry_document()
        .objects
        .values()
        .any(|object| object.kind == "project")
    {
        compile_resolved_versioned(&resolved, libraries, limits, production, &original).map(|_| ())
    } else {
        Ok(())
    }
}

/// Compile step tempos to the legacy plan and ramps to version 3.
pub fn compile_versioned(document: &Document) -> Result<VersionedPlan, Diagnostics> {
    compile_versioned_with_limits(document, &PlanLimits::default())
}

/// Compile a parsed document under an explicit caller resource allowance.
pub fn compile_versioned_with_limits(
    document: &Document,
    limits: &PlanLimits,
) -> Result<VersionedPlan, Diagnostics> {
    reject_unresolved_imports(document)?;
    let (prepared, production) =
        crate::production_data::prepare_document(document, &BTreeMap::new())?;
    if has_library_syntax(&prepared) {
        let resolved = local_resolved(&prepared);
        let libraries = LibrarySet::resolve(&resolved)?;
        compile_resolved_versioned(&resolved, libraries, limits, production, document)
    } else {
        let mut compiler = Compiler::new(&prepared);
        compiler.allow_ramps = true;
        compiler.run_versioned(limits, production, document)
    }
}

/// Validate a document through the same resolution path used by [`compile`].
pub fn check_versioned(document: &Document) -> Result<(), Diagnostics> {
    check_versioned_with_limits(document, &PlanLimits::default())
}

/// Validate a parsed document through compilation under caller limits.
pub fn check_versioned_with_limits(
    document: &Document,
    limits: &PlanLimits,
) -> Result<(), Diagnostics> {
    reject_unresolved_imports(document)?;
    if document
        .objects
        .values()
        .any(|object| object.kind == "project")
    {
        compile_versioned_with_limits(document, limits).map(|_| ())
    } else if has_library_syntax(document) {
        LibrarySet::resolve(&local_resolved(document)).map(|_| ())
    } else {
        compile_versioned_with_limits(document, limits).map(|_| ())
    }
}

#[cfg(test)]
mod hit_expansion_tests {
    use super::*;

    fn expand(body: &str) -> CResult<Vec<ExpandedEvent>> {
        let document = crate::parse(&format!(
            "maac 1; track t {{ target = &kit:events; }} {body}"
        ))
        .unwrap();
        let mut compiler = Compiler::new(&document);
        compiler.allow_hits = true;
        compiler.score_end = Rational::from_integer(100.into());
        compiler.read_patterns()?;
        compiler.validate_pattern_graph()?;
        compiler.read_tracks_and_places()?;
        compiler.expand_places()?;
        Ok(compiler.events)
    }

    #[test]
    fn nested_hits_share_addresses_and_stretch_but_ignore_transpose_and_cut() {
        let events = expand(r#"
            pattern inner { length = 2q; hit h { at = 1q; key = ""; onset_offset = 3ms; } }
            pattern outer { length = 8q; use u { pattern = &inner; at = 0q; stretch = 2; count = 2; transpose = 1200ct; boundary = cut; } }
            place p { pattern = &outer; track = &t; at = 1q; stretch = 2; count = 2; transpose = -300ct; boundary = cut; }
        "#).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| e.address.as_str())
                .collect::<Vec<_>>(),
            ["p/0/u/0/h", "p/0/u/1/h", "p/1/u/0/h", "p/1/u/1/h"]
        );
        assert_eq!(
            events
                .iter()
                .map(|e| e.score_on_q.clone())
                .collect::<Vec<_>>(),
            [5, 13, 21, 29].map(|n| Rational::from_integer(n.into()))
        );
        for event in events {
            assert!(matches!(event.kind, ExpandedKind::Hit { ref key } if key.is_empty()));
            assert_eq!(
                event.onset_offset_seconds,
                Rational::new(3.into(), 1000.into())
            );
            assert_eq!(event.velocity, Rational::one());
        }
    }

    #[test]
    fn final_overrides_delete_and_nonrepeated_inserts() {
        let events = expand(r#"
            pattern pat { length = 2q; hit h { at = 0q; key = "old"; } }
            place p { pattern = &pat; track = &t; at = 4q; stretch = 2; count = 2;
                override edit { event = "0/h"; set = { at = 3q; key = "new"; velocity = 0.25; onset_offset = -1ms; order = -4; label = "x"; }; }
                override gone { event = "1/h"; delete = true; }
                insert once { hit added { at = 1q; key = "insert"; } }
            }
        "#).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].address, "p/0/h");
        assert_eq!(events[0].score_on_q, Rational::from_integer(7.into()));
        assert_eq!(events[0].velocity, Rational::new(1.into(), 4.into()));
        assert_eq!(events[0].order, -4);
        assert_eq!(
            events[0].onset_offset_seconds,
            Rational::new((-1).into(), 1000.into())
        );
        assert!(matches!(events[0].kind, ExpandedKind::Hit { ref key } if key == "new"));
        assert_eq!(events[1].address, "p/once/added");
        assert_eq!(events[1].score_on_q, Rational::from_integer(5.into()));
    }

    #[test]
    fn mixed_leaves_cut_only_notes_and_lower_hits_without_note_fields() {
        let events = expand(
            r#"
            pattern pat { length = 2q;
                hit h { at = 1q; key = "kick"; }
                note n { at = 1q; dur = 3q; pitch = A4; }
            }
            place p { pattern = &pat; track = &t; at = 0q; stretch = 2; boundary = cut; }
        "#,
        )
        .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].score_on_q, Rational::from_integer(2.into()));
        let ExpandedKind::Note(note) = &events[1].kind else {
            panic!("expected note");
        };
        assert_eq!(note.dur_q, Rational::from_integer(2.into()));
        assert!(events[0].note().is_err());
        let document = crate::parse("maac 1;").unwrap();
        let compiler = Compiler::new(&document);
        assert!(matches!(compiler.lower_event_kind(&events[0]).unwrap(),
            EventKind::Hit { key, velocity } if key == "kick" && velocity == Rational::one()));
    }

    #[test]
    fn rejects_invalid_hit_source_fields_and_retains_default_capability_gate() {
        for field in [
            "dur = 1q;",
            "pitch = A4;",
            "release_offset = 1ms;",
            "velocity = -1;",
            "order = 1.5;",
        ] {
            assert!(
                expand(&format!(
                    r#"pattern pat {{ length = 2q; hit h {{ at = 0q; key = "x"; {field} }} }}"#
                ))
                .is_err(),
                "{field}"
            );
        }
        let document =
            crate::parse(r#"maac 1; pattern pat { length = 2q; hit h { at = 0q; key = "x"; } }"#)
                .unwrap();
        let mut compiler = Compiler::new(&document);
        assert!(compiler.read_patterns().is_err());
        let document = crate::parse(r#"maac 1;
            project p { score = [0q, 2q]; rate = 48000Hz; tempo = &clock; meter = &metre; output = &sine:out; }
            tempo clock { points = [(0q, 120bpm, step)]; }
            meter metre { points = [(0q, 4, 4)]; }
            node sine { type = "core.sine/1"; }
            track t { target = &sine:events; }
            pattern pat { length = 2q; hit h { at = 0q; key = "x"; } }
            place place { pattern = &pat; track = &t; at = 0q; }
        "#).unwrap();
        for result in [
            compile(&document).map(|_| ()),
            compile_versioned(&document).map(|_| ()),
            check(&document),
            check_versioned(&document),
        ] {
            assert!(result
                .unwrap_err()
                .iter()
                .any(|d| d.code == DiagnosticCode::Capability));
        }
    }

    #[test]
    fn hit_overrides_reject_note_fields_and_invalid_velocity() {
        for replacement in [
            "dur = 1q",
            "pitch = A4",
            "release_velocity = 0.5",
            "release_offset = 1ms",
            "velocity = 2",
        ] {
            assert!(
                expand(&format!(
                    r#"pattern pat {{ length = 2q; hit h {{ at = 0q; key = "x"; }} }}
                place p {{ pattern = &pat; track = &t; at = 0q;
                override edit {{ event = "0/h"; set = {{ {replacement}; }}; }} }}"#
                ))
                .is_err(),
                "{replacement}"
            );
        }
    }
}
