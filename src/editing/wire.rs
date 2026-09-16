//! Closed wire shapes and bounded tagged-value validation; no semantic inference.

use std::fmt;
use std::str::FromStr;

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, Zero};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::{error, EditError, EditResult, Operation, Path, Transaction};
use super::{MAX_DOCUMENT_BYTES, MAX_OPERATIONS, MAX_TRANSACTION_BYTES, PROTOCOL_VERSION};

const MAX_JSON_DEPTH: usize = 96;
const MAX_VALUES: usize = 200_000;
const MAX_TYPED_DEPTH: usize = 64;

struct Unique(Value);

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = Unique;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON with unique keys and integer-only JSON numbers")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Unique, E> {
                Ok(Unique(Value::Bool(value)))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }

            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Unique, E> {
                Err(E::custom(
                    "JSON floating-point numbers are forbidden; use tagged rationals",
                ))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Unique, E> {
                Ok(Unique(Value::String(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Unique, E> {
                Ok(Unique(Value::String(value)))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Unique, A::Error> {
                let mut items = Vec::new();
                while let Some(Unique(value)) = sequence.next_element()? {
                    if items.len() >= MAX_VALUES {
                        return Err(de::Error::custom("maac-edit resource limit: JSON sequence"));
                    }
                    items.push(value);
                }
                Ok(Unique(Value::Array(items)))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
                let mut fields = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if fields.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate JSON key `{key}`")));
                    }
                    if fields.len() >= MAX_VALUES {
                        return Err(de::Error::custom("maac-edit resource limit: JSON object"));
                    }
                    let Unique(value) = map.next_value()?;
                    fields.insert(key, value);
                }
                Ok(Unique(Value::Object(fields)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

pub(super) fn read_json(bytes: &[u8], limit: usize) -> EditResult<Value> {
    if bytes.len() > limit {
        return Err(error("E_RESOURCE_LIMIT", "JSON input byte limit exceeded"));
    }
    // Bound physical JSON nesting before entering Serde's recursive parser.
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > MAX_JSON_DEPTH {
                        return Err(error("E_RESOURCE_LIMIT", "JSON nesting limit exceeded"));
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let Unique(value) = Unique::deserialize(&mut deserializer).map_err(|e| {
        let message = e.to_string();
        let code = if message.starts_with("maac-edit resource limit:") {
            "E_RESOURCE_LIMIT"
        } else {
            "E_SYNTAX"
        };
        EditError::new(code, message)
    })?;
    deserializer
        .end()
        .map_err(|e| EditError::new("E_SYNTAX", e.to_string()))?;
    bounded_encoding(&value, limit)?;
    Ok(value)
}

fn string_size(text: &str) -> usize {
    2 + text
        .bytes()
        .map(|byte| match byte {
            b'"' | b'\\' => 2,
            0..=31 => 6,
            _ => 1,
        })
        .sum::<usize>()
}

/// Check programmatically built Values without recursively serializing first.
/// Returns an upper bound using canonical control-character escapes.
pub(super) fn bounded_encoding(value: &Value, limit: usize) -> EditResult<usize> {
    let mut stack = vec![(value, 0usize)];
    let (mut bytes, mut values) = (0usize, 0usize);
    while let Some((value, depth)) = stack.pop() {
        values += 1;
        if values > MAX_VALUES || depth > MAX_JSON_DEPTH {
            return Err(error("E_RESOURCE_LIMIT", "JSON structure limit exceeded"));
        }
        match value {
            Value::Null => bytes += 4,
            Value::Bool(value) => bytes += if *value { 4 } else { 5 },
            Value::Number(number) => {
                if !number.is_i64() && !number.is_u64() {
                    return Err(error(
                        "E_SYNTAX",
                        "JSON floating-point values are forbidden",
                    ));
                }
                bytes += number.to_string().len();
            }
            Value::String(text) => {
                if text.len() > limit {
                    return Err(error("E_RESOURCE_LIMIT", "JSON string limit exceeded"));
                }
                bytes += string_size(text);
            }
            Value::Array(items) => {
                if items.len() > MAX_VALUES {
                    return Err(error("E_RESOURCE_LIMIT", "JSON sequence limit exceeded"));
                }
                bytes += 2 + items.len().saturating_sub(1);
                stack.extend(items.iter().map(|item| (item, depth + 1)));
            }
            Value::Object(fields) => {
                if fields.len() > MAX_VALUES {
                    return Err(error("E_RESOURCE_LIMIT", "JSON object limit exceeded"));
                }
                bytes += 2 + fields.len().saturating_sub(1);
                for (name, child) in fields {
                    if name.len() > limit {
                        return Err(error("E_RESOURCE_LIMIT", "JSON key limit exceeded"));
                    }
                    bytes += string_size(name) + 1;
                    stack.push((child, depth + 1));
                }
            }
        }
        if bytes > limit {
            return Err(error("E_RESOURCE_LIMIT", "JSON byte limit exceeded"));
        }
    }
    Ok(bytes)
}

fn keys(fields: &Map<String, Value>, required: &[&str], optional: &[&str]) -> EditResult<()> {
    if required.iter().any(|name| !fields.contains_key(*name)) {
        return Err(error("E_SYNTAX", "required wire field is missing"));
    }
    if fields
        .keys()
        .any(|name| !required.contains(&name.as_str()) && !optional.contains(&name.as_str()))
    {
        return Err(error("E_SYNTAX", "unknown wire field"));
    }
    Ok(())
}

fn record(value: &Value) -> EditResult<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| error("E_SYNTAX", "expected a JSON object"))
}

fn text(value: &Value) -> EditResult<&str> {
    value
        .as_str()
        .ok_or_else(|| error("E_SYNTAX", "expected a string"))
}

fn identifier(value: &str) -> EditResult<()> {
    let bytes = value.as_bytes();
    if bytes.len() > 128 {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "identifier exceeds 128 ASCII bytes",
        ));
    }
    if bytes.is_empty()
        || !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return Err(error("E_SYNTAX", "invalid source identifier"));
    }
    Ok(())
}

fn path(value: &Value, allow_empty: bool) -> EditResult<Path> {
    let values = value
        .as_array()
        .ok_or_else(|| error("E_SYNTAX", "path must be an array"))?;
    if values.len() > MAX_TYPED_DEPTH {
        return Err(error("E_RESOURCE_LIMIT", "path nesting limit exceeded"));
    }
    if values.is_empty() && !allow_empty {
        return Err(error("E_SYNTAX", "path must be nonempty"));
    }
    values
        .iter()
        .map(|value| {
            let value = text(value)?;
            identifier(value)?;
            Ok(value.to_owned())
        })
        .collect()
}

fn integer(value: &Value, denominator: bool) -> EditResult<BigInt> {
    let text = text(value)?;
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.len() > 1234 {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "rational component exceeds the bit allowance",
        ));
    }
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return Err(error("E_SYNTAX", "invalid rational integer string"));
    }
    let integer = BigInt::from_str(text).map_err(|_| error("E_SYNTAX", "invalid integer"))?;
    if integer.magnitude().bits() > crate::MAX_RATIONAL_BITS {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "rational component exceeds the bit allowance",
        ));
    }
    if denominator && (integer.is_zero() || integer.is_negative()) {
        return Err(error("E_RANGE", "rational denominator must be positive"));
    }
    Ok(integer)
}

