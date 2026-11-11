//! Semantic normalization N(A) (MaaC-1 §20.2).
//!
//! One implementation computes the execution view of an authored document:
//! specified defaults are expanded, execution units made canonical, and pitch
//! letters, `ratio` and `bar` lowered. The editing view is N(A) itself; the
//! execution hash covers [`execution_view`], which drops display labels and
//! the fields added after the hash was defined, at their defaults.
//! Normalization reads the authored document and never changes it.
//!
//! Input and output use the typed syntax-tree JSON of §20.1. Defaults come
//! from the object kinds below, from each graph node's parameter descriptors,
//! and from the resolved libraries that instrument and effect nodes name.

use std::fmt;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{ToPrimitive, Zero};
use serde_json::{json, Map, Value};

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::graph::{EffectProgram, GraphProgram, GraphStage, GraphUnit, InstrumentProgram};
use crate::library::LibrarySet;
use crate::music::{MeterMap, MeterPoint, Pitch};
use crate::plan::Processor;
use crate::processor::{declaration_of, Config, ConfigValue, Declaration, Holds};
use crate::syntax::Unit;

/// Why a document, object or value has no N(A), with a MaaC diagnostic code
/// and the object and field it concerns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizationError {
    pub code: String,
    pub message: String,
    pub object_path: Vec<String>,
    pub field_path: Vec<String>,
}

impl NormalizationError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            object_path: Vec::new(),
            field_path: Vec::new(),
        }
    }

    /// Place an error that names no object at `path`.
    fn within(mut self, path: &[String]) -> Self {
        if self.object_path.is_empty() {
            self.object_path = path.to_vec();
        }
        self
    }

    fn from_diagnostic(diagnostic: Diagnostic) -> Self {
        Self {
            code: diagnostic.code_str().into(),
            message: diagnostic.message,
            object_path: diagnostic.object_path,
            field_path: diagnostic.field_path,
        }
    }

    fn from_diagnostics(diagnostics: Diagnostics) -> Self {
        diagnostics
            .into_vec()
            .into_iter()
            .find(Diagnostic::is_error)
            .map(Self::from_diagnostic)
            .unwrap_or_else(|| Self::new("E_RANGE", "the libraries did not resolve"))
    }
}

impl fmt::Display for NormalizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NormalizationError {}

type Result<T> = std::result::Result<T, NormalizationError>;

fn error(code: &str, message: impl Into<String>) -> NormalizationError {
    NormalizationError::new(code, message)
}

/// The fixed context a document's objects and values normalize in: its
/// meter, its project seed and the libraries its source resolves.
pub struct NormalizationContext<'a> {
    base: &'a Value,
    meter: MeterMap,
    seed: Value,
    source: &'a str,
    libraries: Option<&'a LibrarySet>,
}

impl<'a> NormalizationContext<'a> {
    /// The context of `base`, an authored document in typed syntax-tree JSON.
    /// Its project gives the meter and seed; a document without one (a
    /// library) normalizes in 4/4 with seed 0. `source` names `base` among
    /// `libraries`, which instrument, effect and graph nodes need.
    pub fn new(
        base: &'a Value,
        source: &'a str,
        libraries: Option<&'a LibrarySet>,
    ) -> std::result::Result<Self, NormalizationError> {
        let objects = objects(base)?;
        let (meter, seed) = match objects.values().find(|object| object["kind"] == "project") {
            Some(project) => (
                meter_map(objects, project)?,
                project["fields"]
                    .get("seed")
                    .cloned()
                    .unwrap_or_else(|| number(0)),
            ),
            None => (
                MeterMap::new(vec![MeterPoint::new(BigRational::zero(), 4, 4)])
                    .map_err(|e| error("E_METER_BOUNDARY", e.to_string()))?,
                number(0),
            ),
        };
        Ok(Self {
            base,
            meter,
            seed,
            source,
            libraries,
        })
    }
}

/// N(A) of the context's whole document, labels kept: the editing view.
pub fn normalize_document(
    context: &NormalizationContext<'_>,
) -> std::result::Result<Value, NormalizationError> {
    let mut result = Map::new();
    for (id, object) in objects(context.base)? {
        let path = [id.clone()];
        result.insert(id.clone(), context.object(&path, object, Scope::Top)?);
    }
    Ok(json!({"version": 1, "objects": result}))
}

/// N(A) of `object` at `base_path`, in the context of the base document's
/// enclosing objects. `object` may be an expectation rather than the base's
/// own object: what it leaves out takes the specified default.
pub fn normalize_object(
    context: &NormalizationContext<'_>,
    base_path: &[String],
    object: &Value,
) -> std::result::Result<Value, NormalizationError> {
    let (_, enclosing) = base_path
        .split_last()
        .ok_or_else(|| error("E_REFERENCE", "object path is empty"))?;
    let scope = context.scope_at(enclosing)?;
    context.object(base_path, object, scope)
}

