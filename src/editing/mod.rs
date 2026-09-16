//! Authored revision identity and a bounded, host-integrated Protocol 2 kernel.
//!
//! This module performs no I/O and never invokes the renderer. [`EditContext`]
//! is a required, trusted semantic boundary, not an optional validation hook.
//! No general MaaC semantic context or source-text editor is supplied here.
//! Consequently this API alone does not establish Document conformance.

mod foundation;
mod source;
mod wire;

pub use foundation::FoundationEditContext;
pub use source::SourceDocument;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub const REVISION_ALGORITHM: &str = "maac.revision.authored.sha256/1";
pub const PROTOCOL_VERSION: u32 = 2;
pub const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TRANSACTION_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_OPERATIONS: usize = 1024;
pub const MAX_IMPACT_PATHS: usize = 1024;
const MAX_WORK_BYTES: usize = 256 * 1024 * 1024;

pub type Path = Vec<String>;
pub type EditResult<T> = Result<T, EditError>;

/// A typed-tree diagnostic. There is no source span without a text mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EditError {
    pub code: String,
    pub message: String,
    pub object_path: Path,
    pub field_path: Path,
}

impl EditError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            object_path: Vec::new(),
            field_path: Vec::new(),
        }
    }

    pub fn at(mut self, object: &[String], field: &[String]) -> Self {
        self.object_path = object.to_vec();
        self.field_path = field.to_vec();
        self
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EditError {}

fn error(code: &str, message: &str) -> EditError {
    EditError::new(code, message)
}

/// A shape-checked authored typed tree. Semantic validity is a separate gate.
/// Field omissions, explicit empty records, units, constructors and labels are
/// retained; only rational scalar representation is canonicalized.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoredDocument {
    tree: Value,
    canonical: Vec<u8>,
    revision: String,
}

impl AuthoredDocument {
    pub fn from_json(bytes: &[u8]) -> EditResult<Self> {
        Self::from_value(wire::read_json(bytes, MAX_DOCUMENT_BYTES)?)
    }

    pub fn from_document(document: &crate::syntax::Document) -> EditResult<Self> {
        wire::preflight_document(document)?;
        Self::from_value(document.to_syntax_json_value())
    }

    fn from_value(mut tree: Value) -> EditResult<Self> {
        wire::bounded_encoding(&tree, MAX_DOCUMENT_BYTES)?;
        wire::document(&mut tree)?;
        let canonical = crate::production_identity::canonical_json_bytes(&tree)
            .map_err(|e| EditError::new("E_SYNTAX", e.to_string()))?;
        if canonical.len() > MAX_DOCUMENT_BYTES {
            return Err(error("E_RESOURCE_LIMIT", "canonical document is too large"));
        }
        let revision = format!("sha256:{:x}", Sha256::digest(&canonical));
        Ok(Self {
            tree,
            canonical,
            revision,
        })
    }

