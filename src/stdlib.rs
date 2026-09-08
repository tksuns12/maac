//! Versioned MaaC source libraries embedded for deterministic offline imports.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::bundle::{sha256_digest, SourceBundle};
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::exact::Rational;
use crate::graph::{GraphUnit, ParameterRate};
use crate::library::LibrarySet;
use crate::plan::rational_serde;

pub const BASIC_ID: &str = "std/basic/1.0.0";
pub const BASIC_SOURCE_PATH: &str = "@builtin/std/basic/1.0.0.maac";
pub const BASIC_SOURCE: &str = include_str!("../stdlib/basic/1.0.0.maac");
pub const ACOUSTIC_ID: &str = "std/acoustic/1.0.0";
pub const ACOUSTIC_SOURCE_PATH: &str = "@builtin/std/acoustic/1.0.0.maac";
pub const ACOUSTIC_SOURCE: &str = include_str!("../stdlib/acoustic/1.0.0.maac");

#[derive(Clone, Copy, Debug)]
pub struct BuiltinSource {
    pub path: &'static str,
    pub source: &'static str,
}

struct LibraryDefinition {
    id: &'static str,
    source: BuiltinSource,
    editorial: &'static str,
    alias: &'static str,
}

const DEFINITIONS: &[LibraryDefinition] = &[
    LibraryDefinition {
        id: BASIC_ID,
        source: BuiltinSource {
            path: BASIC_SOURCE_PATH,
            source: BASIC_SOURCE,
        },
        editorial: include_str!("../stdlib/basic/1.0.0.json"),
        alias: "basic",
    },
    LibraryDefinition {
        id: ACOUSTIC_ID,
        source: BuiltinSource {
            path: ACOUSTIC_SOURCE_PATH,
            source: ACOUSTIC_SOURCE,
        },
        editorial: include_str!("../stdlib/acoustic/1.0.0.json"),
        alias: "acoustic",
    },
];

fn definition(id: &str) -> Option<&'static LibraryDefinition> {
    DEFINITIONS.iter().find(|definition| definition.id == id)
}