/// `value` written at `field` of the base object at `base_path`, normalized
/// as that field's type: pitches lower to keys and units become canonical.
/// The defaults of the object's other fields are not expanded into it.
pub fn normalize_value(
    context: &NormalizationContext<'_>,
    base_path: &[String],
    field: &[String],
    value: &Value,
) -> std::result::Result<Value, NormalizationError> {
    let object = object_at(context.base, base_path)?;
    let kind = object["kind"].as_str().unwrap_or_default();
    if let [name] = field {
        if matches!(
            (kind, name.as_str()),
            ("chord" | "voicing", "pitches") | ("chord", "pick")
        ) {
            return context.field(kind, name, value, true);
        }
    }
    let pitch = (matches!(kind, "note" | "piece") && field == ["pitch"])
        || (kind == "override" && field == ["set", "pitch"]);
    context.value(value, pitch, true)
}

/// The execution view of an editing view: N(A) without the `label` field of
/// each source object. A `label` inside a record is data and stays.
///
/// The fields added to the language after the execution hash was defined
/// are left out at their defaults, so that earlier documents keep their
/// hash: a note's or chord's `slur` and `slide`, the `rest` of an
/// `after(...)`, a groove's `delay`, a vary's `velocity` and `timing`, a
/// voice's `connected`, a `synth.noise/1` node's `config.per_note`, and a
/// `synth.adsr/1` node's `config.on_join` with the config record it leaves
/// empty.
pub fn execution_view(editing_view: &Value) -> Value {
    fn strip(objects: &mut Map<String, Value>) {
        for object in objects.values_mut() {
            let kind = object["kind"].as_str().unwrap_or_default().to_owned();
            if let Some(fields) = object.get_mut("fields").and_then(Value::as_object_mut) {
                fields.remove("label");
                leave_out_late_defaults(&kind, fields);
            }
            if let Some(children) = object.get_mut("children").and_then(Value::as_object_mut) {
                strip(children);
            }
        }
    }
    let mut view = editing_view.clone();
    if let Some(objects) = view.get_mut("objects").and_then(Value::as_object_mut) {
        strip(objects);
    }
    view
}

fn leave_out_late_defaults(kind: &str, fields: &mut Map<String, Value>) {
    fn remove_at(fields: &mut Map<String, Value>, name: &str, default: &Value) {
        if fields.get(name) == Some(default) {
            fields.remove(name);
        }
    }
    match kind {
        "note" | "chord" => {
            remove_at(fields, "slur", &boolean(false));
            remove_at(fields, "slide", &quantity(0, 1, Unit::S));
            // `after(id)` follows at once.
            if let Some(args) = fields
                .get_mut("at")
                .filter(|at| at["t"] == "call" && at["fn"] == "after")
                .and_then(|at| at.get_mut("args"))
                .and_then(Value::as_array_mut)
            {
                if args.len() == 2 && args[1] == quantity(0, 1, Unit::Q) {
                    args.pop();
                }
            }
        }
        "groove" => remove_at(fields, "delay", &quantity(0, 1, Unit::S)),
        "vary" => {
            remove_at(fields, "velocity", &number(0));
            remove_at(fields, "timing", &quantity(0, 1, Unit::S));
        }
        "voice" => remove_at(fields, "connected", &boolean(false)),
        "node" => {
            let Some(keys) = fields
                .get("type")
                .and_then(|value| value["v"].as_str())
                .and_then(declaration_of)
                .map(Declaration::config_keys)
            else {
                return;
            };
            if !keys.iter().any(|key| key.late) {
                return;
            }
            let Some(config) = fields
                .get_mut("config")
                .and_then(|config| config.get_mut("fields"))
                .and_then(Value::as_object_mut)
            else {
                return;
            };
            for key in keys.iter().filter(|key| key.late) {
                if let Some(default) = key.default {
                    remove_at(config, key.key, &config_value(default));
                }
            }
            // A config made only of late keys was not written before them.
            if config.is_empty() && keys.iter().all(|key| key.late) {
                fields.remove("config");
            }
        }
        _ => {}
    }
}