    pub fn tree(&self) -> &Value {
        &self.tree
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    Set {
        object: Path,
        field: Path,
        value: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        expect: Option<Value>,
        #[serde(skip_serializing_if = "is_false")]
        expect_absent: bool,
    },
    Unset {
        object: Path,
        field: Path,
        #[serde(skip_serializing_if = "Option::is_none")]
        expect: Option<Value>,
    },
    InsertObject {
        parent: Path,
        id: String,
        object_value: Value,
    },
    DeleteObject {
        object: Path,
        #[serde(skip_serializing_if = "Option::is_none")]
        expect_object: Option<Value>,
    },
    RenameId {
        object: Path,
        new_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Transaction {
    version: u32,
    pub base_revision: String,
    pub operations: Vec<Operation>,
}

impl Transaction {
    pub fn new(base_revision: String, operations: Vec<Operation>) -> EditResult<Self> {
        let transaction = Self {
            version: PROTOCOL_VERSION,
            base_revision,
            operations,
        };
        // The same boundary applies to programmatic and serialized callers.
        Self::from_json(&transaction.to_json()?)
    }

    pub fn from_json(bytes: &[u8]) -> EditResult<Self> {
        wire::transaction(wire::read_json(bytes, MAX_TRANSACTION_BYTES)?)
    }

    pub fn to_json(&self) -> EditResult<Vec<u8>> {
        wire::preflight_transaction(self)?;
        let bytes =
            serde_json::to_vec(self).map_err(|e| EditError::new("E_SYNTAX", e.to_string()))?;
        if bytes.len() > MAX_TRANSACTION_BYTES {
            return Err(error("E_RESOURCE_LIMIT", "transaction is too large"));
        }
        Ok(bytes)
    }

    pub fn version(&self) -> u32 {
        self.version
    }
}

/// Required semantic services, provided by a trusted host implementation.
///
/// Implementations MUST be pure with respect to externally visible state,
/// enforce their own work/dependency bounds, and reject unsupported contexts
/// with E_CAPABILITY. Returning the input unchanged is not a general normalizer.
/// All expectation methods receive the immutable original authored base, even
/// after a candidate rename, meter edit, deletion or insertion.
pub trait EditContext {
    /// Validate the complete source meaning: references, types, required fields,
    /// temporal/pattern/override/writer/causality constraints and dependencies.
    /// A shape check or renderer-only check is not sufficient.
    fn validate_document(&self, authored: &Value) -> EditResult<()>;

    /// Normalize an object subtree in the fixed base type/name/meter/dependency
    /// context. Retain labels and expand declared defaults. `object` may be an
    /// expectation rather than the actual base object. Never alter `base`.
    fn normalize_object(
        &self,
        base: &Value,
        base_path: &[String],
        object: &Value,
    ) -> EditResult<Value>;

    /// Normalize a field expectation in its fixed base semantic type. Do not
    /// evaluate it in the candidate or equate pitches merely by frequency.
    fn normalize_value(
        &self,
        base: &Value,
        base_path: &[String],
        field: &[String],
        value: &Value,
    ) -> EditResult<Value>;

    /// Rewrite occurrence-address strings and schema-declared structural
    /// mappings for a rename. Generic tagged references are already rewritten.
    /// The object has moved to `new_path`; `before` is the pre-rename candidate.
    /// Unsupported schemas/mappings MUST fail, not be ignored. This callback
    /// never receives or rewrites the remaining transaction operations.
    fn rewrite_structural_references(
        &self,
        before: &Value,
        candidate: &mut Value,
        old_path: &[String],
        new_path: &[String],
    ) -> EditResult<()>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Rename {
    pub from: Path,
    pub to: Path,
}

/// Conservative bounded impact, not a claim of minimal cache invalidation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EditImpact {
    pub affected_source_paths: Vec<Path>,
    pub total_affected_source_paths: usize,
    pub source_paths_truncated: bool,
    pub all_expanded_events_may_be_affected: bool,
    pub full_render_invalidated: bool,
    pub renamed_identities: Vec<Rename>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppliedTransaction {
    pub new_revision: String,
    pub inverse: Transaction,
    pub impact: EditImpact,
    pub diagnostics: Vec<EditError>,
}

/// Apply atomically to the typed authored tree. Source text is not rewritten.
///
/// All fallible work, including final validation, inverse construction and
/// bounds checks, finishes before the single assignment to `document`.
/// A callback error leaves the caller's tree and revision unchanged.
pub fn apply_transaction(
    document: &mut AuthoredDocument,
    transaction: &Transaction,
    context: &impl EditContext,
) -> EditResult<AppliedTransaction> {
    let encoded = transaction.to_json()?;
    let transaction = Transaction::from_json(&encoded)?;
    if transaction.base_revision != document.revision {
        return Err(error(
            "E_CONFLICT",
            "base revision does not match authored source",
        ));
    }
    check_work(
        document.canonical.len(),
        encoded.len(),
        transaction.operations.len(),
    )?;
    context.validate_document(&document.tree)?;
    let base = &document.tree;
    let mut candidate = base.clone();
    let mut identities: BTreeMap<Path, Path> = object_paths(base)
        .into_keys()
        .map(|path| (path.clone(), path))
        .collect();
    let mut renames = Vec::new();
    for operation in &transaction.operations {
        apply_one(base, &mut candidate, &mut identities, operation, context)?;
        if let Operation::RenameId { object, new_id } = operation {
            let mut to = object.clone();
            *to.last_mut().expect("validated nonempty path") = new_id.clone();
            renames.push(Rename {
                from: object.clone(),
                to,
            });
        }
        // Intermediate semantic invalidity is permitted, but resource and
        // typed-tree structure limits apply throughout the private transaction.
        wire::bounded_encoding(&candidate, MAX_DOCUMENT_BYTES)?;
        wire::document(&mut candidate)?;
    }
    context.validate_document(&candidate)?;
    let next = AuthoredDocument::from_value(candidate)?;
    let inverse = make_inverse(base, &next)?;
    let impact = impact(base, &next.tree, renames);
    let diagnostics = if impact.renamed_identities.is_empty() {
        Vec::new()
    } else {
        vec![error(
            "W_IDENTITY_CHANGE",
            "renamed IDs can change randomness, event/reduction order and hashes",
        )]
    };
    let result = AppliedTransaction {
        new_revision: next.revision.clone(),
        inverse,
        impact,
        diagnostics,
    };
    *document = next;
    Ok(result)
}

fn object_at<'a>(tree: &'a Value, path: &[String]) -> EditResult<&'a Value> {
    let mut objects = &tree["objects"];
    let mut object = None;
    for id in path {
        let next = objects
            .get(id)
            .ok_or_else(|| error("E_REFERENCE", "object path does not exist").at(path, &[]))?;
        object = Some(next);
        objects = &next["children"];
    }
    object.ok_or_else(|| error("E_REFERENCE", "object path is empty").at(path, &[]))
}

fn object_at_mut<'a>(tree: &'a mut Value, path: &[String]) -> EditResult<&'a mut Value> {
    let mut objects = &mut tree["objects"];
    for (index, id) in path.iter().enumerate() {
        let next = objects
            .get_mut(id)
            .ok_or_else(|| error("E_REFERENCE", "object path does not exist").at(path, &[]))?;
        if index + 1 == path.len() {
            return Ok(next);
        }
        objects = &mut next["children"];
    }
    Err(error("E_REFERENCE", "object path is empty").at(path, &[]))
}