pub(super) fn document(tree: &mut Value) -> EditResult<()> {
    let fields = record(tree)?;
    keys(fields, &["version", "objects"], &[])?;
    let version = fields["version"]
        .as_u64()
        .ok_or_else(|| error("E_SYNTAX", "typed-tree version must be an integer"))?;
    if version != 1 {
        return Err(error(
            "E_CAPABILITY",
            "only typed syntax-tree version 1 is supported",
        ));
    }
    let objects = tree["objects"]
        .as_object_mut()
        .ok_or_else(|| error("E_SYNTAX", "objects must be an ID dictionary"))?;
    let mut visits = 0;
    for (id, object) in objects {
        identifier(id)?;
        object_inner(object, 0, &mut visits)?;
    }
    Ok(())
}

pub(super) fn typed_object(object: &mut Value) -> EditResult<()> {
    bounded_encoding(object, MAX_DOCUMENT_BYTES)?;
    object_inner(object, 0, &mut 0)
}

pub(super) fn typed_value(value: &mut Value) -> EditResult<()> {
    bounded_encoding(value, MAX_TRANSACTION_BYTES)?;
    value_inner(value, 0, &mut 0)
}

fn visit(depth: usize, visits: &mut usize) -> EditResult<()> {
    *visits += 1;
    if depth > MAX_TYPED_DEPTH || *visits > MAX_VALUES {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "typed-tree structure limit exceeded",
        ));
    }
    Ok(())
}

