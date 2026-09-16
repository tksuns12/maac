//! Production semantic context for the currently implemented core editing slice.
//!
//! This context deliberately covers the source-only foundation that the Rust
//! implementation can validate without executing assets or a renderer. Unknown
//! required extensions, local library/import syntax, native production nodes,
//! and native message performance remain explicit E_CAPABILITY boundaries.

use std::collections::{BTreeMap, BTreeSet};

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{ToPrimitive, Zero};
use serde_json::{json, Map, Value};

use crate::diagnostic::Diagnostics;
use crate::music::{MeterMap, MeterPoint, Pitch};
use crate::syntax::{Document, Unit};

use super::{
    AuthoredDocument, EditContext, EditError, EditImpact, EditResult, Path, RenderInvalidationScope,
};

/// Semantic and normalization context for the implemented source-only core.
///
/// This is intentionally narrower than MaaC/1 Document conformance. It accepts
/// the core source shapes handled by the foundation semantic validator,
/// including core control, kit, audio and warp objects, but refuses capabilities
/// whose full normalization contract is not implemented here.
#[derive(Clone, Copy, Debug, Default)]
pub struct FoundationEditContext;

impl FoundationEditContext {
    /// Validate and compute the label-retaining N(A) view used by editing
    /// preconditions. This is not the execution-hash projection.
    pub fn normalize_document(&self, document: &AuthoredDocument) -> EditResult<Value> {
        self.validate_document(document.tree())?;
        normalize_document_tree(document.tree())
    }
}

impl EditContext for FoundationEditContext {
    fn validate_document(&self, authored: &Value) -> EditResult<()> {
        let document = crate::syntax::document_from_syntax_json_value(authored)
            .map_err(|message| EditError::new("E_SYNTAX", message))?;
        reject_unsupported_capabilities(&document)?;
        let instruments = BTreeMap::new();
        crate::semantic::validate_source_document_profile(&document, &instruments)
            .map_err(map_diagnostics)?;
        validate_occurrence_targets(authored)
    }

    fn normalize_object(
        &self,
        base: &Value,
        _base_path: &[String],
        object: &Value,
    ) -> EditResult<Value> {
        let meter = meter_map(base)?;
        let seed = project_seed(base)?;
        normalize_object_inner(object, &meter, &seed)
    }

    fn normalize_value(
        &self,
        base: &Value,
        base_path: &[String],
        field: &[String],
        value: &Value,
    ) -> EditResult<Value> {
        let meter = meter_map(base)?;
        let object = object_at(base, base_path)?;
        let kind = object["kind"].as_str().unwrap_or_default();
        let pitch = (kind == "note" && field.len() == 1 && field[0] == "pitch")
            || (kind == "override" && field.len() == 2 && field[0] == "set" && field[1] == "pitch");
        normalize_value_inner(value, pitch, true, &meter)
    }

    fn rewrite_structural_references(
        &self,
        before: &Value,
        candidate: &mut Value,
        old_path: &[String],
        new_path: &[String],
    ) -> EditResult<()> {
        rewrite_occurrence_addresses(before, candidate, old_path, new_path)
    }

    fn refine_impact(
        &self,
        base: &Value,
        candidate: &Value,
        impact: &mut EditImpact,
    ) -> EditResult<()> {
        refine_foundation_impact(base, candidate, impact)
    }
}

fn map_diagnostics(diagnostics: Diagnostics) -> EditError {
    let diagnostic = diagnostics
        .into_vec()
        .into_iter()
        .find(|diagnostic| diagnostic.is_error())
        .unwrap_or_else(|| {
            crate::Diagnostic::error(
                crate::DiagnosticCode::Range,
                "semantic validation failed",
                None,
            )
        });
    EditError {
        code: diagnostic.code_str().to_owned(),
        message: diagnostic.message,
        object_path: diagnostic.object_path,
        field_path: diagnostic.field_path,
    }
}

