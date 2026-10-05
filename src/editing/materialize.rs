//! Exact, bounded structural preparation of a materialize-instance transaction.

use std::collections::{BTreeMap, BTreeSet};

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, ToPrimitive, Zero};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::{
    apply_transaction, wire, AuthoredDocument, EditContext, EditError, EditResult, Operation, Path,
    Transaction, MAX_DOCUMENT_BYTES, MAX_OPERATIONS,
};

/// Complete occurrence provenance, including leaves suppressed by a delete
/// override and the once-only leaves of placement inserts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MaterializedEventMapping {
    pub old_event_address: String,
    pub new_event_address: String,
    pub old_source_object_path: Path,
    pub new_source_object_path: Path,
}

/// A revision-bound ordinary Protocol 2 transaction. Preparation validates the
/// complete candidate and its inverse without mutating the caller's document.
/// Applying this plan changes occurrence identities, which can change ID-seeded
/// randomness, tie ordering and hashes even when musical event values agree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaterializeInstancePlan {
    pub transaction: Transaction,
    pub placement: String,
    pub pattern: String,
    pub mappings: Vec<MaterializedEventMapping>,
    pub diagnostics: Vec<EditError>,
}

const MAX_MAPPINGS: usize = 100_000;

fn limit(message: &str) -> EditError {
    EditError::new("E_RESOURCE_LIMIT", message)
}

fn rational(value: &Value) -> EditResult<BigRational> {
    let n = value["n"]
        .as_str()
        .and_then(|text| text.parse::<BigInt>().ok())
        .ok_or_else(|| EditError::new("E_UNIT", "expected an exact rational"))?;
    let d = value["d"]
        .as_str()
        .and_then(|text| text.parse::<BigInt>().ok())
        .filter(|number| *number > BigInt::zero())
        .ok_or_else(|| EditError::new("E_UNIT", "expected a positive rational denominator"))?;
    checked(BigRational::new(n, d))
}

fn checked(value: BigRational) -> EditResult<BigRational> {
    if value.numer().magnitude().bits() > crate::MAX_RATIONAL_BITS
        || value.denom().magnitude().bits() > crate::MAX_RATIONAL_BITS
    {
        return Err(limit("materialized rational exceeds the bit allowance"));
    }
    Ok(value)
}

fn tagged(value: &BigRational, unit: Option<&str>) -> Value {
    let mut result =
        json!({"t":"number","n":value.numer().to_string(),"d":value.denom().to_string()});
    if let Some(unit) = unit {
        result["t"] = json!("quantity");
        result["u"] = json!(unit);
    }
    result
}

fn scalar(object: &Value, field: &str, default: i64) -> EditResult<BigRational> {
    object["fields"]
        .get(field)
        .map(rational)
        .unwrap_or_else(|| Ok(BigRational::from_integer(default.into())))
}

fn count(object: &Value) -> EditResult<usize> {
    let value = scalar(object, "count", 1)?;
    if !value.is_integer() || value <= BigRational::zero() {
        return Err(EditError::new(
            "E_RANGE",
            "repetition count must be a positive integer",
        ));
    }
    value
        .to_integer()
        .to_usize()
        .filter(|value| *value < MAX_OPERATIONS)
        .ok_or_else(|| limit("materialized repetition count exceeds the transaction bound"))
}

fn reference(id: &str) -> Value {
    json!({"t":"ref","path":[id],"port":null})
}

fn reference_id(value: &Value) -> EditResult<String> {
    if value["t"] != "ref" || !value["port"].is_null() {
        return Err(EditError::new(
            "E_REFERENCE",
            "expected an object reference",
        ));
    }
    let path = value["path"]
        .as_array()
        .ok_or_else(|| EditError::new("E_REFERENCE", "reference path is missing"))?;
    path.iter()
        .map(|part| {
            part.as_str()
                .ok_or_else(|| EditError::new("E_REFERENCE", "invalid reference path"))
        })
        .collect::<EditResult<Vec<_>>>()
        .map(|parts| parts.join("."))
}

fn pattern<'a>(catalog: &'a Value, id: &str) -> EditResult<&'a Value> {
    catalog["objects"]
        .get(id)
        .filter(|object| object["kind"] == "pattern")
        .ok_or_else(|| EditError::new("E_REFERENCE", format!("pattern `{id}` does not exist")))
}