fn object_inner(object: &mut Value, depth: usize, visits: &mut usize) -> EditResult<()> {
    visit(depth, visits)?;
    let map = record(object)?;
    keys(map, &["kind", "fields", "children"], &[])?;
    identifier(text(&map["kind"])?)?;
    let fields = record(&map["fields"])?;
    let children = record(&map["children"])?;
    if fields.keys().any(|name| children.contains_key(name)) {
        return Err(error(
            "E_DUPLICATE_FIELD",
            "field name conflicts with child ID",
        ));
    }
    for (name, field) in object["fields"].as_object_mut().expect("checked fields") {
        identifier(name)?;
        value_inner(field, depth + 1, visits)?;
    }
    for (id, child) in object["children"]
        .as_object_mut()
        .expect("checked children")
    {
        identifier(id)?;
        object_inner(child, depth + 1, visits)?;
    }
    Ok(())
}

fn value_inner(value: &mut Value, depth: usize, visits: &mut usize) -> EditResult<()> {
    visit(depth, visits)?;
    let tag = text(
        record(value)?
            .get("t")
            .ok_or_else(|| error("E_SYNTAX", "typed value needs a tag"))?,
    )?
    .to_owned();
    let fields = value.as_object_mut().expect("checked object");
    match tag.as_str() {
        "number" | "quantity" => {
            let required: &[&str] = if tag == "number" {
                &["t", "n", "d"]
            } else {
                &["t", "n", "d", "u"]
            };
            keys(fields, required, &[])?;
            if tag == "quantity"
                && !matches!(
                    text(&fields["u"])?,
                    "q" | "s" | "ms" | "frame" | "Hz" | "kHz" | "bpm" | "ct" | "dB"
                )
            {
                return Err(error("E_SYNTAX", "unknown quantity unit"));
            }
            let n = integer(&fields["n"], false)?;
            let d = integer(&fields["d"], true)?;
            let divisor = n.gcd(&d);
            fields.insert("n".into(), Value::String((n / &divisor).to_string()));
            fields.insert("d".into(), Value::String((d / divisor).to_string()));
        }
        "string" | "symbol" => {
            keys(fields, &["t", "v"], &[])?;
            text(&fields["v"])?;
        }
        "boolean" => {
            keys(fields, &["t", "v"], &[])?;
            if !fields["v"].is_boolean() {
                return Err(error("E_SYNTAX", "boolean payload must be a JSON boolean"));
            }
        }
        "ref" => {
            keys(fields, &["t", "path", "port"], &[])?;
            path(&fields["path"], false)?;
            if !fields["port"].is_null() {
                identifier(text(&fields["port"])?)?;
            }
        }
        "record" => {
            keys(fields, &["t", "fields"], &[])?;
            let nested = fields
                .get_mut("fields")
                .expect("checked field")
                .as_object_mut()
                .ok_or_else(|| error("E_SYNTAX", "record fields must be a dictionary"))?;
            for (name, value) in nested {
                identifier(name)?;
                value_inner(value, depth + 1, visits)?;
            }
        }
        "list" | "tuple" | "call" => {
            let items = if tag == "call" {
                keys(fields, &["t", "fn", "args"], &[])?;
                if !matches!(text(&fields["fn"])?, "key" | "degree" | "ratio" | "bar") {
                    return Err(error("E_SYNTAX", "unknown core constructor"));
                }
                "args"
            } else {
                keys(fields, &["t", "items"], &[])?;
                "items"
            };
            let values = fields
                .get_mut(items)
                .expect("checked field")
                .as_array_mut()
                .ok_or_else(|| error("E_SYNTAX", "sequence payload must be an array"))?;
            for value in values {
                value_inner(value, depth + 1, visits)?;
            }
        }
        _ => return Err(error("E_SYNTAX", "unknown typed-value tag")),
    }
    Ok(())
}

fn payload(fields: &Map<String, Value>, key: &str, object: bool) -> EditResult<Value> {
    let mut value = fields[key].clone();
    if object {
        typed_object(&mut value)?;
    } else {
        typed_value(&mut value)?;
    }
    Ok(value)
}

fn optional_payload(
    fields: &Map<String, Value>,
    key: &str,
    object: bool,
) -> EditResult<Option<Value>> {
    if fields.contains_key(key) {
        payload(fields, key, object).map(Some)
    } else {
        Ok(None)
    }
}

fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

