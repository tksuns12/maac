//! Source-preserving projection for validated Protocol 2 transactions.

use std::collections::BTreeMap;

use serde_json::Value as JsonValue;

use super::{
    apply_one, apply_transaction, object_paths, wire, AppliedTransaction, AuthoredDocument,
    EditContext, EditError, EditResult, Operation, Path, Transaction,
};
use crate::diagnostic::Span;
use crate::syntax::{Document, Field, Object, Value as SyntaxValue, ValueKind};

/// Editable source text paired with its canonical authored typed graph.
///
/// Transactions are validated against the typed graph first. Only after the
/// complete candidate, inverse, and impact have succeeded are source edits
/// projected into a private string. Untouched source slices are copied
/// byte-for-byte; comments and formatting outside changed values/objects remain
/// unchanged.
#[derive(Clone, Debug)]
pub struct SourceDocument {
    source: String,
    authored: AuthoredDocument,
}

impl SourceDocument {
    pub fn parse(source: impl Into<String>) -> EditResult<Self> {
        let source = source.into();
        let parsed = crate::syntax::parse(&source).map_err(diagnostics_error)?;
        let authored = AuthoredDocument::from_document(&parsed)?;
        Ok(Self { source, authored })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn authored(&self) -> &AuthoredDocument {
        &self.authored
    }

    pub fn revision(&self) -> &str {
        self.authored.revision()
    }

    /// Apply a transaction atomically to both the typed authority and source text.
    ///
    /// Projection reparses after each operation so every later candidate path is
    /// resolved against the text state the user would see. This does not rewrite
    /// the whole document: existing values, IDs, and objects are edited at their
    /// parser spans, while new fields/objects receive deterministic minimal
    /// formatting.
    pub fn apply(
        &mut self,
        transaction: &Transaction,
        context: &impl EditContext,
    ) -> EditResult<AppliedTransaction> {
        let original = self.authored.clone();
        let mut committed = original.clone();
        let result = apply_transaction(&mut committed, transaction, context)?;

        let base = original.tree();
        let mut replay = base.clone();
        let mut identities: BTreeMap<Path, Path> = object_paths(base)
            .into_keys()
            .map(|path| (path.clone(), path))
            .collect();
        let mut text = self.source.clone();

        for operation in &transaction.operations {
            let parsed = crate::syntax::parse(&text).map_err(diagnostics_error)?;
            let before = replay.clone();
            apply_one(base, &mut replay, &mut identities, operation, context)?;
            wire::bounded_encoding(&replay, super::MAX_DOCUMENT_BYTES)?;
            wire::document(&mut replay)?;
            text = project_operation(&text, &parsed, &before, &replay, operation)?;

            let reparsed = crate::syntax::parse(&text).map_err(diagnostics_error)?;
            let projected = AuthoredDocument::from_document(&reparsed)?;
            if projected.tree() != &replay {
                return Err(EditError::new(
                    "E_SYNTAX",
                    "source projection did not reproduce the candidate authored tree",
                ));
            }
        }

        if replay != *committed.tree() {
            return Err(EditError::new(
                "E_SYNTAX",
                "source projection candidate diverged from validated transaction result",
            ));
        }
        let final_parsed = crate::syntax::parse(&text).map_err(diagnostics_error)?;
        let final_authored = AuthoredDocument::from_document(&final_parsed)?;
        if final_authored.revision() != result.new_revision {
            return Err(EditError::new(
                "E_CONFLICT",
                "source projection revision does not match the committed authored revision",
            ));
        }

        self.source = text;
        self.authored = committed;
        Ok(result)
    }
}

fn diagnostics_error(diagnostics: crate::Diagnostics) -> EditError {
    if let Some(first) = diagnostics.first() {
        EditError::new(first.code.as_str(), first.message.clone())
            .at(&first.object_path, &first.field_path)
    } else {
        EditError::new("E_SYNTAX", "source parsing failed without a diagnostic")
    }
}

#[derive(Clone, Debug)]
struct TextEdit {
    start: usize,
    end: usize,
    replacement: String,
}

fn edit(span: Span, replacement: String) -> TextEdit {
    TextEdit {
        start: span.start,
        end: span.end,
        replacement,
    }
}

fn apply_text_edits(source: &str, mut edits: Vec<TextEdit>) -> EditResult<String> {
    edits.sort_by(|a, b| b.start.cmp(&a.start).then_with(|| b.end.cmp(&a.end)));
    let mut previous_start = source.len();
    for item in &edits {
        if item.start > item.end || item.end > source.len() || item.end > previous_start {
            return Err(EditError::new(
                "E_SYNTAX",
                "source projection produced overlapping or invalid text edits",
            ));
        }
        if !source.is_char_boundary(item.start) || !source.is_char_boundary(item.end) {
            return Err(EditError::new(
                "E_SYNTAX",
                "source projection did not align to UTF-8 boundaries",
            ));
        }
        previous_start = item.start;
    }
    let mut result = source.to_owned();
    for item in edits {
        result.replace_range(item.start..item.end, &item.replacement);
    }
    Ok(result)
}

fn project_operation(
    source: &str,
    parsed: &Document,
    before: &JsonValue,
    after: &JsonValue,
    operation: &Operation,
) -> EditResult<String> {
    match operation {
        Operation::Set { object, field, .. } => project_set(source, parsed, after, object, field),
        Operation::Unset { object, field, .. } => {
            let target = syntax_object(parsed, object)?;
            let field = syntax_field(target, field)?;
            apply_text_edits(source, vec![edit(field.span, String::new())])
        }
        Operation::InsertObject { parent, id, .. } => {
            let object = json_object(after, &joined(parent, id))?;
            let body = source_object(id, object, 0)?;
            if parent.is_empty() {
                let separator = if source.ends_with('\n') { "" } else { "\n" };
                apply_text_edits(
                    source,
                    vec![TextEdit {
                        start: source.len(),
                        end: source.len(),
                        replacement: format!("{separator}{body}\n"),
                    }],
                )
            } else {
                let parent_object = syntax_object(parsed, parent)?;
                let indent = line_indent(source, parent_object.span.start);
                let child_indent = format!("{indent}  ");
                let body = source_object(id, object, child_indent.len())?;
                let insertion = format!("\n{child_indent}{body}");
                apply_text_edits(
                    source,
                    vec![TextEdit {
                        start: parent_object.span.end - 1,
                        end: parent_object.span.end - 1,
                        replacement: insertion,
                    }],
                )
            }
        }
        Operation::DeleteObject { object, .. } => {
            let target = syntax_object(parsed, object)?;
            apply_text_edits(source, vec![edit(target.span, String::new())])
        }
        Operation::RenameId { object, new_id } => {
            project_rename(source, parsed, before, after, object, new_id)
        }
    }
}

fn project_set(
    source: &str,
    parsed: &Document,
    after: &JsonValue,
    object_path: &[String],
    field_path: &[String],
) -> EditResult<String> {
    let object = syntax_object(parsed, object_path)?;
    if let Ok(existing) = syntax_field(object, field_path) {
        let value = json_field(json_object(after, object_path)?, field_path)?
            .ok_or_else(|| EditError::new("E_REFERENCE", "candidate field is absent"))?;
        return apply_text_edits(
            source,
            vec![edit(existing.value.span, source_value(value)?)],
        );
    }

    let leaf = field_path
        .last()
        .ok_or_else(|| EditError::new("E_REFERENCE", "field path is empty"))?;
    let value = json_field(json_object(after, object_path)?, field_path)?
        .ok_or_else(|| EditError::new("E_REFERENCE", "candidate field is absent"))?;
    let assignment = format!("{leaf} = {};", source_value(value)?);

    if field_path.len() == 1 {
        let insertion = member_insertion(source, object.span, &assignment);
        return apply_text_edits(source, vec![insertion]);
    }

    let parent_path = &field_path[..field_path.len() - 1];
    let parent = syntax_value(object, parent_path)?;
    if !matches!(parent.kind, ValueKind::Record(_)) {
        return Err(EditError::new(
            "E_REFERENCE",
            "field parent is not an authored record",
        ));
    }
    let insertion = member_insertion(source, parent.span, &assignment);
    apply_text_edits(source, vec![insertion])
}

fn member_insertion(source: &str, span: Span, assignment: &str) -> TextEdit {
    let close = span.end - 1;
    let inside = &source[span.start + 1..close];
    let replacement = if inside.contains('\n') {
        let indent = line_indent(source, span.start);
        format!("\n{indent}  {assignment}")
    } else if inside.trim().is_empty() {
        format!(" {assignment} ")
    } else {
        format!(" {assignment}")
    };
    TextEdit {
        start: close,
        end: close,
        replacement,
    }
}

fn project_rename(
    source: &str,
    parsed: &Document,
    before: &JsonValue,
    after: &JsonValue,
    old_path: &[String],
    new_id: &str,
) -> EditResult<String> {
    let target = syntax_object(parsed, old_path)?;
    let mut edits = vec![edit(target.id_span, new_id.to_owned())];
    let mut new_path = old_path.to_vec();
    *new_path
        .last_mut()
        .ok_or_else(|| EditError::new("E_REFERENCE", "rename path is empty"))? = new_id.to_owned();

    let mut path = Vec::new();
    for (id, object) in &parsed.objects {
        path.push(id.clone());
        collect_rename_value_edits(
            object, &mut path, before, after, old_path, &new_path, &mut edits,
        )?;
        path.pop();
    }
    apply_text_edits(source, edits)
}

fn collect_rename_value_edits(
    object: &Object,
    path: &mut Path,
    before: &JsonValue,
    after: &JsonValue,
    old_path: &[String],
    new_path: &[String],
    edits: &mut Vec<TextEdit>,
) -> EditResult<()> {
    let before_object = json_object(before, path)?;
    let mapped = renamed_path(path, old_path, new_path);
    let after_object = json_object(after, &mapped)?;

    for (name, field) in &object.fields {
        let Some(before_value) = before_object["fields"].get(name) else {
            continue;
        };
        let Some(after_value) = after_object["fields"].get(name) else {
            continue;
        };
        collect_value_edits(&field.value, before_value, after_value, edits)?;
    }

    for (id, child) in &object.children {
        path.push(id.clone());
        collect_rename_value_edits(child, path, before, after, old_path, new_path, edits)?;
        path.pop();
    }
    Ok(())
}

fn collect_value_edits(
    syntax: &SyntaxValue,
    before: &JsonValue,
    after: &JsonValue,
    edits: &mut Vec<TextEdit>,
) -> EditResult<()> {
    if before == after {
        return Ok(());
    }
    match (&syntax.kind, before["t"].as_str(), after["t"].as_str()) {
        (ValueKind::Record(fields), Some("record"), Some("record")) => {
            let before_fields = before["fields"]
                .as_object()
                .ok_or_else(|| EditError::new("E_SYNTAX", "typed record is malformed"))?;
            let after_fields = after["fields"]
                .as_object()
                .ok_or_else(|| EditError::new("E_SYNTAX", "typed record is malformed"))?;
            if before_fields.keys().eq(after_fields.keys()) && fields.len() == before_fields.len() {
                for (name, field) in fields {
                    collect_value_edits(
                        &field.value,
                        &before_fields[name],
                        &after_fields[name],
                        edits,
                    )?;
                }
                return Ok(());
            }
        }
        (ValueKind::List(items), Some("list"), Some("list"))
        | (ValueKind::Tuple(items), Some("tuple"), Some("tuple")) => {
            let before_items = before["items"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "typed sequence is malformed"))?;
            let after_items = after["items"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "typed sequence is malformed"))?;
            if items.len() == before_items.len() && items.len() == after_items.len() {
                for ((item, left), right) in items.iter().zip(before_items).zip(after_items) {
                    collect_value_edits(item, left, right, edits)?;
                }
                return Ok(());
            }
        }
        (ValueKind::Call { args, .. }, Some("call"), Some("call"))
            if before["fn"] == after["fn"] =>
        {
            let before_args = before["args"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "typed call is malformed"))?;
            let after_args = after["args"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "typed call is malformed"))?;
            if args.len() == before_args.len() && args.len() == after_args.len() {
                for ((arg, left), right) in args.iter().zip(before_args).zip(after_args) {
                    collect_value_edits(arg, left, right, edits)?;
                }
                return Ok(());
            }
        }
        _ => {}
    }
    edits.push(edit(syntax.span, source_value(after)?));
    Ok(())
}

