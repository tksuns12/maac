//! Extension identifiers this implementation supports (§1.2), and the §26
//! checks every boundary applies to an `extension` before its semantics.
//!
//! An extension with any other namespace is preserved for Document-only
//! inspection (parsing, the authored tree and its revision hash) and refused
//! everywhere else: validation, editing, compilation and execution identity.

use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
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