pub(super) fn transaction(value: Value) -> EditResult<Transaction> {
    let fields = record(&value)?;
    // Version refusal precedes revision validation and operation interpretation.
    let version = fields
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| error("E_SYNTAX", "transaction version must be an integer"))?;
    if version != u64::from(PROTOCOL_VERSION) {
        return Err(error(
            "E_CAPABILITY",
            "only editing Protocol 2 is supported",
        ));
    }
    keys(fields, &["version", "base_revision", "operations"], &[])?;
    let base_revision = text(&fields["base_revision"])?;
    if !digest(base_revision) {
        return Err(error("E_SYNTAX", "invalid authored base revision"));
    }
    let source = fields["operations"]
        .as_array()
        .ok_or_else(|| error("E_SYNTAX", "operations must be an array"))?;
    if source.is_empty() {
        return Err(error("E_SYNTAX", "transaction must contain an operation"));
    }
    if source.len() > MAX_OPERATIONS {
        return Err(error("E_RESOURCE_LIMIT", "operation count limit exceeded"));
    }
    let mut operations = Vec::with_capacity(source.len());
    for operation in source {
        let fields = record(operation)?;
        let op = fields
            .get("op")
            .map(text)
            .transpose()?
            .ok_or_else(|| error("E_SYNTAX", "operation needs an op field"))?;
        operations.push(match op {
            "set" => {
                keys(
                    fields,
                    &["op", "object", "field", "value"],
                    &["expect", "expect_absent"],
                )?;
                let expect_absent = match fields.get("expect_absent") {
                    None => false,
                    Some(Value::Bool(true)) if !fields.contains_key("expect") => true,
                    _ => {
                        return Err(error(
                            "E_SYNTAX",
                            "expect_absent must be true and cannot coexist with expect",
                        ))
                    }
                };
                Operation::Set {
                    object: path(&fields["object"], false)?,
                    field: path(&fields["field"], false)?,
                    value: payload(fields, "value", false)?,
                    expect: optional_payload(fields, "expect", false)?,
                    expect_absent,
                }
            }
            "unset" => {
                keys(fields, &["op", "object", "field"], &["expect"])?;
                Operation::Unset {
                    object: path(&fields["object"], false)?,
                    field: path(&fields["field"], false)?,
                    expect: optional_payload(fields, "expect", false)?,
                }
            }
            "insert_object" => {
                keys(fields, &["op", "parent", "id", "object_value"], &[])?;
                let id = text(&fields["id"])?;
                identifier(id)?;
                Operation::InsertObject {
                    parent: path(&fields["parent"], true)?,
                    id: id.to_owned(),
                    object_value: payload(fields, "object_value", true)?,
                }
            }
            "delete_object" => {
                keys(fields, &["op", "object"], &["expect_object"])?;
                Operation::DeleteObject {
                    object: path(&fields["object"], false)?,
                    expect_object: optional_payload(fields, "expect_object", true)?,
                }
            }
            "rename_id" => {
                keys(fields, &["op", "object", "new_id"], &[])?;
                let new_id = text(&fields["new_id"])?;
                identifier(new_id)?;
                Operation::RenameId {
                    object: path(&fields["object"], false)?,
                    new_id: new_id.to_owned(),
                }
            }
            _ => return Err(error("E_SYNTAX", "unknown editing operation")),
        });
    }
    let transaction = Transaction {
        version: PROTOCOL_VERSION,
        base_revision: base_revision.to_owned(),
        operations,
    };
    preflight_transaction(&transaction)?;
    Ok(transaction)
}

/// Bound public programmatic transactions before recursive Serialize sees their
/// arbitrary Values. The JSON reader is not their only possible entrypoint.
pub(super) fn preflight_transaction(transaction: &Transaction) -> EditResult<()> {
    if transaction.operations.len() > MAX_OPERATIONS {
        return Err(error("E_RESOURCE_LIMIT", "operation count limit exceeded"));
    }
    if transaction.base_revision.len() > 71 {
        return Err(error("E_SYNTAX", "invalid authored base revision"));
    }
    let mut bytes = 128usize;
    for operation in &transaction.operations {
        let (object, field, values, name): (&Path, &[String], Vec<&Value>, Option<&str>) =
            match operation {
                Operation::Set {
                    object,
                    field,
                    value,
                    expect,
                    ..
                } => (
                    object,
                    field,
                    std::iter::once(value).chain(expect.iter()).collect(),
                    None,
                ),
                Operation::Unset {
                    object,
                    field,
                    expect,
                } => (object, field, expect.iter().collect(), None),
                Operation::InsertObject {
                    parent,
                    id,
                    object_value,
                } => (parent, &[], vec![object_value], Some(id)),
                Operation::DeleteObject {
                    object,
                    expect_object,
                } => (object, &[], expect_object.iter().collect(), None),
                Operation::RenameId { object, new_id } => (object, &[], Vec::new(), Some(new_id)),
            };
        if object.len() > MAX_TYPED_DEPTH || field.len() > MAX_TYPED_DEPTH {
            return Err(error("E_RESOURCE_LIMIT", "path nesting limit exceeded"));
        }
        for id in object.iter().chain(field).map(String::as_str).chain(name) {
            identifier(id)?;
            bytes += string_size(id) + 1;
        }
        for value in values {
            bytes += bounded_encoding(value, MAX_TRANSACTION_BYTES)?;
        }
        bytes += 128;
        if bytes > MAX_TRANSACTION_BYTES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "transaction aggregate allowance exceeded",
            ));
        }
    }
    Ok(())
}

