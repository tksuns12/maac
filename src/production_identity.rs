//! Execution identity and render keys: the SHA-256 of a document's
//! execution view N(A) (MaaC-1 §20.2), and of a render's complete context.
//! Canonical serialization is shared by source and render-context hashing.
//! This module performs no I/O.

use std::{collections::BTreeMap, fmt};

use num_rational::BigRational;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::library::LibrarySet;
use crate::music::{MeterMap, MeterPoint};
use crate::normalization::{execution_view, normalize_document, NormalizationContext};
use crate::syntax::{Document, Unit, Value, ValueKind};

use serde_json::{json, Value as JsonValue};

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

/// The execution identity of a document compiled on its own, with the
/// libraries its own declarations resolve to. Call only for a document that
/// validates; it MUST still include its production extensions.
pub fn execution_identity(document: &Document) -> Result<ExecutionIdentity, IdentityError> {
    let libraries = crate::compiler::local_libraries(document)
        .map_err(|diagnostics| error(format!("libraries: {diagnostics:?}")))?;
    execution_identity_with(document, crate::compiler::LOCAL_SOURCE, libraries.as_ref())
}

/// The execution identity of a bundle's entry composition, with the
/// libraries and assets the bundle resolves. Call only for a bundle that
/// compiles.
pub fn bundle_execution_identity(
    bundle: &crate::bundle::SourceBundle,
) -> Result<ExecutionIdentity, IdentityError> {
    let (entry, original, libraries) = crate::compiler::bundle_libraries(bundle)
        .map_err(|diagnostics| error(format!("libraries: {diagnostics:?}")))?;
    execution_identity_with(&original, &entry, Some(&libraries))
}

/// The execution identity of `document`, named `source` among the resolved
/// `libraries` its instrument and effect nodes use.
pub(crate) fn execution_identity_with(
    document: &Document,
    source: &str,
    libraries: Option<&LibrarySet>,
) -> Result<ExecutionIdentity, IdentityError> {
    if document.version != 1 {
        return Err(error("execution identity supports MaaC/1 only"));
    }
    reject_unsupported_extensions(document)?;
    let authored = document.to_syntax_json_value();
    let context = NormalizationContext::new(&authored, source, libraries)
        .map_err(|e| error(e.to_string()))?;
    let normalized = execution_view(&normalize_document(&context).map_err(|e| {
        error(match e.object_path.is_empty() {
            true => e.message,
            false => format!("{}: {}", e.object_path.join("."), e.message),
        })
    })?);
    let bytes = canonical_json_bytes(&normalized)?;
    Ok(ExecutionIdentity {
        algorithm: EXECUTION_ALGORITHM.into(),
        execution_hash: digest(&bytes),
        source_input_hash: digest(&canonical_json_bytes(&authored)?),
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
    format!(
        "sha256:{}",
        crate::bundle::lower_hex(&Sha256::digest(bytes))
    )
}
fn valid_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
/// §26: a host must not claim semantic normalization of a document whose
/// extension it does not understand.
fn reject_unsupported_extensions(document: &Document) -> Result<(), IdentityError> {
    match crate::extensions::first_unsupported(document) {
        Some(namespace) => Err(error(format!(
            "normalization is not defined for unsupported extension namespace `{namespace}`"
        ))),
        None => Ok(()),
    }
}

fn error(message: impl Into<String>) -> IdentityError {
    IdentityError(message.into())
}
fn exact_number(value: &Value) -> Result<&BigRational, IdentityError> {
    match &value.kind {
        ValueKind::Number(n) => Ok(n),
        _ => Err(error("constructor requires an exact dimensionless number")),
    }
}
/// The meter map of a document's project.
pub(crate) fn meter_map(document: &Document) -> Result<MeterMap, IdentityError> {
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
    use serde_json::{json, Map};

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
        execution_identity(&crate::syntax::parse(source).unwrap()).unwrap()
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
        let objects = std::mem::take(&mut document.objects);
        document.objects = objects.into_iter().rev().collect();
        assert_eq!(
            execution_identity(&document).unwrap().execution_hash,
            original.execution_hash
        );
    }

    #[test]
    fn complete_extensions_and_payload_labels_are_never_discarded() {
        // Preservation test at the identity boundary: unselected and payload
        // data stay in the hash. Bundle compilation validates this data.
        let with_extension = format!("{SOURCE} extension ext {{ namespace=\"maac.production/1\";schema=&schema;render_affecting=true;data={{deliveries={{release={{limit=-14;}};unused={{limit=-20;}};}};label=\"payload\";}};}}");
        let first = identity(&with_extension);
        for changed in [
            with_extension.replace("limit=-20", "limit=-21"),
            with_extension.replace("payload", "different"),
        ] {
            assert_ne!(first.execution_hash, identity(&changed).execution_hash);
        }
        assert!(first.normalized_source_json.contains("payload"));
        assert!(first.normalized_source_json.contains("unused"));
    }

    #[test]
    fn unsupported_extensions_have_no_execution_identity() {
        let source = format!("{SOURCE} extension ext {{ namespace=\"unknown/1\";schema=&schema;render_affecting=false;data={{}};}}");
        let error = execution_identity(&crate::syntax::parse(&source).unwrap()).unwrap_err();
        assert!(error.0.contains("`unknown/1`"), "{error}");
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
    fn complete_production_delivery_changes_affect_execution_identity() {
        fn production(source: &str) -> ExecutionIdentity {
            let mut bundle = crate::bundle::SourceBundle::new("examples/production.maac", source);
            bundle.assets.insert(
                "schemas/production.schema.json".into(),
                include_bytes!("../schemas/production.schema.json").to_vec(),
            );
            crate::compiler::compile_bundle(&bundle).unwrap();
            identity(source)
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
}