fn syntax_object<'a>(document: &'a Document, path: &[String]) -> EditResult<&'a Object> {
    let mut object = document
        .objects
        .get(
            path.first()
                .ok_or_else(|| EditError::new("E_REFERENCE", "empty object path"))?,
        )
        .ok_or_else(|| EditError::new("E_REFERENCE", "object path does not exist"))?;
    for id in &path[1..] {
        object = object
            .children
            .get(id)
            .ok_or_else(|| EditError::new("E_REFERENCE", "object path does not exist"))?;
    }
    Ok(object)
}

fn syntax_field<'a>(object: &'a Object, path: &[String]) -> EditResult<&'a Field> {
    let mut field = object
        .fields
        .get(
            path.first()
                .ok_or_else(|| EditError::new("E_REFERENCE", "empty field path"))?,
        )
        .ok_or_else(|| EditError::new("E_REFERENCE", "field is not authored"))?;
    for name in &path[1..] {
        let ValueKind::Record(fields) = &field.value.kind else {
            return Err(EditError::new(
                "E_REFERENCE",
                "intermediate field is not a record",
            ));
        };
        field = fields
            .get(name)
            .ok_or_else(|| EditError::new("E_REFERENCE", "field is not authored"))?;
    }
    Ok(field)
}

fn syntax_value<'a>(object: &'a Object, path: &[String]) -> EditResult<&'a SyntaxValue> {
    Ok(&syntax_field(object, path)?.value)
}