fn reject_unsupported_capabilities(document: &Document) -> EditResult<()> {
    for object in document.objects.values() {
        if matches!(
            object.kind.as_str(),
            "extension" | "import" | "library" | "instrument" | "preset" | "wavetable"
        ) {
            return Err(EditError::new(
                "E_CAPABILITY",
                format!(
                    "{} editing normalization is outside FoundationEditContext",
                    object.kind
                ),
            )
            .at(std::slice::from_ref(&object.id), &[]));
        }
        if object.kind == "node" {
            if object.field("instrument").is_some() {
                return Err(EditError::new(
                    "E_CAPABILITY",
                    "local instrument editing requires a library-aware context",
                )
                .at(std::slice::from_ref(&object.id), &[]));
            }
            if object
                .field("type")
                .and_then(|field| field.value.as_string())
                .is_some_and(|kind| kind.starts_with("fx."))
            {
                return Err(EditError::new(
                    "E_CAPABILITY",
                    "native production node editing requires a production-aware context",
                )
                .at(std::slice::from_ref(&object.id), &[]));
            }
        }
    }
    Ok(())
}

fn object_at<'a>(tree: &'a Value, path: &[String]) -> EditResult<&'a Value> {
    let mut objects = tree["objects"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    let mut object = None;
    for id in path {
        let next = objects.get(id).ok_or_else(|| {
            EditError::new("E_REFERENCE", "object path does not exist").at(path, &[])
        })?;
        object = Some(next);
        objects = next["children"]
            .as_object()
            .ok_or_else(|| EditError::new("E_SYNTAX", "children must be a dictionary"))?;
    }
    object.ok_or_else(|| EditError::new("E_REFERENCE", "object path is empty"))
}

fn typed_rational(value: &Value) -> EditResult<BigRational> {
    let numerator = value["n"]
        .as_str()
        .ok_or_else(|| EditError::new("E_SYNTAX", "rational numerator must be a string"))?
        .parse::<BigInt>()
        .map_err(|_| EditError::new("E_SYNTAX", "invalid rational numerator"))?;
    let denominator = value["d"]
        .as_str()
        .ok_or_else(|| EditError::new("E_SYNTAX", "rational denominator must be a string"))?
        .parse::<BigInt>()
        .map_err(|_| EditError::new("E_SYNTAX", "invalid rational denominator"))?;
    if denominator <= BigInt::zero() {
        return Err(EditError::new(
            "E_RANGE",
            "rational denominator must be positive",
        ));
    }
    Ok(BigRational::new(numerator, denominator))
}

fn rational(value: &BigRational, unit: Option<Unit>) -> Value {
    match unit {
        Some(unit) => json!({
            "t":"quantity",
            "n":value.numer().to_string(),
            "d":value.denom().to_string(),
            "u":unit.as_str()
        }),
        None => json!({
            "t":"number",
            "n":value.numer().to_string(),
            "d":value.denom().to_string()
        }),
    }
}