/// Where an object sits, for the nodes beneath it.
#[derive(Clone, Copy)]
enum Scope<'a> {
    Top,
    Instrument(&'a InstrumentProgram),
    Effect(&'a EffectProgram),
    Graph(&'a GraphProgram, GraphStage),
    Nested,
}

/// Every object kind normalization defines.
const KINDS: &[&str] = &[
    "project",
    "tempo",
    "meter",
    "tuning",
    "pattern",
    "track",
    "place",
    "groove",
    "vary",
    "voicing",
    "vibrato",
    "harmony",
    "voicings",
    "curve",
    "automation",
    "node",
    "connect",
    "region",
    "note",
    "chord",
    "hit",
    "message",
    "expression",
    "use",
    "phrase",
    "override",
    "insert",
    "import",
    "library",
    "instrument",
    "effect",
    "preset",
    "wavetable",
    "sample",
    "voice",
    "shared",
    "piece",
    "control",
    "modulate",
    "asset",
    "audio",
    "extension",
];

impl<'a> NormalizationContext<'a> {
    fn object(&self, path: &[String], object: &Value, scope: Scope<'a>) -> Result<Value> {
        self.object_inner(path, object, scope)
            .map_err(|e| e.within(path))
    }

    fn object_inner(&self, path: &[String], object: &Value, scope: Scope<'a>) -> Result<Value> {
        let kind = object["kind"]
            .as_str()
            .ok_or_else(|| error("E_SYNTAX", "object kind must be a string"))?;
        if !KINDS.contains(&kind) {
            return Err(error(
                "E_CAPABILITY",
                format!("normalization is not defined for object kind `{kind}`"),
            ));
        }
        let constructors = kind != "extension";
        let mut fields = Map::new();
        for (name, field) in object["fields"]
            .as_object()
            .ok_or_else(|| error("E_SYNTAX", "object fields must be a dictionary"))?
        {
            fields.insert(name.clone(), self.field(kind, name, field, constructors)?);
        }
        let id = path.last().map(String::as_str).unwrap_or_default();
        match kind {
            "project" => {
                fields
                    .entry("tail")
                    .or_insert_with(|| quantity(0, 1, Unit::S));
                fields.entry("seed").or_insert_with(|| number(0));
                fields
                    .entry("requires")
                    .or_insert_with(|| json!({"t": "list", "items": []}));
            }
            "note" | "chord" => {
                for (name, value) in [
                    ("velocity", number(1)),
                    ("release_velocity", fraction(1, 2)),
                    ("onset_offset", quantity(0, 1, Unit::S)),
                    ("release_offset", quantity(0, 1, Unit::S)),
                    ("order", number(0)),
                    ("slur", boolean(false)),
                    ("slide", quantity(0, 1, Unit::S)),
                ] {
                    fields.entry(name).or_insert(value);
                }
                // A strum strikes one string every 8 ms with no tilt.
                if fields.contains_key("strum") {
                    fields
                        .entry("stagger")
                        .or_insert_with(|| quantity(1, 125, Unit::S));
                    fields.entry("tilt").or_insert_with(|| number(0));
                }
            }
            // A groove sits on the beat.
            "groove" => {
                fields
                    .entry("delay")
                    .or_insert_with(|| quantity(0, 1, Unit::S));
            }
            // A vary moves nothing it does not name.
            "vary" => {
                fields.entry("velocity").or_insert_with(|| number(0));
                fields
                    .entry("timing")
                    .or_insert_with(|| quantity(0, 1, Unit::S));
            }
            // A vibrato starts at once, at full depth, centered.
            "vibrato" => {
                fields
                    .entry("delay")
                    .or_insert_with(|| quantity(0, 1, Unit::S));
                fields
                    .entry("rise")
                    .or_insert_with(|| quantity(0, 1, Unit::S));
                fields.entry("shape").or_insert_with(|| symbol("centered"));
            }
            "hit" => {
                for (name, value) in [
                    ("velocity", number(1)),
                    ("onset_offset", quantity(0, 1, Unit::S)),
                    ("order", number(0)),
                ] {
                    fields.entry(name).or_insert(value);
                }
            }
            "message" => {
                for (name, value) in [
                    ("onset_offset", quantity(0, 1, Unit::S)),
                    ("order", number(0)),
                ] {
                    fields.entry(name).or_insert(value);
                }
            }
            "place" | "use" => {
                for (name, value) in [
                    ("count", number(1)),
                    ("stretch", number(1)),
                    ("transpose", quantity(0, 1, Unit::Ct)),
                    ("boundary", symbol("spill")),
                ] {
                    fields.entry(name).or_insert(value);
                }
            }
            "audio" => {
                if fields.get("mode").and_then(|mode| mode["v"].as_str()) == Some("rate") {
                    fields.entry("speed").or_insert_with(|| number(1));
                    fields.entry("reverse").or_insert_with(|| boolean(false));
                }
                for (name, value) in [
                    ("gain", number(1)),
                    ("fade_in", quantity(0, 1, Unit::S)),
                    ("fade_out", quantity(0, 1, Unit::S)),
                    ("fade_shape", symbol("linear")),
                ] {
                    fields.entry(name).or_insert(value);
                }
            }
            // A core PCM sample is mono unless it declares two channels; a
            // WAV sample takes its channels from the file.
            "sample" if fields.contains_key("format") => {
                fields.entry("channels").or_insert_with(|| number(1));
            }
            "piece" => {
                fields
                    .entry("pitch")
                    .or_insert_with(|| json!({"t": "call", "fn": "key", "args": [number(60)]}));
                fields.entry("voices").or_insert_with(|| {
                    number(i64::from(crate::kit_instrument::DEFAULT_PIECE_VOICES))
                });
            }
            "effect" => {
                let effect = self.effect(id)?;
                fields
                    .entry("input")
                    .or_insert_with(|| number(i64::from(effect.input_channels)));
            }
            "voice" => {
                fields.entry("connected").or_insert_with(|| boolean(false));
            }
            // An automation in a placement starts at the placement or at the
            // event it names; a global automation always writes its `at`.
            "automation" => {
                fields
                    .entry("at")
                    .or_insert_with(|| quantity(0, 1, Unit::Q));
            }
            "node" => self.node(path, object, &mut fields, scope)?,
            _ => {}
        }
        let child_scope = self.child_scope(id, object, scope)?;
        let mut children = Map::new();
        for (child_id, child) in object["children"]
            .as_object()
            .ok_or_else(|| error("E_SYNTAX", "children must be a dictionary"))?
        {
            let mut child_path = path.to_vec();
            child_path.push(child_id.clone());
            children.insert(
                child_id.clone(),
                self.object(&child_path, child, child_scope)?,
            );
        }
        Ok(json!({"kind": kind, "fields": fields, "children": children}))
    }

    /// The scope of the children of the base object at `path`.
    fn scope_at(&self, path: &[String]) -> Result<Scope<'a>> {
        let mut scope = Scope::Top;
        let mut siblings = objects(self.base)?;
        for id in path {
            let object = siblings
                .get(id)
                .ok_or_else(|| error("E_REFERENCE", "object path does not exist").within(path))?;
            scope = self.child_scope(id, object, scope)?;
            siblings = object["children"]
                .as_object()
                .ok_or_else(|| error("E_SYNTAX", "children must be a dictionary"))?;
        }
        Ok(scope)
    }

    fn child_scope(&self, id: &str, object: &Value, scope: Scope<'a>) -> Result<Scope<'a>> {
        let kind = object["kind"].as_str().unwrap_or_default();
        Ok(match (kind, scope) {
            ("instrument", _) if has_pieces(object) => {
                let libraries = self.libraries("a kit")?;
                if !libraries
                    .kits
                    .iter()
                    .any(|kit| kit.source.file == self.source && kit.source.object == id)
                {
                    return Err(error(
                        "E_REFERENCE",
                        format!("kit `{id}` is not among the resolved libraries"),
                    ));
                }
                Scope::Nested
            }
            ("instrument", _) => Scope::Instrument(self.program(id)?),
            ("effect", _) => Scope::Effect(self.effect(id)?),
            ("shared", Scope::Effect(effect)) => Scope::Graph(&effect.graph, GraphStage::Shared),
            ("voice", Scope::Instrument(program)) => {
                Scope::Graph(&program.voice, GraphStage::Voice)
            }
            ("shared", Scope::Instrument(program)) => Scope::Graph(
                program
                    .shared
                    .as_ref()
                    .ok_or_else(|| error("E_REFERENCE", "shared graph is not resolved"))?,
                GraphStage::Shared,
            ),
            ("voice" | "shared", _) => {
                return Err(error(
                    "E_CAPABILITY",
                    "a graph outside an instrument or effect has no normalization",
                ))
            }
            _ => Scope::Nested,
        })
    }

    fn libraries(&self, what: &str) -> Result<&'a LibrarySet> {
        self.libraries.ok_or_else(|| {
            error(
                "E_CAPABILITY",
                format!("normalizing {what} needs its resolved libraries"),
            )
        })
    }

    fn program(&self, id: &str) -> Result<&'a InstrumentProgram> {
        self.libraries("an instrument")?
            .programs
            .iter()
            .find(|program| program.source.file == self.source && program.source.object == id)
            .ok_or_else(|| {
                error(
                    "E_REFERENCE",
                    format!("instrument `{id}` is not among the resolved libraries"),
                )
            })
    }

    fn effect(&self, id: &str) -> Result<&'a EffectProgram> {
        self.libraries("an effect")?
            .effects
            .iter()
            .find(|effect| effect.source.file == self.source && effect.source.object == id)
            .ok_or_else(|| {
                error(
                    "E_REFERENCE",
                    format!("effect `{id}` is not among the resolved libraries"),
                )
            })
    }

    fn node(
        &self,
        path: &[String],
        object: &Value,
        fields: &mut Map<String, Value>,
        scope: Scope<'a>,
    ) -> Result<()> {
        let mut config = record_fields(fields, "config")?;
        let mut params = record_fields(fields, "params")?;
        let takes_config = match scope {
            Scope::Top => {
                self.top_node(path, object, fields, &mut config, &mut params)?;
                true
            }
            Scope::Graph(graph, stage) => {
                let id = path.last().map(String::as_str).unwrap_or_default();
                graph_node(id, fields, &mut config, &mut params, graph, stage)?
            }
            _ => {
                return Err(error(
                    "E_CAPABILITY",
                    "node is outside a supported graph context",
                ))
            }
        };
        if takes_config {
            fields.insert("config".into(), record(config));
        }
        fields.insert("params".into(), record(params));
        Ok(())
    }

    /// A node of the composition: an instrument or effect instance, a
    /// production processor or a core processor.
    fn top_node(
        &self,
        path: &[String],
        object: &Value,
        fields: &Map<String, Value>,
        config: &mut Map<String, Value>,
        params: &mut Map<String, Value>,
    ) -> Result<()> {
        if fields.contains_key("instrument") || fields.contains_key("effect") {
            let libraries = self.libraries("an instrument or effect node")?;
            let node = syntax_object(path, object)?;
            let instance = libraries
                .resolve_instance(self.source, &node)
                .map_err(NormalizationError::from_diagnostics)?;
            // A kit instance's pieces declare their voices; an effect has none.
            if !instance.kit && instance.effect_input.is_none() {
                config
                    .entry("voices")
                    .or_insert_with(|| number(i64::from(instance.voices)));
            }
            for (name, value) in &instance.params {
                let spec = libraries
                    .instance_control_spec(&instance, name)
                    .ok_or_else(|| {
                        error("E_REFERENCE", format!("control `{name}` has no descriptor"))
                    })?;
                params
                    .entry(name.clone())
                    .or_insert_with(|| rational(value, graph_unit(spec.unit)));
            }
            return Ok(());
        }
        let processor = fields
            .get("type")
            .and_then(|value| value["v"].as_str())
            .ok_or_else(|| error("E_RANGE", "node.type is required"))?;
        if processor.starts_with("fx.") {
            let node = crate::semantic::production_node(&syntax_object(path, object)?)
                .map_err(NormalizationError::from_diagnostic)?;
            production_defaults(&node, config, params);
            return Ok(());
        }
        match processor {
            "core.sine/1" => {
                config.entry("voices").or_insert_with(|| number(64));
                for (name, value) in [
                    ("attack", quantity(1, 200, Unit::S)),
                    ("release", quantity(2, 25, Unit::S)),
                    ("level", fraction(1, 5)),
                ] {
                    params.entry(name).or_insert(value);
                }
            }
            "core.onepole/1" => {
                params
                    .entry("cutoff")
                    .or_insert_with(|| quantity(1000, 1, Unit::Hz));
            }
            "core.gain/1" => {
                params.entry("gain").or_insert_with(|| number(1));
            }
            "core.fader/1" => {
                params
                    .entry("level")
                    .or_insert_with(|| quantity(0, 1, Unit::Db));
            }
            "core.pan/1" => {
                params.entry("pan").or_insert_with(|| number(0));
            }
            "core.noise/1" => {
                config.entry("seed").or_insert_with(|| self.seed.clone());
            }
            "core.kit/1" => {
                config.entry("voices").or_insert_with(|| number(64));
                params.entry("level").or_insert_with(|| number(1));
            }
            "core.lfo/1" => {
                config.entry("wave").or_insert_with(|| symbol("sine"));
                config.entry("phase").or_insert_with(|| number(0));
            }
            "core.constant/1" => {
                params.entry("value").or_insert_with(|| number(0));
            }
            "core.matrix/1" | "core.delay/1" | "core.sum/1" => {}
            other => {
                return Err(error(
                    "E_CAPABILITY",
                    format!("processor `{other}` has no normalization"),
                ))
            }
        }
        Ok(())
    }

    fn field(&self, kind: &str, name: &str, value: &Value, constructors: bool) -> Result<Value> {
        // One picked position is a list of one.
        if kind == "chord" && name == "pick" && value["t"] != "list" {
            let position = self.value(value, false, constructors)?;
            return Ok(json!({"t": "list", "items": [position]}));
        }
        // Each spelled chord or voicing pitch lowers like a note's `pitch`.
        if matches!(kind, "chord" | "voicing") && name == "pitches" && value["t"] == "list" {
            let items = value["items"]
                .as_array()
                .ok_or_else(|| error("E_SYNTAX", "items must be an array"))?
                .iter()
                .map(|item| self.value(item, true, constructors))
                .collect::<Result<Vec<_>>>()?;
            return Ok(json!({"t": "list", "items": items}));
        }
        let pitch = (matches!(kind, "note" | "piece") && name == "pitch")
            || (kind == "override" && name == "set");
        self.value(value, pitch, constructors)
    }

    fn value(&self, value: &Value, pitch: bool, constructors: bool) -> Result<Value> {
        if pitch && matches!(value["t"].as_str(), Some("symbol" | "string")) {
            if let Some(text) = value["v"].as_str() {
                let key = match Pitch::parse_spelled(text)
                    .map_err(|e| error("E_RANGE", e.to_string()))?
                {
                    Pitch::Spelled(spelling) => spelling
                        .key()
                        .map_err(|e| error("E_RANGE", e.to_string()))?,
                    _ => return Err(error("E_RANGE", "expected a spelled pitch")),
                };
                return Ok(json!({"t": "call", "fn": "key", "args": [number(key)]}));
            }
        }
        match value["t"].as_str() {
            Some("quantity") if value["u"] == "ms" => Ok(rational(
                &(typed_rational(value)? / BigRational::from_integer(1000.into())),
                Some(Unit::S),
            )),
            Some("quantity") if value["u"] == "kHz" => Ok(rational(
                &(typed_rational(value)? * BigRational::from_integer(1000.into())),
                Some(Unit::Hz),
            )),
            Some("call") if constructors && value["fn"] == "bar" => {
                let args = call_args(value, 2, "bar")?;
                let bar = typed_rational(&args[0])?;
                if !bar.is_integer() {
                    return Err(error("E_RANGE", "bar number must be an integer"));
                }
                let bar = bar
                    .to_integer()
                    .to_i64()
                    .ok_or_else(|| error("E_RESOURCE_LIMIT", "bar number is out of range"))?;
                let q = self
                    .meter
                    .bar_to_q(bar, &typed_rational(&args[1])?)
                    .map_err(|e| error("E_RANGE", e.to_string()))?;
                Ok(rational(&q, Some(Unit::Q)))
            }
            Some("call") if constructors && value["fn"] == "ratio" => {
                let args = call_args(value, 2, "ratio")?;
                let frequency = match args[1]["u"].as_str() {
                    Some("Hz") if args[1]["t"] == "quantity" => typed_rational(&args[1])?,
                    Some("kHz") if args[1]["t"] == "quantity" => {
                        typed_rational(&args[1])? * BigRational::from_integer(1000.into())
                    }
                    _ => return Err(error("E_UNIT", "ratio requires a frequency reference")),
                };
                Ok(rational(
                    &(typed_rational(&args[0])? * frequency),
                    Some(Unit::Hz),
                ))
            }
            Some("record") => {
                let mut fields = Map::new();
                for (name, child) in value["fields"]
                    .as_object()
                    .ok_or_else(|| error("E_SYNTAX", "record fields must be a dictionary"))?
                {
                    fields.insert(
                        name.clone(),
                        self.value(child, pitch && name == "pitch", constructors)?,
                    );
                }
                Ok(record(fields))
            }
            Some(tag @ ("list" | "tuple")) => {
                let items = value["items"]
                    .as_array()
                    .ok_or_else(|| error("E_SYNTAX", "items must be an array"))?
                    .iter()
                    .map(|item| self.value(item, false, constructors))
                    .collect::<Result<Vec<_>>>()?;
                Ok(json!({"t": tag, "items": items}))
            }
            Some("call") => {
                let mut args = value["args"]
                    .as_array()
                    .ok_or_else(|| error("E_SYNTAX", "call args must be an array"))?
                    .iter()
                    .map(|arg| self.value(arg, false, constructors))
                    .collect::<Result<Vec<_>>>()?;
                // `after(id)` follows `id` with no rest.
                if constructors && value["fn"] == "after" && args.len() == 1 {
                    args.push(quantity(0, 1, Unit::Q));
                }
                Ok(json!({"t": "call", "fn": value["fn"].clone(), "args": args}))
            }
            _ => Ok(value.clone()),
        }
    }
}