fn json_object<'a>(tree: &'a JsonValue, path: &[String]) -> EditResult<&'a JsonValue> {
    let mut objects = &tree["objects"];
    let mut object = None;
    for id in path {
        let next = objects
            .get(id)
            .ok_or_else(|| EditError::new("E_REFERENCE", "typed object path does not exist"))?;
        object = Some(next);
        objects = &next["children"];
    }
    object.ok_or_else(|| EditError::new("E_REFERENCE", "typed object path is empty"))
}

fn json_field<'a>(object: &'a JsonValue, path: &[String]) -> EditResult<Option<&'a JsonValue>> {
    let mut fields = &object["fields"];
    for (index, name) in path.iter().enumerate() {
        let Some(value) = fields.get(name) else {
            return Ok(None);
        };
        if index + 1 == path.len() {
            return Ok(Some(value));
        }
        if value["t"] != "record" {
            return Err(EditError::new(
                "E_REFERENCE",
                "intermediate typed field is not a record",
            ));
        }
        fields = &value["fields"];
    }
    Err(EditError::new("E_REFERENCE", "typed field path is empty"))
}

fn joined(parent: &[String], id: &str) -> Path {
    let mut path = parent.to_vec();
    path.push(id.to_owned());
    path
}

fn renamed_path(path: &[String], old: &[String], new: &[String]) -> Path {
    if path.starts_with(old) {
        let mut result = new.to_vec();
        result.extend_from_slice(&path[old.len()..]);
        result
    } else {
        path.to_vec()
    }
}

