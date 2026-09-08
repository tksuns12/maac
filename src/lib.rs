pub mod cli;
pub mod compiler;
pub mod diagnostic;
pub mod dsp;
pub mod exact;
pub mod export;
pub mod music;
pub mod plan;
pub mod semantic;
pub mod syntax;

pub use compiler::{check, compile};
pub use diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
pub use dsp::render;
pub use exact::{parse_rational, Rational, RationalError, MAX_RATIONAL_BITS};
pub use plan::Plan;
pub use syntax::{
    parse, parse_file, parse_file_with_options, parse_with_options, Document, Field, Object,
    ParseFileError, ParseOptions, Reference, Unit, Value, ValueKind,
};

/// Load and independently validate a standalone performance-plan artifact.
/// Filesystem ownership stays with the caller; this boundary accepts bytes so
/// hosts can choose their own bounded input source.
pub fn load_plan(bytes: &[u8]) -> Result<Plan, plan::PlanError> {
    Plan::from_json(bytes)
}

#[cfg(test)]
mod syntax_tests;