/// A node of an instrument's or effect's graph. Config defaults are those of
/// its own `type` and config, so an expectation that leaves one out means the
/// default; params expand from the resolved node's descriptors. Returns
/// whether the node takes config.
fn graph_node(
    id: &str,
    fields: &Map<String, Value>,
    config: &mut Map<String, Value>,
    params: &mut Map<String, Value>,
    graph: &GraphProgram,
    stage: GraphStage,
) -> Result<bool> {
    let node = graph
        .nodes
        .iter()
        .find(|node| node.id == id)
        .ok_or_else(|| {
            error(
                "E_REFERENCE",
                format!("graph node `{id}` is not in its resolved graph"),
            )
        })?;
    let processor = fields
        .get("type")
        .and_then(|value| value["v"].as_str())
        .ok_or_else(|| error("E_RANGE", "node.type is required"))?;
    let declaration = node.processor.declaration();
    if processor != declaration.identity {
        return Err(error(
            "E_REFERENCE",
            format!(
                "graph node `{id}` is a {processor} here but resolved as a {}",
                declaration.identity
            ),
        ));
    }
    expand_graph_config_defaults(declaration, config);
    for param in declaration.params {
        let spec = param.spec(stage);
        params
            .entry(param.name)
            .or_insert_with(|| rational(&spec.default, graph_unit(spec.unit)));
    }
    Ok(writes_graph_config(declaration))
}