fn line_indent(source: &str, offset: usize) -> String {
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    source[line_start..offset]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

pub(crate) fn authored_source(tree: &JsonValue) -> EditResult<String> {
    if tree["version"] != 1 {
        return Err(EditError::new(
            "E_VERSION",
            "typed authored tree must use MaaC/1",
        ));
    }
    let objects = tree["objects"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "typed document objects are malformed"))?;
    let mut result = String::from("maac 1;\n");
    for (id, object) in objects {
        result.push_str(&source_object(id, object, 0)?);
        result.push('\n');
    }
    Ok(result)
}

fn source_object(id: &str, object: &JsonValue, indent: usize) -> EditResult<String> {
    let kind = object["kind"]
        .as_str()
        .ok_or_else(|| EditError::new("E_SYNTAX", "typed object kind is malformed"))?;
    let mut result = format!("{kind} {id} {{");
    let child_indent = " ".repeat(indent + 2);
    let close_indent = " ".repeat(indent);
    let fields = object["fields"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "typed object fields are malformed"))?;
    let children = object["children"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "typed object children are malformed"))?;
    for (name, value) in fields {
        result.push_str(&format!(
            "\n{child_indent}{name} = {};",
            source_value(value)?
        ));
    }
    for (child_id, child) in children {
        let rendered = source_object(child_id, child, indent + 2)?;
        result.push_str(&format!("\n{child_indent}{rendered}"));
    }
    if !fields.is_empty() || !children.is_empty() {
        result.push_str(&format!("\n{close_indent}"));
    }
    result.push('}');
    Ok(result)
}