fn children_mut<'a>(
    tree: &'a mut Value,
    parent: &[String],
) -> EditResult<&'a mut Map<String, Value>> {
    let value = if parent.is_empty() {
        &mut tree["objects"]
    } else {
        &mut object_at_mut(tree, parent)?["children"]
    };
    value
        .as_object_mut()
        .ok_or_else(|| error("E_REFERENCE", "parent is not an object container").at(parent, &[]))
}

fn field_at<'a>(object: &'a Value, field: &[String]) -> EditResult<Option<&'a Value>> {
    let mut fields = &object["fields"];
    for (index, name) in field.iter().enumerate() {
        let Some(value) = fields.get(name) else {
            return Ok(None);
        };
        if index + 1 == field.len() {
            return Ok(Some(value));
        }
        if value.get("t").and_then(Value::as_str) != Some("record") {
            return Err(error("E_REFERENCE", "intermediate field is not a record"));
        }
        fields = &value["fields"];
    }
    Err(error("E_REFERENCE", "field path is empty"))
}

fn field_parent_mut<'a>(
    object: &'a mut Value,
    field: &[String],
) -> EditResult<&'a mut Map<String, Value>> {
    let mut fields = &mut object["fields"];
    for name in &field[..field.len() - 1] {
        let value = fields
            .get_mut(name)
            .ok_or_else(|| error("E_REFERENCE", "intermediate record is absent"))?;
        if value.get("t").and_then(Value::as_str) != Some("record") {
            return Err(error("E_REFERENCE", "intermediate field is not a record"));
        }
        fields = &mut value["fields"];
    }
    fields
        .as_object_mut()
        .ok_or_else(|| error("E_REFERENCE", "field parent is not a record"))
}

fn check_field_expectation(
    base: &Value,
    identities: &BTreeMap<Path, Path>,
    object: &[String],
    field: &[String],
    expectation: Option<&Value>,
    expect_absent: bool,
    context: &impl EditContext,
) -> EditResult<()> {
    if expectation.is_none() && !expect_absent {
        return Ok(());
    }
    let base_path = identities.get(object).ok_or_else(|| {
        error("E_CONFLICT", "precondition has no surviving base identity").at(object, field)
    })?;
    let original = object_at(base, base_path)?;
    if expect_absent {
        if field_at(original, field)?.is_some() {
            return Err(error("E_CONFLICT", "field was authored in the base").at(object, field));
        }
        return Ok(());
    }
    let mut normalized = context.normalize_object(base, base_path, original)?;
    wire::typed_object(&mut normalized)?;
    let actual = field_at(&normalized, field)?.ok_or_else(|| {
        error(
            "E_CONFLICT",
            "base field is absent and has no declared default",
        )
        .at(object, field)
    })?;
    let mut expected = context.normalize_value(
        base,
        base_path,
        field,
        expectation.expect("expectation checked above"),
    )?;
    wire::typed_value(&mut expected)?;
    if actual != &expected {
        return Err(
            error("E_CONFLICT", "field precondition does not match the base").at(object, field),
        );
    }
    Ok(())
}

