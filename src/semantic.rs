//! Source-graph validation for the MaaC/1 foundation profile.
//!
//! This module intentionally stops at the source graph boundary.  It checks
//! every declaration, including declarations that are not reachable from the
//! project output, resolves the typed references needed by the compiler, and
//! exposes the validated syntax tree through a small read-only API.  Event
//! expansion, sample scheduling, DSP, and same-sample graph validation belong
//! to later plan/renderer stages.

use std::collections::{BTreeMap, BTreeSet};

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, ToPrimitive, Zero};

use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
use crate::syntax::{Document, Field, Object, Reference, Unit, Value, ValueKind};

/// Hard source-graph limits for the foundation compiler.
pub const MAX_SOURCE_OBJECTS: usize = 200_000;
pub const MAX_SOURCE_NODES: usize = 256;
pub const MAX_SOURCE_CURVE_POINTS: usize = 65_536;
pub const MAX_SOURCE_TEMPO_POINTS: usize = 4_096;
pub const MAX_SOURCE_CONNECTIONS: usize = 4_096;

/// The four reference processors supported by the foundation profile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProcessorKind {
    Sine,
    OnePole,
    Pan,
    Sum,
}

impl ProcessorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sine => "core.sine/1",
            Self::OnePole => "core.onepole/1",
            Self::Pan => "core.pan/1",
            Self::Sum => "core.sum/1",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "core.sine/1" => Self::Sine,
            "core.onepole/1" => Self::OnePole,
            "core.pan/1" => Self::Pan,
            "core.sum/1" => Self::Sum,
            _ => return None,
        })
    }
}

/// Port direction in a validated processor descriptor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PortDirection {
    Input,
    Output,
}

/// Source-level port kind.  The plan validator adds connection cardinality
/// and same-sample causality checks after lowering.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PortKind {
    Audio,
    Control,
    Events,
}

/// A core processor port descriptor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PortDescriptor {
    pub name: &'static str,
    pub direction: PortDirection,
    pub kind: PortKind,
    pub channels: u32,
    pub accepts_multiple: bool,
    pub zero_default: bool,
}

/// Parameter units understood by the supported core processors.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ParameterUnit {
    Dimensionless,
    Seconds,
    Hertz,
    Decibels,
}

/// Range policy from the reference processor descriptor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RangePolicy {
    Error,
    Clamp,
}

/// A core processor parameter descriptor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ParameterDescriptor {
    pub name: &'static str,
    pub unit: ParameterUnit,
    pub range: RangePolicy,
}

/// Normalized processor metadata retained for compiler consumers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeDescriptor {
    pub id: String,
    pub processor: ProcessorKind,
    pub config: BTreeMap<String, BigRational>,
    pub params: BTreeMap<String, ValueType>,
}

/// A compact source value type used by reference and automation accessors.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ValueType {
    Number,
    Quantity(Unit),
    String,
    Boolean,
    Reference,
    Record,
    List,
    Tuple,
    Symbol,
    Call,
}

/// The target resolved by a source reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReferenceTarget {
    Object {
        id: String,
        kind: String,
    },
    Port {
        node: String,
        port: String,
        direction: PortDirection,
        kind: PortKind,
        channels: u32,
    },
    Parameter {
        node: String,
        parameter: String,
        unit: ParameterUnit,
        range: RangePolicy,
    },
}

/// A validated, source-preserving graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceGraph {
    document: Document,
    project_id: String,
    nodes: BTreeMap<String, NodeDescriptor>,
    references: BTreeMap<String, ReferenceTarget>,
}

impl SourceGraph {
    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn project(&self) -> &Object {
        // validate_source only constructs SourceGraph after checking this
        // invariant.  Keeping the lookup here avoids cloning a second project
        // object into the public representation.
        self.document
            .object(&self.project_id)
            .expect("validated source graph must contain its project")
    }

    pub fn object(&self, id: &str) -> Option<&Object> {
        self.document.object(id)
    }

    pub fn field(&self, object_id: &str, field: &str) -> Option<&Field> {
        self.object(object_id)
            .and_then(|object| object.field(field))
    }

    pub fn objects(&self) -> impl Iterator<Item = (&str, &Object)> {
        self.document.objects()
    }

    pub fn objects_of_kind(&self, kind: &str) -> Vec<&Object> {
        self.document
            .objects()
            .filter(|(_, object)| object.kind == kind)
            .map(|(_, object)| object)
            .collect()
    }

    pub fn node(&self, id: &str) -> Option<&NodeDescriptor> {
        self.nodes.get(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = (&str, &NodeDescriptor)> {
        self.nodes
            .iter()
            .map(|(id, descriptor)| (id.as_str(), descriptor))
    }

    pub fn resolve(&self, reference: &Reference) -> Option<&ReferenceTarget> {
        self.references.get(&reference_key(reference))
    }

    pub fn resolve_text(&self, text: &str) -> Option<&ReferenceTarget> {
        self.references.get(text)
    }
}

/// Validate a parsed source document against the foundation source profile.
pub fn validate_source(document: &Document) -> Result<SourceGraph, Diagnostics> {
    let mut validator = Validator::new(document);
    validator.validate();
    if validator.diagnostics.has_errors() {
        return Err(validator.diagnostics);
    }

    let project_id = validator.project_id.expect("validated project invariant");
    Ok(SourceGraph {
        document: document.clone(),
        project_id,
        nodes: validator.nodes,
        references: validator.references,
    })
}

/// Short alias used by compiler callers.
pub fn validate(document: &Document) -> Result<SourceGraph, Diagnostics> {
    validate_source(document)
}

struct Validator<'a> {
    document: &'a Document,
    diagnostics: Diagnostics,
    project_id: Option<String>,
    project_score: Option<(BigRational, BigRational)>,
    project_rate_hz: Option<BigRational>,
    nodes: BTreeMap<String, NodeDescriptor>,
    references: BTreeMap<String, ReferenceTarget>,
    automation_writers: BTreeSet<String>,
    total_objects: usize,
}

impl<'a> Validator<'a> {
    fn new(document: &'a Document) -> Self {
        Self {
            document,
            diagnostics: Diagnostics::new(),
            project_id: None,
            project_score: None,
            project_rate_hz: None,
            nodes: BTreeMap::new(),
            references: BTreeMap::new(),
            automation_writers: BTreeSet::new(),
            total_objects: 0,
        }
    }

    fn validate(&mut self) {
        if self.document.version != 1 {
            self.push(
                DiagnosticCode::Version,
                format!("unsupported MaaC version {}", self.document.version),
                None,
                Vec::new(),
                Vec::new(),
            );
        }

        self.total_objects = self
            .document
            .objects()
            .map(|(_, object)| 1 + count_children(object))
            .sum();
        if self.total_objects > MAX_SOURCE_OBJECTS {
            self.push(
                DiagnosticCode::ResourceLimit,
                format!(
                    "source contains {} objects; the foundation limit is {}",
                    self.total_objects, MAX_SOURCE_OBJECTS
                ),
                None,
                Vec::new(),
                Vec::new(),
            );
        }

        let node_count = self
            .document
            .objects()
            .filter(|(_, object)| object.kind == "node")
            .count();
        if node_count > MAX_SOURCE_NODES {
            self.push(
                DiagnosticCode::ResourceLimit,
                format!(
                    "source contains {node_count} nodes; the foundation limit is {}",
                    MAX_SOURCE_NODES
                ),
                None,
                Vec::new(),
                Vec::new(),
            );
        }

        let projects: Vec<Object> = self
            .document
            .objects()
            .filter_map(|(_, object)| (object.kind == "project").then_some(object.clone()))
            .collect();
        if projects.len() != 1 {
            self.push(
                DiagnosticCode::Range,
                format!(
                    "source must contain exactly one project, found {}",
                    projects.len()
                ),
                None,
                Vec::new(),
                Vec::new(),
            );
        } else {
            self.project_id = Some(projects[0].id.clone());
        }

        // Resolve the project clock and processor descriptors before validating
        // objects that refer to them.  Declaration order is explicitly
        // irrelevant in MaaC.
        let objects: Vec<Object> = self
            .document
            .objects()
            .map(|(_, object)| object.clone())
            .collect();
        if let Some(project) = self
            .project_id
            .as_ref()
            .and_then(|id| self.document.object(id))
            .cloned()
        {
            self.validate_project(&project);
        }
        for object in objects.iter().filter(|object| object.kind == "node") {
            self.validate_node(object, std::slice::from_ref(&object.id));
        }
        if let Some(project) = self
            .project_id
            .as_ref()
            .and_then(|id| self.document.object(id))
            .cloned()
        {
            self.validate_project_output(&project);
        }

        // Validate every object before resolving reachability.  A malformed
        // unused node/curve must never disappear merely because it is not
        // connected to project.output.
        for object in &objects {
            if object.kind == "project" || object.kind == "node" {
                continue;
            }
            self.validate_top_level_object(object);
        }
        self.validate_pattern_cycles();
        self.validate_connections();
        self.populate_reference_index();
    }

    fn validate_top_level_object(&mut self, object: &Object) {
        let path = vec![object.id.clone()];
        match object.kind.as_str() {
            "project" => {
                self.check_schema(
                    object,
                    &path,
                    &["score", "rate", "tempo", "meter"],
                    &[
                        "score", "rate", "tempo", "meter", "output", "tail", "seed", "requires",
                        "label",
                    ],
                    &[],
                );
                self.reject_children(object, &path);
            }
            "tempo" => self.validate_tempo(object, &path),
            "meter" => self.validate_meter(object, &path),
            "tuning" => self.validate_tuning(object, &path),
            "pattern" => self.validate_pattern(object, &path),
            "track" => self.validate_track(object, &path),
            "place" => self.validate_place(object, &path),
            "curve" => self.validate_curve(object, &path),
            "automation" => self.validate_automation(object, &path),
            "node" => self.validate_node(object, &path),
            "connect" => self.validate_connect(object, &path),
            "region" => self.validate_region(object, &path),
            "modulate" | "asset" | "audio" | "extension" | "hit" | "message" | "expression" => {
                self.validate_deferred_object(object, &path)
            }
            "note" | "use" | "override" | "insert" => {
                self.check_schema(object, &path, &[], &[], &[]);
                self.push(
                    DiagnosticCode::Capability,
                    format!(
                        "{} is only valid in its declared nesting location",
                        object.kind
                    ),
                    Some(object.kind_span),
                    path.clone(),
                    Vec::new(),
                );
            }
            _ => {
                self.check_schema(object, &path, &[], &[], &[]);
                self.push(
                    DiagnosticCode::UnknownKind,
                    format!("unknown top-level object kind {:?}", object.kind),
                    Some(object.kind_span),
                    path.clone(),
                    Vec::new(),
                );
            }
        }

        // Only the explicitly listed container objects consume children.  A
        // parser still preserves children on every object, so reject them
        // here for maps, routing, regions, and unknown/misplaced declarations
        // instead of silently dropping a malformed nested subtree.
        if !matches!(
            object.kind.as_str(),
            "project"
                | "pattern"
                | "track"
                | "place"
                | "modulate"
                | "asset"
                | "audio"
                | "extension"
                | "hit"
                | "message"
                | "expression"
        ) {
            self.reject_children(object, &path);
        }
    }

