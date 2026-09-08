//! ScoreIR source-to-performance-plan resolution.
//!
//! The compiler is deliberately a finite resolver.  It does not render audio,
//! execute modules, or infer a transport.  Its output is the standalone
//! [`crate::plan::Plan`] consumed by the renderer.  All source arithmetic stays
//! exact until the pitch boundary and all expansion paths are bounded.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::TryFrom;
use std::sync::Arc;

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
use crate::exact::{Rational, MAX_RATIONAL_BITS};
use crate::music::{
    MeterMap, MeterPoint, MusicError, MusicErrorCode, Pitch, TempoMap as ExactTempoMap,
    TempoPoint as ExactTempoPoint, TempoShape, Tuning,
};
use crate::plan::{
    Automation, AutomationClock, AutomationPoint, Connection, EventKind, EventTarget,
    Interpolation, Node, OutputSettings, Plan, PlanLimits, PortRef, Processor, Region,
    ResolvedEvent, SourceMapping, SourceSpan, TempoMap, TempoPoint,
};
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
    event: &ExpandedNote,
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

fn event_music_error(error: MusicError, event: &ExpandedNote, fields: &[&str]) -> Diagnostics {
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

#[derive(Clone, Debug)]
struct NoteDef {
    id: String,
    span: Span,
    at: Rational,
    dur: Rational,
    pitch: Pitch,
    velocity: Rational,
    release_velocity: f64,
    onset_offset: Rational,
    release_offset: Rational,
    order: i32,
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
    Note(Box<NoteDef>),
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
    note: NoteDef,
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
struct ExpandedNote {
    address: String,
    source: SourceMapping,
    source_span: Span,
    target: EventTarget,
    pitch: Pitch,
    transpose_cents: Rational,
    score_on_q: Rational,
    dur_q: Rational,
    velocity: Rational,
    release_velocity: f64,
    onset_offset_seconds: Rational,
    release_offset_seconds: Rational,
    order: i32,
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
    connections: Vec<Connection>,
    curves: BTreeMap<String, CurveDef>,
    automations: Vec<Automation>,
    regions: Vec<Region>,
    events: Vec<ExpandedNote>,
    expansion_work: u64,
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
            connections: Vec::new(),
            curves: BTreeMap::new(),
            automations: Vec::new(),
            regions: Vec::new(),
            events: Vec::new(),
            expansion_work: 0,
        }
    }

    fn run(mut self) -> CResult<Plan> {
        // SourceGraph owns the source schema and declaration/reference checks.
        // Keep this call ahead of lowering so malformed unused declarations
        // cannot disappear during expansion.
        crate::semantic::validate_source(self.document)?;
        self.validate_document_shape()?;
        self.read_tunings()?;
        self.read_project_and_maps()?;
        self.read_nodes_and_connections()?;
        self.read_patterns()?;
        self.validate_pattern_graph()?;
        self.read_tracks_and_places()?;
        self.read_curves_and_automation()?;
        self.read_regions()?;
        self.expand_places()?;
        let plan = self.finish_plan()?;
        plan.validate().map_err(plan_error)?;
        Ok(plan)
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
                format!("unsupported ScoreIR version {}", self.document.version),
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
                "node" => &["type", "config", "params"][..],
                "connect" => &["from", "to"][..],
                "region" => &["span", "label"][..],
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
                        format!("unknown ScoreIR object kind `{other}`"),
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
        self.read_tempo(&tempo_object)?;
        self.read_meter(&meter_object)?;
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
        self.exact_tempo =
            ExactTempoMap::new(exact).map_err(|error| map_music_error(error, Some(object.span)))?;
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
            let type_field = self.field(object, "type")?;
            let node_type =
                self.string_value(&type_field.value, object, Some(type_field), "type")?;
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
                "core.onepole/1" => {
                    let channels_field = config
                        .and_then(|fields| fields.get("channels"))
                        .ok_or_else(|| {
                            path_diagnostic(
                                DiagnosticCode::Range,
                                "core.onepole/1 requires config.channels",
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
                    Processor::one_pole(u8::try_from(channels).map_err(|_| {
                        diagnostics(
                            DiagnosticCode::Range,
                            "channels is out of range",
                            Some(type_field.value.span),
                        )
                    })?)
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
                Processor::Pan => {
                    node.params.insert("pan".into(), Rational::zero());
                }
                Processor::Sum { .. } => {}
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

    fn parse_note(&self, object: &Object) -> CResult<NoteDef> {
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
        if object
            .children
            .values()
            .any(|child| child.kind == "expression")
        {
            return Err(path_diagnostic(
                DiagnosticCode::Capability,
                "per-note expression is outside the standalone plan profile",
                object,
                None,
            ));
        }
        Ok(NoteDef {
            id: object.id.clone(),
            span: object.span,
            at,
            dur,
            pitch,
            velocity,
            release_velocity,
            onset_offset,
            release_offset,
            order,
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
                    "note" => PatternChild::Note(Box::new(self.parse_note(child)?)),
                    "use" => PatternChild::Use(Box::new(self.parse_use(child)?)),
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
                    PatternChild::Note(note) if note.at >= length => {
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
                        if leaf.kind != "note" {
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
                            note: self.parse_note(leaf)?,
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

    fn read_curves_and_automation(&mut self) -> CResult<()> {
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
            if !matches!(clock.as_str(), "score" | "seconds") {
                return Err(path_diagnostic(
                    DiagnosticCode::Capability,
                    "normalized global automation is unsupported",
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
                    _ => self.seconds_value(&tuple[0], object, Some(field))?,
                };
                let value = self.numeric_parameter_value(&tuple[1], object, Some(field))?;
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
            let at_value = self.field(object, "at")?;
            let at = if curve.clock == "score" {
                self.q_value(&at_value.value, object, Some(at_value), true)?
            } else {
                let score_anchor = match &at_value.value.kind {
                    ValueKind::Quantity { unit: Unit::Q, .. } => true,
                    ValueKind::Call { function, .. } => function == "bar",
                    _ => false,
                };
                if score_anchor {
                    self.exact_tempo
                        .seconds_at(&self.q_value(&at_value.value, object, Some(at_value), true)?)
                        .map_err(|error| map_music_error(error, Some(at_value.value.span)))?
                } else {
                    self.seconds_value(&at_value.value, object, Some(at_value))?
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
            self.automations.push(Automation {
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
                .filter(|child| matches!(child, PatternChild::Note(_)))
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
                PatternChild::Note(note) => {
                    let offset = checked_mul(&state.scale, &note.at, Some(note.span))?;
                    let score_on = checked_add(&state.origin, &offset, Some(note.span))?;
                    let dur = checked_mul(&state.scale, &note.dur, Some(note.span))?;
                    let end = checked_add(&score_on, &dur, Some(note.span))?;
                    let dur = if let Some(cut_end) = &state.inherited_cut_end {
                        if end > *cut_end {
                            checked_sub(cut_end, &score_on, Some(note.span))?
                        } else {
                            dur
                        }
                    } else {
                        dur
                    };
                    if dur <= Rational::zero() {
                        return Err(diagnostics(
                            DiagnosticCode::Interval,
                            "cut boundary removes a note's entire positive gate",
                            Some(note.span),
                        ));
                    }
                    let source_path = vec![pattern.id.clone(), note.id.clone()];
                    state.address.push(note.id.clone());
                    if self.events.len() >= MAX_EXPANDED_NOTES {
                        return Err(diagnostics(
                            DiagnosticCode::ResourceLimit,
                            "expanded note count exceeds 100000",
                            Some(note.span),
                        ));
                    }
                    self.events.push(ExpandedNote {
                        address: state.address.join("/"),
                        source: SourceMapping {
                            object: note.id.clone(),
                            path: source_path,
                            span: Some(SourceSpan {
                                start: note.span.start,
                                end: note.span.end,
                            }),
                        },
                        source_span: note.span,
                        target: state.target.clone(),
                        pitch: note.pitch.clone(),
                        transpose_cents: state.transpose.clone(),
                        score_on_q: score_on,
                        dur_q: dur,
                        velocity: note.velocity.clone(),
                        release_velocity: note.release_velocity,
                        onset_offset_seconds: note.onset_offset.clone(),
                        release_offset_seconds: note.release_offset.clone(),
                        order: note.order,
                    });
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
            let at = insert.note.at.clone();
            let score_on = checked_add(&place.at, &at, Some(insert.note.span))?;
            let source = SourceMapping {
                object: insert.note.id.clone(),
                path: vec![place.id.clone(), insert.id.clone(), insert.note.id.clone()],
                span: Some(SourceSpan {
                    start: insert.note.span.start,
                    end: insert.note.span.end,
                }),
            };
            let event = ExpandedNote {
                address: format!("{}/{}/{}", place.id, insert.id, insert.note.id),
                source,
                source_span: insert.note.span,
                target: target.clone(),
                pitch: insert.note.pitch.clone(),
                transpose_cents: Rational::zero(),
                score_on_q: score_on,
                dur_q: insert.note.dur.clone(),
                velocity: insert.note.velocity.clone(),
                release_velocity: insert.note.release_velocity,
                onset_offset_seconds: insert.note.onset_offset.clone(),
                release_offset_seconds: insert.note.release_offset.clone(),
                order: insert.note.order,
            };
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
            match name.as_str() {
                "at" => {
                    let at = self.q_value(value, object, None, false)?;
                    self.events[index].score_on_q = checked_add(&place.at, &at, Some(value.span))?;
                }
                "dur" => self.events[index].dur_q = self.q_value(value, object, None, false)?,
                "pitch" => {
                    self.events[index].pitch = self.parse_pitch(value, object, None)?;
                    self.events[index].transpose_cents = Rational::zero();
                }
                "velocity" => {
                    self.events[index].velocity = self.rational_value(value, object, None)?
                }
                "release_velocity" => {
                    let velocity = self.rational_value(value, object, None)?;
                    self.events[index].release_velocity = velocity.to_f64().ok_or_else(|| {
                        diagnostics(
                            DiagnosticCode::Nonfinite,
                            "release velocity is not finite",
                            Some(value.span),
                        )
                    })?;
                }
                "onset_offset" => {
                    self.events[index].onset_offset_seconds =
                        self.seconds_value(value, object, None)?
                }
                "release_offset" => {
                    self.events[index].release_offset_seconds =
                        self.seconds_value(value, object, None)?
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
        if self.events[index].dur_q <= Rational::zero()
            || self.events[index].velocity.is_negative()
            || self.events[index].velocity > Rational::one()
            || !(0.0..=1.0).contains(&self.events[index].release_velocity)
        {
            return Err(diagnostics(
                DiagnosticCode::Range,
                "occurrence override produces an invalid note",
                Some(self.events[index].source_span),
            ));
        }
        Ok(())
    }

    fn finish_plan(&self) -> CResult<Plan> {
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
                &expanded.dur_q,
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
                &expanded.release_offset_seconds,
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
            // A source pitch can be above Nyquist when an occurrence
            // transposition brings it back into range.  Apply all final
            // transforms before the receiver-specific Nyquist check.
            let mut frequency = expanded
                .pitch
                .resolve_hz(None)
                .map_err(|error| event_music_error(error, expanded, &["pitch"]))?;
            let transpose = expanded.transpose_cents.to_f64().ok_or_else(|| {
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
            events.push(ResolvedEvent {
                address: expanded.address.clone(),
                source: expanded.source.clone(),
                target: expanded.target.clone(),
                kind: EventKind::Note {
                    pitch_hz: frequency,
                    velocity: expanded.velocity.clone(),
                },
                score_on_q: expanded.score_on_q.clone(),
                score_off_q: Some(score_off),
                onset_offset_seconds: expanded.onset_offset_seconds.clone(),
                release_offset_seconds: expanded.release_offset_seconds.clone(),
                on_seconds,
                off_seconds: Some(off_seconds),
                release_velocity: expanded.release_velocity,
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
            version: 1,
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
            automation: self.automations.clone(),
            regions: self.regions.clone(),
            source_mappings: Vec::new(),
        };
        Ok(plan)
    }

    fn output_channels(&self) -> CResult<u8> {
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
            Processor::OnePole { channels } => Ok(channels),
            Processor::Sine { .. } => Ok(1),
        }
    }
}

fn value_span(value: &Rational, field: Option<&Field>) -> Span {
    field.map(|field| field.value.span).unwrap_or_else(|| {
        let _ = value;
        Span::default()
    })
}

/// Compile a parsed ScoreIR document into a validated standalone performance plan.
pub fn compile(document: &Document) -> Result<Plan, Diagnostics> {
    Compiler::new(document).run()
}

/// Validate a document through the same resolution path used by [`compile`].
pub fn check(document: &Document) -> Result<(), Diagnostics> {
    compile(document).map(|_| ())
}

trait FieldValueRef {
    fn value_ref(&self) -> &Value;
}

impl FieldValueRef for Field {
    fn value_ref(&self) -> &Value {
        &self.value
    }
}