fn apply_one(
    base: &Value,
    candidate: &mut Value,
    identities: &mut BTreeMap<Path, Path>,
    operation: &Operation,
    context: &impl EditContext,
) -> EditResult<()> {
    match operation {
        Operation::Set {
            object,
            field,
            value,
            expect,
            expect_absent,
        } => {
            object_at(candidate, object)?;
            check_field_expectation(
                base,
                identities,
                object,
                field,
                expect.as_ref(),
                *expect_absent,
                context,
            )
            .map_err(|e| e.at(object, field))?;
            let target = object_at_mut(candidate, object)?;
            if field.len() == 1 && target["children"].get(&field[0]).is_some() {
                return Err(
                    error("E_DUPLICATE_FIELD", "field conflicts with a child ID").at(object, field),
                );
            }
            field_parent_mut(target, field)
                .map_err(|e| e.at(object, field))?
                .insert(
                    field.last().expect("validated field path").clone(),
                    value.clone(),
                );
        }
        Operation::Unset {
            object,
            field,
            expect,
        } => {
            object_at(candidate, object)?;
            check_field_expectation(
                base,
                identities,
                object,
                field,
                expect.as_ref(),
                false,
                context,
            )
            .map_err(|e| e.at(object, field))?;
            let removed = field_parent_mut(object_at_mut(candidate, object)?, field)
                .map_err(|e| e.at(object, field))?
                .remove(field.last().expect("validated field path"));
            if removed.is_none() {
                return Err(
                    error("E_REFERENCE", "cannot unset an unauthored field").at(object, field)
                );
            }
        }
        Operation::InsertObject {
            parent,
            id,
            object_value,
        } => {
            if !parent.is_empty() && object_at(candidate, parent)?["fields"].get(id).is_some() {
                return Err(
                    error("E_DUPLICATE_FIELD", "child ID conflicts with a field").at(parent, &[]),
                );
            }
            let children = children_mut(candidate, parent)?;
            if children.contains_key(id) {
                return Err(error("E_DUPLICATE_ID", "insert requires an unused ID").at(parent, &[]));
            }
            children.insert(id.clone(), object_value.clone());
            // Inserted subtrees deliberately acquire no immutable-base identity.
        }
        Operation::DeleteObject {
            object,
            expect_object,
        } => {
            object_at(candidate, object)?;
            if let Some(expected) = expect_object {
                let base_path = identities.get(object).ok_or_else(|| {
                    error("E_CONFLICT", "precondition has no surviving base identity")
                        .at(object, &[])
                })?;
                let mut actual =
                    context.normalize_object(base, base_path, object_at(base, base_path)?)?;
                let mut expected = context.normalize_object(base, base_path, expected)?;
                wire::typed_object(&mut actual)?;
                wire::typed_object(&mut expected)?;
                if actual != expected {
                    return Err(
                        error("E_CONFLICT", "object precondition does not match the base")
                            .at(object, &[]),
                    );
                }
            }
            children_mut(candidate, &object[..object.len() - 1])?
                .remove(object.last().expect("validated object path"));
            identities.retain(|path, _| !path.starts_with(object));
        }
        Operation::RenameId { object, new_id } => {
            object_at(candidate, object)?;
            let parent = &object[..object.len() - 1];
            if !parent.is_empty()
                && object_at(candidate, parent)?["fields"]
                    .get(new_id)
                    .is_some()
            {
                return Err(
                    error("E_DUPLICATE_FIELD", "new ID conflicts with a field").at(parent, &[])
                );
            }
            let before = candidate.clone();
            let siblings = children_mut(candidate, parent)?;
            if siblings.contains_key(new_id) {
                return Err(
                    error("E_DUPLICATE_ID", "rename requires an unused sibling ID").at(object, &[]),
                );
            }
            let moved = siblings
                .remove(object.last().expect("validated object path"))
                .expect("resolved object exists");
            siblings.insert(new_id.clone(), moved);
            let mut new_path = parent.to_vec();
            new_path.push(new_id.clone());
            rewrite_references(candidate, object, &new_path);
            context.rewrite_structural_references(&before, candidate, object, &new_path)?;
            let moved: Vec<_> = identities
                .iter()
                .filter(|(path, _)| path.starts_with(object))
                .map(|(path, original)| {
                    let mut renamed = new_path.clone();
                    renamed.extend_from_slice(&path[object.len()..]);
                    (renamed, original.clone())
                })
                .collect();
            identities.retain(|path, _| !path.starts_with(object));
            identities.extend(moved);
        }
    }
    Ok(())
}

fn rewrite_references(tree: &mut Value, old: &[String], new: &[String]) {
    fn visit(objects: &mut Value, old: &[String], new: &[String]) {
        for object in objects
            .as_object_mut()
            .expect("validated objects")
            .values_mut()
        {
            for field in object["fields"]
                .as_object_mut()
                .expect("validated fields")
                .values_mut()
            {
                rewrite_references_value(field, old, new);
            }
            visit(&mut object["children"], old, new);
        }
    }
    visit(&mut tree["objects"], old, new);
}