/// Fill in the config defaults of a graph node with this declaration.
pub(crate) fn expand_graph_config_defaults(
    declaration: &Declaration,
    config: &mut Map<String, Value>,
) {
    for key in declaration.config_keys() {
        if let Some(default) = key.default {
            config
                .entry(key.key)
                .or_insert_with(|| config_value(default));
        }
        if key.holds == Holds::Zones {
            crate::sample_instrument::expand_zone_defaults(config);
        }
    }
}

fn config_value(value: ConfigValue) -> Value {
    match value {
        ConfigValue::Number(value) => number(value),
        ConfigValue::Boolean(value) => boolean(value),
        ConfigValue::Symbol(value) => symbol(value),
    }
}

/// Whether N(A) writes a `config` record for a graph node with this
/// declaration.
pub(crate) fn writes_graph_config(declaration: &Declaration) -> bool {
    declaration.config != Config::None
}

/// The defaults of a production processor (`fx.*`), as lowered.
fn production_defaults(
    node: &crate::plan::Node,
    config: &mut Map<String, Value>,
    params: &mut Map<String, Value>,
) {
    match &node.processor {
        Processor::Compressor {
            sidechain_channels, ..
        } => {
            config.entry("detector").or_insert_with(|| {
                symbol(if sidechain_channels.is_some() {
                    "external"
                } else {
                    "internal"
                })
            });
        }
        Processor::Reverb { .. } => {
            config
                .entry("predelay")
                .or_insert_with(|| quantity(0, 1, Unit::S));
            config.entry("damping").or_insert_with(|| fraction(1, 2));
        }
        Processor::Limiter { .. } => {
            config
                .entry("lookahead")
                .or_insert_with(|| quantity(3, 2000, Unit::S));
        }
        _ => {}
    }
    for (name, value) in &node.params {
        let unit = match &node.processor {
            Processor::Eq { .. } => match name.as_str() {
                "frequency" => Some(Unit::Hz),
                "gain" => Some(Unit::Db),
                _ => None,
            },
            Processor::Compressor { .. } => match name.as_str() {
                "attack" | "release" => Some(Unit::S),
                "threshold" | "knee" | "makeup" => Some(Unit::Db),
                _ => None,
            },
            Processor::Reverb { .. } if name == "decay" => Some(Unit::S),
            Processor::Limiter { .. } => match name.as_str() {
                "release" => Some(Unit::S),
                "gain" | "ceiling" => Some(Unit::Db),
                _ => None,
            },
            _ => None,
        };
        params
            .entry(name.clone())
            .or_insert_with(|| rational(value, unit));
    }
}

