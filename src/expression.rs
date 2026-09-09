use crate::Rational;
use std::sync::Arc;

// Keep the immutable curve shared across scheduled events and live voices. Exact
// rational coordinates preserve knot selection even below binary64 resolution.
#[derive(Clone, Debug)]
pub(crate) struct ExpressionRuntime<T> {
    pub(crate) curve: Arc<T>,
    pub(crate) coordinate_per_frame: Rational,
    pub(crate) gate: u64,
}

impl<T> ExpressionRuntime<T> {
    pub(crate) fn coordinate_at(&self, on_frame: u64, frame: u64) -> Rational {
        &self.coordinate_per_frame
            * Rational::from_integer(frame.saturating_sub(on_frame).min(self.gate).into())
    }
}
