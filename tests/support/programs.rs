//! Instrument programs for the tests that drive an `InstrumentRuntime`: the
//! library resolves each from MaaC source, as it does for a project.

#![allow(dead_code)]

use maac::bundle::SourceBundle;
use maac::graph::InstrumentProgram;
use maac::library::LibrarySet;
use maac::Diagnostics;

/// The program of `instrument`, the source of one `instrument` declaration,
/// or the diagnostics that refuse it.
pub fn resolve(instrument: &str) -> Result<InstrumentProgram, Diagnostics> {
    let source = format!("maac 1;\nlibrary studio {{ version = \"1.0.0\"; }}\n{instrument}");
    let bundle = SourceBundle::new("main.maac", source.as_str()).resolve()?;
    Ok(LibrarySet::resolve(&bundle)?.programs.remove(0))
}

/// The program of `instrument`, the source of one `instrument` declaration.
pub fn program(instrument: &str) -> InstrumentProgram {
    resolve(instrument).unwrap_or_else(|diagnostics| panic!("{diagnostics:?}\n{instrument}"))
}