/// A spanless syntax object for the library and production resolvers.
fn syntax_object(path: &[String], object: &Value) -> Result<crate::syntax::Object> {
    let id = path
        .last()
        .ok_or_else(|| error("E_REFERENCE", "object path is empty"))?;
    let document = crate::syntax::document_from_syntax_json_value(
        &json!({"version": 1, "objects": {id.as_str(): object}}),
    )
    .map_err(|message| error("E_SYNTAX", message))?;
    document
        .objects
        .get(id)
        .cloned()
        .ok_or_else(|| error("E_SYNTAX", "object did not convert"))
}

fn has_pieces(object: &Value) -> bool {
    object["children"]
        .as_object()
        .is_some_and(|children| children.values().any(|child| child["kind"] == "piece"))
}

fn objects(tree: &Value) -> Result<&Map<String, Value>> {
    tree["objects"]
        .as_object()
        .ok_or_else(|| error("E_SYNTAX", "objects must be a dictionary"))
}

fn object_at<'v>(tree: &'v Value, path: &[String]) -> Result<&'v Value> {
    let mut siblings = objects(tree)?;
    let mut object = None;
    for id in path {
        let next = siblings
            .get(id)
            .ok_or_else(|| error("E_REFERENCE", "object path does not exist").within(path))?;
        object = Some(next);
        siblings = next["children"]
            .as_object()
            .ok_or_else(|| error("E_SYNTAX", "children must be a dictionary"))?;
    }
    object.ok_or_else(|| error("E_REFERENCE", "object path is empty"))
}