fn number(numerator: i64, denominator: i64) -> Value {
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

fn record(fields: Map<String, Value>) -> Value {
    json!({"t":"record","fields":fields})
}

fn symbol(value: &str) -> Value {
    json!({"t":"symbol","v":value})
}

fn boolean(value: bool) -> Value {
    json!({"t":"boolean","v":value})
}

fn normalize_value_inner(
    value: &Value,
    pitch: bool,
    constructors: bool,
    meter: &MeterMap,
) -> EditResult<Value> {
    if pitch {
        if let Some(text) = value.get("v").and_then(Value::as_str) {
            if matches!(value["t"].as_str(), Some("symbol" | "string")) {
                let parsed = Pitch::parse_spelled(text)
                    .map_err(|error| EditError::new("E_RANGE", error.to_string()))?;
                let key = match parsed {
                    Pitch::Spelled(spelling) => spelling
                        .key()
                        .map_err(|error| EditError::new("E_RANGE", error.to_string()))?,
                    _ => return Err(EditError::new("E_RANGE", "expected a spelled pitch")),
                };
                return Ok(json!({
                    "t":"call",
                    "fn":"key",
                    "args":[number(key, 1)]
                }));
            }
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
            let args = value["args"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "bar args must be an array"))?;
            if args.len() != 2 {
                return Err(EditError::new("E_RANGE", "bar requires two arguments"));
            }
            let bar = typed_rational(&args[0])?;
            if !bar.is_integer() {
                return Err(EditError::new("E_RANGE", "bar number must be an integer"));
            }
            let bar = bar
                .to_integer()
                .to_i64()
                .ok_or_else(|| EditError::new("E_RESOURCE_LIMIT", "bar number is out of range"))?;
            let q = meter
                .bar_to_q(bar, &typed_rational(&args[1])?)
                .map_err(|error| EditError::new("E_RANGE", error.to_string()))?;
            Ok(rational(&q, Some(Unit::Q)))
        }
        Some("call") if constructors && value["fn"] == "ratio" => {
            let args = value["args"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "ratio args must be an array"))?;
            if args.len() != 2 {
                return Err(EditError::new("E_RANGE", "ratio requires two arguments"));
            }
            let ratio = typed_rational(&args[0])?;
            let frequency = match args[1]["t"].as_str() {
                Some("quantity") if args[1]["u"] == "Hz" => typed_rational(&args[1])?,
                Some("quantity") if args[1]["u"] == "kHz" => {
                    typed_rational(&args[1])? * BigRational::from_integer(1000.into())
                }
                _ => {
                    return Err(EditError::new(
                        "E_UNIT",
                        "ratio requires a frequency reference",
                    ))
                }
            };
            Ok(rational(&(ratio * frequency), Some(Unit::Hz)))
        }
        Some("record") => {
            let mut fields = Map::new();
            for (name, child) in value["fields"]
                .as_object()
                .ok_or_else(|| EditError::new("E_SYNTAX", "record fields must be a dictionary"))?
            {
                fields.insert(
                    name.clone(),
                    normalize_value_inner(child, pitch && name == "pitch", constructors, meter)?,
                );
            }
            Ok(record(fields))
        }
        Some("list" | "tuple") => {
            let tag = value["t"].as_str().expect("matched string");
            let mut items = Vec::new();
            for child in value["items"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "items must be an array"))?
            {
                items.push(normalize_value_inner(child, false, constructors, meter)?);
            }
            Ok(json!({"t":tag,"items":items}))
        }
        Some("call") => {
            let mut args = Vec::new();
            for child in value["args"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "call args must be an array"))?
            {
                args.push(normalize_value_inner(child, false, constructors, meter)?);
            }
            Ok(json!({"t":"call","fn":value["fn"].clone(),"args":args}))
        }
        _ => Ok(value.clone()),
    }
}

