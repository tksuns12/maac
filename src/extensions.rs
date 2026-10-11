//! Extension identifiers this implementation supports (§1.2), and the §26
//! checks every boundary applies to an `extension` before its semantics.
//!
//! An extension with any other namespace is preserved for Document-only
//! inspection (parsing, the authored tree and its revision hash) and refused
//! everywhere else: validation, editing, compilation and execution identity.

use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
use crate::located::{object_fields, Location};
use crate::syntax::{Document, Object, ValueKind};

/// Every extension namespace and `project.requires` capability this
/// implementation understands.
pub const SUPPORTED: [&str; 3] = [
    crate::production_data::CAPABILITY,
    crate::takes::CAPABILITY,
    crate::takes::CAPABILITY_V2,
];

pub fn is_supported(identifier: &str) -> bool {
    SUPPORTED.contains(&identifier)
}

/// A schema this implementation embeds for one of its namespaces. A
/// descriptor asset can name it with `builtin` instead of pinning a copied
/// file (§14.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuiltinSchema {
    /// The namespace the schema validates, which is also its `builtin` value.
    pub namespace: &'static str,
    /// The resolved identity, in the reserved `@builtin/` namespace.
    pub path: &'static str,
    /// The exact bytes of the published schema file.
    pub bytes: &'static [u8],
}

/// Every built-in schema. Each is frozen by its namespace version.
pub const SCHEMAS: [BuiltinSchema; 3] = [
    BuiltinSchema {
        namespace: crate::production_data::CAPABILITY,
        path: "@builtin/schema/maac.production/1.json",
        bytes: crate::production_data::SCHEMA_BYTES,
    },
    BuiltinSchema {
        namespace: crate::takes::CAPABILITY,
        path: "@builtin/schema/maac.takes/1.json",
        bytes: crate::takes::SCHEMA_BYTES,
    },
    BuiltinSchema {
        namespace: crate::takes::CAPABILITY_V2,
        path: "@builtin/schema/maac.takes/2.json",
        bytes: crate::takes::SCHEMA_V2_BYTES,
    },
];

/// The built-in schema that a `builtin` value names, if any.
pub fn builtin_schema(namespace: &str) -> Option<BuiltinSchema> {
    SCHEMAS
        .into_iter()
        .find(|schema| schema.namespace == namespace)
}

/// The built-in schema with this resolved identity, if any.
pub(crate) fn builtin_schema_at(path: &str) -> Option<BuiltinSchema> {
    SCHEMAS.into_iter().find(|schema| schema.path == path)
}

/// Check an asset that names a built-in schema, as the `schema` of an
/// extension of `namespace`. It must be a descriptor with only `builtin`
/// besides `kind` and an optional `label`, and `builtin` must name that
/// namespace (§14.1).
pub(crate) fn builtin_descriptor(
    asset: &Object,
    namespace: &str,
) -> Result<BuiltinSchema, Diagnostic> {
    let at = Location::object(asset);
    if let Some((name, field)) = asset
        .fields
        .iter()
        .find(|(name, _)| !matches!(name.as_str(), "kind" | "builtin" | "label"))
    {
        let message = if matches!(name.as_str(), "path" | "hash") {
            "an asset uses either `path` and `hash`, or `builtin`"
        } else {
            "unknown built-in descriptor field"
        };
        return Err(at.at(name, field.name_span).error("E_ASSET", message));
    }
    let fields = object_fields(asset, &["kind", "builtin"], "asset", "asset")?;
    let kind = fields.get("kind")?;
    if kind.symbol()? != "descriptor" {
        return Err(kind.error(
            "E_ASSET",
            "only a descriptor asset can name a built-in schema",
        ));
    }
    let builtin = fields.get("builtin")?;
    let id = builtin.string()?;
    let schema = builtin_schema(id)
        .ok_or_else(|| builtin.error("E_REFERENCE", format!("unknown built-in schema `{id}`")))?;
    if schema.namespace != namespace {
        return Err(builtin.error(
            "E_ASSET",
            format!("schema must be the built-in schema of `{namespace}`"),
        ));
    }
    Ok(schema)
}

/// A §26 violation on one extension's `namespace` field.
pub(crate) struct Finding {
    pub code: DiagnosticCode,
    pub message: String,
    pub span: Span,
}

/// An extension's namespace must be supported and listed in
/// `project.requires`. Shape errors in the field are reported elsewhere.
pub(crate) fn findings(document: &Document, extension: &Object) -> Vec<Finding> {
    let Some(field) = extension.field("namespace") else {
        return Vec::new();
    };
    let Some(namespace) = field.value.as_string() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if !is_supported(namespace) {
        found.push(Finding {
            code: DiagnosticCode::Capability,
            message: format!("unsupported extension namespace `{namespace}`"),
            span: field.value.span,
        });
    }
    if !required(document).any(|identifier| identifier == namespace) {
        found.push(Finding {
            code: DiagnosticCode::Reference,
            message: format!("extension namespace `{namespace}` is not listed in project.requires"),
            span: field.value.span,
        });
    }
    found
}

fn required(document: &Document) -> impl Iterator<Item = &str> {
    document
        .objects
        .values()
        .filter(|object| object.kind == "project")
        .filter_map(|project| match &project.field("requires")?.value.kind {
            ValueKind::List(items) => Some(items),
            _ => None,
        })
        .flatten()
        .filter_map(|item| item.as_string())
}

/// Check every top-level extension, reporting at its `namespace` field.
pub(crate) fn check(document: &Document) -> Result<(), Diagnostics> {
    let mut diagnostics = Diagnostics::new();
    for (id, object) in &document.objects {
        if object.kind != "extension" {
            continue;
        }
        for finding in findings(document, object) {
            diagnostics.push(
                Diagnostic::error(finding.code, finding.message, Some(finding.span))
                    .object_path([id.clone()])
                    .field_path(["namespace".to_owned()]),
            );
        }
    }
    if diagnostics.has_errors() {
        Err(diagnostics)
    } else {
        Ok(())
    }
}

/// Names the first extension whose namespace is not supported, if any.
pub(crate) fn first_unsupported(document: &Document) -> Option<&str> {
    document
        .objects
        .values()
        .filter(|object| object.kind == "extension")
        .filter_map(|object| object.field("namespace")?.value.as_string())
        .find(|namespace| !is_supported(namespace))
}