fn meter_map(objects: &Map<String, Value>, project: &Value) -> Result<MeterMap> {
    let meter_id = project["fields"]["meter"]["path"]
        .as_array()
        .and_then(|path| path.first())
        .and_then(Value::as_str)
        .ok_or_else(|| error("E_REFERENCE", "project meter reference is missing"))?;
    let meter = objects
        .get(meter_id)
        .ok_or_else(|| error("E_REFERENCE", "project meter object is missing"))?;
    let points = meter["fields"]["points"]["items"]
        .as_array()
        .ok_or_else(|| error("E_RANGE", "meter points are missing"))?;
    let mut result = Vec::new();
    for point in points {
        let items = point["items"]
            .as_array()
            .ok_or_else(|| error("E_RANGE", "meter point must be a tuple"))?;
        if items.len() != 3 {
            return Err(error("E_RANGE", "meter point must have three values"));
        }
        if items[0]["u"] != "q" {
            return Err(error("E_UNIT", "meter anchors require q positions"));
        }
        let integer = |value: &Value| -> Result<u32> {
            let value = typed_rational(value)?;
            if !value.is_integer() {
                return Err(error("E_RANGE", "meter signature requires integers"));
            }
            value
                .to_integer()
                .to_u32()
                .ok_or_else(|| error("E_RANGE", "meter signature is out of range"))
        };
        result.push(MeterPoint::new(
            typed_rational(&items[0])?,
            integer(&items[1])?,
            integer(&items[2])?,
        ));
    }
    MeterMap::new(result).map_err(|e| error("E_METER_BOUNDARY", e.to_string()))
}

