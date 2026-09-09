//! Explicit production input identities; canonical serialization is shared by
//! source and render-context hashing. This module performs no I/O.

use std::{collections::BTreeMap, fmt};

use num_rational::BigRational;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::graph::{
    parameter_descriptor_for_stage, GraphProcessor, GraphProgram, GraphStage, GraphUnit,
    InstrumentProgram,
};
use crate::music::{MeterMap, MeterPoint, Pitch};
use crate::plan::{Plan, Processor};
use crate::syntax::{Document, Object, Unit, Value, ValueKind};

use serde_json::{json, Map, Value as JsonValue};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityError(pub String);

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IdentityError {}

/// MaaC canonical JSON: scalar-key ordering and lowercase control escapes.
/// Floating-point JSON numbers are rejected; precision-bearing source values
/// must use tagged rationals, and numerical environment values use strings.
pub fn canonical_json_bytes(value: &JsonValue) -> Result<Vec<u8>, IdentityError> {
    fn string(text: &str, output: &mut Vec<u8>) {
        output.push(b'"');
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in text.bytes() {
            match byte {
                b'"' | b'\\' => {
                    output.push(b'\\');
                    output.push(byte);
                }
                0..=31 => output.extend_from_slice(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 15)],
                ]),
                _ => output.push(byte),
            }
        }
        output.push(b'"');
    }
    fn write(value: &JsonValue, output: &mut Vec<u8>, depth: usize) -> Result<(), IdentityError> {
        if depth > 256 || output.len() > MAX_CANONICAL_BYTES {
            return Err(error("canonical JSON resource limit exceeded"));
        }
        match value {
            JsonValue::Null => output.extend_from_slice(b"null"),
            JsonValue::Bool(true) => output.extend_from_slice(b"true"),
            JsonValue::Bool(false) => output.extend_from_slice(b"false"),
            JsonValue::Number(number) if number.is_i64() || number.is_u64() => output.extend_from_slice(number.to_string().as_bytes()),
            JsonValue::Number(_) => return Err(error("canonical JSON forbids floating-point numbers; use exact strings or tagged rationals")),
            JsonValue::String(text) => string(text, output),
            JsonValue::Array(items) => {
                output.push(b'[');
                for (index, item) in items.iter().enumerate() { if index > 0 { output.push(b','); } write(item, output, depth + 1)?; }
                output.push(b']');
            }
            JsonValue::Object(fields) => {
                output.push(b'{');
                let sorted: BTreeMap<_, _> = fields.iter().collect();
                for (index, (name, value)) in sorted.into_iter().enumerate() {
                    if index > 0 { output.push(b','); }
                    string(name, output); output.push(b':'); write(value, output, depth + 1)?;
                }
                output.push(b'}');
            }
        }
        if output.len() > MAX_CANONICAL_BYTES {
            return Err(error("canonical JSON resource limit exceeded"));
        }
        Ok(())
    }
    let mut bytes = Vec::new();
    write(value, &mut bytes, 0)?;
    Ok(bytes)
}

const MAX_CANONICAL_BYTES: usize = 128 * 1024 * 1024;
pub const EXECUTION_ALGORITHM: &str = "maac.execution.sha256/1";
pub const RENDER_ALGORITHM: &str = "maac.production.render.sha256/1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIdentity {
    pub algorithm: String,
    pub execution_hash: String,
    /// Additional exact syntax-input fingerprint; not a replacement for the
    /// semantically normalized execution hash. Source labels are retained here.
    pub source_input_hash: String,
    /// Compact normalized entry-source graph, retained as independently
    /// checkable hash evidence. Imported source bodies are not inlined.
    pub normalized_source_json: String,
}

impl ExecutionIdentity {
    pub fn validate(&self) -> Result<(), IdentityError> {
        if self.algorithm != EXECUTION_ALGORITHM {
            return Err(error("unknown execution identity algorithm"));
        }
        if self.normalized_source_json.len() > MAX_CANONICAL_BYTES {
            return Err(error("normalized source is too large"));
        }
        let source: JsonValue =
            serde_json::from_str(&self.normalized_source_json).map_err(|e| error(e.to_string()))?;
        let canonical = canonical_json_bytes(&source)?;
        if canonical != self.normalized_source_json.as_bytes()
            || digest(&canonical) != self.execution_hash
        {
            return Err(error(
                "execution identity does not match canonical source evidence",
            ));
        }
        if source.get("version") != Some(&json!(1))
            || !source.get("objects").is_some_and(JsonValue::is_object)
        {
            return Err(error("execution identity requires a MaaC/1 source graph"));
        }
        if !valid_digest(&self.source_input_hash) {
            return Err(error("invalid source input SHA-256"));
        }
        Ok(())
    }
}