/// Look up an exact versioned identifier; aliases and version fallback are not supported.
pub fn lookup(id: &str) -> Option<BuiltinSource> {
    definition(id).map(|definition| definition.source)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LibraryInfo {
    pub library: String,
    pub source_path: String,
    pub source_hash: String,
}

/// List exact embedded identities without resolving instrument definitions.
pub fn libraries() -> Vec<LibraryInfo> {
    DEFINITIONS
        .iter()
        .map(|definition| LibraryInfo {
            library: definition.id.into(),
            source_path: definition.source.path.into(),
            source_hash: sha256_digest(definition.source.source.as_bytes()),
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Catalog {
    pub library: String,
    pub source_path: String,
    pub source_hash: String,
    pub instruments: Vec<InstrumentInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstrumentInfo {
    pub name: String,
    pub family: String,
    pub description: String,
    pub channels: u8,
    pub guidance: MusicalGuidance,
    pub controls: BTreeMap<String, ControlInfo>,
    /// A complete composition using the versioned built-in import.
    pub usage: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ControlInfo {
    pub unit: GraphUnit,
    pub rate: ParameterRate,
    #[serde(with = "rational_serde")]
    pub default: Rational,
    #[serde(with = "rational_serde")]
    pub min: Rational,
    #[serde(with = "rational_serde")]
    pub max: Rational,
    pub min_open: bool,
    pub max_open: bool,
}

/// Musical recommendations describe the authored sound, not compiler pitch limits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MusicalGuidance {
    pub pitch_low: String,
    pub pitch_high: String,
    pub midi_min: u8,
    pub midi_max: u8,
    pub pitch_behavior: String,
    pub tested_pitches: Vec<String>,
    #[serde(with = "rational_serde")]
    pub duration_min_seconds: Rational,
    #[serde(with = "rational_serde")]
    pub duration_max_seconds: Rational,
    pub example_pitch: String,
    #[serde(with = "rational_serde")]
    pub example_duration_seconds: Rational,
    pub notes: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Editorial {
    name: String,
    family: String,
    description: String,
    guidance: MusicalGuidance,
}

/// Describe the original basic library; its identity does not track newer libraries.
pub fn catalog() -> Result<Catalog, Diagnostics> {
    catalog_for(BASIC_ID)
}

/// Validate one exact embedded library and derive controls from executable definitions.
pub fn catalog_for(library: &str) -> Result<Catalog, Diagnostics> {
    let definition = definition(library)
        .ok_or_else(|| reference_error(format!("unknown built-in library `{library}`")))?;
    let bundle = SourceBundle::new(
        "catalog.maac",
        format!("maac 1; library catalog {{ version = \"1\"; }} import selected {{ builtin = \"{}\"; }}", definition.id),
    );
    let libraries = LibrarySet::resolve(&bundle.resolve()?)?;
    let editorial: Vec<Editorial> = serde_json::from_str(definition.editorial)
        .map_err(|error| catalog_error(format!("invalid built-in catalog metadata: {error}")))?;
    let mut metadata = BTreeMap::new();
    for item in editorial {
        let name = item.name.clone();
        if metadata.insert(name.clone(), item).is_some() {
            return Err(catalog_error(format!(
                "duplicate built-in catalog metadata for `{name}`"
            )));
        }
    }
    let exports: BTreeSet<_> = libraries
        .programs
        .iter()
        .filter(|program| program.source.file == definition.source.path)
        .map(|program| program.source.object.as_str())
        .collect();
    if exports != metadata.keys().map(String::as_str).collect() {
        return Err(catalog_error(
            "built-in catalog metadata must exactly match instrument exports",
        ));
    }
    let mut instruments = Vec::with_capacity(libraries.programs.len());
    for program in libraries
        .programs
        .into_iter()
        .filter(|program| program.source.file == definition.source.path)
    {
        let name = &program.source.object;
        let item = metadata.remove(name).ok_or_else(|| {
            catalog_error(format!("missing built-in catalog metadata for `{name}`"))
        })?;
        let guidance = &item.guidance;
        if item.family.is_empty()
            || item.description.is_empty()
            || guidance.pitch_low.is_empty()
            || guidance.pitch_high.is_empty()
            || guidance.tested_pitches.is_empty()
            || guidance.midi_min > guidance.midi_max
            || guidance.midi_max > 127
            || guidance.duration_min_seconds <= Rational::from_integer(0.into())
            || guidance.duration_max_seconds < guidance.duration_min_seconds
            || guidance.example_duration_seconds < guidance.duration_min_seconds
            || guidance.example_duration_seconds > guidance.duration_max_seconds
        {
            return Err(catalog_error(format!(
                "invalid musical guidance for `{name}`"
            )));
        }
        for (pitch, midi) in [
            (&guidance.pitch_low, guidance.midi_min),
            (&guidance.pitch_high, guidance.midi_max),
        ] {
            if crate::music::Pitch::parse_spelled(pitch)
                .ok()
                .and_then(|pitch| pitch.key_index())
                != Some(i64::from(midi))
            {
                return Err(catalog_error(format!(
                    "pitch and MIDI guidance disagree for `{name}`"
                )));
            }
        }
        let example_midi = crate::music::Pitch::parse_spelled(&guidance.example_pitch)
            .ok()
            .and_then(|pitch| pitch.key_index());
        if !example_midi.is_some_and(|midi| {
            (i64::from(guidance.midi_min)..=i64::from(guidance.midi_max)).contains(&midi)
        }) || guidance
            .tested_pitches
            .iter()
            .any(|pitch| crate::music::Pitch::parse_spelled(pitch).is_err())
        {
            return Err(catalog_error(format!(
                "invalid example or tested pitch for `{name}`"
            )));
        }
        let mut controls = BTreeMap::new();
        for (name, control) in &program.controls {
            let spec = program.control_spec(name).ok_or_else(|| {
                catalog_error(format!("missing parameter specification for `{name}`"))
            })?;
            controls.insert(
                name.clone(),
                ControlInfo {
                    unit: spec.unit,
                    rate: spec.rate,
                    default: control.default.clone(),
                    min: spec.min,
                    max: spec.max,
                    min_open: spec.min_open,
                    max_open: spec.max_open,
                },
            );
        }
        let usage = usage_source(definition, name, guidance);
        instruments.push(InstrumentInfo {
            name: item.name,
            family: item.family,
            description: item.description,
            channels: program.channels(),
            guidance: item.guidance,
            controls,
            usage,
        });
    }
    Ok(Catalog {
        library: definition.id.into(),
        source_path: definition.source.path.into(),
        source_hash: sha256_digest(definition.source.source.as_bytes()),
        instruments,
    })
}

/// Describe one bare instrument export, such as `mellow_piano`.
pub fn instrument(name: &str) -> Result<InstrumentInfo, Diagnostics> {
    instrument_in(BASIC_ID, name)
}

/// Describe a bare export from one exact embedded library.
pub fn instrument_in(library: &str, name: &str) -> Result<InstrumentInfo, Diagnostics> {
    catalog_for(library)?
        .instruments
        .into_iter()
        .find(|instrument| instrument.name == name)
        .ok_or_else(|| {
            reference_error(format!(
                "unknown built-in instrument `{name}` in `{library}`"
            ))
        })
}

fn usage_source(definition: &LibraryDefinition, name: &str, guidance: &MusicalGuidance) -> String {
    let library = definition.id;
    let alias = definition.alias;
    let duration_quarters = &guidance.example_duration_seconds * Rational::from_integer(2.into());
    let score_quarters = (&duration_quarters + Rational::from_integer(1.into()))
        .max(Rational::from_integer(4.into()));
    format!(
        r#"maac 1;
import {alias} {{ builtin = "{library}"; }}
project demo {{
  score = [0q, {score_quarters}q]; rate = 48000Hz; tempo = &clock; meter = &metre;
  output = &sound:out; tail = 3s;
}}
tempo clock {{ points = [(0q, 120bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
pattern phrase {{
  length = {score_quarters}q;
  note first {{ at = 0q; dur = {duration_quarters}q; pitch = {}; velocity = 0.8; }}
}}
track melody {{ target = &sound:events; }}
place play {{ pattern = &phrase; track = &melody; at = 0q; }}
node sound {{ instrument = &{alias}.{name}; config = {{ voices = 16; }}; }}
"#,
        guidance.example_pitch
    )
}

fn catalog_error(message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(DiagnosticCode::Conflict, message, None));
    diagnostics
}

fn reference_error(message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(DiagnosticCode::Reference, message, None));
    diagnostics
}
