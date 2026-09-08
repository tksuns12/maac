//! Self-contained reusable instrument resources embedded in version 2 plans.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::bundle::{DependencyIdentity, SourceIdentity, MAX_BUNDLE_SOURCES};
use crate::graph::{
    GraphProcessor, InstrumentProgram, MAX_EMBEDDED_SAMPLES, MAX_GRAPH_PROGRAMS,
    MAX_TOTAL_GRAPH_EDGES, MAX_TOTAL_GRAPH_NODES,
};
use crate::library::{LibraryMetadata, WavetableSource, MAX_LIBRARY_METADATA_BYTES};
use crate::plan::PlanError;
use crate::wavetable::Wavetable;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentResources {
    pub entry_source: String,
    pub programs: Vec<InstrumentProgram>,
    pub wavetables: Vec<Wavetable>,
    pub wavetable_sources: Vec<WavetableSource>,
    pub source_files: Vec<SourceIdentity>,
    pub dependencies: Vec<DependencyIdentity>,
    pub libraries: Vec<LibraryMetadata>,
}

impl InstrumentResources {
    /// Validate the standalone resource payload independently of compiler or
    /// filesystem state. Aggregate ceilings are checked before graph walks.
    pub fn validate(&self) -> Result<(), PlanError> {
        if self.programs.len() > MAX_GRAPH_PROGRAMS
            || self.source_files.len() > MAX_BUNDLE_SOURCES
            || self.wavetables.len() > crate::library::MAX_LIBRARY_WAVETABLES
            || self
                .programs
                .len()
                .saturating_add(self.wavetables.len())
                .saturating_add(self.wavetable_sources.len())
                .saturating_add(self.source_files.len())
                .saturating_add(self.dependencies.len())
                .saturating_add(self.libraries.len())
                > crate::plan::PlanLimits::MAX_OBJECTS
        {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "instruments",
                "instrument program or source-file limit exceeded",
            ));
        }

        let total_nodes = self.programs.iter().fold(0usize, |total, program| {
            total
                .saturating_add(program.voice.nodes.len())
                .saturating_add(program.shared.as_ref().map_or(0, |graph| graph.nodes.len()))
        });
        let total_edges = self.programs.iter().fold(0usize, |total, program| {
            let graph_edges = |graph: &crate::graph::GraphProgram| {
                graph
                    .connections
                    .len()
                    .saturating_add(graph.modulations.len())
            };
            total
                .saturating_add(graph_edges(&program.voice))
                .saturating_add(program.shared.as_ref().map_or(0, graph_edges))
        });
        let raw_samples = self.wavetables.iter().fold(0usize, |total, table| {
            total.saturating_add(table.samples.len())
        });
        if total_nodes > MAX_TOTAL_GRAPH_NODES
            || total_edges > MAX_TOTAL_GRAPH_EDGES
            || raw_samples > MAX_EMBEDDED_SAMPLES
        {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "instruments",
                "aggregate instrument graph or sample limit exceeded",
            ));
        }

        validate_path(&self.entry_source, "instruments.entry_source")?;

        let mut source_by_path = BTreeMap::new();
        for (index, source) in self.source_files.iter().enumerate() {
            let path = format!("instruments.source_files[{index}]");
            validate_path(&source.path, format!("{path}.path"))?;
            validate_hash(&source.hash, format!("{path}.hash"))?;
            if source_by_path
                .insert(source.path.as_str(), source.hash.as_str())
                .is_some()
            {
                return Err(error(
                    "E_DUPLICATE_ID",
                    format!("{path}.path"),
                    "duplicate source-file identity",
                ));
            }
        }
        if !source_by_path.contains_key(self.entry_source.as_str()) {
            return Err(error(
                "E_REFERENCE",
                "instruments.entry_source",
                "entry source has no embedded source identity",
            ));
        }

        let mut dependency_ids = BTreeSet::new();
        for (index, dependency) in self.dependencies.iter().enumerate() {
            let path = format!("instruments.dependencies[{index}]");
            validate_path(&dependency.source, format!("{path}.source"))?;
            validate_identifier(&dependency.alias, format!("{path}.alias"))?;
            validate_path(&dependency.path, format!("{path}.path"))?;
            validate_hash(&dependency.hash, format!("{path}.hash"))?;
            if !source_by_path.contains_key(dependency.source.as_str()) {
                return Err(error(
                    "E_REFERENCE",
                    format!("{path}.source"),
                    "dependency declaring source has no source identity",
                ));
            }
            let Some(expected_hash) = source_by_path.get(dependency.path.as_str()) else {
                return Err(error(
                    "E_REFERENCE",
                    format!("{path}.path"),
                    "dependency target has no source identity",
                ));
            };
            if *expected_hash != dependency.hash {
                return Err(error(
                    "E_REFERENCE",
                    format!("{path}.hash"),
                    "dependency pin differs from its source identity",
                ));
            }
            if !dependency_ids.insert((dependency.source.as_str(), dependency.alias.as_str())) {
                return Err(error(
                    "E_DUPLICATE_ID",
                    path,
                    "duplicate dependency alias in a source",
                ));
            }
        }
        validate_dependency_graph(&self.dependencies)?;

        let mut program_ids = BTreeSet::new();
        for (index, program) in self.programs.iter().enumerate() {
            let path = format!("instruments.programs[{index}]");
            if !program_ids.insert(program.id.as_str()) {
                return Err(error(
                    "E_DUPLICATE_ID",
                    format!("{path}.id"),
                    "duplicate instrument program ID",
                ));
            }
            program
                .validate()
                .map_err(|error| contextualize(error, &path))?;
            if !source_by_path.contains_key(program.source.file.as_str()) {
                return Err(error(
                    "E_REFERENCE",
                    format!("{path}.source.file"),
                    "program source has no source identity",
                ));
            }
            validate_identifier(&program.source.object, format!("{path}.source.object"))?;
        }

        let mut table_ids = BTreeSet::new();
        for (index, table) in self.wavetables.iter().enumerate() {
            let path = format!("instruments.wavetables[{index}]");
            validate_identifier(&table.id, format!("{path}.id"))?;
            if !table_ids.insert(table.id.as_str()) {
                return Err(error(
                    "E_DUPLICATE_ID",
                    format!("{path}.id"),
                    "duplicate wavetable ID",
                ));
            }
            table
                .validate()
                .map_err(|error| contextualize(error, &path))?;
        }
        let mut sourced_tables = BTreeSet::new();
        for (index, source) in self.wavetable_sources.iter().enumerate() {
            let path = format!("instruments.wavetable_sources[{index}]");
            validate_identifier(&source.table, format!("{path}.table"))?;
            if !table_ids.contains(source.table.as_str()) {
                return Err(error(
                    "E_REFERENCE",
                    format!("{path}.table"),
                    "wavetable provenance refers to a missing embedded table",
                ));
            }
            if !sourced_tables.insert(source.table.as_str()) {
                return Err(error(
                    "E_DUPLICATE_ID",
                    format!("{path}.table"),
                    "duplicate wavetable provenance",
                ));
            }
            validate_path(&source.file, format!("{path}.file"))?;
            if !source_by_path.contains_key(source.file.as_str()) {
                return Err(error(
                    "E_REFERENCE",
                    format!("{path}.file"),
                    "wavetable declaring file has no source identity",
                ));
            }
            validate_identifier(&source.object, format!("{path}.object"))?;
            validate_path(&source.path, format!("{path}.path"))?;
            validate_hash(&source.hash, format!("{path}.hash"))?;
        }
        if sourced_tables.len() != table_ids.len() {
            return Err(error(
                "E_REFERENCE",
                "instruments.wavetable_sources",
                "every embedded wavetable requires exactly one provenance record",
            ));
        }
        for (program_index, program) in self.programs.iter().enumerate() {
            for (stage, graph) in std::iter::once(("voice", &program.voice))
                .chain(program.shared.as_ref().map(|graph| ("shared", graph)))
            {
                for (node_index, node) in graph.nodes.iter().enumerate() {
                    if let GraphProcessor::Wavetable { table } = &node.processor {
                        if !table_ids.contains(table.as_str()) {
                            return Err(error(
                                "E_REFERENCE",
                                format!(
                                    "instruments.programs[{program_index}].{stage}.nodes[{node_index}].processor.table"
                                ),
                                "graph refers to a missing embedded wavetable",
                            ));
                        }
                    }
                }
            }
        }

        let mut library_files = BTreeSet::new();
        for (index, library) in self.libraries.iter().enumerate() {
            let path = format!("instruments.libraries[{index}]");
            validate_path(&library.file, format!("{path}.file"))?;
            if !source_by_path.contains_key(library.file.as_str()) {
                return Err(error(
                    "E_REFERENCE",
                    format!("{path}.file"),
                    "library metadata file has no source identity",
                ));
            }
            if !library_files.insert(library.file.as_str()) {
                return Err(error(
                    "E_DUPLICATE_ID",
                    format!("{path}.file"),
                    "duplicate library metadata for source file",
                ));
            }
            validate_identifier(&library.object, format!("{path}.object"))?;
            validate_required_metadata(&library.version, format!("{path}.version"))?;
            if let Some(creator) = &library.creator {
                validate_optional_metadata(creator, format!("{path}.creator"))?;
            }
            if let Some(license) = &library.license {
                validate_optional_metadata(license, format!("{path}.license"))?;
            }
        }
        Ok(())
    }
}