fn normalize_document_tree(tree: &Value) -> EditResult<Value> {
    let meter = meter_map(tree)?;
    let seed = project_seed(tree)?;
    let mut objects = Map::new();
    for (id, object) in tree["objects"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?
    {
        objects.insert(id.clone(), normalize_object_inner(object, &meter, &seed)?);
    }
    Ok(json!({"version":1,"objects":objects}))
}

fn normalize_object_inner(
    object: &Value,
    meter: &MeterMap,
    project_seed: &Value,
) -> EditResult<Value> {
    let kind = object["kind"]
        .as_str()
        .ok_or_else(|| EditError::new("E_SYNTAX", "object kind must be a string"))?;
    let constructors = kind != "extension";
    let mut fields = Map::new();
    for (name, field) in object["fields"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "object fields must be a dictionary"))?
    {
        let pitch = (kind == "note" && name == "pitch") || (kind == "override" && name == "set");
        fields.insert(
            name.clone(),
            normalize_value_inner(field, pitch, constructors, meter)?,
        );
    }

    match kind {
        "project" => {
            fields
                .entry("tail")
                .or_insert_with(|| quantity(0, 1, Unit::S));
            fields.entry("seed").or_insert_with(|| number(0, 1));
            fields
                .entry("requires")
                .or_insert_with(|| json!({"t":"list","items":[]}));
        }
        "note" => {
            for (name, value) in [
                ("velocity", number(1, 1)),
                ("release_velocity", number(1, 2)),
                ("onset_offset", quantity(0, 1, Unit::S)),
                ("release_offset", quantity(0, 1, Unit::S)),
                ("order", number(0, 1)),
            ] {
                fields.entry(name).or_insert(value);
            }
        }
        "hit" => {
            for (name, value) in [
                ("velocity", number(1, 1)),
                ("onset_offset", quantity(0, 1, Unit::S)),
                ("order", number(0, 1)),
            ] {
                fields.entry(name).or_insert(value);
            }
        }
        "message" => {
            for (name, value) in [
                ("onset_offset", quantity(0, 1, Unit::S)),
                ("order", number(0, 1)),
            ] {
                fields.entry(name).or_insert(value);
            }
        }
        "place" | "use" => {
            for (name, value) in [
                ("count", number(1, 1)),
                ("stretch", number(1, 1)),
                ("transpose", quantity(0, 1, Unit::Ct)),
                ("boundary", symbol("spill")),
            ] {
                fields.entry(name).or_insert(value);
            }
        }
        "audio" => {
            if fields.get("mode").and_then(|value| value["v"].as_str()) == Some("rate") {
                fields.entry("speed").or_insert_with(|| number(1, 1));
                fields.entry("reverse").or_insert_with(|| boolean(false));
            }
            for (name, value) in [
                ("gain", number(1, 1)),
                ("fade_in", quantity(0, 1, Unit::S)),
                ("fade_out", quantity(0, 1, Unit::S)),
                ("fade_shape", symbol("linear")),
            ] {
                fields.entry(name).or_insert(value);
            }
        }
        "node" => normalize_node_fields(&mut fields, project_seed)?,
        _ => {}
    }

    let mut children = Map::new();
    for (id, child) in object["children"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "children must be a dictionary"))?
    {
        children.insert(
            id.clone(),
            normalize_object_inner(child, meter, project_seed)?,
        );
    }
    Ok(json!({"kind":kind,"fields":fields,"children":children}))
}

fn record_fields(fields: &mut Map<String, Value>, name: &str) -> EditResult<Map<String, Value>> {
    match fields.remove(name) {
        None => Ok(Map::new()),
        Some(value) if value["t"] == "record" => value["fields"]
            .as_object()
            .cloned()
            .ok_or_else(|| EditError::new("E_SYNTAX", "record fields must be a dictionary")),
        Some(_) => Err(EditError::new("E_UNIT", format!("{name} must be a record"))),
    }
}