/// Count private invocation patterns before allocating them. This includes
/// empty patterns, so an empty repeated source cannot bypass the work bound.
fn invocation_bound(
    catalog: &Value,
    id: &str,
    depth: usize,
    visiting: &mut BTreeSet<String>,
) -> EditResult<usize> {
    // The new wrapper occupies depth zero; private copies begin at depth one.
    if depth >= crate::compiler::MAX_PATTERN_DEPTH {
        return Err(limit(
            "materialized pattern nesting exceeds the pattern depth bound",
        ));
    }
    if !visiting.insert(id.into()) {
        return Err(EditError::new(
            "E_PATTERN_CYCLE",
            "materialized pattern graph is cyclic",
        ));
    }
    let mut total = 1usize;
    let source = pattern(catalog, id)?;
    for child in source["children"]
        .as_object()
        .into_iter()
        .flat_map(|children| children.values())
    {
        if child["kind"] == "use" {
            let target = reference_id(&child["fields"]["pattern"])?;
            let nested = invocation_bound(catalog, &target, depth + 1, visiting)?;
            total = total.saturating_add(count(child)?.saturating_mul(nested));
            if total >= MAX_OPERATIONS {
                return Err(limit(
                    "materialized invocation count exceeds the transaction bound",
                ));
            }
        }
    }
    visiting.remove(id);
    Ok(total)
}

struct Builder<'a> {
    catalog: &'a Value,
    source_paths: &'a BTreeMap<String, Path>,
    placement: &'a str,
    reserved: BTreeSet<String>,
    serial: usize,
    operations: Vec<Operation>,
    added_bytes: usize,
    mappings: Vec<MaterializedEventMapping>,
    mapping_bytes: usize,
}