// A value-root walker also used for nested values; it never examines arbitrary
// strings, object keys, or transaction payloads as if they were references.
fn rewrite_references_value(value: &mut Value, old: &[String], new: &[String]) {
    if value.get("t").and_then(Value::as_str) == Some("ref") {
        let path = value["path"].as_array_mut().expect("validated reference");
        if path.len() >= old.len()
            && path
                .iter()
                .zip(old)
                .all(|(a, b)| a.as_str() == Some(b.as_str()))
        {
            let mut replacement: Vec<_> = new.iter().cloned().map(Value::String).collect();
            replacement.extend_from_slice(&path[old.len()..]);
            *path = replacement;
        }
    } else if value.get("t").and_then(Value::as_str) == Some("record") {
        for nested in value["fields"]
            .as_object_mut()
            .expect("validated record")
            .values_mut()
        {
            rewrite_references_value(nested, old, new);
        }
    } else {
        let key = match value.get("t").and_then(Value::as_str) {
            Some("list" | "tuple") => "items",
            Some("call") => "args",
            _ => return,
        };
        for nested in value[key].as_array_mut().expect("validated sequence") {
            rewrite_references_value(nested, old, new);
        }
    }
}

fn object_paths(tree: &Value) -> BTreeMap<Path, &Value> {
    fn visit<'a>(objects: &'a Value, parent: &mut Path, result: &mut BTreeMap<Path, &'a Value>) {
        for (id, object) in objects.as_object().expect("validated objects") {
            parent.push(id.clone());
            result.insert(parent.clone(), object);
            visit(&object["children"], parent, result);
            parent.pop();
        }
    }
    let mut result = BTreeMap::new();
    visit(&tree["objects"], &mut Vec::new(), &mut result);
    result
}

fn check_work(
    document_bytes: usize,
    transaction_bytes: usize,
    operations: usize,
) -> EditResult<()> {
    let work = document_bytes
        .checked_add(transaction_bytes)
        .and_then(|n| n.checked_mul(operations + 4));
    if work.is_none_or(|n| n > MAX_WORK_BYTES) {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "transaction work allowance exceeded",
        ));
    }
    Ok(())
}

fn make_inverse(base: &Value, next: &AuthoredDocument) -> EditResult<Transaction> {
    let old = base["objects"].as_object().expect("validated objects");
    let new = next.tree["objects"].as_object().expect("validated objects");
    let keys: BTreeSet<_> = old.keys().chain(new.keys()).collect();
    let mut changed: Vec<_> = keys
        .into_iter()
        .filter(|id| old.get(*id) != new.get(*id))
        .collect();
    // Protocol 2 has no empty transaction. For an unchanged valid document,
    // replacing one complete object is an authored-tree identity operation.
    if changed.is_empty() {
        changed.push(old.keys().next().ok_or_else(|| {
            error(
                "E_RANGE",
                "a valid editable core document must contain a project",
            )
        })?);
    }
    let mut operations = Vec::new();
    // Delete every changed/new root before inserting old roots: a rename's
    // inverse must not transiently duplicate two almost-limit-sized subtrees.
    for id in &changed {
        if new.contains_key(*id) {
            operations.push(Operation::DeleteObject {
                object: vec![(*id).clone()],
                expect_object: None,
            });
        }
    }
    for id in changed {
        if let Some(original) = old.get(id) {
            operations.push(Operation::InsertObject {
                parent: Vec::new(),
                id: id.clone(),
                object_value: original.clone(),
            });
        }
    }
    // No intermediate-state expectations are copied. Check the inverse's
    // complete acceptance budget before committing the forward transaction.
    let inverse = Transaction::new(next.revision.clone(), operations)?;
    check_work(
        next.canonical.len(),
        inverse.to_json()?.len(),
        inverse.operations.len(),
    )?;
    Ok(inverse)
}

fn impact(base: &Value, candidate: &Value, renames: Vec<Rename>) -> EditImpact {
    let before = object_paths(base);
    let after = object_paths(candidate);
    let keys: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    let mut paths = Vec::new();
    let mut total = 0;
    for path in keys {
        if before.get(path) != after.get(path) {
            total += 1;
            if paths.len() < MAX_IMPACT_PATHS {
                paths.push(path.clone());
            }
        }
    }
    EditImpact {
        source_paths_truncated: total > paths.len(),
        affected_source_paths: paths,
        total_affected_source_paths: total,
        all_expanded_events_may_be_affected: base != candidate,
        full_render_invalidated: base != candidate,
        renamed_identities: renames,
    }
}