fn normalize_node_fields(fields: &mut Map<String, Value>, project_seed: &Value) -> EditResult<()> {
    let processor = fields
        .get("type")
        .and_then(|value| value["v"].as_str())
        .ok_or_else(|| EditError::new("E_RANGE", "node.type is required"))?
        .to_owned();
    if processor.starts_with("fx.") {
        return Err(EditError::new(
            "E_CAPABILITY",
            "native production node normalization is not implemented",
        ));
    }

    let mut config = record_fields(fields, "config")?;
    let mut params = record_fields(fields, "params")?;
    match processor.as_str() {
        "core.sine/1" => {
            config.entry("voices").or_insert_with(|| number(64, 1));
            for (name, value) in [
                ("attack", quantity(1, 200, Unit::S)),
                ("release", quantity(2, 25, Unit::S)),
                ("level", number(1, 5)),
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
            params.entry("gain").or_insert_with(|| number(1, 1));
        }
        "core.fader/1" => {
            params
                .entry("level")
                .or_insert_with(|| quantity(0, 1, Unit::Db));
        }
        "core.pan/1" => {
            params.entry("pan").or_insert_with(|| number(0, 1));
        }
        "core.noise/1" => {
            config.entry("seed").or_insert_with(|| project_seed.clone());
        }
        "core.kit/1" => {
            config.entry("voices").or_insert_with(|| number(64, 1));
            params.entry("level").or_insert_with(|| number(1, 1));
        }
        "core.lfo/1" => {
            config.entry("wave").or_insert_with(|| symbol("sine"));
            config.entry("phase").or_insert_with(|| number(0, 1));
        }
        "core.constant/1" => {
            params.entry("value").or_insert_with(|| number(0, 1));
        }
        "core.matrix/1" | "core.delay/1" | "core.sum/1" => {}
        other => {
            return Err(EditError::new(
                "E_CAPABILITY",
                format!("processor `{other}` is outside FoundationEditContext"),
            ))
        }
    }
    fields.insert("config".into(), record(config));
    fields.insert("params".into(), record(params));
    Ok(())
}

fn project_seed(tree: &Value) -> EditResult<Value> {
    let project = tree["objects"]
        .as_object()
        .and_then(|objects| objects.values().find(|object| object["kind"] == "project"))
        .ok_or_else(|| EditError::new("E_RANGE", "one project is required"))?;
    match project["fields"].get("seed") {
        Some(seed) => Ok(seed.clone()),
        None => Ok(number(0, 1)),
    }
}

fn meter_map(tree: &Value) -> EditResult<MeterMap> {
    let objects = tree["objects"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    let project = objects
        .values()
        .find(|object| object["kind"] == "project")
        .ok_or_else(|| EditError::new("E_RANGE", "one project is required"))?;
    let meter_id = project["fields"]["meter"]["path"]
        .as_array()
        .and_then(|path| path.first())
        .and_then(Value::as_str)
        .ok_or_else(|| EditError::new("E_REFERENCE", "project meter reference is missing"))?;
    let meter = objects
        .get(meter_id)
        .ok_or_else(|| EditError::new("E_REFERENCE", "project meter object is missing"))?;
    let points = meter["fields"]["points"]["items"]
        .as_array()
        .ok_or_else(|| EditError::new("E_RANGE", "meter points are missing"))?;
    let mut result = Vec::new();
    for point in points {
        let items = point["items"]
            .as_array()
            .ok_or_else(|| EditError::new("E_RANGE", "meter point must be a tuple"))?;
        if items.len() != 3 {
            return Err(EditError::new(
                "E_RANGE",
                "meter point must have three values",
            ));
        }
        if items[0]["u"] != "q" {
            return Err(EditError::new(
                "E_UNIT",
                "meter anchors require q positions",
            ));
        }
        let integer = |value: &Value| -> EditResult<u32> {
            let value = typed_rational(value)?;
            if !value.is_integer() {
                return Err(EditError::new(
                    "E_RANGE",
                    "meter signature requires integers",
                ));
            }
            value
                .to_integer()
                .to_u32()
                .ok_or_else(|| EditError::new("E_RANGE", "meter signature is out of range"))
        };
        result.push(MeterPoint::new(
            typed_rational(&items[0])?,
            integer(&items[1])?,
            integer(&items[2])?,
        ));
    }
    MeterMap::new(result).map_err(|error| EditError::new("E_METER_BOUNDARY", error.to_string()))
}

fn source_object_id(value: &Value) -> Option<&str> {
    if value["t"] != "ref" || !value["port"].is_null() {
        return None;
    }
    let path = value["path"].as_array()?;
    (path.len() == 1).then(|| path[0].as_str()).flatten()
}

fn positive_count(object: &Value) -> Option<u64> {
    match object["fields"].get("count") {
        None => Some(1),
        Some(value) => {
            let value = typed_rational(value).ok()?;
            if !value.is_integer() || value <= BigRational::zero() {
                return None;
            }
            value.to_integer().to_u64()
        }
    }
}

fn resolve_occurrence(root: &Value, place: &Value, address: &str) -> Option<Vec<(Path, usize)>> {
    let segments: Vec<_> = address.split('/').collect();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let place_index = segments[0].parse::<u64>().ok()?;
    if place_index >= positive_count(place)? {
        return None;
    }
    let objects = root["objects"].as_object()?;
    let mut pattern_id = source_object_id(place["fields"].get("pattern")?)?.to_owned();
    let mut position = 1usize;
    let mut mappings = Vec::new();

    loop {
        let pattern = objects.get(&pattern_id)?;
        if pattern["kind"] != "pattern" {
            return None;
        }
        let child_id = *segments.get(position)?;
        let child = pattern["children"].get(child_id)?;
        let source_path = vec![pattern_id.clone(), child_id.to_owned()];
        mappings.push((source_path, position));
        match child["kind"].as_str()? {
            "note" | "hit" | "message" => {
                return (position + 1 == segments.len()).then_some(mappings);
            }
            "use" => {
                let repeat_position = position + 1;
                let repeat = segments.get(repeat_position)?.parse::<u64>().ok()?;
                if repeat >= positive_count(child)? {
                    return None;
                }
                pattern_id = source_object_id(child["fields"].get("pattern")?)?.to_owned();
                position += 2;
            }
            _ => return None,
        }
    }
}

fn validate_occurrence_targets(root: &Value) -> EditResult<()> {
    let objects = root["objects"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    for (place_id, place) in objects {
        if place["kind"] != "place" {
            continue;
        }
        let mut targets = BTreeSet::new();
        let Some(children) = place["children"].as_object() else {
            continue;
        };
        for (override_id, override_object) in children {
            if override_object["kind"] != "override" {
                continue;
            }
            let event = override_object["fields"]["event"]["v"]
                .as_str()
                .ok_or_else(|| {
                    EditError::new("E_UNIT", "override.event must be a string")
                        .at(&[place_id.clone(), override_id.clone()], &["event".into()])
                })?;
            if !targets.insert(event.to_owned()) {
                return Err(EditError::new(
                    "E_INSTANCE_TARGET",
                    "an occurrence may have at most one override",
                )
                .at(&[place_id.clone(), override_id.clone()], &["event".into()]));
            }
            if resolve_occurrence(root, place, event).is_none() {
                return Err(EditError::new(
                    "E_INSTANCE_TARGET",
                    format!("override target `{event}` does not exist"),
                )
                .at(&[place_id.clone(), override_id.clone()], &["event".into()]));
            }
        }
    }
    Ok(())
}

fn rewrite_occurrence_addresses(
    before: &Value,
    candidate: &mut Value,
    old_path: &[String],
    new_path: &[String],
) -> EditResult<()> {
    if old_path.len() != 2 || new_path.len() != 2 {
        return Ok(());
    }
    let parent = object_at(before, &old_path[..1])?;
    if parent["kind"] != "pattern" {
        return Ok(());
    }

    let before_objects = before["objects"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    let candidate_objects = candidate["objects"]
        .as_object_mut()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    for (place_id, before_place) in before_objects {
        if before_place["kind"] != "place" {
            continue;
        }
        let Some(before_children) = before_place["children"].as_object() else {
            continue;
        };
        for (override_id, before_override) in before_children {
            if before_override["kind"] != "override" {
                continue;
            }
            let Some(event) = before_override["fields"]["event"]["v"].as_str() else {
                continue;
            };
            let Some(mappings) = resolve_occurrence(before, before_place, event) else {
                continue;
            };
            let Some((_, segment_index)) = mappings
                .iter()
                .find(|(source_path, _)| source_path == old_path)
            else {
                continue;
            };
            let mut segments: Vec<_> = event.split('/').map(str::to_owned).collect();
            segments[*segment_index] = new_path[1].clone();
            let rewritten = segments.join("/");
            let target = candidate_objects
                .get_mut(place_id)
                .and_then(|place| place["children"].get_mut(override_id))
                .and_then(|object| object["fields"].get_mut("event"))
                .ok_or_else(|| {
                    EditError::new(
                        "E_REFERENCE",
                        "override moved while rewriting occurrence address",
                    )
                })?;
            target["v"] = Value::String(rewritten);
        }
    }
    Ok(())
}

const MAX_REPORTED_EVENT_ADDRESSES: usize = 1024;

#[derive(Clone, Debug)]
struct EventOccurrence {
    address: String,
    dependencies: Vec<Path>,
}

fn refine_foundation_impact(
    base: &Value,
    candidate: &Value,
    impact: &mut EditImpact,
) -> EditResult<()> {
    if execution_relevant_view(base) == execution_relevant_view(candidate) {
        impact.affected_expanded_event_addresses.clear();
        impact.total_affected_expanded_event_addresses = 0;
        impact.event_addresses_truncated = false;
        impact.all_expanded_events_may_be_affected = false;
        impact.render_invalidation_scope = RenderInvalidationScope::None;
        impact.full_render_invalidated = false;
        return Ok(());
    }

    let changed = directly_changed_paths(base, candidate);
    let Some(addresses) = bounded_event_impact(base, candidate, &changed) else {
        return Ok(());
    };
    let total = addresses.len();
    impact.affected_expanded_event_addresses = addresses
        .into_iter()
        .take(MAX_REPORTED_EVENT_ADDRESSES)
        .collect();
    impact.total_affected_expanded_event_addresses = total;
    impact.event_addresses_truncated = total > impact.affected_expanded_event_addresses.len();
    impact.all_expanded_events_may_be_affected = false;
    impact.render_invalidation_scope = RenderInvalidationScope::AffectedEventsAndDependents;
    impact.full_render_invalidated = false;
    Ok(())
}

fn execution_relevant_view(tree: &Value) -> Value {
    fn strip(objects: &mut Value) {
        let Some(objects) = objects.as_object_mut() else {
            return;
        };
        for object in objects.values_mut() {
            if let Some(fields) = object.get_mut("fields").and_then(Value::as_object_mut) {
                fields.remove("label");
            }
            if let Some(children) = object.get_mut("children") {
                strip(children);
            }
        }
    }
    let mut copy = tree.clone();
    strip(&mut copy["objects"]);
    copy
}

fn directly_changed_paths(base: &Value, candidate: &Value) -> Vec<Path> {
    fn own(value: &Value) -> Option<(&Value, &Value)> {
        Some((value.get("kind")?, value.get("fields")?))
    }
    let before = foundation_object_paths(base);
    let after = foundation_object_paths(candidate);
    before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| match (before.get(path), after.get(path)) {
            (Some(left), Some(right)) => own(left) != own(right),
            _ => true,
        })
        .collect()
}

fn foundation_object_paths(tree: &Value) -> BTreeMap<Path, &Value> {
    fn visit<'a>(objects: &'a Value, path: &mut Path, out: &mut BTreeMap<Path, &'a Value>) {
        let Some(objects) = objects.as_object() else {
            return;
        };
        for (id, object) in objects {
            path.push(id.clone());
            out.insert(path.clone(), object);
            visit(&object["children"], path, out);
            path.pop();
        }
    }
    let mut out = BTreeMap::new();
    visit(&tree["objects"], &mut Vec::new(), &mut out);
    out
}

fn bounded_event_impact(
    base: &Value,
    candidate: &Value,
    changed: &[Path],
) -> Option<BTreeSet<String>> {
    if changed.is_empty() {
        return Some(BTreeSet::new());
    }
    for path in changed {
        let kind = top_level_kind(base, path).or_else(|| top_level_kind(candidate, path))?;
        if !matches!(kind, "pattern" | "place") {
            return None;
        }
    }

    let mut affected = BTreeSet::new();
    for tree in [base, candidate] {
        for event in enumerate_event_occurrences(tree)? {
            if changed.iter().any(|path| {
                event
                    .dependencies
                    .iter()
                    .any(|dependency| dependency == path)
                    || path.first().is_some_and(|id| {
                        event.dependencies.iter().any(|dependency| {
                            dependency.first() == Some(id) && kind_is_place(tree, id)
                        })
                    })
            }) {
                affected.insert(event.address);
            }
        }
    }
    Some(affected)
}

fn top_level_kind<'a>(tree: &'a Value, path: &[String]) -> Option<&'a str> {
    let id = path.first()?;
    tree["objects"].get(id)?["kind"].as_str()
}