/// Normalize an accepted entry document using its compiled plan's resolved
/// processor descriptors. Call only after source/bundle/capability validation;
/// the original document MUST still include imports and production extensions.
/// The plan supplies exact defaults and local instrument descriptor context,
/// never the compact source structure or expanded event representation.
pub fn execution_identity(
    document: &Document,
    plan: &Plan,
) -> Result<ExecutionIdentity, IdentityError> {
    if document.version != 1 {
        return Err(error("execution identity supports MaaC/1 only"));
    }
    let meter = meter_map(document)?;
    let normalizer = Normalizer { plan, meter };
    let mut objects = Map::new();
    for (id, object) in &document.objects {
        objects.insert(id.clone(), normalizer.object(object, Scope::Top)?);
    }
    let normalized = json!({"version":1,"objects":objects});
    let bytes = canonical_json_bytes(&normalized)?;
    Ok(ExecutionIdentity {
        algorithm: EXECUTION_ALGORITHM.into(),
        execution_hash: digest(&bytes),
        source_input_hash: digest(&canonical_json_bytes(&document.to_syntax_json_value())?),
        normalized_source_json: String::from_utf8(bytes).map_err(|e| error(e.to_string()))?,
    })
}

/// Hash a complete caller-supplied render context alongside normalized source
/// identity and the exact standalone plan artifact. Required records cannot be
/// omitted; extra provenance fields are retained. Selection must be a record
/// keyed by stable target IDs, making target order immaterial.
pub fn render_key(
    identity: &ExecutionIdentity,
    plan_bytes: &[u8],
    context: &JsonValue,
) -> Result<String, IdentityError> {
    identity.validate()?;
    let fields = context
        .as_object()
        .ok_or_else(|| error("render context must be an object"))?;
    for key in [
        "selection",
        "dependencies",
        "processors",
        "engine",
        "converter",
        "dither",
        "analyzer",
        "interval",
    ] {
        if !fields.get(key).is_some_and(JsonValue::is_object) {
            return Err(error(format!("render context requires object `{key}`")));
        }
    }
    if fields["selection"]
        .as_object()
        .is_none_or(|targets| targets.is_empty())
    {
        return Err(error("render selection must be nonempty"));
    }
    let value = json!({"algorithm":RENDER_ALGORITHM,"execution_hash":identity.execution_hash,"plan_hash":digest(plan_bytes),"context":context});
    Ok(digest(&canonical_json_bytes(&value)?))
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn valid_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn error(message: impl Into<String>) -> IdentityError {
    IdentityError(message.into())
}
fn rational(value: &BigRational, unit: Option<Unit>) -> JsonValue {
    match unit {
        Some(unit) => {
            json!({"t":"quantity","n":value.numer().to_string(),"d":value.denom().to_string(),"u":unit.as_str()})
        }
        None => json!({"t":"number","n":value.numer().to_string(),"d":value.denom().to_string()}),
    }
}
fn number(value: i64) -> JsonValue {
    rational(&BigRational::from_integer(value.into()), None)
}
fn quantity(n: i64, d: i64, unit: Unit) -> JsonValue {
    rational(&BigRational::new(n.into(), d.into()), Some(unit))
}
fn symbol(value: &str) -> JsonValue {
    json!({"t":"symbol","v":value})
}
fn record(fields: Map<String, JsonValue>) -> JsonValue {
    json!({"t":"record","fields":fields})
}
fn graph_unit(unit: GraphUnit) -> Option<Unit> {
    match unit {
        GraphUnit::Dimensionless => None,
        GraphUnit::Hertz => Some(Unit::Hz),
        GraphUnit::Seconds => Some(Unit::S),
    }
}
fn record_fields(
    fields: &mut Map<String, JsonValue>,
    key: &str,
) -> Result<Map<String, JsonValue>, IdentityError> {
    match fields.remove(key) {
        None => Ok(Map::new()),
        Some(value) => value
            .get("fields")
            .and_then(JsonValue::as_object)
            .cloned()
            .ok_or_else(|| error(format!("{key} must be a tagged record"))),
    }
}

#[derive(Clone, Copy)]
enum Scope<'a> {
    Top,
    Instrument(&'a InstrumentProgram),
    Graph(&'a GraphProgram, GraphStage),
    Nested,
}

struct Normalizer<'a> {
    plan: &'a Plan,
    meter: MeterMap,
}

impl Normalizer<'_> {
    fn value(
        &self,
        value: &Value,
        pitch: bool,
        constructors: bool,
    ) -> Result<JsonValue, IdentityError> {
        if pitch {
            if let ValueKind::Symbol(text) | ValueKind::String(text) = &value.kind {
                let key = match Pitch::parse_spelled(text).map_err(|e| error(e.to_string()))? {
                    Pitch::Spelled(spelling) => spelling.key().map_err(|e| error(e.to_string()))?,
                    _ => return Err(error("unexpected spelled-pitch representation")),
                };
                return Ok(json!({"t":"call","fn":"key","args":[number(key)]}));
            }
        }
        match &value.kind {
            ValueKind::Quantity {
                value,
                unit: Unit::Ms,
            } => Ok(rational(
                &(value / BigRational::from_integer(1000.into())),
                Some(Unit::S),
            )),
            ValueKind::Quantity {
                value,
                unit: Unit::KHz,
            } => Ok(rational(
                &(value * BigRational::from_integer(1000.into())),
                Some(Unit::Hz),
            )),
            ValueKind::Call { function, args } if constructors && function == "bar" => {
                if args.len() != 2 {
                    return Err(error("bar requires two arguments"));
                }
                let bar = exact_number(&args[0])?
                    .to_integer()
                    .to_i64()
                    .ok_or_else(|| error("bar number out of range"))?;
                if !exact_number(&args[0])?.is_integer() {
                    return Err(error("bar number must be an integer"));
                }
                let q = self
                    .meter
                    .bar_to_q(bar, exact_number(&args[1])?)
                    .map_err(|e| error(e.to_string()))?;
                Ok(rational(&q, Some(Unit::Q)))
            }
            ValueKind::Call { function, args } if constructors && function == "ratio" => {
                if args.len() != 2 {
                    return Err(error("ratio requires two arguments"));
                }
                let hz = exact_frequency(&args[1])?;
                Ok(rational(&(exact_number(&args[0])? * hz), Some(Unit::Hz)))
            }
            ValueKind::Call { function, args } => Ok(
                json!({"t":"call","fn":function,"args":args.iter().map(|v| self.value(v,false,constructors)).collect::<Result<Vec<_>,_>>()?}),
            ),
            ValueKind::List(items) | ValueKind::Tuple(items) => Ok(
                json!({"t":if matches!(value.kind,ValueKind::List(_)) {"list"} else {"tuple"},"items":items.iter().map(|v|self.value(v,false,constructors)).collect::<Result<Vec<_>,_>>()?}),
            ),
            ValueKind::Record(fields) => {
                let mut result = Map::new();
                for (name, field) in fields {
                    result.insert(
                        name.clone(),
                        self.value(&field.value, pitch && name == "pitch", constructors)?,
                    );
                }
                Ok(record(result))
            }
            _ => Ok(value.to_syntax_json_value()),
        }
    }

    fn object(&self, object: &Object, scope: Scope<'_>) -> Result<JsonValue, IdentityError> {
        let known = matches!(
            object.kind.as_str(),
            "project"
                | "tempo"
                | "meter"
                | "tuning"
                | "pattern"
                | "track"
                | "place"
                | "curve"
                | "automation"
                | "node"
                | "connect"
                | "region"
                | "note"
                | "use"
                | "override"
                | "insert"
                | "import"
                | "instrument"
                | "preset"
                | "wavetable"
                | "voice"
                | "shared"
                | "control"
                | "modulate"
                | "asset"
                | "extension"
        );
        if !known {
            return Err(error(format!(
                "normalization is not defined for object kind `{}`",
                object.kind
            )));
        }
        let constructors = object.kind != "extension";
        let mut fields = Map::new();
        for (name, field) in &object.fields {
            if name == "label" {
                continue;
            }
            let pitch = (object.kind == "note" && name == "pitch")
                || (object.kind == "override" && name == "set");
            fields.insert(name.clone(), self.value(&field.value, pitch, constructors)?);
        }
        let mut child_scope = Scope::Nested;
        match object.kind.as_str() {
            "project" => {
                fields
                    .entry("tail")
                    .or_insert_with(|| quantity(0, 1, Unit::S));
                fields.entry("seed").or_insert_with(|| number(0));
                fields
                    .entry("requires")
                    .or_insert_with(|| json!({"t":"list","items":[]}));
            }
            "tuning" => {
                fields.entry("reference_index").or_insert_with(|| number(0));
                fields
                    .entry("reference_frequency")
                    .or_insert_with(|| quantity(440, 1, Unit::Hz));
            }
            "note" => {
                for (name, value) in [
                    ("velocity", number(1)),
                    (
                        "release_velocity",
                        rational(&BigRational::new(1.into(), 2.into()), None),
                    ),
                    ("onset_offset", quantity(0, 1, Unit::S)),
                    ("release_offset", quantity(0, 1, Unit::S)),
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
            "node" => self.node(object, scope, &mut fields)?,
            "instrument" => {
                let resources = self.plan.instruments.as_ref().ok_or_else(|| {
                    error("local instrument normalization requires compiled resources")
                })?;
                let program = resources
                    .programs
                    .iter()
                    .find(|p| {
                        p.source.file == resources.entry_source && p.source.object == object.id
                    })
                    .ok_or_else(|| {
                        error("local instrument descriptor missing from compiled plan")
                    })?;
                child_scope = Scope::Instrument(program);
            }
            "voice" | "shared" => {
                let Scope::Instrument(program) = scope else {
                    return Err(error("instrument graph has no descriptor context"));
                };
                child_scope = if object.kind == "voice" {
                    Scope::Graph(&program.voice, GraphStage::Voice)
                } else {
                    Scope::Graph(
                        program
                            .shared
                            .as_ref()
                            .ok_or_else(|| error("shared graph descriptor missing"))?,
                        GraphStage::Shared,
                    )
                };
            }
            _ => {}
        }
        let mut children = Map::new();
        for (id, child) in &object.children {
            children.insert(id.clone(), self.object(child, child_scope)?);
        }
        Ok(json!({"kind":object.kind,"fields":fields,"children":children}))
    }

    fn node(
        &self,
        object: &Object,
        scope: Scope<'_>,
        fields: &mut Map<String, JsonValue>,
    ) -> Result<(), IdentityError> {
        let mut config = record_fields(fields, "config")?;
        let mut accepts_config = true;
        let mut params = record_fields(fields, "params")?;
        match scope {
            Scope::Top => {
                let node = self
                    .plan
                    .nodes
                    .iter()
                    .find(|n| n.id == object.id)
                    .ok_or_else(|| error(format!("compiled node `{}` missing", object.id)))?;
                match &node.processor {
                    Processor::Sine { voices } => {
                        config
                            .entry("voices")
                            .or_insert_with(|| number(i64::from(*voices)));
                    }
                    Processor::Instrument { voices, .. } => {
                        config
                            .entry("voices")
                            .or_insert_with(|| number(i64::from(*voices)));
                    }
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
                        config.entry("damping").or_insert_with(|| {
                            rational(&BigRational::new(1.into(), 2.into()), None)
                        });
                    }
                    _ => {}
                }
                for (name, value) in &node.params {
                    let unit = match &node.processor {
                        Processor::Instrument { .. } => graph_unit(
                            self.plan
                                .instrument_control_spec(node, name)
                                .ok_or_else(|| error("instrument control descriptor missing"))?
                                .unit,
                        ),
                        Processor::Sine { .. } if matches!(name.as_str(), "attack" | "release") => {
                            Some(Unit::S)
                        }
                        Processor::OnePole { .. } => Some(Unit::Hz),
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
                        _ => None,
                    };
                    params.entry(name).or_insert_with(|| rational(value, unit));
                }
            }
            Scope::Graph(graph, stage) => {
                let node = graph
                    .nodes
                    .iter()
                    .find(|n| n.id == object.id)
                    .ok_or_else(|| error("local graph node descriptor missing"))?;
                accepts_config = !matches!(
                    node.processor,
                    GraphProcessor::Sine
                        | GraphProcessor::Saw
                        | GraphProcessor::Square
                        | GraphProcessor::Triangle
                        | GraphProcessor::Adsr
                        | GraphProcessor::Lfo
                        | GraphProcessor::Pan
                );
                if let GraphProcessor::Noise { seed } | GraphProcessor::Pluck { seed } =
                    &node.processor
                {
                    config
                        .entry("seed")
                        .or_insert_with(|| number(i64::from(*seed)));
                }
                // The descriptor is the authority for values/units. This is the
                // bounded union of parameter names in the accepted synth/1 set.
                for name in [
                    "ratio",
                    "frequency",
                    "phase",
                    "level",
                    "position",
                    "decay",
                    "damping",
                    "attack",
                    "sustain",
                    "release",
                    "cutoff",
                    "pan",
                ] {
                    if let Some(spec) = parameter_descriptor_for_stage(&node.processor, name, stage)
                    {
                        params.entry(name).or_insert_with(|| {
                            rational(
                                node.params.get(name).unwrap_or(&spec.default),
                                graph_unit(spec.unit),
                            )
                        });
                    }
                }
            }
            _ => return Err(error("node is outside a supported graph context")),
        }
        if accepts_config {
            fields.insert("config".into(), record(config));
        }
        fields.insert("params".into(), record(params));
        Ok(())
    }
}

fn exact_number(value: &Value) -> Result<&BigRational, IdentityError> {
    match &value.kind {
        ValueKind::Number(n) => Ok(n),
        _ => Err(error("constructor requires an exact dimensionless number")),
    }
}
fn exact_frequency(value: &Value) -> Result<BigRational, IdentityError> {
    match &value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::Hz,
        } => Ok(value.clone()),
        ValueKind::Quantity {
            value,
            unit: Unit::KHz,
        } => Ok(value * BigRational::from_integer(1000.into())),
        _ => Err(error("ratio requires an exact Hz reference")),
    }
}
fn meter_map(document: &Document) -> Result<MeterMap, IdentityError> {
    let project = document
        .objects
        .values()
        .find(|o| o.kind == "project")
        .ok_or_else(|| error("execution identity requires one project"))?;
    let reference = project
        .field("meter")
        .and_then(|f| f.value.reference())
        .ok_or_else(|| error("project meter reference missing"))?;
    let meter = reference
        .path
        .first()
        .and_then(|id| document.object(id))
        .ok_or_else(|| error("project meter object missing"))?;
    let Some(ValueKind::List(points)) = meter.field("points").map(|f| &f.value.kind) else {
        return Err(error("meter points missing"));
    };
    let mut result = Vec::new();
    for point in points {
        let ValueKind::Tuple(values) = &point.kind else {
            return Err(error("meter point must be a tuple"));
        };
        if values.len() != 3 {
            return Err(error("meter point must have three values"));
        }
        let ValueKind::Quantity {
            value: q,
            unit: Unit::Q,
        } = &values[0].kind
        else {
            return Err(error("meter-map anchors require explicit q positions"));
        };
        let integer = |v: &Value| -> Result<u32, IdentityError> {
            let n = exact_number(v)?;
            if !n.is_integer() {
                return Err(error("meter requires integer signature"));
            }
            n.to_integer()
                .to_u32()
                .ok_or_else(|| error("meter signature out of range"))
        };
        result.push(MeterPoint::new(
            q.clone(),
            integer(&values[1])?,
            integer(&values[2])?,
        ));
    }
    MeterMap::new(result).map_err(|e| error(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_vector_has_exact_control_escapes_and_unicode_order() {
        let value =
            json!({"😀": "é/", "\n": "\u{0000}\u{0008}\t\n\r\u{001f}\"\\", "a": [true, null, 1]});
        let expected = "{\"\\u000a\":\"\\u0000\\u0008\\u0009\\u000a\\u000d\\u001f\\\"\\\\\",\"a\":[true,null,1],\"😀\":\"é/\"}";
        assert_eq!(canonical_json_bytes(&value).unwrap(), expected.as_bytes());
    }

    const SOURCE: &str = r#"maac 1;
project p { score=[0q,8q]; rate=48000Hz; tempo=&t; meter=&m; output=&s:out; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
node s { type="core.sine/1"; }
pattern riff { length=1q; note n { at=0q; dur=1q; pitch=C4; } }
track notes { target=&s:events; }
place play { pattern=&riff; track=&notes; at=0q; }
"#;

    fn identity(source: &str) -> ExecutionIdentity {
        let document = crate::syntax::parse(source).unwrap();
        let plan = crate::compiler::compile(&document).unwrap();
        execution_identity(&document, &plan).unwrap()
    }

    #[test]
    fn canonical_digest_matches_independent_openssl_vector() {
        // Independently obtained with openssl dgst -sha256 over the literal
        // bytes {"objects":{},"version":1}, without a newline.
        assert_eq!(
            digest(&canonical_json_bytes(&json!({"version":1,"objects":{}})).unwrap()),
            "sha256:640cbf3a419bcf5d175b1aff137c759a9da8d22ee342c8b3b8415c6b2ddaee42"
        );
        assert!(canonical_json_bytes(&json!({"inexact":0.5})).is_err());
    }

    #[test]
    fn labels_comments_and_declaration_order_do_not_change_execution_hash() {
        let original = identity(SOURCE);
        let labeled = SOURCE
            .replace("node s {", "// new comment\nnode s { label=\"New label\";")
            .replace("note n {", "note n { label=\"Notation\";");
        let other = identity(&labeled);
        assert_eq!(original.execution_hash, other.execution_hash);
        assert_ne!(original.source_input_hash, other.source_input_hash);
        let mut document = crate::syntax::parse(SOURCE).unwrap();
        let plan = crate::compiler::compile(&document).unwrap();
        let objects = std::mem::take(&mut document.objects);
        document.objects = objects.into_iter().rev().collect();
        assert_eq!(
            execution_identity(&document, &plan).unwrap().execution_hash,
            original.execution_hash
        );
    }

    #[test]
    fn supported_defaults_units_and_pitch_spellings_normalize_equally() {
        let explicit = SOURCE.replace("output=&s:out;", "output=&s:out; tail=0ms; seed=0; requires=[];")
            .replace("rate=48000Hz", "rate=48kHz")
            .replace("type=\"core.sine/1\";", "type=\"core.sine/1\"; config={voices=64;}; params={attack=5ms;release=0.08s;level=2/10;};")
            .replace("pitch=C4;", "pitch=key(60);velocity=1;release_velocity=0.5;onset_offset=0ms;release_offset=0s;order=0;")
            .replace("track=&notes; at=0q;", "track=&notes; at=0q;count=1;stretch=1;transpose=0ct;boundary=spill;");
        assert_eq!(
            identity(SOURCE).execution_hash,
            identity(&explicit).execution_hash
        );
    }

    #[test]
    fn ratio_and_global_bar_positions_lower_without_flattening_source() {
        let source = SOURCE
            .replace("(0q,4,4)", "(0q,3,4)")
            .replace("pitch=C4", "pitch=ratio(3/2,400Hz)")
            .replace("track=&notes; at=0q;", "track=&notes; at=bar(2,1);count=3;");
        let lowered = source
            .replace("ratio(3/2,400Hz)", "600Hz")
            .replace("bar(2,1)", "3q");
        let normalized = identity(&source);
        assert_eq!(normalized.execution_hash, identity(&lowered).execution_hash);
        let value: JsonValue = serde_json::from_str(&normalized.normalized_source_json).unwrap();
        assert_eq!(
            value["objects"]["riff"]["children"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(value["objects"]["play"]["fields"]["count"], number(3));
        assert!(value.get("events").is_none());
    }

    #[test]
    fn complete_extensions_and_unknown_payload_labels_are_never_discarded() {
        let base = crate::syntax::parse(SOURCE).unwrap();
        let plan = crate::compiler::compile(&base).unwrap();
        // Preservation test at the identity boundary: an unknown required
        // capability would still be rejected by the renderer before execution.
        let with_extension = format!("{SOURCE} extension ext {{ namespace=\"unknown/1\";schema=&schema;render_affecting=false;data={{deliveries={{release={{limit=-14;}};unused={{limit=-20;}};}};label=\"payload\";}};}}");
        let first =
            execution_identity(&crate::syntax::parse(&with_extension).unwrap(), &plan).unwrap();
        for changed in [
            with_extension.replace("limit=-20", "limit=-21"),
            with_extension.replace("payload", "different"),
        ] {
            let second =
                execution_identity(&crate::syntax::parse(&changed).unwrap(), &plan).unwrap();
            assert_ne!(first.execution_hash, second.execution_hash);
        }
        assert!(first.normalized_source_json.contains("payload"));
        assert!(first.normalized_source_json.contains("unused"));
    }

    #[test]
    fn local_instrument_descriptor_defaults_and_instance_defaults_are_expanded() {
        let source = r#"maac 1;
project p {score=[0q,1q];rate=48000Hz;tempo=&t;meter=&m;output=&s:out;}
tempo t {points=[(0q,120bpm,step)];} meter m {points=[(0q,4,4)];}
instrument sound {channels=1;
 voice v {channels=1;output=&osc:out;amplitude=&env;
  node osc {type="synth.sine/1";}
  node env {type="synth.adsr/1";}
 }
 control amount {target=&v.osc.params.level;default=0.5;}
}
node s {instrument=&sound;}
"#;
        let explicit = source
            .replace(
                "type=\"synth.sine/1\";",
                "type=\"synth.sine/1\";params={ratio=1;frequency=0Hz;phase=0;level=1;};",
            )
            .replace(
                "type=\"synth.adsr/1\";",
                "type=\"synth.adsr/1\";params={attack=0ms;decay=0s;sustain=1;release=0s;};",
            )
            .replace(
                "instrument=&sound;",
                "instrument=&sound;config={voices=64;};params={amount=1/2;};",
            );
        let normalized = identity(source);
        assert_eq!(
            normalized.execution_hash,
            identity(&explicit).execution_hash
        );
        let value: JsonValue = serde_json::from_str(&normalized.normalized_source_json).unwrap();
        assert!(
            value["objects"]["sound"]["children"]["v"]["children"]["osc"]["fields"]
                .get("config")
                .is_none()
        );
    }

    fn context() -> JsonValue {
        json!({
            "selection":{"master":{"port":"s:out","rate":48000,"encoding":"wav_f32le"}},
            "dependencies":{}, "processors":{"s":"core.sine/1"},
            "engine":{"rate":48000,"arithmetic":"binary64-nearest-even"},
            "converter":{"identity":"maac.src.kaiser/1"}, "dither":{"master":"none"},
            "analyzer":{"identity":"maac.analysis.bs1770-5/1"},
            "interval":{"start":"0","duration":"4"}
        })
    }

    #[test]
    fn render_key_covers_selection_plan_and_numerical_context_separately() {
        let identity = identity(SOURCE);
        let original = context();
        let key = render_key(&identity, b"standalone-plan", &original).unwrap();
        let mut selection = original.clone();
        selection["selection"]["stem"] =
            json!({"port":"s:out","rate":44100,"encoding":"wav_pcm16le"});
        assert_ne!(
            key,
            render_key(&identity, b"standalone-plan", &selection).unwrap()
        );
        assert_ne!(
            key,
            render_key(&identity, b"different-plan", &original).unwrap()
        );
        let mut environment = original.clone();
        environment["engine"]["arithmetic"] = json!("different-arithmetic");
        assert_ne!(
            key,
            render_key(&identity, b"standalone-plan", &environment).unwrap()
        );
        let source_again = identity.clone();
        assert_eq!(source_again.execution_hash, identity.execution_hash);
        let reordered: Map<_, _> = selection
            .as_object()
            .unwrap()
            .iter()
            .rev()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(
            render_key(&identity, b"standalone-plan", &selection).unwrap(),
            render_key(&identity, b"standalone-plan", &JsonValue::Object(reordered)).unwrap()
        );
        let mut missing = original;
        missing.as_object_mut().unwrap().remove("analyzer");
        assert!(render_key(&identity, b"standalone-plan", &missing).is_err());
    }

    #[test]
    fn retained_identity_rejects_changed_evidence_and_noncanonical_json() {
        let identity = identity(SOURCE);
        identity.validate().unwrap();
        let mut changed = identity.clone();
        changed.normalized_source_json.push(' ');
        assert!(changed.validate().is_err());
        changed = identity.clone();
        changed.execution_hash = "sha256:bad".into();
        assert!(changed.validate().is_err());
        changed = identity;
        changed.normalized_source_json = changed
            .normalized_source_json
            .replace("core.sine/1", "core.sine/2");
        assert!(changed.validate().is_err());
    }

    #[test]
    fn production_processor_defaults_normalize_like_explicit_native_values() {
        for (kind, config, defaults, explicit_config) in [
            (
                "fx.eq/1",
                "channels=1;mode=peak;",
                "frequency=1kHz;q=1;gain=0dB;",
                "channels=1;mode=peak;",
            ),
            (
                "fx.compressor/1",
                "channels=1;",
                "threshold=-18dB;ratio=4;knee=6dB;attack=10ms;release=100ms;makeup=0dB;",
                "channels=1;detector=internal;",
            ),
            (
                "fx.reverb/1",
                "channels=1;",
                "decay=1500ms;mix=0.2;",
                "channels=1;predelay=0ms;damping=0.5;",
            ),
        ] {
            let source = SOURCE.replace("output=&s:out;", "output=&fx:out;requires=[\"maac.production/1\"];")
                + &format!("node fx {{type=\"{kind}\";config={{{config}}};}} connect edge {{from=&s:out;to=&fx:in;}}");
            let explicit = source.replace(
                &format!("config={{{config}}};"),
                &format!("config={{{explicit_config}}};params={{{defaults}}};"),
            );
            assert_eq!(
                identity(&source).execution_hash,
                identity(&explicit).execution_hash,
                "{kind}"
            );
        }
    }

    #[test]
    fn complete_production_delivery_changes_affect_execution_identity() {
        fn production(source: &str) -> ExecutionIdentity {
            let document = crate::syntax::parse(source).unwrap();
            let mut bundle = crate::bundle::SourceBundle::new("examples/production.maac", source);
            bundle.assets.insert(
                "production.schema.json".into(),
                include_bytes!("../production.schema.json").to_vec(),
            );
            let plan = crate::compiler::compile_bundle(&bundle).unwrap();
            execution_identity(&document, &plan).unwrap()
        }
        let source = include_str!("../examples/production.maac");
        let first = production(source);
        let changed = source.replace("96kHz", "48kHz");
        assert_ne!(
            changed, source,
            "example must contain the separate 96k delivery"
        );
        assert_ne!(first.execution_hash, production(&changed).execution_hash);
        let changed = source.replace("max = -1;", "max = -2;");
        assert_ne!(changed, source, "example must contain the chosen limit");
        assert_ne!(first.execution_hash, production(&changed).execution_hash);
    }

    #[test]
    fn project_bar_normalization_and_compilation_use_the_declared_meter() {
        let source = SOURCE
            .split("pattern riff")
            .next()
            .unwrap()
            .replace("[0q,8q]", "[bar(2,1),bar(3,1)]")
            .replace("(0q,4,4)", "(0q,3,4)");
        let literal = source.replace("[bar(2,1),bar(3,1)]", "[3q,6q]");
        let document = crate::syntax::parse(&source).unwrap();
        let literal_document = crate::syntax::parse(&literal).unwrap();
        let plan = crate::compiler::compile(&document).unwrap();
        let literal_plan = crate::compiler::compile(&literal_document).unwrap();
        assert_eq!(
            plan.output.score_start_q,
            BigRational::from_integer(3.into())
        );
        assert_eq!(plan.output.score_end_q, BigRational::from_integer(6.into()));
        assert_eq!(plan.output.total_frames, 72_000);
        assert_eq!(plan.output.score_start_q, literal_plan.output.score_start_q);
        assert_eq!(plan.output.score_end_q, literal_plan.output.score_end_q);
        assert_eq!(plan.output.total_frames, literal_plan.output.total_frames);
        assert_eq!(
            execution_identity(&document, &plan).unwrap().execution_hash,
            execution_identity(&literal_document, &literal_plan)
                .unwrap()
                .execution_hash
        );
    }
}