    fn check_schema(
        &mut self,
        object: &Object,
        path: &[String],
        required: &[&str],
        allowed: &[&str],
        _allowed_children: &[&str],
    ) {
        for required_name in required {
            if !object.fields.contains_key(*required_name) {
                self.push(
                    DiagnosticCode::Range,
                    format!("{} requires field `{required_name}`", object.kind),
                    Some(object.span),
                    path.to_vec(),
                    vec![(*required_name).to_owned()],
                );
            }
        }
        for (name, field) in &object.fields {
            if name == "label" {
                self.expect_string(field, path, "label");
                continue;
            }
            if !allowed.iter().any(|allowed_name| *allowed_name == name) {
                self.push(
                    DiagnosticCode::UnknownField,
                    format!("unknown field `{name}` on {}", object.kind),
                    Some(field.span),
                    path.to_vec(),
                    vec![name.clone()],
                );
            }
        }
    }

    fn reject_children(&mut self, object: &Object, path: &[String]) {
        let children: Vec<Object> = object.children.values().cloned().collect();
        for child in &children {
            self.validate_misplaced_child(child, path);
        }
    }

    fn validate_misplaced_child(&mut self, child: &Object, parent_path: &[String]) {
        let path = child_path(parent_path, &child.id);
        if is_known_kind(&child.kind) {
            self.push(
                DiagnosticCode::Capability,
                format!(
                    "{} `{}` is not allowed inside this object",
                    child.kind, child.id
                ),
                Some(child.kind_span),
                path.clone(),
                Vec::new(),
            );
            self.check_schema(child, &path, &[], &[], &[]);
        } else {
            self.push(
                DiagnosticCode::UnknownKind,
                format!("unknown nested object kind {:?}", child.kind),
                Some(child.kind_span),
                path.clone(),
                Vec::new(),
            );
        }
        self.reject_children(child, &path);
    }

    fn validate_deferred_object(&mut self, object: &Object, path: &[String]) {
        let (required, allowed): (&[&str], &[&str]) = match object.kind.as_str() {
            "modulate" => (
                &["target", "from", "amount"],
                &["target", "from", "amount", "label"],
            ),
            "asset" => (
                &["kind", "path", "hash"],
                &[
                    "kind", "path", "hash", "format", "rate", "channels", "frames", "label",
                ],
            ),
            "audio" => (
                &["asset", "at", "source", "mode"],
                &[
                    "asset",
                    "at",
                    "source",
                    "mode",
                    "speed",
                    "reverse",
                    "gain",
                    "fade_in",
                    "fade_out",
                    "fade_shape",
                    "track",
                    "warp",
                    "processor",
                    "label",
                ],
            ),
            "extension" => (
                &["namespace", "schema", "render_affecting", "data"],
                &["namespace", "schema", "render_affecting", "data", "label"],
            ),
            "hit" => (
                &["at", "key"],
                &["at", "key", "velocity", "onset_offset", "order", "label"],
            ),
            "message" => (
                &["at", "protocol", "bytes"],
                &["at", "protocol", "bytes", "onset_offset", "order", "label"],
            ),
            "expression" => (&["kind", "curve"], &["kind", "curve", "label"]),
            _ => (&[], &["label"]),
        };
        self.check_schema(object, path, required, allowed, &[]);
        match object.kind.as_str() {
            "hit" => self.validate_hit_shape(object, path),
            "message" => self.validate_message_shape(object, path),
            "expression" => self.validate_expression_shape(object, path),
            "modulate" => self.validate_modulate_shape(object, path),
            "asset" => self.validate_asset_shape(object, path),
            "audio" => self.validate_audio_shape(object, path),
            "extension" => self.validate_extension_shape(object, path),
            _ => {}
        }
        self.reject_children(object, path);
        self.push(
            DiagnosticCode::Capability,
            format!(
                "{} is recognized but outside the foundation capability profile",
                object.kind
            ),
            Some(object.kind_span),
            path.to_vec(),
            Vec::new(),
        );
    }