fn kind_is_place(tree: &Value, id: &str) -> bool {
    tree["objects"]
        .get(id)
        .and_then(|object| object["kind"].as_str())
        == Some("place")
}

fn enumerate_event_occurrences(tree: &Value) -> Option<Vec<EventOccurrence>> {
    let objects = tree["objects"].as_object()?;
    let mut out = Vec::new();
    for (place_id, place) in objects {
        if place["kind"] != "place" {
            continue;
        }
        let pattern = source_object_id(place["fields"].get("pattern")?)?;
        let count = positive_count(place)?;
        for repetition in 0..count {
            let mut address = vec![place_id.clone(), repetition.to_string()];
            let dependencies = vec![vec![place_id.clone()]];
            enumerate_pattern_occurrences(
                objects,
                pattern,
                &mut address,
                &dependencies,
                &mut out,
                0,
            )?;
        }
        enumerate_insert_occurrences(place_id, place, &mut out)?;
    }
    Some(out)
}

fn enumerate_pattern_occurrences(
    objects: &Map<String, Value>,
    pattern_id: &str,
    address: &mut Vec<String>,
    inherited_dependencies: &[Path],
    out: &mut Vec<EventOccurrence>,
    depth: usize,
) -> Option<()> {
    if depth >= 64 {
        return None;
    }
    let pattern = objects.get(pattern_id)?;
    if pattern["kind"] != "pattern" {
        return None;
    }
    let children = pattern["children"].as_object()?;
    for (child_id, child) in children {
        let mut dependencies = inherited_dependencies.to_vec();
        dependencies.push(vec![pattern_id.to_owned()]);
        dependencies.push(vec![pattern_id.to_owned(), child_id.clone()]);
        match child["kind"].as_str()? {
            "note" | "hit" | "message" => {
                address.push(child_id.clone());
                out.push(EventOccurrence {
                    address: address.join("/"),
                    dependencies,
                });
                address.pop();
            }
            "use" => {
                let nested = source_object_id(child["fields"].get("pattern")?)?;
                let count = positive_count(child)?;
                for repetition in 0..count {
                    address.push(child_id.clone());
                    address.push(repetition.to_string());
                    enumerate_pattern_occurrences(
                        objects,
                        nested,
                        address,
                        &dependencies,
                        out,
                        depth + 1,
                    )?;
                    address.pop();
                    address.pop();
                }
            }
            _ => {}
        }
    }
    Some(())
}

fn enumerate_insert_occurrences(
    place_id: &str,
    place: &Value,
    out: &mut Vec<EventOccurrence>,
) -> Option<()> {
    for (insert_id, insert) in place["children"].as_object()? {
        if insert["kind"] != "insert" {
            continue;
        }
        for (leaf_id, leaf) in insert["children"].as_object()? {
            if !matches!(leaf["kind"].as_str(), Some("note" | "hit" | "message")) {
                continue;
            }
            out.push(EventOccurrence {
                address: format!("{place_id}/{insert_id}/{leaf_id}"),
                dependencies: vec![
                    vec![place_id.to_owned()],
                    vec![place_id.to_owned(), insert_id.clone()],
                    vec![place_id.to_owned(), insert_id.clone(), leaf_id.clone()],
                ],
            });
        }
    }
    Some(())
}