impl Builder<'_> {
    fn check_copy_budget(&self, object: &Value) -> EditResult<()> {
        let bytes = wire::bounded_encoding(object, MAX_DOCUMENT_BYTES)?;
        if self.added_bytes.saturating_add(bytes) > MAX_DOCUMENT_BYTES {
            return Err(limit(
                "materialized subtree copy exceeds the document byte bound",
            ));
        }
        if self.reserved.len()
            >= self.catalog["objects"]
                .as_object()
                .map_or(0, Map::len)
                .saturating_add(MAX_OPERATIONS)
        {
            return Err(limit(
                "materialized object allocation exceeds the operation bound",
            ));
        }
        Ok(())
    }

    fn fresh(&mut self) -> EditResult<String> {
        loop {
            let id = format!("_mi{}", self.serial);
            self.serial += 1;
            if self.reserved.insert(id.clone()) {
                return Ok(id);
            }
            if self.serial > self.reserved.len().saturating_add(MAX_OPERATIONS) {
                return Err(limit(
                    "private materialization ID allocation exceeded its bound",
                ));
            }
        }
    }

    fn add_object(&mut self, id: String, object: Value) -> EditResult<()> {
        let bytes = wire::bounded_encoding(&object, MAX_DOCUMENT_BYTES)?;
        self.added_bytes = self.added_bytes.saturating_add(bytes + id.len() + 4);
        if self.added_bytes > MAX_DOCUMENT_BYTES {
            return Err(limit(
                "materialized document exceeds the document byte bound",
            ));
        }
        self.push(Operation::InsertObject {
            parent: Vec::new(),
            id,
            object_value: object,
        })
    }

    fn push(&mut self, operation: Operation) -> EditResult<()> {
        if self.operations.len() >= MAX_OPERATIONS {
            return Err(limit(
                "materialization exceeds the transaction operation bound",
            ));
        }
        self.operations.push(operation);
        Ok(())
    }

    fn mapping(
        &mut self,
        old: String,
        new: String,
        old_path: Path,
        new_path: Path,
    ) -> EditResult<()> {
        if self.mappings.len() >= MAX_MAPPINGS {
            return Err(limit("materialization exceeds the event mapping bound"));
        }
        let mapping = MaterializedEventMapping {
            old_event_address: old,
            new_event_address: new,
            old_source_object_path: old_path,
            new_source_object_path: new_path,
        };
        self.mapping_bytes = self.mapping_bytes.saturating_add(
            serde_json::to_vec(&mapping)
                .map_err(|error| EditError::new("E_SYNTAX", error.to_string()))?
                .len(),
        );
        if self.mapping_bytes > MAX_DOCUMENT_BYTES {
            return Err(limit("materialization provenance exceeds the byte bound"));
        }
        self.mappings.push(mapping);
        Ok(())
    }

    /// References are copied per occurrence. A small memo only shares multiple
    /// references within the same leaf (or one override), never across leaves.
    fn private_dependencies(
        &mut self,
        value: &mut Value,
        memo: &mut BTreeMap<String, String>,
    ) -> EditResult<()> {
        if value["t"] == "ref" && value["port"].is_null() {
            let old = reference_id(value)?;
            let dependency = self.catalog["objects"].get(&old).ok_or_else(|| {
                EditError::new(
                    "E_REFERENCE",
                    format!("materialized dependency `{old}` is missing"),
                )
            })?;
            if !matches!(dependency["kind"].as_str(), Some("curve" | "tuning")) {
                return Err(EditError::new(
                    "E_CAPABILITY",
                    "event dependency is not a curve or tuning",
                ));
            }
            let new = if let Some(new) = memo.get(&old) {
                new.clone()
            } else {
                let new = self.fresh()?;
                // Inserting before traversal also protects against accidental
                // recursive dependency schemas in a future host extension.
                memo.insert(old, new.clone());
                self.check_copy_budget(dependency)?;
                let mut copied = dependency.clone();
                self.private_dependencies(&mut copied, memo)?;
                self.add_object(new.clone(), copied)?;
                new
            };
            *value = reference(&new);
            return Ok(());
        }
        match value {
            Value::Object(fields) => {
                for child in fields.values_mut() {
                    self.private_dependencies(child, memo)?;
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.private_dependencies(item, memo)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn copy_pattern(
        &mut self,
        source_id: &str,
        old_prefix: &str,
        new_prefix: &str,
    ) -> EditResult<String> {
        let source = pattern(self.catalog, source_id)?;
        self.check_copy_budget(source)?;
        let source = source.clone();
        let new_id = self.fresh()?;
        let mut copied = source.clone();
        let mut children = Map::new();
        // Fresh use IDs must not collide with retained leaf IDs or fields.
        let mut local_ids: BTreeSet<String> = source["children"]
            .as_object()
            .into_iter()
            .flat_map(|children| children.keys().cloned())
            .chain(
                source["fields"]
                    .as_object()
                    .into_iter()
                    .flat_map(|fields| fields.keys().cloned()),
            )
            .collect();
        let mut local_serial = 0usize;
        for (child_id, child) in source["children"]
            .as_object()
            .into_iter()
            .flat_map(|children| children.iter())
        {
            match child["kind"].as_str() {
                Some(kind @ ("note" | "chord" | "hit" | "message" | "audio")) => {
                    let mut leaf = child.clone();
                    // Audio leaves share their immutable, hash-pinned asset.
                    if kind != "audio" {
                        self.private_dependencies(&mut leaf, &mut BTreeMap::new())?;
                    }
                    let mut old_path = self
                        .source_paths
                        .get(source_id)
                        .cloned()
                        .unwrap_or_else(|| vec![source_id.to_owned()]);
                    old_path.push(child_id.clone());
                    // Each chord member is its own occurrence, `chord.k`.
                    let leaves = if kind == "chord" {
                        let members = child["fields"]["pitches"]["items"]
                            .as_array()
                            .map_or(0, Vec::len);
                        (0..members).map(|k| format!("{child_id}.{k}")).collect()
                    } else {
                        vec![child_id.clone()]
                    };
                    for leaf_address in leaves {
                        self.mapping(
                            format!("{old_prefix}/{leaf_address}"),
                            format!("{new_prefix}/{leaf_address}"),
                            old_path.clone(),
                            vec![new_id.clone(), child_id.clone()],
                        )?;
                    }
                    children.insert(child_id.clone(), leaf);
                }
                Some("use") => {
                    let target = reference_id(&child["fields"]["pattern"])?;
                    let length = rational(&pattern(self.catalog, &target)?["fields"]["length"])?;
                    let step = checked(scalar(child, "stretch", 1)? * length)?;
                    let at = scalar(child, "at", 0)?;
                    for repetition in 0..count(child)? {
                        let use_id = loop {
                            let candidate = format!("_use{local_serial}");
                            local_serial += 1;
                            if local_ids.insert(candidate.clone()) {
                                break candidate;
                            }
                        };
                        let nested_id = self.copy_pattern(
                            &target,
                            &format!("{old_prefix}/{child_id}/{repetition}"),
                            &format!("{new_prefix}/{use_id}/0"),
                        )?;
                        let mut invocation = child.clone();
                        invocation["fields"]["pattern"] = reference(&nested_id);
                        invocation["fields"]["count"] = tagged(&BigRational::one(), None);
                        let offset =
                            checked(step.clone() * BigRational::from_integer(repetition.into()))?;
                        invocation["fields"]["at"] =
                            tagged(&checked(at.clone() + offset)?, Some("q"));
                        children.insert(use_id, invocation);
                    }
                }
                _ => {
                    return Err(EditError::new(
                        "E_CAPABILITY",
                        "unsupported materialized pattern child",
                    ))
                }
            }
        }
        copied["children"] = Value::Object(children);
        self.add_object(new_id.clone(), copied)?;
        Ok(new_id)
    }
}

fn set_changes(
    builder: &mut Builder<'_>,
    original: &Value,
    candidate: &Value,
    path: Path,
) -> EditResult<()> {
    for (field, value) in candidate["fields"]
        .as_object()
        .into_iter()
        .flat_map(|fields| fields.iter())
    {
        if original["fields"].get(field) != Some(value) {
            builder.push(Operation::Set {
                object: path.clone(),
                field: vec![field.clone()],
                value: value.clone(),
                expect: None,
                expect_absent: false,
            })?;
        }
    }
    for (id, child) in candidate["children"]
        .as_object()
        .into_iter()
        .flat_map(|children| children.iter())
    {
        let mut child_path = path.clone();
        child_path.push(id.clone());
        set_changes(builder, &original["children"][id], child, child_path)?;
    }
    Ok(())
}

pub(super) fn prepare(
    context: &impl EditContext,
    document: &AuthoredDocument,
    catalog: &Value,
    source_paths: &BTreeMap<String, Path>,
    placement_id: &str,
    new_pattern_id: &str,
) -> EditResult<MaterializeInstancePlan> {
    // Check caller-provided IDs through the same closed Protocol 2 wire shape.
    Transaction::new(
        document.revision().into(),
        vec![Operation::InsertObject {
            parent: Vec::new(),
            id: new_pattern_id.into(),
            object_value: json!({"kind":"pattern","fields":{},"children":{}}),
        }],
    )?;
    let original_place = document.tree()["objects"]
        .get(placement_id)
        .filter(|place| place["kind"] == "place")
        .ok_or_else(|| {
            EditError::new("E_REFERENCE", "materialization target must be a placement")
                .at(&[placement_id.into()], &[])
        })?;
    if document.tree()["objects"].get(new_pattern_id).is_some() {
        return Err(
            EditError::new("E_DUPLICATE_ID", "materialized pattern ID already exists")
                .at(&[new_pattern_id.into()], &[]),
        );
    }
    let place = catalog["objects"].get(placement_id).ok_or_else(|| {
        EditError::new(
            "E_REFERENCE",
            "resolved materialization placement is missing",
        )
    })?;
    let source_id = reference_id(&place["fields"]["pattern"])?;
    let repetitions = count(place)?;
    let invocations = invocation_bound(catalog, &source_id, 1, &mut BTreeSet::new())?;
    if repetitions.saturating_mul(invocations).saturating_add(6) > MAX_OPERATIONS {
        return Err(limit(
            "materialized invocations exceed the transaction operation bound",
        ));
    }
    let length = rational(&pattern(catalog, &source_id)?["fields"]["length"])?;
    let stretch = scalar(place, "stretch", 1)?;
    let step = checked(stretch.clone() * length)?;
    let wrapper_length = checked(step.clone() * BigRational::from_integer(repetitions.into()))?;
    let mut reserved: BTreeSet<String> = document.tree()["objects"]
        .as_object()
        .into_iter()
        .flat_map(|objects| objects.keys().cloned())
        .collect();
    reserved.insert(new_pattern_id.into());
    let mut builder = Builder {
        catalog,
        source_paths,
        placement: placement_id,
        reserved,
        serial: 0,
        operations: Vec::new(),
        added_bytes: document.canonical_bytes().len(),
        mappings: Vec::new(),
        mapping_bytes: 0,
    };
    let mut wrapper_children = Map::new();
    for repetition in 0..repetitions {
        let use_id = format!("_repeat{repetition}");
        let copied_id = builder.copy_pattern(
            &source_id,
            &format!("{placement_id}/{repetition}"),
            &format!("{placement_id}/0/{use_id}/0"),
        )?;
        let at = checked(step.clone() * BigRational::from_integer(repetition.into()))?;
        wrapper_children.insert(use_id, json!({
            "kind":"use", "fields":{
                "pattern":reference(&copied_id), "at":tagged(&at, Some("q")),
                "count":tagged(&BigRational::one(), None),
                "stretch":tagged(&stretch, None),
                "transpose":place["fields"].get("transpose").cloned().unwrap_or_else(|| tagged(&BigRational::zero(), Some("ct"))),
                "boundary":place["fields"].get("boundary").cloned().unwrap_or_else(|| json!({"t":"symbol","v":"spill"})),
            }, "children":{}
        }));
    }
    let mut wrapper_fields = Map::new();
    wrapper_fields.insert("length".into(), tagged(&wrapper_length, Some("q")));
    // Labels are retained on copied source patterns as well as the new wrapper.
    if let Some(label) = pattern(catalog, &source_id)?["fields"].get("label") {
        wrapper_fields.insert("label".into(), label.clone());
    }
    builder.add_object(
        new_pattern_id.into(),
        json!({"kind":"pattern","fields":wrapper_fields,"children":wrapper_children}),
    )?;
    let replacements: BTreeMap<String, String> = builder
        .mappings
        .iter()
        .map(|mapping| {
            (
                mapping.old_event_address.clone(),
                mapping.new_event_address.clone(),
            )
        })
        .collect();
    let mut copied_place = original_place.clone();
    for (child_id, child) in place["children"]
        .as_object()
        .into_iter()
        .flat_map(|children| children.iter())
    {
        match child["kind"].as_str() {
            Some("override") => {
                let event = child["fields"]["event"]["v"].as_str().ok_or_else(|| {
                    EditError::new("E_INSTANCE_TARGET", "override target is missing")
                })?;
                let old_address = format!("{placement_id}/{event}");
                let new_address = replacements.get(&old_address).ok_or_else(|| {
                    EditError::new(
                        "E_INSTANCE_TARGET",
                        "override has no materialized event mapping",
                    )
                })?;
                let mut copied = child.clone();
                copied["fields"]["event"] = json!({"t":"string","v":new_address.strip_prefix(&format!("{placement_id}/")).expect("local mapping")});
                if let Some(set) = copied["fields"].get_mut("set") {
                    builder.private_dependencies(set, &mut BTreeMap::new())?;
                }
                copied_place["children"][child_id] = copied;
            }
            Some("insert") => {
                let mut copied = child.clone();
                for (leaf_id, leaf) in copied["children"]
                    .as_object_mut()
                    .into_iter()
                    .flat_map(|children| children.iter_mut())
                {
                    if leaf["kind"] != "audio" {
                        builder.private_dependencies(leaf, &mut BTreeMap::new())?;
                    }
                    let address = format!("{placement_id}/{child_id}/{leaf_id}");
                    let path = vec![placement_id.into(), child_id.clone(), leaf_id.clone()];
                    builder.mapping(address.clone(), address, path.clone(), path)?;
                }
                copied_place["children"][child_id] = copied;
            }
            _ => {
                return Err(EditError::new(
                    "E_CAPABILITY",
                    "unsupported placement materialization child",
                ))
            }
        }
    }
    copied_place["fields"]["pattern"] = reference(new_pattern_id);
    copied_place["fields"]["count"] = tagged(&BigRational::one(), None);
    copied_place["fields"]["stretch"] = tagged(&BigRational::one(), None);
    copied_place["fields"]["transpose"] = tagged(&BigRational::zero(), Some("ct"));
    copied_place["fields"]["boundary"] = json!({"t":"symbol","v":"spill"});
    set_changes(
        &mut builder,
        original_place,
        &copied_place,
        vec![placement_id.into()],
    )?;
    let transaction = Transaction::new(document.revision().into(), builder.operations)?;
    // This also proves inverse size, semantic validity and ordinary kernel work
    // bounds. No externally visible state changes during preparation.
    let mut candidate = document.clone();
    apply_transaction(&mut candidate, &transaction, context)?;
    Ok(MaterializeInstancePlan {
        transaction,
        placement: builder.placement.into(),
        pattern: new_pattern_id.into(),
        mappings: builder.mappings,
        diagnostics: vec![EditError::new(
            "W_IDENTITY_CHANGE",
            "materialized occurrence IDs can change randomness, event/reduction order and hashes",
        )],
    })
}
