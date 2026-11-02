//! Notes in succession (§9): `at = after(id)` or `at = after(id, rest)`
//! places a note or chord where another note or chord of the same pattern
//! ends, after an optional rest.

use std::collections::BTreeMap;

use num_traits::Zero;

use crate::diagnostic::{DiagnosticCode, Span};
use crate::exact::Rational;
use crate::syntax::{Object, Unit, Value, ValueKind};

/// Pattern children that may follow, and be followed by, another.
fn in_line(child: &Object) -> bool {
    matches!(child.kind.as_str(), "note" | "chord")
}

/// A problem with one child's succession, at the span to report.
#[derive(Clone, Debug)]
pub(crate) struct SuccessionError {
    pub child: String,
    pub code: DiagnosticCode,
    pub message: String,
    pub span: Span,
}

/// `after(id)` or `after(id, rest)`: the child it follows and the rest.
fn after_call(value: &Value) -> Option<&[Value]> {
    match &value.kind {
        ValueKind::Call { function, args } if function == "after" => Some(args),
        _ => None,
    }
}

/// Whether a child's `at` is written as `after(...)`.
pub(crate) fn follows(child: &Object) -> bool {
    child
        .field("at")
        .is_some_and(|field| after_call(&field.value).is_some())
}

fn q(value: &Value) -> Option<&Rational> {
    match &value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::Q,
        } => Some(value),
        _ => None,
    }
}

/// The position of every note and chord of `pattern` whose `at` is
/// `after(...)`, in pattern-local q. A chain is followed to a child with a
/// written position; a child that cannot be placed is reported once, and
/// the children that follow it are left out without a report of their own.
pub(crate) fn resolve(pattern: &Object) -> (BTreeMap<String, Rational>, Vec<SuccessionError>) {
    let mut placed = BTreeMap::new();
    let mut errors = Vec::new();
    let mut failed = std::collections::BTreeSet::new();
    for child in pattern.children.values().filter(|child| follows(child)) {
        if !in_line(child) {
            errors.push(SuccessionError {
                child: child.id.clone(),
                code: DiagnosticCode::Range,
                message: format!(
                    "only notes and chords can follow another; this is a {}",
                    child.kind
                ),
                span: child.field("at").expect("follows has `at`").value.span,
            });
            failed.insert(child.id.clone());
            continue;
        }
        let mut chain: Vec<(&Object, Rational)> = Vec::new();
        let mut current = child;
        // Walk back to a written position, a placed child or a failure.
        let start = loop {
            if let Some(at) = placed.get(&current.id) {
                break Some(Rational::clone(at));
            }
            if failed.contains(&current.id) {
                break None;
            }
            let field = current.field("at").expect("follows has `at`");
            let Some(args) = after_call(&field.value) else {
                break q(&field.value).cloned();
            };
            let error = |code, message: &str| SuccessionError {
                child: current.id.clone(),
                code,
                message: message.to_owned(),
                span: field.value.span,
            };
            let previous = match args {
                [previous] | [previous, _] => previous.as_symbol(),
                _ => None,
            };
            let Some(previous) = previous else {
                errors.push(error(
                    DiagnosticCode::Range,
                    "after requires the ID of a note or chord, and an optional rest",
                ));
                failed.insert(current.id.clone());
                break None;
            };
            let rest = match args.get(1) {
                None => Rational::zero(),
                Some(rest) => match q(rest) {
                    Some(rest) if *rest >= Rational::zero() => rest.clone(),
                    Some(_) => {
                        errors.push(error(
                            DiagnosticCode::Range,
                            "after's rest must be nonnegative",
                        ));
                        failed.insert(current.id.clone());
                        break None;
                    }
                    None => {
                        errors.push(error(DiagnosticCode::Unit, "after's rest requires q"));
                        failed.insert(current.id.clone());
                        break None;
                    }
                },
            };
            let Some(before) = pattern
                .children
                .get(previous)
                .filter(|before| in_line(before))
            else {
                errors.push(error(
                    DiagnosticCode::Reference,
                    &format!(
                        "after names `{previous}`, which is not a note or chord of this pattern"
                    ),
                ));
                failed.insert(current.id.clone());
                break None;
            };
            if before.id == child.id || chain.iter().any(|(link, _)| link.id == before.id) {
                errors.push(error(
                    DiagnosticCode::PatternCycle,
                    "notes in succession return to themselves",
                ));
                failed.insert(current.id.clone());
                break None;
            }
            let Some(dur) = before.field("dur").and_then(|field| q(&field.value)) else {
                // The note before reports its own missing or malformed `dur`.
                failed.insert(current.id.clone());
                break None;
            };
            chain.push((current, dur + rest));
            current = before;
        };
        // Place the chain from its written start forwards.
        match start {
            Some(mut at) => {
                for (link, step) in chain.into_iter().rev() {
                    at += step;
                    placed.insert(link.id.clone(), at.clone());
                }
            }
            None => failed.extend(chain.into_iter().map(|(link, _)| link.id.clone())),
        }
    }
    (placed, errors)
}

/// `pattern` with each `after(...)` replaced by the position it resolves to,
/// keeping the field's span, so that later stages read a plain `at`.
pub(crate) fn lower(pattern: &Object, placed: &BTreeMap<String, Rational>) -> Object {
    let mut lowered = pattern.clone();
    for (id, at) in placed {
        if let Some(field) = lowered
            .children
            .get_mut(id)
            .and_then(|child| child.fields.get_mut("at"))
        {
            field.value.kind = ValueKind::Quantity {
                value: at.clone(),
                unit: Unit::Q,
            };
        }
    }
    lowered
}