    fn validate_project(&mut self, object: &Object) {
        let path = vec![object.id.clone()];
        self.check_schema(
            object,
            &path,
            &["score", "rate", "tempo", "meter"],
            &[
                "score", "rate", "tempo", "meter", "output", "tail", "seed", "requires", "label",
            ],
            &[],
        );
        let score = object.field("score").and_then(|field| {
            let items = match &field.value.kind {
                ValueKind::List(items) if items.len() == 2 => items,
                ValueKind::List(_) => {
                    self.push(
                        DiagnosticCode::Range,
                        "project.score must contain exactly two positions",
                        Some(field.value.span),
                        path.clone(),
                        vec!["score".into()],
                    );
                    return None;
                }
                _ => {
                    self.push(
                        DiagnosticCode::Unit,
                        "project.score must be a list of q positions",
                        Some(field.value.span),
                        path.clone(),
                        vec!["score".into()],
                    );
                    return None;
                }
            };
            let start = self.position_q(&items[0], &path, "score")?;
            let end = self.position_q(&items[1], &path, "score")?;
            if start >= end {
                self.push(
                    DiagnosticCode::Interval,
                    "project.score start must be before score end",
                    Some(field.value.span),
                    path.clone(),
                    vec!["score".into()],
                );
                return None;
            }
            Some((start, end))
        });
        self.project_score = score;

        if let Some(field) = object.field("rate") {
            if let Some(rate) = self.quantity(field, &[Unit::Hz], &path, "rate") {
                if rate <= BigRational::zero() || !is_integer(&rate) {
                    self.push(
                        DiagnosticCode::Range,
                        "project.rate must be a positive integer Hz value",
                        Some(field.value.span),
                        path.clone(),
                        vec!["rate".into()],
                    );
                }
                self.project_rate_hz = Some(rate);
            }
        }
        if let Some(field) = object.field("tempo") {
            self.expect_object_ref(field, "tempo", &["tempo"], &path);
        }
        if let Some(field) = object.field("meter") {
            self.expect_object_ref(field, "meter", &["meter"], &path);
        }
        if let Some(field) = object.field("tail") {
            if let Some(tail) = self.quantity(field, &[Unit::S, Unit::Ms], &path, "tail") {
                if tail < BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "project.tail must be nonnegative",
                        Some(field.value.span),
                        path.clone(),
                        vec!["tail".into()],
                    );
                }
            }
        }
        if let Some(field) = object.field("seed") {
            if let Some(seed) = self.number(field, &path, "seed") {
                if seed < BigRational::zero() || !is_integer(&seed) {
                    self.push(
                        DiagnosticCode::Range,
                        "project.seed must be an unsigned 64-bit integer",
                        Some(field.value.span),
                        path.clone(),
                        vec!["seed".into()],
                    );
                } else if seed.numer().to_u64().is_none() {
                    self.push(
                        DiagnosticCode::Range,
                        "project.seed exceeds the unsigned 64-bit range",
                        Some(field.value.span),
                        path.clone(),
                        vec!["seed".into()],
                    );
                }
            }
        }
        if let Some(field) = object.field("requires") {
            if let ValueKind::List(items) = &field.value.kind {
                for item in items {
                    if !matches!(item.kind, ValueKind::String(_)) {
                        self.push(
                            DiagnosticCode::Unit,
                            "project.requires entries must be exact identifier strings",
                            Some(item.span),
                            path.clone(),
                            vec!["requires".into()],
                        );
                    }
                }
            } else {
                self.push(
                    DiagnosticCode::Unit,
                    "project.requires must be a list",
                    Some(field.value.span),
                    path,
                    vec!["requires".into()],
                );
            }
        }
    }

    fn validate_project_output(&mut self, object: &Object) {
        let path = vec![object.id.clone()];
        if let Some(field) = object.field("output") {
            self.expect_port_ref(
                field,
                "output",
                Some(PortDirection::Output),
                Some(PortKind::Audio),
                &path,
            );
        }
    }

    fn validate_tempo(&mut self, object: &Object, path: &[String]) {
        self.check_schema(object, path, &["points"], &["points", "label"], &[]);
        let Some(field) = object.field("points") else {
            return;
        };
        let ValueKind::List(points) = &field.value.kind else {
            self.push_type(field, path, "points", "a list of tempo points");
            return;
        };
        if points.is_empty() {
            self.push(
                DiagnosticCode::Tempo,
                "tempo.points must be nonempty",
                Some(field.value.span),
                path.to_vec(),
                vec!["points".into()],
            );
        }
        if points.len() > MAX_SOURCE_TEMPO_POINTS {
            self.push(
                DiagnosticCode::ResourceLimit,
                "tempo.points exceeds the source limit",
                Some(field.value.span),
                path.to_vec(),
                vec!["points".into()],
            );
        }
        let mut previous: Option<BigRational> = None;
        for (index, point) in points.iter().enumerate() {
            let point_path = field_path(path, "points");
            let ValueKind::Tuple(items) = &point.kind else {
                self.push_type_value(point, &point_path, "tempo point tuple");
                continue;
            };
            if items.len() != 3 {
                self.push(
                    DiagnosticCode::Tempo,
                    "tempo points must be (q, bpm, shape)",
                    Some(point.span),
                    point_path.clone(),
                    vec![index.to_string()],
                );
                continue;
            }
            let Some(position) =
                self.quantity_value(&items[0], &[Unit::Q], &point_path, "tempo point position")
            else {
                continue;
            };
            if let Some(previous) = &previous {
                if position <= *previous {
                    self.push(
                        DiagnosticCode::Tempo,
                        "tempo point positions must be strictly increasing",
                        Some(items[0].span),
                        point_path.clone(),
                        vec![index.to_string()],
                    );
                }
            }
            previous = Some(position.clone());
            let Some(bpm) = self.quantity_value(&items[1], &[Unit::Bpm], &point_path, "tempo BPM")
            else {
                continue;
            };
            if bpm <= BigRational::zero() {
                self.push(
                    DiagnosticCode::Tempo,
                    "tempo BPM must be positive",
                    Some(items[1].span),
                    point_path.clone(),
                    vec![index.to_string()],
                );
            }
            let shape = items[2].as_symbol();
            if !matches!(shape, Some("step" | "linear")) {
                self.push(
                    DiagnosticCode::Tempo,
                    "tempo shape must be step or linear",
                    Some(items[2].span),
                    point_path.clone(),
                    vec![index.to_string()],
                );
            } else if shape == Some("linear") {
                self.push(
                    DiagnosticCode::Capability,
                    "linear tempo ramps are outside the foundation capability profile",
                    Some(items[2].span),
                    path.to_vec(),
                    vec!["points".into()],
                );
            }
            if index + 1 == points.len() && shape != Some("step") {
                self.push(
                    DiagnosticCode::Tempo,
                    "the final tempo point must use step shape",
                    Some(items[2].span),
                    path.to_vec(),
                    vec!["points".into()],
                );
            }
        }
    }

    fn validate_meter(&mut self, object: &Object, path: &[String]) {
        self.check_schema(object, path, &["points"], &["points", "label"], &[]);
        let Some(field) = object.field("points") else {
            return;
        };
        let ValueKind::List(points) = &field.value.kind else {
            self.push_type(field, path, "points", "a list of meter points");
            return;
        };
        if points.is_empty() {
            self.push(
                DiagnosticCode::Range,
                "meter.points must be nonempty",
                Some(field.value.span),
                path.to_vec(),
                vec!["points".into()],
            );
            return;
        }
        let mut previous: Option<BigRational> = None;
        let mut previous_meter: Option<(BigRational, BigRational, BigRational)> = None;
        for (index, point) in points.iter().enumerate() {
            let point_path = field_path(path, "points");
            let ValueKind::Tuple(items) = &point.kind else {
                self.push_type_value(point, &point_path, "meter point tuple");
                continue;
            };
            if items.len() != 3 {
                self.push(
                    DiagnosticCode::Range,
                    "meter points must be (q, numerator, denominator)",
                    Some(point.span),
                    point_path.clone(),
                    vec![index.to_string()],
                );
                continue;
            }
            let Some(position) =
                self.quantity_value(&items[0], &[Unit::Q], &point_path, "meter point position")
            else {
                continue;
            };
            if index == 0 && !position.is_zero() {
                self.push(
                    DiagnosticCode::MeterBoundary,
                    "the first meter point must be at 0q",
                    Some(items[0].span),
                    path.to_vec(),
                    vec!["points".into()],
                );
            }
            if let Some(previous) = &previous {
                if position <= *previous {
                    self.push(
                        DiagnosticCode::MeterBoundary,
                        "meter point positions must be strictly increasing",
                        Some(items[0].span),
                        path.to_vec(),
                        vec!["points".into()],
                    );
                }
            }
            previous = Some(position.clone());
            let Some(numerator) = self.integer_value(&items[1], &point_path, "meter numerator")
            else {
                continue;
            };
            let Some(denominator) = self.integer_value(&items[2], &point_path, "meter denominator")
            else {
                continue;
            };
            if numerator <= BigRational::zero()
                || denominator <= BigRational::zero()
                || !is_integer(&numerator)
                || !is_integer(&denominator)
            {
                self.push(
                    DiagnosticCode::Range,
                    "meter numerator and denominator must be positive integers",
                    Some(point.span),
                    path.to_vec(),
                    vec!["points".into()],
                );
            } else if !denominator
                .numer()
                .to_u64()
                .is_some_and(|value| value <= 1024 && value.is_power_of_two())
            {
                self.push(
                    DiagnosticCode::Range,
                    "meter denominator must be a power of two <= 1024",
                    Some(items[2].span),
                    path.to_vec(),
                    vec!["points".into()],
                );
            } else {
                if let Some((previous_position, previous_numerator, previous_denominator)) =
                    &previous_meter
                {
                    if position > *previous_position {
                        let bar_length = previous_numerator
                            * BigRational::from_integer(BigInt::from(4))
                            / previous_denominator;
                        let bars = (position.clone() - previous_position) / bar_length;
                        if !is_integer(&bars) {
                            self.push(
                                DiagnosticCode::MeterBoundary,
                                "meter changes must occur on a preceding-meter bar boundary",
                                Some(items[0].span),
                                path.to_vec(),
                                vec!["points".into()],
                            );
                        }
                    }
                }
                previous_meter = Some((position, numerator, denominator));
            }
        }
    }

    fn validate_tuning(&mut self, object: &Object, path: &[String]) {
        self.check_schema(
            object,
            path,
            &["period", "steps", "reference_index", "reference_frequency"],
            &[
                "period",
                "steps",
                "reference_index",
                "reference_frequency",
                "label",
            ],
            &[],
        );
        let period_value = if let Some(field) = object.field("period") {
            let Some(period) = self.quantity(field, &[Unit::Ct], path, "period") else {
                return;
            };
            if period <= BigRational::zero() {
                self.push(
                    DiagnosticCode::Range,
                    "tuning.period must be positive",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["period".into()],
                );
            }
            Some(period)
        } else {
            None
        };
        if let Some(field) = object.field("steps") {
            let ValueKind::List(items) = &field.value.kind else {
                self.push_type(field, path, "steps", "a list of cents");
                return;
            };
            if items.is_empty() {
                self.push(
                    DiagnosticCode::Range,
                    "tuning.steps must be nonempty",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["steps".into()],
                );
            }
            let mut previous: Option<BigRational> = None;
            for (index, item) in items.iter().enumerate() {
                let Some(value) = self.quantity_value(item, &[Unit::Ct], path, "tuning step")
                else {
                    continue;
                };
                if index == 0 && !value.is_zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "tuning.steps must begin at 0ct",
                        Some(item.span),
                        path.to_vec(),
                        vec!["steps".into()],
                    );
                }
                if let Some(period) = &period_value {
                    if value >= *period {
                        self.push(
                            DiagnosticCode::Range,
                            "tuning.steps must remain below tuning.period",
                            Some(item.span),
                            path.to_vec(),
                            vec!["steps".into()],
                        );
                    }
                }
                if let Some(previous) = &previous {
                    if value <= *previous {
                        self.push(
                            DiagnosticCode::Range,
                            "tuning.steps must be strictly increasing",
                            Some(item.span),
                            path.to_vec(),
                            vec!["steps".into()],
                        );
                    }
                }
                previous = Some(value);
            }
        }
        if let Some(field) = object.field("reference_index") {
            if let Some(index) = self.integer(field, path, "reference_index") {
                let step_count = object
                    .field("steps")
                    .and_then(|steps| match &steps.value.kind {
                        ValueKind::List(items) => Some(items.len()),
                        _ => None,
                    });
                if index < BigRational::zero()
                    || step_count
                        .is_some_and(|count| index.to_usize().is_none_or(|index| index >= count))
                {
                    self.push(
                        DiagnosticCode::Range,
                        "tuning.reference_index must address an entry in tuning.steps",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["reference_index".into()],
                    );
                }
            }
        }
        if let Some(field) = object.field("reference_frequency") {
            if let Some(value) =
                self.quantity(field, &[Unit::Hz, Unit::KHz], path, "reference_frequency")
            {
                if value <= BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "tuning.reference_frequency must be positive",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["reference_frequency".into()],
                    );
                }
            }
        }
    }

    fn validate_pattern(&mut self, object: &Object, path: &[String]) {
        self.check_schema(object, path, &["length"], &["length", "label"], &[]);
        let length = object
            .field("length")
            .and_then(|field| self.quantity(field, &[Unit::Q], path, "length"));
        if let Some(length) = &length {
            if length <= &BigRational::zero() {
                self.push(
                    DiagnosticCode::Range,
                    "pattern.length must be positive",
                    object.field("length").map(|field| field.value.span),
                    path.to_vec(),
                    vec!["length".into()],
                );
            }
        }
        let children: Vec<Object> = object.children.values().cloned().collect();
        for child in &children {
            let child_path = child_path(path, &child.id);
            match child.kind.as_str() {
                "note" => self.validate_note(child, &child_path),
                "use" => self.validate_use(child, &child_path),
                "hit" | "message" => self.validate_deferred_object(child, &child_path),
                _ => self.validate_misplaced_child(child, path),
            }
            if let Some(length) = &length {
                if let Some(at) = child
                    .field("at")
                    .and_then(|field| self.position_q_value(&field.value, &child_path, "at"))
                {
                    if at >= *length {
                        self.push(
                            DiagnosticCode::Interval,
                            "pattern child onset must be before pattern length",
                            child.field("at").map(|field| field.value.span),
                            child_path,
                            vec!["at".into()],
                        );
                    }
                }
            }
        }
    }

    fn validate_note(&mut self, object: &Object, path: &[String]) {
        self.check_schema(
            object,
            path,
            &["at", "dur", "pitch"],
            &[
                "at",
                "dur",
                "pitch",
                "velocity",
                "release_velocity",
                "onset_offset",
                "release_offset",
                "order",
                "label",
            ],
            &[],
        );
        if let Some(field) = object.field("at") {
            if let Some(value) = self.quantity(field, &[Unit::Q], path, "at") {
                if value < BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "note.at must be nonnegative",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["at".into()],
                    );
                }
            }
        }
        if let Some(field) = object.field("dur") {
            if let Some(value) = self.quantity(field, &[Unit::Q], path, "dur") {
                if value <= BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "note.dur must be positive",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["dur".into()],
                    );
                }
            }
        }
        if let Some(field) = object.field("pitch") {
            self.validate_pitch(field, path);
        }
        self.validate_unit_range_field(
            object,
            path,
            "velocity",
            BigRational::zero(),
            BigRational::one(),
        );
        self.validate_unit_range_field(
            object,
            path,
            "release_velocity",
            BigRational::zero(),
            BigRational::one(),
        );
        self.validate_seconds(object, path, "onset_offset");
        self.validate_seconds(object, path, "release_offset");
        if let Some(field) = object.field("order") {
            self.integer(field, path, "order");
        }
        let children: Vec<Object> = object.children.values().cloned().collect();
        for child in &children {
            let child_path = child_path(path, &child.id);
            if child.kind == "expression" {
                self.validate_deferred_object(child, &child_path);
            } else {
                self.validate_misplaced_child(child, path);
            }
        }
    }

    fn validate_use(&mut self, object: &Object, path: &[String]) {
        self.check_schema(
            object,
            path,
            &["pattern"],
            &[
                "pattern",
                "at",
                "count",
                "stretch",
                "transpose",
                "boundary",
                "label",
            ],
            &[],
        );
        if let Some(field) = object.field("pattern") {
            self.expect_object_ref(field, "pattern", &["pattern"], path);
        }
        if let Some(field) = object.field("at") {
            if let Some(value) = self.quantity(field, &[Unit::Q], path, "at") {
                if value < BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "use.at must be nonnegative",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["at".into()],
                    );
                }
            }
        }
        self.validate_positive_integer_field(object, path, "count");
        self.validate_positive_dimensionless_field(object, path, "stretch");
        self.validate_quantity_field(object, path, "transpose", &[Unit::Ct]);
        self.validate_enum_field(object, path, "boundary", &["spill", "cut"]);
        self.reject_children(object, path);
    }

    fn validate_track(&mut self, object: &Object, path: &[String]) {
        self.check_schema(object, path, &[], &["target", "label"], &[]);
        if let Some(field) = object.field("target") {
            self.expect_port_ref(
                field,
                "target",
                Some(PortDirection::Input),
                Some(PortKind::Events),
                path,
            );
        }
        self.reject_children(object, path);
    }

    fn validate_place(&mut self, object: &Object, path: &[String]) {
        self.check_schema(
            object,
            path,
            &["pattern", "track", "at"],
            &[
                "pattern",
                "track",
                "at",
                "count",
                "stretch",
                "transpose",
                "boundary",
                "label",
            ],
            &[],
        );
        if let Some(field) = object.field("pattern") {
            self.expect_object_ref(field, "pattern", &["pattern"], path);
        }
        if let Some(field) = object.field("track") {
            self.expect_object_ref(field, "track", &["track"], path);
        }
        if let Some(field) = object.field("at") {
            if let Some(value) = self.position_q_value(&field.value, path, "at") {
                let outside = self
                    .project_score
                    .as_ref()
                    .is_some_and(|(start, end)| value < *start || value >= *end);
                if outside {
                    self.push(
                        DiagnosticCode::Interval,
                        "place.at must fall inside project.score",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["at".into()],
                    );
                }
            }
        }
        self.validate_positive_integer_field(object, path, "count");
        self.validate_positive_dimensionless_field(object, path, "stretch");
        self.validate_quantity_field(object, path, "transpose", &[Unit::Ct]);
        self.validate_enum_field(object, path, "boundary", &["spill", "cut"]);
        let children: Vec<Object> = object.children.values().cloned().collect();
        for child in &children {
            let child_path = child_path(path, &child.id);
            match child.kind.as_str() {
                "override" => self.validate_override(child, &child_path),
                "insert" => self.validate_insert(child, &child_path),
                _ => self.validate_misplaced_child(child, path),
            }
        }
    }

    fn validate_override(&mut self, object: &Object, path: &[String]) {
        self.check_schema(
            object,
            path,
            &["event"],
            &["event", "set", "delete", "label"],
            &[],
        );
        if let Some(field) = object.field("event") {
            self.expect_string(field, path, "event");
        }
        let has_set = object.field("set").is_some();
        let has_delete = object.field("delete").is_some();
        if has_set == has_delete {
            self.push(
                DiagnosticCode::Range,
                "override requires exactly one of set or delete",
                Some(object.span),
                path.to_vec(),
                Vec::new(),
            );
        }
        if let Some(field) = object.field("set") {
            self.validate_override_record(field, path);
        }
        if let Some(field) = object.field("delete") {
            if !matches!(field.value.kind, ValueKind::Boolean(true)) {
                self.push(
                    DiagnosticCode::Range,
                    "override.delete must be true",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["delete".into()],
                );
            }
        }
        self.reject_children(object, path);
    }

    fn validate_override_record(&mut self, field: &Field, path: &[String]) {
        let ValueKind::Record(fields) = &field.value.kind else {
            self.push_type(field, path, "set", "a record");
            return;
        };
        for (name, replacement) in fields {
            if !matches!(
                name.as_str(),
                "at" | "dur"
                    | "pitch"
                    | "velocity"
                    | "release_velocity"
                    | "onset_offset"
                    | "release_offset"
                    | "order"
                    | "label"
            ) {
                self.push(
                    DiagnosticCode::UnknownField,
                    format!("override.set cannot replace `{name}`"),
                    Some(replacement.span),
                    path.to_vec(),
                    vec!["set".into(), name.clone()],
                );
                continue;
            }
            match name.as_str() {
                "at" | "dur" => {
                    if let Some(value) =
                        self.quantity_value(&replacement.value, &[Unit::Q], path, name)
                    {
                        if name == "dur" && value <= BigRational::zero()
                            || name == "at" && value < BigRational::zero()
                        {
                            self.push(
                                DiagnosticCode::Range,
                                format!("override.set.{name} is outside its valid range"),
                                Some(replacement.value.span),
                                path.to_vec(),
                                vec!["set".into(), name.clone()],
                            );
                        }
                    }
                }
                "velocity" | "release_velocity" => self.validate_unit_range_value(
                    &replacement.value,
                    path,
                    name,
                    BigRational::zero(),
                    BigRational::one(),
                ),
                "onset_offset" | "release_offset" => {
                    self.validate_seconds_value(&replacement.value, path, name);
                }
                "order" if !matches!(replacement.value.kind, ValueKind::Number(_)) => {
                    self.push_type_value(&replacement.value, path, "integer order");
                }
                "order" => {}
                "pitch" => self.validate_pitch_value(&replacement.value, path),
                "label" if !matches!(replacement.value.kind, ValueKind::String(_)) => {
                    self.push_type_value(&replacement.value, path, "string label");
                }
                "label" => {}
                _ => {}
            }
        }
    }

    fn validate_insert(&mut self, object: &Object, path: &[String]) {
        self.check_schema(object, path, &[], &["label"], &[]);
        if object.children.len() != 1 {
            self.push(
                DiagnosticCode::Range,
                "insert must contain exactly one note, hit, or message",
                Some(object.span),
                path.to_vec(),
                Vec::new(),
            );
        }
        let children: Vec<Object> = object.children.values().cloned().collect();
        for child in &children {
            let child_path = child_path(path, &child.id);
            match child.kind.as_str() {
                "note" => self.validate_note(child, &child_path),
                "hit" | "message" => self.validate_deferred_object(child, &child_path),
                _ => self.validate_misplaced_child(child, path),
            }
        }
    }

    fn validate_curve(&mut self, object: &Object, path: &[String]) {
        self.check_schema(
            object,
            path,
            &["clock", "points"],
            &["clock", "points", "label"],
            &[],
        );
        let clock = object.field("clock").and_then(|field| {
            let Some(clock) = field.value.as_symbol() else {
                self.push_type(field, path, "clock", "score, seconds, or normalized");
                return None;
            };
            if !matches!(clock, "score" | "seconds" | "normalized") {
                self.push(
                    DiagnosticCode::Range,
                    "curve.clock must be score, seconds, or normalized",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["clock".into()],
                );
                return None;
            }
            Some(clock)
        });
        let Some(field) = object.field("points") else {
            return;
        };
        let ValueKind::List(points) = &field.value.kind else {
            self.push_type(field, path, "points", "a list of curve points");
            return;
        };
        if points.is_empty() {
            self.push(
                DiagnosticCode::Range,
                "curve.points must be nonempty",
                Some(field.value.span),
                path.to_vec(),
                vec!["points".into()],
            );
            return;
        }
        if points.len() > MAX_SOURCE_CURVE_POINTS {
            self.push(
                DiagnosticCode::ResourceLimit,
                "curve.points exceeds the source limit",
                Some(field.value.span),
                path.to_vec(),
                vec!["points".into()],
            );
        }
        let mut previous: Option<BigRational> = None;
        let mut value_dimension: Option<Option<CurveDimension>> = None;
        for (index, point) in points.iter().enumerate() {
            let ValueKind::Tuple(items) = &point.kind else {
                self.push_type_value(point, path, "curve point tuple");
                continue;
            };
            if items.len() != 3 {
                self.push(
                    DiagnosticCode::Range,
                    "curve points must be (position, value, shape)",
                    Some(point.span),
                    path.to_vec(),
                    vec!["points".into()],
                );
                continue;
            }
            let position = match clock {
                Some("score") => self.quantity_value(&items[0], &[Unit::Q], path, "curve position"),
                Some("seconds") => {
                    self.quantity_value(&items[0], &[Unit::S, Unit::Ms], path, "curve position")
                }
                Some("normalized") => self.number_value(&items[0], path, "curve position"),
                _ => None,
            };
            if let Some(position) = position {
                if index == 0 && !position.is_zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "the first curve position must be zero",
                        Some(items[0].span),
                        path.to_vec(),
                        vec!["points".into()],
                    );
                }
                if let Some(previous) = &previous {
                    if position <= *previous {
                        self.push(
                            DiagnosticCode::Range,
                            "curve positions must be strictly increasing",
                            Some(items[0].span),
                            path.to_vec(),
                            vec!["points".into()],
                        );
                    }
                }
                previous = Some(position.clone());
                if clock == Some("normalized")
                    && index + 1 == points.len()
                    && position != BigRational::one()
                {
                    self.push(
                        DiagnosticCode::Range,
                        "normalized curves must end at 1",
                        Some(items[0].span),
                        path.to_vec(),
                        vec!["points".into()],
                    );
                }
            }
            let dimension = value_dimension_of(&items[1]);
            if dimension.is_none() {
                self.push(
                    DiagnosticCode::Unit,
                    "curve point values must be numeric or explicitly unit-valued",
                    Some(items[1].span),
                    path.to_vec(),
                    vec!["points".into()],
                );
            }
            match value_dimension {
                None => value_dimension = Some(dimension),
                Some(None) if dimension.is_some() => value_dimension = Some(dimension),
                Some(Some(previous_dimension))
                    if dimension.is_some_and(|current| current != previous_dimension) =>
                {
                    self.push(
                        DiagnosticCode::Unit,
                        "curve point values must use one compatible unit",
                        Some(items[1].span),
                        path.to_vec(),
                        vec!["points".into()],
                    );
                }
                _ => {}
            }
            let Some(shape) = items[2].as_symbol() else {
                self.push_type_value(&items[2], path, "curve interpolation shape");
                continue;
            };
            if !matches!(shape, "step" | "linear" | "exponential") {
                self.push(
                    DiagnosticCode::Range,
                    "curve shape must be step, linear, or exponential",
                    Some(items[2].span),
                    path.to_vec(),
                    vec!["points".into()],
                );
            }
            if index + 1 == points.len() && shape != "step" {
                self.push(
                    DiagnosticCode::Range,
                    "the final curve shape must be step",
                    Some(items[2].span),
                    path.to_vec(),
                    vec!["points".into()],
                );
            }
            if shape == "exponential" {
                if matches!(
                    dimension,
                    Some(CurveDimension::Decibels | CurveDimension::Cents)
                ) {
                    self.push(
                        DiagnosticCode::Range,
                        "exponential interpolation is forbidden for dB and cents curves",
                        Some(items[2].span),
                        path.to_vec(),
                        vec!["points".into()],
                    );
                }
                if index + 1 < points.len() {
                    if let (Some(start), Some(end)) = (
                        numeric_value(&items[1]),
                        numeric_value(&points[index + 1].tuple_item(1)),
                    ) {
                        if start <= BigRational::zero() || end <= BigRational::zero() {
                            self.push(
                                DiagnosticCode::Range,
                                "exponential curve endpoints must be positive",
                                Some(point.span),
                                path.to_vec(),
                                vec!["points".into()],
                            );
                        }
                    }
                }
            }
        }
    }

    fn validate_automation(&mut self, object: &Object, path: &[String]) {
        self.check_schema(
            object,
            path,
            &["target", "curve", "at"],
            &["target", "curve", "at", "label"],
            &[],
        );
        let target = object
            .field("target")
            .and_then(|field| self.expect_parameter_ref(field, path));
        let curve_clock = object.field("curve").and_then(|field| {
            let target = self.expect_object_ref(field, "curve", &["curve"], path);
            target
                .and_then(|target| {
                    self.document.object(match &target {
                        ReferenceTarget::Object { id, .. } => id.as_str(),
                        _ => "",
                    })
                })
                .and_then(|curve| curve.field("clock"))
                .and_then(|field| field.value.as_symbol())
        });
        if let Some(field) = object.field("at") {
            match curve_clock {
                Some("score") => {
                    self.position_q_value(&field.value, path, "at");
                }
                Some("seconds") => match field.value.kind {
                    ValueKind::Call { .. } => {
                        self.position_q_value(&field.value, path, "at");
                    }
                    _ => {
                        self.quantity(field, &[Unit::Q, Unit::S, Unit::Ms], path, "at");
                    }
                },
                Some("normalized") => self.push(
                    DiagnosticCode::Capability,
                    "normalized curves cannot be global automation",
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["curve".into()],
                ),
                _ => {}
            }
        }
        if let Some(target) = target {
            self.check_curve_target_units(object, path, target);
        }
    }

    fn validate_node(&mut self, object: &Object, path: &[String]) {
        self.check_schema(
            object,
            path,
            &["type"],
            &[
                "type",
                "config",
                "params",
                "implementation",
                "state",
                "label",
            ],
            &[],
        );
        let Some(type_field) = object.field("type") else {
            return;
        };
        let Some(processor_name) = type_field.value.as_string() else {
            self.push_type(type_field, path, "type", "a processor identifier string");
            return;
        };
        let Some(processor) = ProcessorKind::parse(processor_name) else {
            self.validate_generic_node_records(object, path);
            self.push(
                DiagnosticCode::Capability,
                format!(
                    "processor `{processor_name}` is outside the foundation capability profile"
                ),
                Some(type_field.value.span),
                path.to_vec(),
                vec!["type".into()],
            );
            return;
        };
        if object.field("implementation").is_some() || object.field("state").is_some() {
            self.push(
                DiagnosticCode::Capability,
                "core processors cannot declare implementation or state",
                Some(object.span),
                path.to_vec(),
                Vec::new(),
            );
        }
        let config = self.record_field(object, "config", path).cloned();
        let params = self.record_field(object, "params", path).cloned();
        let config_map = config
            .as_ref()
            .map(|record| self.validate_processor_config(processor, record, path))
            .unwrap_or_default();
        let param_types = params
            .as_ref()
            .map(|record| self.validate_processor_params(processor, record, path))
            .unwrap_or_default();
        self.nodes.insert(
            object.id.clone(),
            NodeDescriptor {
                id: object.id.clone(),
                processor,
                config: config_map,
                params: param_types,
            },
        );
    }

    fn validate_generic_node_records(&mut self, object: &Object, path: &[String]) {
        for name in ["config", "params"] {
            if let Some(field) = object.field(name) {
                if !matches!(field.value.kind, ValueKind::Record(_)) {
                    self.push_type(field, path, name, "a record");
                }
            }
        }
    }

    fn validate_processor_config(
        &mut self,
        processor: ProcessorKind,
        fields: &BTreeMap<String, Field>,
        path: &[String],
    ) -> BTreeMap<String, BigRational> {
        let allowed: &[&str] = match processor {
            ProcessorKind::Sine => &["voices"],
            ProcessorKind::OnePole => &["channels"],
            ProcessorKind::Pan => &[],
            ProcessorKind::Sum => &["channels"],
        };
        let mut result = BTreeMap::new();
        for (name, field) in fields {
            if !allowed.contains(&name.as_str()) {
                self.push(
                    DiagnosticCode::UnknownField,
                    format!("unknown config field `{name}` for {}", processor.as_str()),
                    Some(field.span),
                    path.to_vec(),
                    vec!["config".into(), name.clone()],
                );
                continue;
            }
            let Some(value) = self.number_value(&field.value, path, name) else {
                continue;
            };
            if value <= BigRational::zero() || !is_integer(&value) {
                self.push(
                    DiagnosticCode::Range,
                    format!("config.{name} must be a positive integer"),
                    Some(field.value.span),
                    path.to_vec(),
                    vec!["config".into(), name.clone()],
                );
            } else {
                result.insert(name.clone(), value);
            }
        }
        match processor {
            ProcessorKind::Sine if !fields.contains_key("voices") => {}
            ProcessorKind::OnePole | ProcessorKind::Sum if !fields.contains_key("channels") => self
                .push(
                    DiagnosticCode::Range,
                    "processor config requires channels",
                    None,
                    path.to_vec(),
                    vec!["config".into(), "channels".into()],
                ),
            _ => {}
        }
        result
    }

    fn validate_processor_params(
        &mut self,
        processor: ProcessorKind,
        fields: &BTreeMap<String, Field>,
        path: &[String],
    ) -> BTreeMap<String, ValueType> {
        let allowed: &[&str] = match processor {
            ProcessorKind::Sine => &["attack", "release", "level"],
            ProcessorKind::OnePole => &["cutoff"],
            ProcessorKind::Pan => &["pan"],
            ProcessorKind::Sum => &[],
        };
        let mut result = BTreeMap::new();
        for (name, field) in fields {
            if !allowed.contains(&name.as_str()) {
                self.push(
                    DiagnosticCode::UnknownField,
                    format!("unknown parameter `{name}` for {}", processor.as_str()),
                    Some(field.span),
                    path.to_vec(),
                    vec!["params".into(), name.clone()],
                );
                continue;
            }
            let valid = match (processor, name.as_str()) {
                (ProcessorKind::Sine, "attack" | "release") => self
                    .quantity(field, &[Unit::S, Unit::Ms], path, name)
                    .is_some_and(|v| {
                        if v < BigRational::zero() {
                            self.push(
                                DiagnosticCode::Range,
                                format!("params.{name} must be nonnegative"),
                                Some(field.value.span),
                                path.to_vec(),
                                vec!["params".into(), name.into()],
                            );
                            false
                        } else {
                            true
                        }
                    }),
                (ProcessorKind::Sine, "level") => self.number_nonnegative(field, path, name),
                (ProcessorKind::OnePole, "cutoff") => self
                    .quantity(field, &[Unit::Hz, Unit::KHz], path, name)
                    .is_some_and(|v| {
                        if v <= BigRational::zero() {
                            self.push(
                                DiagnosticCode::Range,
                                "onepole cutoff must be positive",
                                Some(field.value.span),
                                path.to_vec(),
                                vec!["params".into(), name.into()],
                            );
                            false
                        } else if self
                            .project_rate_hz
                            .as_ref()
                            .is_some_and(|rate| v * BigRational::from_integer(BigInt::from(2)) >= *rate)
                        {
                            self.push(
                                DiagnosticCode::Range,
                                "onepole cutoff must be strictly below the engine Nyquist frequency",
                                Some(field.value.span),
                                path.to_vec(),
                                vec!["params".into(), name.into()],
                            );
                            false
                        } else {
                            true
                        }
                    }),
                (ProcessorKind::Pan, "pan") => {
                    self.number_value(&field.value, path, name).is_some()
                }
                _ => false,
            };
            if valid {
                result.insert(name.clone(), value_type(&field.value));
            }
        }
        result
    }

    fn validate_connect(&mut self, object: &Object, path: &[String]) {
        self.check_schema(object, path, &["from", "to"], &["from", "to", "label"], &[]);
        let from = object.field("from").and_then(|field| {
            self.expect_port_ref(field, "from", Some(PortDirection::Output), None, path)
        });
        let to = object.field("to").and_then(|field| {
            self.expect_port_ref(field, "to", Some(PortDirection::Input), None, path)
        });
        if let (Some(from), Some(to)) = (from, to) {
            if from.kind != to.kind || from.channels != to.channels {
                self.push(
                    DiagnosticCode::PortType,
                    "connection ports must have matching kind and channel count",
                    Some(object.span),
                    path.to_vec(),
                    Vec::new(),
                );
            }
        }
    }

    fn validate_region(&mut self, object: &Object, path: &[String]) {
        self.check_schema(object, path, &["span"], &["span", "label"], &[]);
        let Some(field) = object.field("span") else {
            return;
        };
        let ValueKind::List(items) = &field.value.kind else {
            self.push_type(field, path, "span", "a two-position q list");
            return;
        };
        if items.len() != 2 {
            self.push(
                DiagnosticCode::Interval,
                "region.span must contain two positions",
                Some(field.value.span),
                path.to_vec(),
                vec!["span".into()],
            );
            return;
        }
        let (Some(start), Some(end)) = (
            self.position_q_value(&items[0], path, "span"),
            self.position_q_value(&items[1], path, "span"),
        ) else {
            return;
        };
        if start >= end {
            self.push(
                DiagnosticCode::Interval,
                "region span start must be before end",
                Some(field.value.span),
                path.to_vec(),
                vec!["span".into()],
            );
        }
        let outside = self
            .project_score
            .as_ref()
            .is_some_and(|(score_start, score_end)| start < *score_start || end > *score_end);
        if outside {
            self.push(
                DiagnosticCode::Interval,
                "region span must lie within project.score",
                Some(field.value.span),
                path.to_vec(),
                vec!["span".into()],
            );
        }
    }

    fn validate_hit_shape(&mut self, object: &Object, path: &[String]) {
        if let Some(field) = object.field("at") {
            if let Some(value) = self.quantity(field, &[Unit::Q], path, "at") {
                if value < BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "hit.at must be nonnegative",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["at".into()],
                    );
                }
            }
        }
        if let Some(field) = object.field("key") {
            self.expect_string(field, path, "key");
        }
        self.validate_unit_range_field(
            object,
            path,
            "velocity",
            BigRational::zero(),
            BigRational::one(),
        );
        self.validate_seconds(object, path, "onset_offset");
        if let Some(field) = object.field("order") {
            self.integer(field, path, "order");
        }
    }

    fn validate_message_shape(&mut self, object: &Object, path: &[String]) {
        if let Some(field) = object.field("at") {
            if let Some(value) = self.quantity(field, &[Unit::Q], path, "at") {
                if value < BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "message.at must be nonnegative",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["at".into()],
                    );
                }
            }
        }
        if let Some(field) = object.field("protocol") {
            self.expect_string(field, path, "protocol");
        }
        if let Some(field) = object.field("bytes") {
            let ValueKind::List(items) = &field.value.kind else {
                self.push_type(field, path, "bytes", "a list of byte integers");
                return;
            };
            for item in items {
                if let Some(value) = self.integer_value(item, path, "message byte") {
                    if value < BigRational::zero()
                        || value > BigRational::from_integer(BigInt::from(255))
                    {
                        self.push(
                            DiagnosticCode::Range,
                            "message bytes must be in 0..255",
                            Some(item.span),
                            path.to_vec(),
                            vec!["bytes".into()],
                        );
                    }
                }
            }
        }
        self.validate_seconds(object, path, "onset_offset");
        if let Some(field) = object.field("order") {
            self.integer(field, path, "order");
        }
    }

    fn validate_expression_shape(&mut self, object: &Object, path: &[String]) {
        if object.field("kind").is_some() {
            self.validate_enum_field(
                object,
                path,
                "kind",
                &["pitch", "gain", "pressure", "timbre"],
            );
        }
        if let Some(field) = object.field("curve") {
            self.expect_object_ref(field, "curve", &["curve"], path);
        }
    }

    fn validate_modulate_shape(&mut self, object: &Object, path: &[String]) {
        if let Some(field) = object.field("target") {
            self.expect_parameter_ref(field, path);
        }
        if let Some(field) = object.field("from") {
            self.expect_port_ref(
                field,
                "from",
                Some(PortDirection::Output),
                Some(PortKind::Control),
                path,
            );
        }
        if let Some(field) = object.field("amount") {
            self.number_or_quantity(field, path, "amount");
        }
    }

    fn validate_asset_shape(&mut self, object: &Object, path: &[String]) {
        let kind = object
            .field("kind")
            .and_then(|field| field.value.as_symbol())
            .map(str::to_owned);
        if kind.is_some() {
            self.validate_enum_field(
                object,
                path,
                "kind",
                &["audio", "blob", "descriptor", "module"],
            );
        }
        if let Some(field) = object.field("path") {
            self.expect_string(field, path, "path");
        }
        if let Some(field) = object.field("hash") {
            if let Some(hash) = self.expect_string(field, path, "hash") {
                let valid = hash.len() == "sha256:".len() + 64
                    && hash.starts_with("sha256:")
                    && hash["sha256:".len()..]
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
                if !valid {
                    self.push(
                        DiagnosticCode::Hash,
                        "asset.hash must be sha256: followed by 64 lowercase hexadecimal digits",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["hash".into()],
                    );
                }
            }
        }
        if kind.as_deref() == Some("audio") {
            for name in ["format", "rate", "channels", "frames"] {
                if object.field(name).is_none() {
                    self.push(
                        DiagnosticCode::Range,
                        format!("audio asset requires field `{name}`"),
                        Some(object.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                }
            }
            if let Some(field) = object.field("format") {
                self.expect_string(field, path, "format");
            }
            if let Some(field) = object.field("rate") {
                if let Some(rate) = self.quantity(field, &[Unit::Hz, Unit::KHz], path, "rate") {
                    if rate <= BigRational::zero() || !is_integer(&rate) {
                        self.push(
                            DiagnosticCode::Range,
                            "audio asset rate must be a positive integer Hz value",
                            Some(field.value.span),
                            path.to_vec(),
                            vec!["rate".into()],
                        );
                    }
                }
            }
            if let Some(field) = object.field("channels") {
                if let Some(channels) = self.integer(field, path, "channels") {
                    if channels <= BigRational::zero() {
                        self.push(
                            DiagnosticCode::Range,
                            "audio asset channels must be positive",
                            Some(field.value.span),
                            path.to_vec(),
                            vec!["channels".into()],
                        );
                    }
                }
            }
            if let Some(field) = object.field("frames") {
                if let Some(frames) = self.integer(field, path, "frames") {
                    if frames < BigRational::zero() {
                        self.push(
                            DiagnosticCode::Range,
                            "audio asset frames must be nonnegative",
                            Some(field.value.span),
                            path.to_vec(),
                            vec!["frames".into()],
                        );
                    }
                }
            }
        } else {
            for name in ["format", "rate", "channels", "frames"] {
                if let Some(field) = object.field(name) {
                    self.push(
                        DiagnosticCode::UnknownField,
                        format!("asset.{name} is only valid when kind = audio"),
                        Some(field.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                }
            }
        }
    }

    fn validate_audio_shape(&mut self, object: &Object, path: &[String]) {
        if let Some(field) = object.field("asset") {
            self.expect_object_ref(field, "asset", &["asset"], path);
        }
        if let Some(field) = object.field("at") {
            match field.value.kind {
                ValueKind::Quantity {
                    unit: Unit::Q | Unit::S | Unit::Ms,
                    ..
                } => {}
                _ => {
                    self.push_type(field, path, "at", "a q or seconds position");
                }
            }
        }
        if let Some(field) = object.field("source") {
            let valid = self.validate_pair(field, path, "source");
            if valid {
                if let ValueKind::List(items) = &field.value.kind {
                    for item in items {
                        if let Some(frame) =
                            self.quantity_value(item, &[Unit::Frame], path, "source frame")
                        {
                            if frame < BigRational::zero() || !is_integer(&frame) {
                                self.push(
                                    DiagnosticCode::Range,
                                    "audio source frames must be nonnegative integers",
                                    Some(item.span),
                                    path.to_vec(),
                                    vec!["source".into()],
                                );
                            }
                        }
                    }
                }
            }
        }
        if object.field("mode").is_some() {
            self.validate_enum_field(
                object,
                path,
                "mode",
                &["rate", "warp_rate", "warp_preserve"],
            );
        }
        if let Some(field) = object.field("speed") {
            if let Some(speed) = self.number(field, path, "speed") {
                if speed <= BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "audio speed must be positive",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["speed".into()],
                    );
                }
            }
        }
        if let Some(field) = object.field("gain") {
            if let Some(gain) = self.number(field, path, "gain") {
                if gain < BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "audio gain must be nonnegative",
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["gain".into()],
                    );
                }
            }
        }
        for name in ["fade_in", "fade_out"] {
            if let Some(field) = object.field(name) {
                if let Some(fade) = self.quantity(field, &[Unit::S, Unit::Ms], path, name) {
                    if fade < BigRational::zero() {
                        self.push(
                            DiagnosticCode::Range,
                            format!("audio {name} must be nonnegative"),
                            Some(field.value.span),
                            path.to_vec(),
                            vec![name.into()],
                        );
                    }
                }
            }
        }
        if let Some(field) = object.field("reverse") {
            self.expect_boolean(field, path, "reverse");
        }
        if object.field("fade_shape").is_some() {
            self.validate_enum_field(object, path, "fade_shape", &["linear", "equal_power"]);
        }
        if let Some(field) = object.field("track") {
            self.expect_object_ref(field, "track", &["track"], path);
        }
        if let Some(field) = object.field("processor") {
            self.expect_object_ref(field, "processor", &["asset"], path);
        }
        if let Some(field) = object.field("warp") {
            self.validate_pair_list(field, path, "warp");
        }
    }

    fn validate_extension_shape(&mut self, object: &Object, path: &[String]) {
        if let Some(field) = object.field("namespace") {
            self.expect_string(field, path, "namespace");
        }
        if let Some(field) = object.field("schema") {
            self.expect_object_ref(field, "schema", &["asset"], path);
        }
        if let Some(field) = object.field("render_affecting") {
            self.expect_boolean(field, path, "render_affecting");
        }
        if let Some(field) = object.field("data") {
            if !matches!(field.value.kind, ValueKind::Record(_)) {
                self.push_type(field, path, "data", "a record");
            }
        }
    }

    fn validate_pattern_cycles(&mut self) {
        let patterns: BTreeSet<String> = self
            .document
            .objects()
            .filter_map(|(id, object)| (object.kind == "pattern").then_some(id.to_owned()))
            .collect();
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for id in patterns {
            self.visit_pattern(&id, &mut visiting, &mut visited, 0);
        }
    }

    fn visit_pattern(
        &mut self,
        id: &str,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        depth: usize,
    ) {
        if visited.contains(id) {
            return;
        }
        if depth >= crate::syntax::MAX_NESTING_DEPTH {
            self.push(
                DiagnosticCode::ResourceLimit,
                format!(
                    "pattern reference depth exceeds {}",
                    crate::syntax::MAX_NESTING_DEPTH
                ),
                self.document.object(id).map(|object| object.span),
                vec![id.into()],
                vec!["pattern".into()],
            );
            return;
        }
        if !visiting.insert(id.to_owned()) {
            self.push(
                DiagnosticCode::PatternCycle,
                "pattern reference graph contains a cycle",
                self.document.object(id).map(|o| o.span),
                vec![id.into()],
                vec!["pattern".into()],
            );
            return;
        }
        let targets: Vec<String> = self
            .document
            .object(id)
            .map(|pattern| {
                pattern
                    .children
                    .values()
                    .filter(|child| child.kind == "use")
                    .filter_map(|child| {
                        child
                            .field("pattern")
                            .and_then(|field| ref_object_id(&field.value))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for target in targets {
            if self
                .document
                .object(&target)
                .is_some_and(|object| object.kind == "pattern")
            {
                self.visit_pattern(&target, visiting, visited, depth + 1);
            }
        }
        visiting.remove(id);
        visited.insert(id.to_owned());
    }

    fn validate_connections(&mut self) {
        let count = self
            .document
            .objects()
            .filter(|(_, object)| object.kind == "connect")
            .count();
        if count > MAX_SOURCE_CONNECTIONS {
            self.push(
                DiagnosticCode::ResourceLimit,
                "connection count exceeds source limit",
                None,
                Vec::new(),
                Vec::new(),
            );
        }
    }

    fn populate_reference_index(&mut self) {
        let objects: Vec<(String, Object)> = self
            .document
            .objects()
            .map(|(id, object)| (id.to_owned(), object.clone()))
            .collect();
        for (id, object) in objects {
            self.references.insert(
                format!("&{id}"),
                ReferenceTarget::Object {
                    id: id.to_owned(),
                    kind: object.kind.clone(),
                },
            );
            if object.kind == "node" {
                if let Some(descriptor) = self.nodes.get(&id).cloned() {
                    for port in ports_for(descriptor.processor, &descriptor.config) {
                        self.references.insert(
                            format!("&{id}:{}", port.name),
                            ReferenceTarget::Port {
                                node: id.to_owned(),
                                port: port.name.to_owned(),
                                direction: port.direction,
                                kind: port.kind,
                                channels: port.channels,
                            },
                        );
                    }
                    for parameter in parameters_for(descriptor.processor) {
                        self.references.insert(
                            format!("&{id}.params.{}", parameter.name),
                            ReferenceTarget::Parameter {
                                node: id.to_owned(),
                                parameter: parameter.name.to_owned(),
                                unit: parameter.unit,
                                range: parameter.range,
                            },
                        );
                    }
                }
            }
        }
    }

    fn check_curve_target_units(
        &mut self,
        automation: &Object,
        path: &[String],
        target: ReferenceTarget,
    ) {
        let ReferenceTarget::Parameter { unit, .. } = target else {
            return;
        };
        let Some(curve_ref) = automation
            .field("curve")
            .and_then(|field| field.value.reference())
        else {
            return;
        };
        let Some(curve_id) = self
            .document
            .object(
                curve_ref
                    .path
                    .first()
                    .map(String::as_str)
                    .unwrap_or_default(),
            )
            .filter(|object| object.kind == "curve")
            .map(|object| object.id.clone())
        else {
            return;
        };
        let points: Vec<Value> = self
            .document
            .object(&curve_id)
            .and_then(|curve| curve.field("points"))
            .and_then(|field| match &field.value.kind {
                ValueKind::List(items) => Some(items.clone()),
                _ => None,
            })
            .unwrap_or_default();
        for point in &points {
            if let ValueKind::Tuple(items) = &point.kind {
                if let Some(value) = items.get(1) {
                    let compatible = matches!(
                        (unit, value_dimension_of(value)),
                        (ParameterUnit::Dimensionless, Some(CurveDimension::Number))
                            | (ParameterUnit::Seconds, Some(CurveDimension::Seconds))
                            | (ParameterUnit::Hertz, Some(CurveDimension::Hertz))
                            | (ParameterUnit::Decibels, Some(CurveDimension::Decibels))
                    );
                    if !compatible {
                        self.push(
                            DiagnosticCode::Unit,
                            "automation curve values do not match target parameter units",
                            Some(value.span),
                            path.to_vec(),
                            vec!["curve".into()],
                        );
                    }
                }
            }
        }
    }

    fn expect_object_ref(
        &mut self,
        field: &Field,
        name: &str,
        expected: &[&str],
        path: &[String],
    ) -> Option<ReferenceTarget> {
        let Some(reference) = field.value.reference() else {
            self.push_type(field, path, name, "a reference");
            return None;
        };
        if reference.port.is_some() || reference.path.len() != 1 {
            self.push(
                DiagnosticCode::Reference,
                format!("{name} must reference one top-level object"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        }
        let id = &reference.path[0];
        let Some(object) = self.document.object(id) else {
            self.push(
                DiagnosticCode::Reference,
                format!("reference `{id}` does not resolve"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        };
        let object_kind = object.kind.clone();
        if !expected.contains(&object_kind.as_str()) {
            self.push(
                DiagnosticCode::Reference,
                format!(
                    "reference `{id}` has kind {object_kind}, expected {}",
                    expected.join(" or ")
                ),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        }
        Some(ReferenceTarget::Object {
            id: id.clone(),
            kind: object_kind,
        })
    }

    fn expect_port_ref(
        &mut self,
        field: &Field,
        name: &str,
        direction: Option<PortDirection>,
        kind: Option<PortKind>,
        path: &[String],
    ) -> Option<PortDescriptor> {
        let Some(reference) = field.value.reference() else {
            self.push_type(field, path, name, "a port reference");
            return None;
        };
        let Some(port_name) = reference.port.as_deref() else {
            self.push(
                DiagnosticCode::Reference,
                format!("{name} must include a port suffix"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        };
        if reference.path.len() != 1 {
            self.push(
                DiagnosticCode::Reference,
                format!("{name} must reference a top-level node port"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        }
        let node_id = &reference.path[0];
        let Some(descriptor) = self.nodes.get(node_id).cloned() else {
            self.push(
                DiagnosticCode::Reference,
                format!(
                    "port reference `{node_id}:{port_name}` does not resolve to a supported node"
                ),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        };
        let Some(port) = ports_for(descriptor.processor, &descriptor.config)
            .into_iter()
            .find(|port| port.name == port_name)
        else {
            self.push(
                DiagnosticCode::Reference,
                format!("node `{node_id}` has no port `{port_name}`"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        };
        if direction.is_some_and(|expected| expected != port.direction) {
            self.push(
                DiagnosticCode::PortType,
                format!("port `{port_name}` has the wrong direction"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
        }
        if kind.is_some_and(|expected| expected != port.kind) {
            self.push(
                DiagnosticCode::PortType,
                format!("port `{port_name}` has the wrong kind"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
        }
        Some(port)
    }

    fn expect_parameter_ref(&mut self, field: &Field, path: &[String]) -> Option<ReferenceTarget> {
        let Some(reference) = field.value.reference() else {
            self.push_type(field, path, "target", "a node parameter reference");
            return None;
        };
        if reference.port.is_some() || reference.path.len() != 3 || reference.path[1] != "params" {
            self.push(
                DiagnosticCode::Reference,
                "parameter target must be &node.params.parameter",
                Some(field.value.span),
                path.to_vec(),
                vec!["target".into()],
            );
            return None;
        }
        let node_id = &reference.path[0];
        let parameter_name = &reference.path[2];
        let Some(descriptor) = self.nodes.get(node_id).cloned() else {
            self.push(
                DiagnosticCode::Reference,
                "parameter target node does not resolve to a supported node",
                Some(field.value.span),
                path.to_vec(),
                vec!["target".into()],
            );
            return None;
        };
        let Some(parameter) = parameters_for(descriptor.processor)
            .into_iter()
            .find(|parameter| parameter.name == parameter_name)
        else {
            self.push(
                DiagnosticCode::Reference,
                format!("node `{node_id}` has no parameter `{parameter_name}`"),
                Some(field.value.span),
                path.to_vec(),
                vec!["target".into()],
            );
            return None;
        };
        let key = format!("{node_id}.{parameter_name}");
        if !self.automation_writers.insert(key) {
            self.push(
                DiagnosticCode::AutomationWriter,
                "a parameter may have only one global automation writer",
                Some(field.value.span),
                path.to_vec(),
                vec!["target".into()],
            );
        }
        Some(ReferenceTarget::Parameter {
            node: node_id.clone(),
            parameter: parameter_name.clone(),
            unit: parameter.unit,
            range: parameter.range,
        })
    }

    fn record_field<'b>(
        &mut self,
        object: &'b Object,
        name: &str,
        path: &[String],
    ) -> Option<&'b BTreeMap<String, Field>> {
        let field = object.field(name)?;
        let ValueKind::Record(fields) = &field.value.kind else {
            self.push_type(field, path, name, "a record");
            return None;
        };
        Some(fields)
    }

    fn position_q(&mut self, value: &Value, path: &[String], name: &str) -> Option<BigRational> {
        self.position_q_value(value, path, name)
    }

    fn position_q_value(
        &mut self,
        value: &Value,
        path: &[String],
        name: &str,
    ) -> Option<BigRational> {
        match &value.kind {
            ValueKind::Quantity {
                value,
                unit: Unit::Q,
            } => Some(value.clone()),
            ValueKind::Call { function, args } if function == "bar" => {
                if args.len() == 2 {
                    self.number_value(&args[0], path, name);
                    self.number_value(&args[1], path, name);
                } else {
                    self.push(
                        DiagnosticCode::Range,
                        "bar requires two arguments",
                        Some(value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                }
                None
            }
            _ => {
                self.push(
                    DiagnosticCode::Unit,
                    format!("{name} requires a q position"),
                    Some(value.span),
                    path.to_vec(),
                    vec![name.into()],
                );
                None
            }
        }
    }

    fn quantity(
        &mut self,
        field: &Field,
        units: &[Unit],
        path: &[String],
        name: &str,
    ) -> Option<BigRational> {
        self.quantity_value(&field.value, units, path, name)
    }

    fn quantity_value(
        &mut self,
        value: &Value,
        units: &[Unit],
        path: &[String],
        name: &str,
    ) -> Option<BigRational> {
        let ValueKind::Quantity {
            value: number,
            unit,
        } = &value.kind
        else {
            self.push(
                DiagnosticCode::Unit,
                format!("{name} requires an explicit unit"),
                Some(value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        };
        let canonical = match unit {
            Unit::Ms if units.contains(&Unit::S) => {
                number / BigRational::from_integer(BigInt::from(1000))
            }
            Unit::KHz if units.contains(&Unit::Hz) => {
                number * BigRational::from_integer(BigInt::from(1000))
            }
            _ => number.clone(),
        };
        if !units.iter().any(|expected| {
            *expected == *unit
                || (*expected == Unit::S && *unit == Unit::Ms)
                || (*expected == Unit::Hz && *unit == Unit::KHz)
        }) {
            self.push(
                DiagnosticCode::Unit,
                format!(
                    "{name} requires {}",
                    units
                        .iter()
                        .map(|unit| unit.as_str())
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
                Some(value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        }
        Some(canonical)
    }

    fn number(&mut self, field: &Field, path: &[String], name: &str) -> Option<BigRational> {
        self.number_value(&field.value, path, name)
    }

    fn number_value(&mut self, value: &Value, path: &[String], name: &str) -> Option<BigRational> {
        let ValueKind::Number(number) = &value.kind else {
            self.push(
                DiagnosticCode::Unit,
                format!("{name} requires a dimensionless number"),
                Some(value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return None;
        };
        Some(number.clone())
    }

    fn integer(&mut self, field: &Field, path: &[String], name: &str) -> Option<BigRational> {
        let value = self.number(field, path, name)?;
        if !is_integer(&value) {
            self.push(
                DiagnosticCode::Range,
                format!("{name} must be an integer"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            None
        } else {
            Some(value)
        }
    }

    fn integer_value(&mut self, value: &Value, path: &[String], name: &str) -> Option<BigRational> {
        let number = self.number_value(value, path, name)?;
        if !is_integer(&number) {
            self.push(
                DiagnosticCode::Range,
                format!("{name} must be an integer"),
                Some(value.span),
                path.to_vec(),
                vec![name.into()],
            );
            None
        } else {
            Some(number)
        }
    }

    fn number_nonnegative(&mut self, field: &Field, path: &[String], name: &str) -> bool {
        self.number_value(&field.value, path, name)
            .is_some_and(|value| {
                if value < BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        format!("{name} must be nonnegative"),
                        Some(field.value.span),
                        path.to_vec(),
                        vec!["params".into(), name.into()],
                    );
                    false
                } else {
                    true
                }
            })
    }

    fn number_or_quantity(&mut self, field: &Field, path: &[String], name: &str) {
        if !matches!(
            field.value.kind,
            ValueKind::Number(_) | ValueKind::Quantity { .. }
        ) {
            self.push_type(field, path, name, "a number or quantity");
        }
    }

    fn validate_pitch(&mut self, field: &Field, path: &[String]) {
        self.validate_pitch_value(&field.value, path);
    }

    fn validate_pitch_value(&mut self, value: &Value, path: &[String]) {
        match &value.kind {
            ValueKind::Symbol(symbol) => {
                if crate::music::Pitch::parse_spelled(symbol).is_err() {
                    self.push(
                        DiagnosticCode::Range,
                        "pitch symbol must be a valid spelled pitch",
                        Some(value.span),
                        path.to_vec(),
                        vec!["pitch".into()],
                    );
                }
            }
            ValueKind::Quantity {
                unit: Unit::Hz | Unit::KHz,
                value: frequency,
            } => {
                if frequency <= &BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        "pitch frequency must be positive",
                        Some(value.span),
                        path.to_vec(),
                        vec!["pitch".into()],
                    );
                }
            }
            ValueKind::Call { function, args } if function == "key" => {
                if args.len() != 1 || !matches!(args[0].kind, ValueKind::Number(_)) {
                    self.push(
                        DiagnosticCode::Range,
                        "key() requires one integer argument",
                        Some(value.span),
                        path.to_vec(),
                        vec!["pitch".into()],
                    );
                }
            }
            ValueKind::Call { function, args } if function == "ratio" => {
                if args.len() != 2
                    || !matches!(args[0].kind, ValueKind::Number(_))
                    || !matches!(
                        args[1].kind,
                        ValueKind::Quantity {
                            unit: Unit::Hz | Unit::KHz,
                            ..
                        }
                    )
                {
                    self.push(
                        DiagnosticCode::Unit,
                        "ratio() requires a dimensionless ratio and Hz base",
                        Some(value.span),
                        path.to_vec(),
                        vec!["pitch".into()],
                    );
                }
            }
            ValueKind::Call { function, args } if function == "degree" => {
                if args.len() != 2
                    || !matches!(args[0].kind, ValueKind::Number(_))
                    || args[1].reference().is_none()
                {
                    self.push(
                        DiagnosticCode::Reference,
                        "degree() requires an integer index and tuning reference",
                        Some(value.span),
                        path.to_vec(),
                        vec!["pitch".into()],
                    );
                }
            }
            _ => self.push(
                DiagnosticCode::Unit,
                "pitch must be a spelled pitch, key(), degree(), ratio(), or positive Hz quantity",
                Some(value.span),
                path.to_vec(),
                vec!["pitch".into()],
            ),
        }
    }

    fn validate_unit_range_field(
        &mut self,
        object: &Object,
        path: &[String],
        name: &str,
        low: BigRational,
        high: BigRational,
    ) {
        if let Some(field) = object.field(name) {
            self.validate_unit_range_value(&field.value, path, name, low, high);
        }
    }

    fn validate_unit_range_value(
        &mut self,
        value: &Value,
        path: &[String],
        name: &str,
        low: BigRational,
        high: BigRational,
    ) {
        if let Some(number) = self.number_value(value, path, name) {
            if number < low || number > high {
                self.push(
                    DiagnosticCode::Range,
                    format!("{name} is outside its valid range"),
                    Some(value.span),
                    path.to_vec(),
                    vec![name.into()],
                );
            }
        }
    }

    fn validate_seconds(&mut self, object: &Object, path: &[String], name: &str) {
        if let Some(field) = object.field(name) {
            self.validate_seconds_value(&field.value, path, name);
        }
    }

    fn validate_seconds_value(&mut self, value: &Value, path: &[String], name: &str) {
        self.quantity_value(value, &[Unit::S, Unit::Ms], path, name);
    }

    fn validate_positive_integer_field(&mut self, object: &Object, path: &[String], name: &str) {
        if let Some(field) = object.field(name) {
            if let Some(value) = self.integer(field, path, name) {
                if value <= BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        format!("{name} must be positive"),
                        Some(field.value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                }
            }
        }
    }

    fn validate_positive_dimensionless_field(
        &mut self,
        object: &Object,
        path: &[String],
        name: &str,
    ) {
        if let Some(field) = object.field(name) {
            if let Some(value) = self.number(field, path, name) {
                if value <= BigRational::zero() {
                    self.push(
                        DiagnosticCode::Range,
                        format!("{name} must be positive"),
                        Some(field.value.span),
                        path.to_vec(),
                        vec![name.into()],
                    );
                }
            }
        }
    }

    fn validate_quantity_field(
        &mut self,
        object: &Object,
        path: &[String],
        name: &str,
        units: &[Unit],
    ) {
        if let Some(field) = object.field(name) {
            self.quantity(field, units, path, name);
        }
    }

    fn validate_enum_field(
        &mut self,
        object: &Object,
        path: &[String],
        name: &str,
        allowed: &[&str],
    ) {
        if let Some(field) = object.field(name) {
            let Some(symbol) = field.value.as_symbol() else {
                self.push_type(field, path, name, "an enum symbol");
                return;
            };
            if !allowed.contains(&symbol) {
                self.push(
                    DiagnosticCode::Range,
                    format!("{name} must be one of {}", allowed.join(", ")),
                    Some(field.value.span),
                    path.to_vec(),
                    vec![name.into()],
                );
            }
        }
    }

    fn validate_pair(&mut self, field: &Field, path: &[String], name: &str) -> bool {
        let ValueKind::List(items) = &field.value.kind else {
            self.push_type(field, path, name, "a two-item list");
            return false;
        };
        if items.len() != 2 {
            self.push(
                DiagnosticCode::Range,
                format!("{name} must contain two items"),
                Some(field.value.span),
                path.to_vec(),
                vec![name.into()],
            );
            return false;
        }
        true
    }

    fn validate_pair_list(&mut self, field: &Field, path: &[String], name: &str) {
        let ValueKind::List(items) = &field.value.kind else {
            self.push_type(field, path, name, "a list");
            return;
        };
        for item in items {
            if !matches!(item.kind, ValueKind::Tuple(ref tuple) if tuple.len() == 2) {
                self.push(
                    DiagnosticCode::Range,
                    format!("{name} entries must be two-item tuples"),
                    Some(item.span),
                    path.to_vec(),
                    vec![name.into()],
                );
            }
        }
    }

    fn expect_string(&mut self, field: &Field, path: &[String], name: &str) -> Option<String> {
        match &field.value.kind {
            ValueKind::String(value) => Some(value.clone()),
            _ => {
                self.push_type(field, path, name, "a string");
                None
            }
        }
    }

    fn expect_boolean(&mut self, field: &Field, path: &[String], name: &str) -> Option<bool> {
        match field.value.kind {
            ValueKind::Boolean(value) => Some(value),
            _ => {
                self.push_type(field, path, name, "a boolean");
                None
            }
        }
    }

    fn push_type(&mut self, field: &Field, path: &[String], name: &str, expected: &str) {
        self.push(
            DiagnosticCode::Unit,
            format!("{name} must be {expected}"),
            Some(field.value.span),
            path.to_vec(),
            vec![name.into()],
        );
    }

    fn push_type_value(&mut self, value: &Value, path: &[String], expected: &str) {
        self.push(
            DiagnosticCode::Unit,
            format!("value must be {expected}"),
            Some(value.span),
            path.to_vec(),
            Vec::new(),
        );
    }

    fn push(
        &mut self,
        code: DiagnosticCode,
        message: impl Into<String>,
        span: Option<Span>,
        object_path: Vec<String>,
        field_path: Vec<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(code, message, span)
                .object_path(object_path)
                .field_path(field_path),
        );
    }
}

fn count_children(object: &Object) -> usize {
    object
        .children
        .values()
        .map(|child| 1 + count_children(child))
        .sum()
}

fn is_known_kind(kind: &str) -> bool {
    matches!(
        kind,
        "project"
            | "tempo"
            | "meter"
            | "tuning"
            | "pattern"
            | "track"
            | "place"
            | "curve"
            | "automation"
            | "modulate"
            | "asset"
            | "audio"
            | "node"
            | "connect"
            | "region"
            | "extension"
            | "note"
            | "hit"
            | "message"
            | "use"
            | "expression"
            | "override"
            | "insert"
    )
}

fn child_path(parent: &[String], id: &str) -> Vec<String> {
    let mut path = parent.to_vec();
    path.push(id.to_owned());
    path
}

fn field_path(parent: &[String], field: &str) -> Vec<String> {
    let mut path = parent.to_vec();
    path.push(field.to_owned());
    path
}

fn is_integer(value: &BigRational) -> bool {
    value.denom() == &BigInt::from(1)
}

fn value_type(value: &Value) -> ValueType {
    match &value.kind {
        ValueKind::Number(_) => ValueType::Number,
        ValueKind::Quantity { unit, .. } => ValueType::Quantity(*unit),
        ValueKind::String(_) => ValueType::String,
        ValueKind::Symbol(_) => ValueType::Symbol,
        ValueKind::Boolean(_) => ValueType::Boolean,
        ValueKind::Reference(_) => ValueType::Reference,
        ValueKind::Call { .. } => ValueType::Call,
        ValueKind::List(_) => ValueType::List,
        ValueKind::Tuple(_) => ValueType::Tuple,
        ValueKind::Record(_) => ValueType::Record,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurveDimension {
    Number,
    Seconds,
    Hertz,
    QuarterNotes,
    Frames,
    BeatsPerMinute,
    Cents,
    Decibels,
}

fn value_dimension_of(value: &Value) -> Option<CurveDimension> {
    match value.kind {
        ValueKind::Number(_) => Some(CurveDimension::Number),
        ValueKind::Quantity { unit, .. } => Some(match unit {
            Unit::S | Unit::Ms => CurveDimension::Seconds,
            Unit::Hz | Unit::KHz => CurveDimension::Hertz,
            Unit::Q => CurveDimension::QuarterNotes,
            Unit::Frame => CurveDimension::Frames,
            Unit::Bpm => CurveDimension::BeatsPerMinute,
            Unit::Ct => CurveDimension::Cents,
            Unit::Db => CurveDimension::Decibels,
        }),
        _ => None,
    }
}

fn numeric_value(value: &Value) -> Option<BigRational> {
    match &value.kind {
        ValueKind::Number(value) => Some(value.clone()),
        ValueKind::Quantity { value, .. } => Some(value.clone()),
        _ => None,
    }
}

fn reference_key(reference: &Reference) -> String {
    let mut key = String::from("&");
    key.push_str(&reference.path.join("."));
    if let Some(port) = &reference.port {
        key.push(':');
        key.push_str(port);
    }
    key
}

fn ref_object_id(value: &Value) -> Option<String> {
    value.reference().and_then(|reference| {
        (reference.port.is_none() && reference.path.len() == 1).then(|| reference.path[0].clone())
    })
}

fn ports_for(
    processor: ProcessorKind,
    config: &BTreeMap<String, BigRational>,
) -> Vec<PortDescriptor> {
    match processor {
        ProcessorKind::Sine => vec![
            PortDescriptor {
                name: "events",
                direction: PortDirection::Input,
                kind: PortKind::Events,
                channels: 0,
                accepts_multiple: true,
                zero_default: true,
            },
            PortDescriptor {
                name: "out",
                direction: PortDirection::Output,
                kind: PortKind::Audio,
                channels: 1,
                accepts_multiple: false,
                zero_default: true,
            },
        ],
        ProcessorKind::OnePole => {
            let channels = config
                .get("channels")
                .and_then(|value| value.to_u32())
                .unwrap_or(1);
            vec![
                PortDescriptor {
                    name: "in",
                    direction: PortDirection::Input,
                    kind: PortKind::Audio,
                    channels,
                    accepts_multiple: false,
                    zero_default: false,
                },
                PortDescriptor {
                    name: "out",
                    direction: PortDirection::Output,
                    kind: PortKind::Audio,
                    channels,
                    accepts_multiple: false,
                    zero_default: true,
                },
            ]
        }
        ProcessorKind::Pan => vec![
            PortDescriptor {
                name: "in",
                direction: PortDirection::Input,
                kind: PortKind::Audio,
                channels: 1,
                accepts_multiple: false,
                zero_default: false,
            },
            PortDescriptor {
                name: "out",
                direction: PortDirection::Output,
                kind: PortKind::Audio,
                channels: 2,
                accepts_multiple: false,
                zero_default: true,
            },
        ],
        ProcessorKind::Sum => {
            let channels = config
                .get("channels")
                .and_then(|value| value.to_u32())
                .unwrap_or(1);
            vec![
                PortDescriptor {
                    name: "in",
                    direction: PortDirection::Input,
                    kind: PortKind::Audio,
                    channels,
                    accepts_multiple: true,
                    zero_default: true,
                },
                PortDescriptor {
                    name: "out",
                    direction: PortDirection::Output,
                    kind: PortKind::Audio,
                    channels,
                    accepts_multiple: false,
                    zero_default: true,
                },
            ]
        }
    }
}

fn parameters_for(processor: ProcessorKind) -> Vec<ParameterDescriptor> {
    match processor {
        ProcessorKind::Sine => vec![
            ParameterDescriptor {
                name: "attack",
                unit: ParameterUnit::Seconds,
                range: RangePolicy::Error,
            },
            ParameterDescriptor {
                name: "release",
                unit: ParameterUnit::Seconds,
                range: RangePolicy::Error,
            },
            ParameterDescriptor {
                name: "level",
                unit: ParameterUnit::Dimensionless,
                range: RangePolicy::Error,
            },
        ],
        ProcessorKind::OnePole => vec![ParameterDescriptor {
            name: "cutoff",
            unit: ParameterUnit::Hertz,
            range: RangePolicy::Error,
        }],
        ProcessorKind::Pan => vec![ParameterDescriptor {
            name: "pan",
            unit: ParameterUnit::Dimensionless,
            range: RangePolicy::Clamp,
        }],
        ProcessorKind::Sum => Vec::new(),
    }
}

trait TupleItem {
    fn tuple_item(&self, index: usize) -> Value;
}

impl TupleItem for Value {
    fn tuple_item(&self, index: usize) -> Value {
        match &self.kind {
            ValueKind::Tuple(items) => items
                .get(index)
                .cloned()
                .unwrap_or_else(|| Value::new(ValueKind::Number(BigRational::zero()), self.span)),
            _ => Value::new(ValueKind::Number(BigRational::zero()), self.span),
        }
    }
}