/// Public Document fields can be changed by Rust callers after parsing. Bound
/// that AST before its existing recursive typed-JSON conversion runs.
pub(super) fn preflight_document(document: &crate::syntax::Document) -> EditResult<()> {
    use crate::syntax::{Object, Value as SyntaxValue, ValueKind};

    enum Item<'a> {
        Object(&'a Object, usize),
        Value(&'a SyntaxValue, usize),
    }
    if document.objects.len() > MAX_VALUES {
        return Err(error(
            "E_RESOURCE_LIMIT",
            "source AST object limit exceeded",
        ));
    }
    let mut stack = Vec::new();
    let (mut count, mut text_bytes) = (0usize, 0usize);
    for (id, object) in &document.objects {
        identifier(id)?;
        text_bytes += id.len();
        stack.push(Item::Object(object, 0));
    }
    while let Some(item) = stack.pop() {
        count += 1;
        if count > MAX_VALUES {
            return Err(error("E_RESOURCE_LIMIT", "source AST node limit exceeded"));
        }
        match item {
            Item::Object(object, depth) => {
                if depth > MAX_TYPED_DEPTH {
                    return Err(error(
                        "E_RESOURCE_LIMIT",
                        "source AST nesting limit exceeded",
                    ));
                }
                if object.fields.len() > MAX_VALUES || object.children.len() > MAX_VALUES {
                    return Err(error(
                        "E_RESOURCE_LIMIT",
                        "source AST container limit exceeded",
                    ));
                }
                text_bytes += object.kind.len() + object.id.len();
                for (name, field) in &object.fields {
                    text_bytes += name.len();
                    stack.push(Item::Value(&field.value, depth + 1));
                }
                for (id, child) in &object.children {
                    identifier(id)?;
                    text_bytes += id.len();
                    stack.push(Item::Object(child, depth + 1));
                }
            }
            Item::Value(value, depth) => {
                if depth > MAX_TYPED_DEPTH {
                    return Err(error(
                        "E_RESOURCE_LIMIT",
                        "source AST nesting limit exceeded",
                    ));
                }
                match &value.kind {
                    ValueKind::Number(number) | ValueKind::Quantity { value: number, .. } => {
                        if number.numer().magnitude().bits() > crate::MAX_RATIONAL_BITS
                            || number.denom().magnitude().bits() > crate::MAX_RATIONAL_BITS
                        {
                            return Err(error(
                                "E_RESOURCE_LIMIT",
                                "source rational bit limit exceeded",
                            ));
                        }
                    }
                    ValueKind::String(text) | ValueKind::Symbol(text) => text_bytes += text.len(),
                    ValueKind::Reference(reference) => {
                        text_bytes += reference.path.iter().map(String::len).sum::<usize>();
                        text_bytes += reference.port.as_ref().map_or(0, String::len);
                    }
                    ValueKind::Record(fields) => {
                        if fields.len() > MAX_VALUES {
                            return Err(error(
                                "E_RESOURCE_LIMIT",
                                "source AST record limit exceeded",
                            ));
                        }
                        for (name, field) in fields {
                            text_bytes += name.len();
                            stack.push(Item::Value(&field.value, depth + 1));
                        }
                    }
                    ValueKind::Call { function, args } => {
                        if args.len() > MAX_VALUES {
                            return Err(error(
                                "E_RESOURCE_LIMIT",
                                "source AST argument limit exceeded",
                            ));
                        }
                        text_bytes += function.len();
                        stack.extend(args.iter().map(|argument| Item::Value(argument, depth + 1)));
                    }
                    ValueKind::List(items) | ValueKind::Tuple(items) => {
                        if items.len() > MAX_VALUES {
                            return Err(error(
                                "E_RESOURCE_LIMIT",
                                "source AST sequence limit exceeded",
                            ));
                        }
                        stack.extend(items.iter().map(|item| Item::Value(item, depth + 1)));
                    }
                    ValueKind::Boolean(_) => {}
                }
            }
        }
        if text_bytes > MAX_DOCUMENT_BYTES || stack.len() > MAX_VALUES {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "source AST aggregate limit exceeded",
            ));
        }
    }
    Ok(())
}