fn validate_identifier(value: &str, path: impl Into<String>) -> Result<(), PlanError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_alphabetic() || byte == b'_'
            } else {
                byte.is_ascii_alphanumeric() || byte == b'_'
            }
        })
    {
        return Err(error(
            "E_RANGE",
            path,
            "identifier is not a bounded ASCII MaaC ID",
        ));
    }
    Ok(())
}

fn validate_path(value: &str, path: impl Into<String>) -> Result<(), PlanError> {
    if value.is_empty()
        || value.len() > 4_096
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains('\0')
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(error(
            "E_RANGE",
            path,
            "path must be normalized project-relative POSIX text",
        ));
    }
    Ok(())
}

fn validate_hash(value: &str, path: impl Into<String>) -> Result<(), PlanError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error(
            "E_RANGE",
            path,
            "hash must be sha256 followed by 64 lowercase hexadecimal digits",
        ));
    }
    Ok(())
}

fn validate_required_metadata(value: &str, path: impl Into<String>) -> Result<(), PlanError> {
    let path = path.into();
    if value.is_empty() {
        return Err(error("E_RANGE", path, "required metadata must be nonempty"));
    }
    validate_optional_metadata(value, path)
}

fn validate_optional_metadata(value: &str, path: impl Into<String>) -> Result<(), PlanError> {
    if value.len() > MAX_LIBRARY_METADATA_BYTES {
        Err(error(
            "E_RESOURCE_LIMIT",
            path,
            format!("metadata exceeds {MAX_LIBRARY_METADATA_BYTES} bytes"),
        ))
    } else {
        Ok(())
    }
}