fn record_fields(fields: &mut Map<String, Value>, name: &str) -> Result<Map<String, Value>> {
    match fields.remove(name) {
        None => Ok(Map::new()),
        Some(value) if value["t"] == "record" => value["fields"]
            .as_object()
            .cloned()
            .ok_or_else(|| error("E_SYNTAX", "record fields must be a dictionary")),
        Some(_) => Err(error("E_UNIT", format!("{name} must be a record"))),
    }
}

fn call_args<'v>(value: &'v Value, count: usize, name: &str) -> Result<&'v Vec<Value>> {
    let args = value["args"]
        .as_array()
        .ok_or_else(|| error("E_SYNTAX", format!("{name} args must be an array")))?;
    if args.len() != count {
        return Err(error(
            "E_RANGE",
            format!("{name} requires {count} arguments"),
        ));
    }
    Ok(args)
}

fn typed_rational(value: &Value) -> Result<BigRational> {
    let part = |name: &str| -> Result<BigInt> {
        value[name]
            .as_str()
            .ok_or_else(|| error("E_SYNTAX", "a rational's parts must be strings"))?
            .parse::<BigInt>()
            .map_err(|_| error("E_SYNTAX", "invalid rational"))
    };
    let denominator = part("d")?;
    if denominator <= BigInt::zero() {
        return Err(error("E_RANGE", "rational denominator must be positive"));
    }
    Ok(BigRational::new(part("n")?, denominator))
}

fn graph_unit(unit: GraphUnit) -> Option<Unit> {
    match unit {
        GraphUnit::Dimensionless => None,
        GraphUnit::Hertz => Some(Unit::Hz),
        GraphUnit::Seconds => Some(Unit::S),
        GraphUnit::Quarter => Some(Unit::Q),
    }
}

fn rational(value: &BigRational, unit: Option<Unit>) -> Value {
    match unit {
        Some(unit) => json!({
            "t": "quantity",
            "n": value.numer().to_string(),
            "d": value.denom().to_string(),
            "u": unit.as_str(),
        }),
        None => json!({
            "t": "number",
            "n": value.numer().to_string(),
            "d": value.denom().to_string(),
        }),
    }
}

fn number(value: i64) -> Value {
    rational(&BigRational::from_integer(value.into()), None)
}

fn fraction(numerator: i64, denominator: i64) -> Value {
    rational(
        &BigRational::new(numerator.into(), denominator.into()),
        None,
    )
}

fn quantity(numerator: i64, denominator: i64, unit: Unit) -> Value {
    rational(
        &BigRational::new(numerator.into(), denominator.into()),
        Some(unit),
    )
}

fn symbol(value: &str) -> Value {
    json!({"t": "symbol", "v": value})
}

fn boolean(value: bool) -> Value {
    json!({"t": "boolean", "v": value})
}

fn record(fields: Map<String, Value>) -> Value {
    json!({"t": "record", "fields": fields})
}
