mod audio_asset;
mod audio_buffer;
mod audio_clip;
pub mod bundle;
pub mod bundle_fs;
pub mod cli;
pub mod compiler;
pub mod diagnostic;
pub mod dsp;
pub mod editing;
pub mod exact;
pub mod export;
mod expression;
pub mod external;
pub mod external_host;
pub mod external_native;
pub mod generic_external_render;
pub mod generic_lock;
pub mod generic_lock_normalization;
pub mod generic_render;
pub mod graph;
pub mod instrument_plan;
pub mod interchange;
mod kit;
pub mod library;
mod module_artifact;
pub mod music;
pub use module_artifact::{
    ModuleArtifact, ModuleArtifactLimits, ModuleExport, MAX_MODULE_ARTIFACT_JSON_BYTES,
    MODULE_ARTIFACT_FORMAT, MODULE_ARTIFACT_VERSION,
};
pub mod plan;
mod plan_artifact;
mod warp_clip;
pub use plan_artifact::{
    MessageAdapterCapability, PerformanceDispatch, PerformanceDispatchKind, PlanArtifact,
};
mod core_control;
pub mod plan_v3;
mod plan_v4;
mod plan_v5;
mod plan_v6;
mod plan_v7;
mod pluck;
pub mod production_analysis;
mod production_compressor;
pub mod production_convert;
pub mod production_data;
pub mod production_delivery;
mod production_eq;
pub mod production_identity;
mod production_reverb;
pub mod semantic;
pub mod stdlib;
pub mod syntax;
pub mod synth;
pub mod tempo;
pub mod voice;
pub mod wavetable;

pub use bundle::SourceBundle;
pub use compiler::{
    check, check_bundle, check_bundle_artifact, check_bundle_artifact_with_limits,
    check_bundle_versioned, check_bundle_versioned_with_limits, check_bundle_with_limits,
    check_versioned, check_versioned_with_limits, check_with_limits, compile, compile_bundle,
    compile_bundle_artifact, compile_bundle_artifact_with_limits, compile_bundle_versioned,
    compile_bundle_versioned_with_limits, compile_bundle_with_limits, compile_versioned,
    compile_versioned_with_limits, compile_with_limits,
};
pub use diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
pub use dsp::{
    render, render_artifact, render_artifact_with_limits, render_ports_artifact_with_limits,
    render_versioned, render_versioned_with_limits, render_with_limits,
};
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
    load_plan_with_limits(bytes, &plan::PlanLimits::default())
}

/// Load the strict standalone plan schema under an explicit caller allowance.
pub fn load_plan_with_limits(
    bytes: &[u8],
    limits: &plan::PlanLimits,
) -> Result<Plan, plan::PlanError> {
    Plan::from_json_with_limits(bytes, limits)
}

#[cfg(test)]
mod syntax_tests;

pub use export::{
    render_wav_to_path_artifact, render_wav_to_path_artifact_with_limits,
    render_wav_to_path_versioned, render_wav_to_path_versioned_with_limits, write_wav_artifact,
    write_wav_artifact_with_limits, write_wav_versioned, write_wav_versioned_with_limits,
};
pub use plan_v3::{AutomationAnchor, AutomationV3, PlanV3, ResolvedEventV3, VersionedPlan};
/// Load and independently validate a version 1, 2, or 3 performance plan.
pub fn load_plan_versioned(bytes: &[u8]) -> Result<VersionedPlan, plan::PlanError> {
    VersionedPlan::from_json(bytes)
}
/// Load either timing representation under an explicit caller allowance.
pub fn load_plan_versioned_with_limits(
    bytes: &[u8],
    limits: &plan::PlanLimits,
) -> Result<VersionedPlan, plan::PlanError> {
    VersionedPlan::from_json_with_limits(bytes, limits)
}
