//! An event's `order` (§6): the integer that sorts events of one class at
//! one frame. A plan holds it in 32 bits.
//!
//! A leaf has an `order`, and an override's `set` may replace it.

use num_bigint::BigInt;
use num_rational::BigRational;

use super::{field_path, Validator};
use crate::diagnostic::{DiagnosticCode, Span};
use crate::syntax::{Object, Value, ValueKind};

impl Validator<'_> {
    /// Check the `order` of leaf `object`.
    pub(super) fn validate_order(&mut self, object: &Object, path: &[String]) {
        if let Some(field) = object.field("order") {
            if let Some(order) = self.integer(field, path, "order") {
                self.validate_order_fits(&order, field.value.span, path, &["order".to_owned()]);
            }
        }
    }

    /// Check the `order` that the `set` of the override at `path` replaces.
    pub(super) fn validate_set_order(&mut self, value: &Value, path: &[String]) {
        let at = ["set".to_owned(), "order".to_owned()];
        match &value.kind {
            ValueKind::Number(order) if !order.is_integer() => self.push(
                DiagnosticCode::Range,
                "order must be an integer",
                Some(value.span),
                path.to_vec(),
                at.to_vec(),
            ),
            ValueKind::Number(order) => self.validate_order_fits(order, value.span, path, &at),
            _ => self.push_type_value(
                value,
                &field_path(&field_path(path, "set"), "order"),
                "integer order",
            ),
        }
    }

    fn validate_order_fits(
        &mut self,
        order: &BigRational,
        span: Span,
        path: &[String],
        field: &[String],
    ) {
        let order = order.to_integer();
        if order < BigInt::from(i32::MIN) || order > BigInt::from(i32::MAX) {
            self.push(
                DiagnosticCode::ResourceLimit,
                "event order exceeds signed 32-bit range",
                Some(span),
                path.to_vec(),
                field.to_vec(),
            );
        }
    }
}