fn source_value(value: &JsonValue) -> EditResult<String> {
    match value["t"].as_str() {
        Some("number") => rational_text(value, ""),
        Some("quantity") => rational_text(
            value,
            value["u"]
                .as_str()
                .ok_or_else(|| EditError::new("E_SYNTAX", "quantity unit is malformed"))?,
        ),
        Some("string") => serde_json::to_string(
            value["v"]
                .as_str()
                .ok_or_else(|| EditError::new("E_SYNTAX", "string payload is malformed"))?,
        )
        .map_err(|error| EditError::new("E_SYNTAX", error.to_string())),
        Some("symbol") => value["v"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| EditError::new("E_SYNTAX", "symbol payload is malformed")),
        Some("boolean") => value["v"]
            .as_bool()
            .map(|boolean| boolean.to_string())
            .ok_or_else(|| EditError::new("E_SYNTAX", "boolean payload is malformed")),
        Some("ref") => {
            let path = value["path"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "reference path is malformed"))?
                .iter()
                .map(|part| {
                    part.as_str()
                        .ok_or_else(|| EditError::new("E_SYNTAX", "reference path is malformed"))
                })
                .collect::<EditResult<Vec<_>>>()?
                .join(".");
            let port = match &value["port"] {
                JsonValue::Null => String::new(),
                JsonValue::String(port) => format!(":{port}"),
                _ => return Err(EditError::new("E_SYNTAX", "reference port is malformed")),
            };
            Ok(format!("&{path}{port}"))
        }
        Some("call") => {
            let function = value["fn"]
                .as_str()
                .ok_or_else(|| EditError::new("E_SYNTAX", "call function is malformed"))?;
            let args = value["args"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "call arguments are malformed"))?
                .iter()
                .map(source_value)
                .collect::<EditResult<Vec<_>>>()?
                .join(", ");
            Ok(format!("{function}({args})"))
        }
        Some("list") | Some("tuple") => {
            let items = value["items"]
                .as_array()
                .ok_or_else(|| EditError::new("E_SYNTAX", "sequence is malformed"))?
                .iter()
                .map(source_value)
                .collect::<EditResult<Vec<_>>>()?
                .join(", ");
            if value["t"] == "list" {
                Ok(format!("[{items}]"))
            } else {
                Ok(format!("({items})"))
            }
        }
        Some("record") => {
            let fields = value["fields"]
                .as_object()
                .ok_or_else(|| EditError::new("E_SYNTAX", "record is malformed"))?;
            if fields.is_empty() {
                return Ok("{}".to_owned());
            }
            let mut parts = Vec::with_capacity(fields.len());
            for (name, field) in fields {
                parts.push(format!("{name} = {};", source_value(field)?));
            }
            Ok(format!("{{ {} }}", parts.join(" ")))
        }
        _ => Err(EditError::new(
            "E_SYNTAX",
            "unknown typed value in source projection",
        )),
    }
}

fn rational_text(value: &JsonValue, unit: &str) -> EditResult<String> {
    let numerator = value["n"]
        .as_str()
        .ok_or_else(|| EditError::new("E_SYNTAX", "rational numerator is malformed"))?;
    let denominator = value["d"]
        .as_str()
        .ok_or_else(|| EditError::new("E_SYNTAX", "rational denominator is malformed"))?;
    if denominator == "1" {
        Ok(format!("{numerator}{unit}"))
    } else {
        Ok(format!("{numerator}/{denominator}{unit}"))
    }
}