fn validate_dependency_graph(dependencies: &[DependencyIdentity]) -> Result<(), PlanError> {
    let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for dependency in dependencies {
        outgoing
            .entry(dependency.source.as_str())
            .or_default()
            .push(dependency.path.as_str());
    }
    for targets in outgoing.values_mut() {
        targets.sort_unstable();
        targets.dedup();
    }

    fn visit<'a>(
        source: &'a str,
        outgoing: &BTreeMap<&'a str, Vec<&'a str>>,
        visiting: &mut BTreeSet<&'a str>,
        longest_from: &mut BTreeMap<&'a str, usize>,
    ) -> Result<usize, PlanError> {
        if let Some(&depth) = longest_from.get(source) {
            return Ok(depth);
        }
        if !visiting.insert(source) {
            return Err(error(
                "E_REFERENCE",
                "instruments.dependencies",
                "embedded dependency graph contains a cycle",
            ));
        }
        let mut longest = 0usize;
        if let Some(targets) = outgoing.get(source) {
            for target in targets {
                let target_depth = visit(target, outgoing, visiting, longest_from)?;
                longest = longest.max(target_depth.checked_add(1).ok_or_else(|| {
                    error(
                        "E_RESOURCE_LIMIT",
                        "instruments.dependencies",
                        "embedded dependency depth exceeds the bundle limit",
                    )
                })?);
            }
        }
        visiting.remove(source);
        if longest > crate::bundle::MAX_IMPORT_DEPTH {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "instruments.dependencies",
                "embedded dependency depth exceeds the bundle limit",
            ));
        }
        longest_from.insert(source, longest);
        Ok(longest)
    }

    let mut visiting = BTreeSet::new();
    let mut longest_from = BTreeMap::new();
    for source in outgoing.keys().copied() {
        visit(source, &outgoing, &mut visiting, &mut longest_from)?;
    }
    Ok(())
}

fn contextualize(mut error: PlanError, prefix: &str) -> PlanError {
    error.path = if error.path.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}.{}", error.path)
    };
    error
}

fn error(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> PlanError {
    PlanError {
        code: code.into(),
        path: path.into(),
        message: message.into(),
        span: None,
    }
}
