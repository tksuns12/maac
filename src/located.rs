//! Typed reads of authored values that fail at their §23 location: the
//! responsible authored object (and child) IDs, the field path inside it, and
//! the span of the offending text. Extension preparers read source data
//! through these, so their diagnostics never name a plan path or an object the
//! author did not write.

use std::collections::BTreeMap;

use crate::diagnostic::{Diagnostic, DiagnosticCode, Span};
use crate::plan::PlanError;
use crate::syntax::{Field, Object, Value, ValueKind};

/// An authored object path, a field path inside the innermost object, and the
/// span to report.
#[derive(Clone, Debug)]
pub(crate) struct Location {
    object: Vec<String>,
    field: Vec<String>,
    span: Span,
}

impl Location {
    /// A top-level object, spanning its kind keyword.
    pub(crate) fn object(object: &Object) -> Self {
        Self {
            object: vec![object.id.clone()],
            field: Vec::new(),
            span: object.kind_span,
        }
    }

    /// A child object of this object, spanning its kind keyword.
    pub(crate) fn child(&self, child: &Object) -> Self {
        let mut object = self.object.clone();
        object.push(child.id.clone());
        Self {
            object,
            field: Vec::new(),
            span: child.kind_span,
        }
    }

    /// A field path segment whose text is `span`.
    pub(crate) fn at(&self, segment: &str, span: Span) -> Self {
        let mut field = self.field.clone();
        field.push(segment.to_owned());
        Self {
            object: self.object.clone(),
            field,
            span,
        }
    }

    pub(crate) fn error(&self, code: &str, message: impl Into<String>) -> Diagnostic {
        let code = DiagnosticCode::from_code(code).expect("a §23 diagnostic code");
        self.diagnostic(code, message)
    }

    /// Report a shared plan-level check's code and message here.
    pub(crate) fn plan(&self, failure: PlanError) -> Diagnostic {
        let code = failure.diagnostic().code;
        self.diagnostic(code, failure.message)
    }

    fn diagnostic(&self, code: DiagnosticCode, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(code, message, Some(self.span))
            .object_path(self.object.clone())
            .field_path(self.field.clone())
    }

    /// Follow `path` through an object's fields and nested records as far as
    /// it exists, spanning the deepest value found.
    pub(crate) fn find(object: &Object, path: &[&str]) -> Self {
        let mut at = Self::object(object);
        let mut fields = Some(&object.fields);
        for segment in path {
            match fields.and_then(|fields| fields.get(*segment)) {
                Some(field) => {
                    at = at.at(segment, field.value.span);
                    fields = match &field.value.kind {
                        ValueKind::Record(nested) => Some(nested),
                        _ => None,
                    };
                }
                None => {
                    at = at.at(segment, at.span);
                    fields = None;
                }
            }
        }
        at
    }
}

/// An authored record, either an object's fields or a record value, and the
/// noun its field-set messages use ("unknown {noun} field").
#[derive(Clone)]
pub(crate) struct Record<'a> {
    pub fields: &'a BTreeMap<String, Field>,
    pub at: Location,
    noun: &'static str,
}

/// An authored value and its location.
#[derive(Clone)]
pub(crate) struct Located<'a> {
    pub value: &'a Value,
    pub at: Location,
    noun: &'static str,
}

/// One entry of an ID-keyed record: its key, located at the key's text, and
/// its value.
pub(crate) struct Entry<'a> {
    pub name: &'a str,
    pub key: Location,
    pub value: Located<'a>,
}

impl<'a> Record<'a> {
    pub(crate) fn object(object: &'a Object, noun: &'static str) -> Self {
        Self {
            fields: &object.fields,
            at: Location::object(object),
            noun,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.fields.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub(crate) fn get(&self, name: &str) -> Result<Located<'a>, Diagnostic> {
        self.optional(name).ok_or_else(|| {
            self.at
                .at(name, self.at.span)
                .error("E_REFERENCE", format!("missing required {name} field"))
        })
    }

    pub(crate) fn optional(&self, name: &str) -> Option<Located<'a>> {
        self.fields.get(name).map(|field| Located {
            value: &field.value,
            at: self.at.at(name, field.value.span),
            noun: self.noun,
        })
    }

    /// Every field must be required or optional, and every required field
    /// present.
    pub(crate) fn exact(&self, required: &[&str], optional: &[&str]) -> Result<(), Diagnostic> {
        if let Some((name, field)) = self.fields.iter().find(|(name, _)| {
            !required.contains(&name.as_str()) && !optional.contains(&name.as_str())
        }) {
            return Err(self
                .at
                .at(name, field.name_span)
                .error("E_UNKNOWN_FIELD", format!("unknown {} field", self.noun)));
        }
        if let Some(name) = required
            .iter()
            .find(|name| !self.fields.contains_key(**name))
        {
            return Err(self.at.at(name, self.at.span).error(
                "E_REFERENCE",
                format!("missing required {} field", self.noun),
            ));
        }
        Ok(())
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = Entry<'a>> + '_ {
        self.fields.iter().map(|(name, field)| Entry {
            name,
            key: self.at.at(name, field.name_span),
            value: Located {
                value: &field.value,
                at: self.at.at(name, field.value.span),
                noun: self.noun,
            },
        })
    }
}

impl<'a> Located<'a> {
    pub(crate) fn error(&self, code: &str, message: impl Into<String>) -> Diagnostic {
        self.at.error(code, message)
    }

    pub(crate) fn plan(&self, failure: PlanError) -> Diagnostic {
        self.at.plan(failure)
    }

    pub(crate) fn record(&self) -> Result<Record<'a>, Diagnostic> {
        match &self.value.kind {
            ValueKind::Record(fields) => Ok(Record {
                fields,
                at: self.at.clone(),
                noun: self.noun,
            }),
            _ => Err(self.error("E_RANGE", "expected record")),
        }
    }

    pub(crate) fn string(&self) -> Result<&'a str, Diagnostic> {
        self.value
            .as_string()
            .ok_or_else(|| self.error("E_RANGE", "expected string"))
    }

    pub(crate) fn symbol(&self) -> Result<&'a str, Diagnostic> {
        self.value
            .as_symbol()
            .ok_or_else(|| self.error("E_RANGE", "expected symbol"))
    }

    /// A list value's items, each located at its index.
    pub(crate) fn items(&self) -> Option<Vec<Located<'a>>> {
        match &self.value.kind {
            ValueKind::List(items) => Some(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, value)| Located {
                        value,
                        at: self.at.at(&index.to_string(), value.span),
                        noun: self.noun,
                    })
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// Every object may carry a string `label`; extension objects bound it.
pub(crate) fn object_fields<'a>(
    object: &'a Object,
    required: &[&str],
    noun: &'static str,
    kind: &str,
) -> Result<Record<'a>, Diagnostic> {
    if let Some(child) = object.children.values().next() {
        return Err(Location::object(object).child(child).error(
            "E_UNKNOWN_KIND",
            format!("{kind} objects cannot have children"),
        ));
    }
    let fields = Record::object(object, noun);
    fields.exact(required, &["label"])?;
    if let Some(label) = fields.optional("label") {
        if label.string()?.len() > 4096 {
            return Err(label.error("E_RESOURCE_LIMIT", "label exceeds 4096 bytes"));
        }
    }
    Ok(fields)
}
