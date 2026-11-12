//! Validation and lowering of reusable MaaC instrument libraries.

use std::collections::{BTreeMap, BTreeSet};

use num_bigint::BigInt;
use num_traits::{One, ToPrimitive};
use serde::{Deserialize, Serialize};

use crate::bundle::{normalize_file_reference, ResolvedBundle, MAX_BUNDLE_SOURCE_BYTES};
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Span};
use crate::exact::Rational;
use crate::graph::{
    parameter_descriptor_for_stage, Control, ControlTarget, EffectProgram, GraphNode,
    GraphProcessor, GraphProgram, GraphStage, GraphUnit, InstrumentProgram, Modulation,
    ParameterSpec, ParameterTarget, ProgramSource, MAX_EMBEDDED_SAMPLES, MAX_GRAPH_PROGRAMS,
    MAX_TOTAL_GRAPH_EDGES, MAX_TOTAL_GRAPH_NODES,
};
use crate::kit_instrument::{
    KitControl, KitPiece, KitProgram, DEFAULT_PIECE_VOICES, MAX_PIECE_VOICES,
};
use crate::plan::{Connection, PlanError, PortRef, SourceSpan};
use crate::processor::{declaration_of, ConfigValue, Declaration, Holds};
use crate::sample_instrument::{InstrumentSample, SampleSource, SampleZone};
use crate::syntax::{Document, Field, Object, Reference, Unit, Value, ValueKind};
use crate::wavetable::Wavetable;

pub const MAX_LIBRARY_PROGRAMS: usize = MAX_GRAPH_PROGRAMS;
pub const MAX_LIBRARY_WAVETABLES: usize = 64;
pub const MAX_LIBRARY_GRAPH_NODES: usize = MAX_TOTAL_GRAPH_NODES;
pub const MAX_LIBRARY_GRAPH_EDGES: usize = MAX_TOTAL_GRAPH_EDGES;
pub const MAX_LIBRARY_SAMPLES: usize = MAX_EMBEDDED_SAMPLES;
pub const MAX_LIBRARY_METADATA_BYTES: usize = 4_096;
pub const DEFAULT_INSTANCE_VOICES: u32 = 64;
pub const MAX_INSTANCE_VOICES: u32 = 4_096;

type ExportKey = (String, String);

/// Resource declarations a graph node's config may reference, by export key.
struct ResourceExports {
    tables: BTreeMap<ExportKey, usize>,
    samples: BTreeMap<ExportKey, usize>,
    /// Channel count of each lowered sample, by export index.
    sample_channels: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryMetadata {
    pub file: String,
    pub object: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub license: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WavetableSource {
    pub table: String,
    pub file: String,
    pub object: String,
    pub path: String,
    pub hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedInstance {
    pub program_id: String,
    pub channels: u8,
    pub voices: u32,
    pub params: BTreeMap<String, Rational>,
    /// Whether `program_id` names a kit, which receives hits.
    pub kit: bool,
    /// For an effect, the width of its `in`; `program_id` then names an
    /// effect.
    pub effect_input: Option<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LibrarySet {
    pub programs: Vec<InstrumentProgram>,
    pub kits: Vec<KitProgram>,
    pub effects: Vec<EffectProgram>,
    pub wavetables: Vec<Wavetable>,
    pub wavetable_sources: Vec<WavetableSource>,
    pub samples: Vec<InstrumentSample>,
    pub sample_sources: Vec<SampleSource>,
    /// Plan audio assets holding asset-backed sample data.
    pub(crate) sample_assets: Vec<crate::audio_asset::AudioAsset>,
    pub metadata: Vec<LibraryMetadata>,
    entry_document: Document,
    resolved_document: Document,
    pattern_source_paths: BTreeMap<String, Vec<String>>,
    program_exports: BTreeMap<ExportKey, usize>,
    kit_exports: BTreeMap<ExportKey, usize>,
    effect_exports: BTreeMap<ExportKey, usize>,
    presets: BTreeMap<ExportKey, Preset>,
    imports: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Preset {
    instrument: ExportKey,
    params: BTreeMap<String, Rational>,
}

impl LibrarySet {
    pub fn resolve(bundle: &ResolvedBundle) -> Result<Self, Diagnostics> {
        let metadata = validate_documents(bundle)?;
        let instrument_keys = export_keys(bundle, "instrument");
        let wavetable_keys = export_keys(bundle, "wavetable");
        let sample_keys = export_keys(bundle, "sample");
        let effect_keys = export_keys(bundle, "effect");
        preflight(
            bundle,
            instrument_keys.len().saturating_add(effect_keys.len()),
            wavetable_keys.len(),
        )?;
        if sample_keys.len() > crate::sample_instrument::MAX_LIBRARY_SAMPLE_DECLARATIONS {
            return Err(simple_error(
                DiagnosticCode::ResourceLimit,
                format!(
                    "sample declarations exceed {}",
                    crate::sample_instrument::MAX_LIBRARY_SAMPLE_DECLARATIONS
                ),
            ));
        }

        // A declaration with `piece` children is a kit; the rest are programs.
        let (kit_keys, instrument_keys): (Vec<_>, Vec<_>) =
            instrument_keys.into_iter().partition(|(file, id)| {
                bundle.documents[file].objects[id]
                    .children
                    .values()
                    .any(|child| child.kind == "piece")
            });
        let program_exports = instrument_keys
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect::<BTreeMap<_, _>>();
        let kit_exports = kit_keys
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect::<BTreeMap<_, _>>();
        let index_keys = |keys: &[ExportKey]| {
            keys.iter()
                .cloned()
                .enumerate()
                .map(|(index, key)| (key, index))
                .collect::<BTreeMap<_, _>>()
        };
        let mut exports = ResourceExports {
            tables: index_keys(&wavetable_keys),
            samples: index_keys(&sample_keys),
            sample_channels: Vec::with_capacity(sample_keys.len()),
        };

        let mut wavetables = Vec::with_capacity(wavetable_keys.len());
        let mut wavetable_sources = Vec::with_capacity(wavetable_keys.len());
        let mut sample_count = 0usize;
        for (index, (file, object_id)) in wavetable_keys.iter().enumerate() {
            let object = &bundle.documents[file].objects[object_id];
            let (table, source) = lower_wavetable(bundle, file, object, format!("table_{index}"))?;
            sample_count = sample_count
                .checked_add(table.samples.len())
                .ok_or_else(|| {
                    simple_error(
                        DiagnosticCode::ResourceLimit,
                        "aggregate wavetable sample count overflowed",
                    )
                })?;
            if sample_count > MAX_LIBRARY_SAMPLES {
                return Err(simple_error(
                    DiagnosticCode::ResourceLimit,
                    format!("embedded wavetable samples exceed {MAX_LIBRARY_SAMPLES}"),
                ));
            }
            wavetables.push(table);
            wavetable_sources.push(source);
        }
        // Pitched samples share the embedded-sample budget with wavetables.
        let mut samples = Vec::with_capacity(sample_keys.len());
        let mut sample_sources = Vec::with_capacity(sample_keys.len());
        let mut sample_assets = Vec::new();
        for (index, (file, object_id)) in sample_keys.iter().enumerate() {
            let object = &bundle.documents[file].objects[object_id];
            let (sample, source, asset) =
                lower_sample(bundle, file, object, format!("sample_{index}"))?;
            sample_assets.extend(asset);
            // Asset-backed samples are not embedded, so only embedded values
            // count against the shared budget.
            sample_count = sample_count.saturating_add(sample.samples.len());
            if sample_count > MAX_LIBRARY_SAMPLES {
                return Err(simple_error(
                    DiagnosticCode::ResourceLimit,
                    format!("embedded wavetable and sample frames exceed {MAX_LIBRARY_SAMPLES}"),
                ));
            }
            exports.sample_channels.push(sample.channels);
            samples.push(sample);
            sample_sources.push(source);
        }

        let mut programs = Vec::with_capacity(instrument_keys.len());
        for (index, (file, object_id)) in instrument_keys.iter().enumerate() {
            programs.push(lower_instrument(
                bundle,
                file,
                &bundle.documents[file].objects[object_id],
                format!("program_{index}"),
                &exports,
            )?);
        }

        let mut kits = Vec::with_capacity(kit_keys.len());
        for (index, (file, object_id)) in kit_keys.iter().enumerate() {
            kits.push(lower_kit(
                bundle,
                file,
                &bundle.documents[file].objects[object_id],
                format!("kit_{index}"),
                &program_exports,
                &kit_exports,
                &programs,
            )?);
        }

        let effect_exports = effect_keys
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect::<BTreeMap<_, _>>();
        let mut effects = Vec::with_capacity(effect_keys.len());
        for (index, (file, object_id)) in effect_keys.iter().enumerate() {
            effects.push(lower_effect(
                bundle,
                file,
                &bundle.documents[file].objects[object_id],
                format!("effect_{index}"),
                &exports,
            )?);
        }

        let mut presets = BTreeMap::new();
        for (file, object_id) in export_keys(bundle, "preset") {
            let object = &bundle.documents[&file].objects[&object_id];
            presets.insert(
                (file.clone(), object_id),
                lower_preset(
                    bundle,
                    &file,
                    object,
                    &program_exports,
                    &programs,
                    &kit_exports,
                    &kits,
                    &effect_exports,
                    &effects,
                )?,
            );
        }

        let documents = resolve_entry_document(bundle)?;
        validate_musical_exports(&documents.musical)?;
        Ok(Self {
            programs,
            kits,
            effects,
            wavetables,
            wavetable_sources,
            samples,
            sample_sources,
            sample_assets,
            metadata,
            entry_document: documents.entry,
            resolved_document: documents.musical,
            pattern_source_paths: documents.pattern_source_paths,
            program_exports,
            kit_exports,
            effect_exports,
            presets,
            imports: bundle.imports.clone(),
        })
    }

    pub fn resolve_instance(
        &self,
        file: &str,
        node: &Object,
    ) -> Result<ResolvedInstance, Diagnostics> {
        require_kind(node, "node", file)?;
        if node.field("effect").is_some() {
            return self.resolve_effect_instance(file, node);
        }
        exact_fields(
            node,
            &["config", "instrument", "params", "preset"],
            &["instrument"],
            file,
        )?;
        no_children(node, file)?;
        let instrument_key = resolve_export_key_from_maps(
            file,
            reference_field(node, "instrument", file)?,
            self.imports.get(file),
        )?;
        if let Some(&kit_index) = self.kit_exports.get(&instrument_key) {
            return self.resolve_kit_instance(file, node, &instrument_key, &self.kits[kit_index]);
        }
        let program_index = *self.program_exports.get(&instrument_key).ok_or_else(|| {
            object_error(
                DiagnosticCode::Reference,
                file,
                node,
                "instrument reference does not name an instrument export",
            )
        })?;
        let program = &self.programs[program_index];
        let mut params = program
            .controls
            .iter()
            .map(|(name, control)| (name.clone(), control.default.clone()))
            .collect::<BTreeMap<_, _>>();

        if let Some(field) = node.field("preset") {
            let preset_key = resolve_export_key_from_maps(
                file,
                expect_reference(field, file, node)?,
                self.imports.get(file),
            )?;
            let preset = self.presets.get(&preset_key).ok_or_else(|| {
                field_error(
                    DiagnosticCode::Reference,
                    file,
                    node,
                    field,
                    "preset reference does not name a preset export",
                )
            })?;
            if preset.instrument != instrument_key {
                return Err(field_error(
                    DiagnosticCode::Reference,
                    file,
                    node,
                    field,
                    "preset selects a different instrument",
                ));
            }
            params.extend(preset.params.clone());
        }
        if let Some(field) = node.field("params") {
            params.extend(lower_control_values(field, file, node, &|name| {
                program.control_spec(name)
            })?);
        }
        let voices = match node.field("config") {
            Some(field) => lower_instance_config(field, file, node)?,
            None => DEFAULT_INSTANCE_VOICES,
        };
        Ok(ResolvedInstance {
            program_id: program.id.clone(),
            channels: program.channels(),
            voices,
            params,
            kit: false,
            effect_input: None,
        })
    }

    /// A kit instance layers kit control defaults, a preset, and its own
    /// params. Piece voices are declared by the kit, so it takes no config.
    fn resolve_kit_instance(
        &self,
        file: &str,
        node: &Object,
        key: &ExportKey,
        kit: &KitProgram,
    ) -> Result<ResolvedInstance, Diagnostics> {
        if let Some(field) = node.field("config") {
            return Err(field_error(
                DiagnosticCode::UnknownField,
                file,
                node,
                field,
                "a kit instance takes no config; each piece declares its voices",
            ));
        }
        let mut params = kit
            .controls
            .iter()
            .map(|(name, control)| (name.clone(), control.default.clone()))
            .collect::<BTreeMap<_, _>>();
        if let Some(field) = node.field("preset") {
            let preset_key = resolve_export_key_from_maps(
                file,
                expect_reference(field, file, node)?,
                self.imports.get(file),
            )?;
            let preset = self.presets.get(&preset_key).ok_or_else(|| {
                field_error(
                    DiagnosticCode::Reference,
                    file,
                    node,
                    field,
                    "preset reference does not name a preset export",
                )
            })?;
            if &preset.instrument != key {
                return Err(field_error(
                    DiagnosticCode::Reference,
                    file,
                    node,
                    field,
                    "preset selects a different instrument",
                ));
            }
            params.extend(preset.params.clone());
        }
        if let Some(field) = node.field("params") {
            params.extend(lower_control_values(field, file, node, &|name| {
                kit.control_spec(&self.programs, name)
            })?);
        }
        Ok(ResolvedInstance {
            program_id: kit.id.clone(),
            channels: kit.channels,
            voices: kit.voices(),
            params,
            kit: true,
            effect_input: None,
        })
    }

    /// An effect instance layers the effect's control defaults, a preset,
    /// and its own params. It has no voices, so it takes no config.
    fn resolve_effect_instance(
        &self,
        file: &str,
        node: &Object,
    ) -> Result<ResolvedInstance, Diagnostics> {
        exact_fields(node, &["effect", "params", "preset"], &["effect"], file)?;
        no_children(node, file)?;
        let key = resolve_export_key_from_maps(
            file,
            reference_field(node, "effect", file)?,
            self.imports.get(file),
        )?;
        let index = *self.effect_exports.get(&key).ok_or_else(|| {
            object_error(
                DiagnosticCode::Reference,
                file,
                node,
                "effect reference does not name an effect export",
            )
        })?;
        let effect = &self.effects[index];
        let mut params = effect
            .controls
            .iter()
            .map(|(name, control)| (name.clone(), control.default.clone()))
            .collect::<BTreeMap<_, _>>();
        if let Some(field) = node.field("preset") {
            let preset_key = resolve_export_key_from_maps(
                file,
                expect_reference(field, file, node)?,
                self.imports.get(file),
            )?;
            let preset = self.presets.get(&preset_key).ok_or_else(|| {
                field_error(
                    DiagnosticCode::Reference,
                    file,
                    node,
                    field,
                    "preset reference does not name a preset export",
                )
            })?;
            if preset.instrument != key {
                return Err(field_error(
                    DiagnosticCode::Reference,
                    file,
                    node,
                    field,
                    "preset selects a different effect",
                ));
            }
            params.extend(preset.params.clone());
        }
        if let Some(field) = node.field("params") {
            params.extend(lower_control_values(field, file, node, &|name| {
                effect.control_spec(name)
            })?);
        }
        Ok(ResolvedInstance {
            program_id: effect.id.clone(),
            channels: effect.channels(),
            voices: 0,
            params,
            kit: false,
            effect_input: Some(effect.input_channels),
        })
    }

    /// What a resolved instance's event port plays: a kit hits on its
    /// pieces' keys, and an instrument notes, with what its voice provides
    /// for. An effect has no event port.
    pub(crate) fn instance_receives(
        &self,
        instance: &ResolvedInstance,
    ) -> Option<crate::semantic::Receives> {
        use crate::semantic::Receives;
        if instance.effect_input.is_some() {
            None
        } else if instance.kit {
            let kit = self.kits.iter().find(|kit| kit.id == instance.program_id)?;
            Some(Receives::Hits(
                kit.pieces.iter().map(|piece| piece.key.clone()).collect(),
            ))
        } else {
            let program = self
                .programs
                .iter()
                .find(|program| program.id == instance.program_id)?;
            Some(Receives::Notes {
                pressure: program.supports_pressure(),
                timbre: program.supports_timbre(),
                connected: program.plays_connected(),
            })
        }
    }

    /// The metadata of a resolved instance's public control.
    pub(crate) fn instance_control_spec(
        &self,
        instance: &ResolvedInstance,
        name: &str,
    ) -> Option<ParameterSpec> {
        if instance.effect_input.is_some() {
            self.effects
                .iter()
                .find(|effect| effect.id == instance.program_id)?
                .control_spec(name)
        } else if instance.kit {
            self.kits
                .iter()
                .find(|kit| kit.id == instance.program_id)?
                .control_spec(&self.programs, name)
        } else {
            self.programs
                .iter()
                .find(|program| program.id == instance.program_id)?
                .control_spec(name)
        }
    }

    /// The public control names of a resolved instance.
    pub(crate) fn instance_control_names(&self, instance: &ResolvedInstance) -> Vec<String> {
        if instance.effect_input.is_some() {
            self.effects
                .iter()
                .find(|effect| effect.id == instance.program_id)
                .map(|effect| effect.controls.keys().cloned().collect())
                .unwrap_or_default()
        } else if instance.kit {
            self.kits
                .iter()
                .find(|kit| kit.id == instance.program_id)
                .map(|kit| kit.controls.keys().cloned().collect())
                .unwrap_or_default()
        } else {
            self.programs
                .iter()
                .find(|program| program.id == instance.program_id)
                .map(|program| program.controls.keys().cloned().collect())
                .unwrap_or_default()
        }
    }

    pub fn entry_document(&self) -> Document {
        self.entry_document.clone()
    }

    pub(crate) fn resolved_document(&self) -> Document {
        self.resolved_document.clone()
    }

    /// Whether any sample plays a plan audio asset, its own or a
    /// composition's, so the plan must carry audio assets.
    pub(crate) fn has_asset_samples(&self) -> bool {
        self.samples.iter().any(|sample| sample.asset.is_some())
    }

    pub(crate) fn pattern_source_paths(&self) -> &BTreeMap<String, Vec<String>> {
        &self.pattern_source_paths
    }
}

const MUSICAL_EXPORT_KINDS: &[&str] = &["curve", "pattern", "tuning"];

fn is_musical_export(object: &Object) -> bool {
    MUSICAL_EXPORT_KINDS.contains(&object.kind.as_str())
}

struct ResolvedEntryDocuments {
    entry: Document,
    musical: Document,
    pattern_source_paths: BTreeMap<String, Vec<String>>,
}

struct MusicalExportState<'a> {
    musical_subtrees: &'a BTreeMap<String, bool>,
    document: &'a mut Document,
    pattern_source_paths: &'a mut BTreeMap<String, Vec<String>>,
    object_count: usize,
    route_work: usize,
    expanded_bytes: usize,
}

fn resolve_entry_document(bundle: &ResolvedBundle) -> Result<ResolvedEntryDocuments, Diagnostics> {
    let mut entry_document = bundle
        .documents
        .get(&bundle.entry)
        .cloned()
        .ok_or_else(|| {
            simple_error(
                DiagnosticCode::Reference,
                format!("entry source `{}` is missing", bundle.entry),
            )
        })?;
    entry_document.objects.retain(|_, object| {
        !matches!(
            object.kind.as_str(),
            "library" | "import" | "instrument" | "effect" | "preset" | "wavetable" | "sample"
        )
    });

    let mut expanded_bytes = 0usize;
    for object in entry_document
        .objects
        .values()
        .filter(|object| is_musical_export(object))
    {
        charge_expanded_musical_bytes(bundle, &bundle.entry, &[], object, &mut expanded_bytes)?;
    }

    let mut resolved_document = entry_document.clone();
    for object in resolved_document.objects.values_mut() {
        rewrite_object_references(bundle, &bundle.entry, &[], object);
    }

    let mut pattern_source_paths = resolved_document
        .objects
        .values()
        .filter(|object| object.kind == "pattern")
        .map(|object| (object.id.clone(), vec![object.id.clone()]))
        .collect::<BTreeMap<_, _>>();
    let object_count = resolved_document
        .objects
        .values()
        .map(count_object)
        .sum::<usize>();
    let mut musical_subtrees = BTreeMap::new();
    index_musical_subtrees(bundle, &bundle.entry, &mut musical_subtrees)?;
    let mut state = MusicalExportState {
        musical_subtrees: &musical_subtrees,
        document: &mut resolved_document,
        pattern_source_paths: &mut pattern_source_paths,
        object_count,
        route_work: 0,
        expanded_bytes,
    };
    add_imported_musical_exports(bundle, &bundle.entry, &[], &mut state)?;
    Ok(ResolvedEntryDocuments {
        entry: entry_document,
        musical: resolved_document,
        pattern_source_paths,
    })
}

fn index_musical_subtrees(
    bundle: &ResolvedBundle,
    file: &str,
    memo: &mut BTreeMap<String, bool>,
) -> Result<bool, Diagnostics> {
    if let Some(has_musical) = memo.get(file) {
        return Ok(*has_musical);
    }
    let document = bundle.documents.get(file).ok_or_else(|| {
        simple_error(
            DiagnosticCode::Reference,
            format!("resolved source `{file}` is missing"),
        )
    })?;
    let imports = bundle.imports.get(file).ok_or_else(|| {
        simple_error(
            DiagnosticCode::Reference,
            format!("resolved imports for `{file}` are missing"),
        )
    })?;
    let mut has_musical = document.objects.values().any(is_musical_export);
    for target in imports.values() {
        has_musical |= index_musical_subtrees(bundle, target, memo)?;
    }
    memo.insert(file.to_owned(), has_musical);
    Ok(has_musical)
}

fn add_imported_musical_exports(
    bundle: &ResolvedBundle,
    declaring_file: &str,
    route: &[String],
    state: &mut MusicalExportState<'_>,
) -> Result<(), Diagnostics> {
    let imports = bundle.imports.get(declaring_file).ok_or_else(|| {
        simple_error(
            DiagnosticCode::Reference,
            format!("resolved imports for `{declaring_file}` are missing"),
        )
    })?;
    for (alias, target_file) in imports {
        if !state
            .musical_subtrees
            .get(target_file)
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        state.route_work = state.route_work.checked_add(1).ok_or_else(|| {
            simple_error(
                DiagnosticCode::ResourceLimit,
                "resolved musical import route count overflowed",
            )
        })?;
        if state.route_work > crate::bundle::MAX_SYNTAX_OBJECTS {
            return Err(simple_error(
                DiagnosticCode::ResourceLimit,
                format!(
                    "resolved musical import routes exceed {}",
                    crate::bundle::MAX_SYNTAX_OBJECTS
                ),
            ));
        }
        let mut target_route = route.to_vec();
        target_route.push(alias.clone());
        let target = bundle.documents.get(target_file).ok_or_else(|| {
            simple_error(
                DiagnosticCode::Reference,
                format!("resolved source `{target_file}` is missing"),
            )
        })?;
        for authored in target
            .objects
            .values()
            .filter(|object| is_musical_export(object))
        {
            charge_expanded_musical_bytes(
                bundle,
                target_file,
                &target_route,
                authored,
                &mut state.expanded_bytes,
            )?;
            let mut object = authored.clone();
            let mut source_path = target_route.clone();
            source_path.push(authored.id.clone());
            let resolved_id = source_path.join(".");
            object.id = resolved_id.clone();
            rewrite_object_references(bundle, target_file, &target_route, &mut object);
            state.object_count = state
                .object_count
                .checked_add(count_object(&object))
                .ok_or_else(|| {
                    simple_error(
                        DiagnosticCode::ResourceLimit,
                        "resolved musical export object count overflowed",
                    )
                })?;
            if state.object_count > crate::bundle::MAX_SYNTAX_OBJECTS {
                return Err(simple_error(
                    DiagnosticCode::ResourceLimit,
                    format!(
                        "resolved musical exports exceed {} syntax objects",
                        crate::bundle::MAX_SYNTAX_OBJECTS
                    ),
                ));
            }
            if object.kind == "pattern" {
                state
                    .pattern_source_paths
                    .insert(resolved_id.clone(), source_path);
            }
            state.document.objects.insert(resolved_id, object);
        }
        add_imported_musical_exports(bundle, target_file, &target_route, state)?;
    }
    Ok(())
}

fn charge_expanded_musical_bytes(
    bundle: &ResolvedBundle,
    declaring_file: &str,
    route: &[String],
    object: &Object,
    expanded_bytes: &mut usize,
) -> Result<(), Diagnostics> {
    add_expanded_bytes(expanded_bytes, object.span.len())?;

    if !route.is_empty() {
        let resolved_id_bytes = route
            .iter()
            .try_fold(object.id.len(), |total, component| {
                total
                    .checked_add(component.len())
                    .and_then(|total| total.checked_add(1))
            })
            .ok_or_else(expanded_musical_bytes_overflow)?;
        add_expanded_bytes(expanded_bytes, resolved_id_bytes)?;
    }
    if object.kind == "pattern" {
        let source_path_bytes = route
            .iter()
            .try_fold(object.id.len(), |total, component| {
                total.checked_add(component.len())
            })
            .ok_or_else(expanded_musical_bytes_overflow)?;
        add_expanded_bytes(expanded_bytes, source_path_bytes)?;
    }
    charge_object_reference_bytes(bundle, declaring_file, route, object, expanded_bytes)
}

fn charge_object_reference_bytes(
    bundle: &ResolvedBundle,
    declaring_file: &str,
    route: &[String],
    object: &Object,
    expanded_bytes: &mut usize,
) -> Result<(), Diagnostics> {
    for field in object.fields.values() {
        charge_value_reference_bytes(bundle, declaring_file, route, &field.value, expanded_bytes)?;
    }
    for child in object.children.values() {
        charge_object_reference_bytes(bundle, declaring_file, route, child, expanded_bytes)?;
    }
    Ok(())
}

fn charge_value_reference_bytes(
    bundle: &ResolvedBundle,
    declaring_file: &str,
    route: &[String],
    value: &Value,
    expanded_bytes: &mut usize,
) -> Result<(), Diagnostics> {
    match &value.kind {
        ValueKind::Reference(reference) if reference.port.is_none() => {
            let path = match reference.path.as_slice() {
                [id] => Some((route, None, id)),
                [alias, id] => bundle
                    .imports
                    .get(declaring_file)
                    .and_then(|imports| imports.get(alias))
                    .and_then(|target_file| bundle.documents.get(target_file))
                    .and_then(|document| document.objects.get(id))
                    .filter(|target| is_musical_export(target))
                    .map(|_| (route, Some(alias), id)),
                _ => None,
            };
            if let Some((route, alias, id)) = path {
                for component in route {
                    add_expanded_bytes(expanded_bytes, component.len())?;
                }
                if let Some(alias) = alias {
                    add_expanded_bytes(expanded_bytes, alias.len())?;
                }
                add_expanded_bytes(expanded_bytes, id.len())?;
            }
        }
        ValueKind::Call { args, .. } | ValueKind::List(args) | ValueKind::Tuple(args) => {
            for value in args {
                charge_value_reference_bytes(bundle, declaring_file, route, value, expanded_bytes)?;
            }
        }
        ValueKind::Record(fields) => {
            for field in fields.values() {
                charge_value_reference_bytes(
                    bundle,
                    declaring_file,
                    route,
                    &field.value,
                    expanded_bytes,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn add_expanded_bytes(total: &mut usize, bytes: usize) -> Result<(), Diagnostics> {
    *total = total
        .checked_add(bytes)
        .ok_or_else(expanded_musical_bytes_overflow)?;
    if *total > MAX_BUNDLE_SOURCE_BYTES {
        return Err(simple_error(
            DiagnosticCode::ResourceLimit,
            format!("expanded musical catalog exceeds {MAX_BUNDLE_SOURCE_BYTES} source bytes"),
        ));
    }
    Ok(())
}

fn expanded_musical_bytes_overflow() -> Diagnostics {
    simple_error(
        DiagnosticCode::ResourceLimit,
        "expanded musical catalog byte count overflowed",
    )
}

fn count_object(object: &Object) -> usize {
    1 + object.children.values().map(count_object).sum::<usize>()
}

fn rewrite_object_references(
    bundle: &ResolvedBundle,
    declaring_file: &str,
    route: &[String],
    object: &mut Object,
) {
    for field in object.fields.values_mut() {
        rewrite_value_references(bundle, declaring_file, route, &mut field.value);
    }
    for child in object.children.values_mut() {
        rewrite_object_references(bundle, declaring_file, route, child);
    }
}

fn rewrite_value_references(
    bundle: &ResolvedBundle,
    declaring_file: &str,
    route: &[String],
    value: &mut Value,
) {
    match &mut value.kind {
        ValueKind::Reference(reference) if reference.port.is_none() => {
            let resolved = match reference.path.as_slice() {
                [id] => {
                    let mut path = route.to_vec();
                    path.push(id.clone());
                    Some(path)
                }
                [alias, id] => bundle
                    .imports
                    .get(declaring_file)
                    .and_then(|imports| imports.get(alias))
                    .and_then(|file| bundle.documents.get(file))
                    .and_then(|document| document.objects.get(id))
                    .filter(|object| is_musical_export(object))
                    .map(|_| {
                        let mut path = route.to_vec();
                        path.push(alias.clone());
                        path.push(id.clone());
                        path
                    }),
                _ => None,
            };
            if let Some(resolved) = resolved {
                reference.path = vec![resolved.join(".")];
            }
        }
        ValueKind::Call { args, .. } | ValueKind::List(args) | ValueKind::Tuple(args) => {
            for item in args {
                rewrite_value_references(bundle, declaring_file, route, item);
            }
        }
        ValueKind::Record(fields) => {
            for field in fields.values_mut() {
                rewrite_value_references(bundle, declaring_file, route, &mut field.value);
            }
        }
        ValueKind::Number(_)
        | ValueKind::Quantity { .. }
        | ValueKind::String(_)
        | ValueKind::Symbol(_)
        | ValueKind::Boolean(_)
        | ValueKind::Reference(_) => {}
    }
}

fn validate_musical_exports(document: &Document) -> Result<(), Diagnostics> {
    if !document.objects.values().any(is_musical_export) {
        return Ok(());
    }
    let models = crate::semantic::validate_musical_catalog(document)?;
    crate::compiler::validate_musical_catalog(document, models)
}

fn validate_documents(bundle: &ResolvedBundle) -> Result<Vec<LibraryMetadata>, Diagnostics> {
    let mut metadata = Vec::new();
    for (file, document) in &bundle.documents {
        let libraries = document
            .objects
            .values()
            .filter(|object| object.kind == "library")
            .collect::<Vec<_>>();
        let projects = document
            .objects
            .values()
            .filter(|object| object.kind == "project")
            .collect::<Vec<_>>();
        if libraries.len() + projects.len() != 1 {
            return Err(simple_error(
                DiagnosticCode::Conflict,
                format!("`{file}` must contain exactly one library or project declaration"),
            ));
        }
        if file != &bundle.entry && libraries.len() != 1 {
            return Err(simple_error(
                DiagnosticCode::Reference,
                format!("imported source `{file}` must be a library document"),
            ));
        }
        if let Some(library) = libraries.first() {
            exact_fields(
                library,
                &["creator", "license", "version"],
                &["version"],
                file,
            )?;
            no_children(library, file)?;
            let version = string_field(library, "version", file)?;
            if version.is_empty() {
                return Err(object_error(
                    DiagnosticCode::Version,
                    file,
                    library,
                    "library version must be nonempty",
                ));
            }
            validate_metadata_length(file, library, "version", &version)?;
            let creator = optional_string_field(library, "creator", file)?;
            if let Some(value) = &creator {
                validate_metadata_length(file, library, "creator", value)?;
            }
            let license = optional_string_field(library, "license", file)?;
            if let Some(value) = &license {
                validate_metadata_length(file, library, "license", value)?;
            }
            metadata.push(LibraryMetadata {
                file: file.clone(),
                object: library.id.clone(),
                version,
                creator,
                license,
            });
            for object in document.objects.values() {
                if !matches!(
                    object.kind.as_str(),
                    "library"
                        | "import"
                        | "instrument"
                        | "effect"
                        | "preset"
                        | "wavetable"
                        | "sample"
                        | "pattern"
                        | "curve"
                        | "tuning"
                ) {
                    return Err(object_error(
                        DiagnosticCode::UnknownKind,
                        file,
                        object,
                        format!("library documents cannot contain `{}` objects", object.kind),
                    ));
                }
            }
        }
    }
    Ok(metadata)
}

fn validate_metadata_length(
    file: &str,
    object: &Object,
    name: &str,
    value: &str,
) -> Result<(), Diagnostics> {
    if value.len() > MAX_LIBRARY_METADATA_BYTES {
        Err(field_error(
            DiagnosticCode::ResourceLimit,
            file,
            object,
            object.field(name).expect("validated metadata field exists"),
            format!("{name} exceeds {MAX_LIBRARY_METADATA_BYTES} bytes"),
        ))
    } else {
        Ok(())
    }
}

fn export_keys(bundle: &ResolvedBundle, kind: &str) -> Vec<ExportKey> {
    bundle
        .documents
        .iter()
        .flat_map(|(file, document)| {
            document
                .objects
                .iter()
                .filter(move |(_, object)| object.kind == kind)
                .map(move |(id, _)| (file.clone(), id.clone()))
        })
        .collect()
}

fn preflight(bundle: &ResolvedBundle, programs: usize, tables: usize) -> Result<(), Diagnostics> {
    if programs > MAX_LIBRARY_PROGRAMS {
        return Err(simple_error(
            DiagnosticCode::ResourceLimit,
            format!("instrument programs exceed {MAX_LIBRARY_PROGRAMS}"),
        ));
    }
    if tables > MAX_LIBRARY_WAVETABLES {
        return Err(simple_error(
            DiagnosticCode::ResourceLimit,
            format!("wavetables exceed {MAX_LIBRARY_WAVETABLES}"),
        ));
    }
    let mut nodes = 0usize;
    let mut edges = 0usize;
    for document in bundle.documents.values() {
        for instrument in document
            .objects
            .values()
            .filter(|object| matches!(object.kind.as_str(), "instrument" | "effect"))
        {
            for graph in instrument
                .children
                .values()
                .filter(|object| matches!(object.kind.as_str(), "voice" | "shared"))
            {
                nodes = nodes.saturating_add(
                    graph
                        .children
                        .values()
                        .filter(|object| object.kind == "node")
                        .count(),
                );
                edges = edges.saturating_add(
                    graph
                        .children
                        .values()
                        .filter(|object| matches!(object.kind.as_str(), "connect" | "modulate"))
                        .count(),
                );
            }
        }
    }
    if nodes > MAX_LIBRARY_GRAPH_NODES {
        return Err(simple_error(
            DiagnosticCode::ResourceLimit,
            format!("aggregate graph nodes exceed {MAX_LIBRARY_GRAPH_NODES}"),
        ));
    }
    if edges > MAX_LIBRARY_GRAPH_EDGES {
        return Err(simple_error(
            DiagnosticCode::ResourceLimit,
            format!("aggregate graph edges exceed {MAX_LIBRARY_GRAPH_EDGES}"),
        ));
    }
    Ok(())
}

fn lower_wavetable(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    id: String,
) -> Result<(Wavetable, WavetableSource), Diagnostics> {
    exact_fields(
        object,
        &["cycle_length", "hash", "path"],
        &["cycle_length", "hash", "path"],
        file,
    )?;
    no_children(object, file)?;
    let declared_path = string_field(object, "path", file)?;
    let asset_path = normalize_file_reference(file, &declared_path)?;
    let bytes = bundle.assets.get(&asset_path).ok_or_else(|| {
        object_error(
            DiagnosticCode::Asset,
            file,
            object,
            format!("resolved asset `{asset_path}` is missing"),
        )
    })?;
    let cycle_length = integer_field(object, "cycle_length", file, 1, u32::MAX)?;
    let table = Wavetable::from_wav(id.clone(), bytes, cycle_length)
        .map_err(|error| plan_diagnostics(error, file, object))?;
    Ok((
        table,
        WavetableSource {
            table: id,
            file: file.to_owned(),
            object: object.id.clone(),
            path: asset_path,
            hash: string_field(object, "hash", file)?,
        },
    ))
}

type LoweredSample = (
    InstrumentSample,
    SampleSource,
    Option<crate::audio_asset::AudioAsset>,
);

/// Lower a `sample` declaration: a mono or stereo WAV, with `format` a mono
/// or stereo core PCM file carried as a plan audio asset, or with `asset` a
/// composition audio asset. Each form adds a 12-TET root key and an optional
/// forward sustain loop in sample frames.
fn lower_sample(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    id: String,
) -> Result<LoweredSample, Diagnostics> {
    let reused = object.field("asset").is_some();
    let native = !reused && object.field("format").is_some();
    if reused {
        exact_fields(object, &["asset", "loop", "root"], &["asset", "root"], file)?;
    } else if native {
        exact_fields(
            object,
            &[
                "channels", "format", "frames", "hash", "loop", "path", "rate", "root",
            ],
            &["format", "frames", "hash", "path", "rate", "root"],
            file,
        )?;
    } else {
        exact_fields(
            object,
            &["hash", "loop", "path", "root"],
            &["hash", "path", "root"],
            file,
        )?;
    }
    no_children(object, file)?;
    let root = object.field("root").expect("required field checked");
    let root_key = key_value(&root.value, file, object, root)?;
    let loop_frames = object
        .field("loop")
        .map(|field| {
            let ValueKind::List(items) = &field.value.kind else {
                return Err(field_error(
                    DiagnosticCode::Unit,
                    file,
                    object,
                    field,
                    "loop requires [start frame, end frame]",
                ));
            };
            let frame = |value: &Value| match &value.kind {
                ValueKind::Quantity {
                    value,
                    unit: Unit::Frame,
                } if value.is_integer() => value.to_integer().to_u64().ok_or_else(|| {
                    field_error(
                        DiagnosticCode::Range,
                        file,
                        object,
                        field,
                        "loop frames must be nonnegative integers",
                    )
                }),
                _ => Err(field_error(
                    DiagnosticCode::Unit,
                    file,
                    object,
                    field,
                    "loop endpoints require integer frames",
                )),
            };
            match items.as_slice() {
                [start, end] => Ok([frame(start)?, frame(end)?]),
                _ => Err(field_error(
                    DiagnosticCode::Range,
                    file,
                    object,
                    field,
                    "loop requires exactly two frame endpoints",
                )),
            }
        })
        .transpose()?;
    if reused {
        return lower_asset_sample(bundle, file, object, id, root_key, loop_frames);
    }
    let declared_path = string_field(object, "path", file)?;
    let asset_path = normalize_file_reference(file, &declared_path)?;
    let bytes = bundle.assets.get(&asset_path);
    let disk = bundle.disk_assets.get(&asset_path);
    if bytes.is_none() && disk.is_none() {
        return Err(object_error(
            DiagnosticCode::Asset,
            file,
            object,
            format!("resolved asset `{asset_path}` is missing"),
        ));
    }
    let hash = string_field(object, "hash", file)?;
    let source = SampleSource {
        sample: id.clone(),
        file: file.to_owned(),
        object: object.id.clone(),
        path: asset_path,
        hash: hash.clone(),
    };
    if !native {
        let bytes = bytes.ok_or_else(|| {
            object_error(
                DiagnosticCode::Capability,
                file,
                object,
                "WAV samples are embedded and cannot be disk media; declare a core PCM format",
            )
        })?;
        let sample = InstrumentSample::from_wav(id, bytes, root_key, loop_frames)
            .map_err(|error| plan_diagnostics(error, file, object))?;
        return Ok((sample, source, None));
    }

    let format = string_field(object, "format", file)?;
    if format != crate::audio_asset::CORE_AUDIO_FORMAT {
        return Err(field_error(
            DiagnosticCode::Asset,
            file,
            object,
            object.field("format").expect("required field checked"),
            format!(
                "sample format must be `{}`",
                crate::audio_asset::CORE_AUDIO_FORMAT
            ),
        ));
    }
    let rate_field = object.field("rate").expect("required field checked");
    let rate_hz = match &rate_field.value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::Hz,
        } if value.is_integer() => value.to_integer().to_u32().filter(|rate| *rate > 0),
        _ => None,
    }
    .ok_or_else(|| {
        field_error(
            DiagnosticCode::Unit,
            file,
            object,
            rate_field,
            "rate requires a positive integer Hz quantity",
        )
    })?;
    let frames_field = object.field("frames").expect("required field checked");
    let frames = match &frames_field.value.kind {
        ValueKind::Number(value) if value.is_integer() => value.to_integer().to_u64(),
        _ => None,
    }
    .ok_or_else(|| {
        field_error(
            DiagnosticCode::Range,
            file,
            object,
            frames_field,
            "frames requires a nonnegative integer",
        )
    })?;
    let channels = match object.field("channels") {
        None => 1,
        Some(_) => integer_field(object, "channels", file, 1, 2)? as u8,
    };
    // Asset ids share the plan's audio-asset namespace with composition
    // assets; the reserved `__` prefix keeps them apart.
    let asset_id = format!("__{id}");
    let asset = crate::audio_asset::AudioAsset {
        id: asset_id.clone(),
        format,
        rate_hz,
        channels,
        frames,
        hash,
        bytes: bytes.cloned().unwrap_or_default(),
        disk: disk.cloned(),
    };
    // Report asset mismatches at the declaration, not the plan asset path.
    asset.validate().map_err(|error| {
        let mut relocated = Diagnostics::new();
        for mut diagnostic in plan_diagnostics(error, file, object) {
            diagnostic.object_path = vec![object.id.clone()];
            diagnostic.field_path.clear();
            relocated.push(diagnostic);
        }
        relocated
    })?;
    let sample = InstrumentSample {
        id,
        rate_hz,
        channels,
        root_key,
        loop_frames,
        samples: Vec::new(),
        asset: Some(crate::sample_instrument::SampleAsset {
            id: asset_id,
            frames,
        }),
    };
    sample
        .validate()
        .map_err(|error| plan_diagnostics(error, file, object))?;
    Ok((sample, source, Some(asset)))
}

/// Lower `sample { asset = &a; ... }`. The sample takes its rate, channels,
/// frames and provenance from an audio asset declared in the same
/// composition document and plays that asset's plan data, so the plan
/// carries the recording once however many clips, kits and samples use it.
fn lower_asset_sample(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    id: String,
    root_key: i64,
    loop_frames: Option<[u64; 2]>,
) -> Result<LoweredSample, Diagnostics> {
    let field = object.field("asset").expect("required field checked");
    let reference = expect_reference(field, file, object)?;
    let asset = match (reference.path.as_slice(), &reference.port) {
        ([name], None) => bundle
            .documents
            .get(file)
            .and_then(|document| document.objects.get(name)),
        _ => None,
    }
    .filter(|asset| {
        asset.kind == "asset"
            && asset.field("kind").and_then(|kind| kind.value.as_symbol()) == Some("audio")
    })
    .ok_or_else(|| {
        field_error(
            DiagnosticCode::Reference,
            file,
            object,
            field,
            "sample asset must name an audio asset declared in the same composition document",
        )
    })?;
    // The composition validates the asset itself; this reads only what the
    // sample needs from it.
    let required = |name: &str| {
        asset.field(name).ok_or_else(|| {
            object_error(
                DiagnosticCode::UnknownField,
                file,
                asset,
                format!("audio asset requires field `{name}`"),
            )
        })
    };
    for name in ["path", "hash", "format", "rate", "channels", "frames"] {
        required(name)?;
    }
    if string_field(asset, "format", file)? != crate::audio_asset::CORE_AUDIO_FORMAT {
        return Err(field_error(
            DiagnosticCode::Asset,
            file,
            object,
            field,
            format!(
                "sample asset format must be `{}`",
                crate::audio_asset::CORE_AUDIO_FORMAT
            ),
        ));
    }
    let rate_field = required("rate")?;
    let rate_hz = match &rate_field.value.kind {
        ValueKind::Quantity {
            value,
            unit: Unit::Hz,
        } if value.is_integer() => value.to_integer().to_u32().filter(|rate| *rate > 0),
        _ => None,
    }
    .ok_or_else(|| {
        field_error(
            DiagnosticCode::Unit,
            file,
            asset,
            rate_field,
            "rate requires a positive integer Hz quantity",
        )
    })?;
    let frames_field = required("frames")?;
    let frames = match &frames_field.value.kind {
        ValueKind::Number(value) if value.is_integer() => value.to_integer().to_u64(),
        _ => None,
    }
    .ok_or_else(|| {
        field_error(
            DiagnosticCode::Range,
            file,
            asset,
            frames_field,
            "frames requires a nonnegative integer",
        )
    })?;
    let channels = integer_field(asset, "channels", file, 1, 2)? as u8;
    let source = SampleSource {
        sample: id.clone(),
        file: file.to_owned(),
        object: object.id.clone(),
        path: normalize_file_reference(file, &string_field(asset, "path", file)?)?,
        hash: string_field(asset, "hash", file)?,
    };
    let sample = InstrumentSample {
        id,
        rate_hz,
        channels,
        root_key,
        loop_frames,
        samples: Vec::new(),
        asset: Some(crate::sample_instrument::SampleAsset {
            id: asset.id.clone(),
            frames,
        }),
    };
    sample
        .validate()
        .map_err(|error| plan_diagnostics(error, file, object))?;
    Ok((sample, source, None))
}

/// A 12-TET key given as a spelled pitch (`C4`) or `key(n)`.
fn key_value(
    value: &Value,
    file: &str,
    object: &Object,
    field: &Field,
) -> Result<i64, Diagnostics> {
    let key = match &value.kind {
        ValueKind::Symbol(text) => crate::music::Pitch::parse_spelled(text)
            .ok()
            .and_then(|pitch| pitch.key_index()),
        ValueKind::Call { function, args } if function == "key" && args.len() == 1 => {
            match &args[0].kind {
                ValueKind::Number(number) if number.is_integer() => number.to_integer().to_i64(),
                _ => None,
            }
        }
        _ => None,
    };
    let key = key.ok_or_else(|| {
        field_error(
            DiagnosticCode::Unit,
            file,
            object,
            field,
            format!("{} requires a spelled pitch or key(n)", field.name),
        )
    })?;
    if !(0..=crate::sample_instrument::MAX_SAMPLE_KEY).contains(&key) {
        return Err(field_error(
            DiagnosticCode::Range,
            file,
            object,
            field,
            format!(
                "{} must be key 0 through {}",
                field.name,
                crate::sample_instrument::MAX_SAMPLE_KEY
            ),
        ));
    }
    Ok(key)
}

/// Lower `zones = [{ sample = &s; low = <pitch>; high = <pitch>; }, ...]`.
/// Every zone sample must have the node's `channels`.
fn lower_sample_zones(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    field: &Field,
    channels: u8,
    exports: &ResourceExports,
) -> Result<Vec<SampleZone>, Diagnostics> {
    let ValueKind::List(items) = &field.value.kind else {
        return Err(field_error(
            DiagnosticCode::Unit,
            file,
            object,
            field,
            "zones requires a list of zone records",
        ));
    };
    let mut zones = Vec::with_capacity(items.len());
    for item in items {
        let ValueKind::Record(record) = &item.kind else {
            return Err(field_error(
                DiagnosticCode::Unit,
                file,
                object,
                field,
                "each zone must be a record",
            ));
        };
        let names = record.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let optional = BTreeSet::from(["key_fade", "velocity", "velocity_fade"]);
        if !["high", "low", "sample"]
            .iter()
            .all(|name| names.contains(name))
            || names
                .iter()
                .any(|name| !matches!(*name, "high" | "low" | "sample") && !optional.contains(name))
        {
            return Err(field_error(
                DiagnosticCode::UnknownField,
                file,
                object,
                field,
                "zone records contain sample, low, and high, and optionally velocity, key_fade, and velocity_fade",
            ));
        }
        let sample_field = &record["sample"];
        let key = resolve_export_key(bundle, file, expect_reference(sample_field, file, object)?)?;
        let index = exports.samples.get(&key).ok_or_else(|| {
            field_error(
                DiagnosticCode::Reference,
                file,
                object,
                sample_field,
                "zone sample does not name a sample export",
            )
        })?;
        let found = exports.sample_channels[*index];
        if found != channels {
            return Err(field_error(
                DiagnosticCode::PortType,
                file,
                object,
                sample_field,
                format!("zone sample has {found} channels but the node has {channels}"),
            ));
        }
        let pair = |name: &str| {
            record
                .get(name)
                .map(|pair_field| number_pair(pair_field, file, object))
                .transpose()
        };
        zones.push(SampleZone {
            sample: format!("sample_{index}"),
            low_key: key_value(&record["low"].value, file, object, &record["low"])?,
            high_key: key_value(&record["high"].value, file, object, &record["high"])?,
            velocity: pair("velocity")?,
            key_fade: pair("key_fade")?,
            velocity_fade: pair("velocity_fade")?,
        });
    }
    crate::sample_instrument::validate_zones(&zones, channels, "zones", |_| Some(channels))
        .map_err(|error| plan_diagnostics(error, file, object))?;
    Ok(zones)
}

/// A `[low, high]` list of two exact dimensionless numbers.
fn number_pair(
    field: &Field,
    file: &str,
    object: &Object,
) -> Result<crate::sample_instrument::SamplePair, Diagnostics> {
    match &field.value.kind {
        ValueKind::List(items) => match items.as_slice() {
            [Value {
                kind: ValueKind::Number(low),
                ..
            }, Value {
                kind: ValueKind::Number(high),
                ..
            }] => Ok(crate::sample_instrument::SamplePair {
                low: low.clone(),
                high: high.clone(),
            }),
            _ => Err(field_error(
                DiagnosticCode::Unit,
                file,
                object,
                field,
                format!("{} requires two dimensionless numbers", field.name),
            )),
        },
        _ => Err(field_error(
            DiagnosticCode::Unit,
            file,
            object,
            field,
            format!("{} requires [low, high]", field.name),
        )),
    }
}

fn lower_instrument(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    id: String,
    exports: &ResourceExports,
) -> Result<InstrumentProgram, Diagnostics> {
    exact_fields(object, &["channels"], &["channels"], file)?;
    let declared_channels = integer_field(object, "channels", file, 1, 2)? as u8;
    let voices = object
        .children
        .values()
        .filter(|child| child.kind == "voice")
        .collect::<Vec<_>>();
    let shared = object
        .children
        .values()
        .filter(|child| child.kind == "shared")
        .collect::<Vec<_>>();
    if voices.len() != 1 || shared.len() > 1 {
        return Err(object_error(
            DiagnosticCode::Conflict,
            file,
            object,
            "instrument requires exactly one voice graph and at most one shared graph",
        ));
    }
    for child in object.children.values() {
        if !matches!(child.kind.as_str(), "voice" | "shared" | "control") {
            return Err(object_error(
                DiagnosticCode::UnknownKind,
                file,
                child,
                format!("unexpected instrument child `{}`", child.kind),
            ));
        }
    }
    let voice = lower_graph(bundle, file, voices[0], GraphStage::Voice, exports)?;
    let shared_graph = shared
        .first()
        .map(|graph| lower_graph(bundle, file, graph, GraphStage::Shared, exports))
        .transpose()?;
    let mut graph_names = BTreeMap::from([(voices[0].id.as_str(), GraphStage::Voice)]);
    if let Some(graph) = shared.first() {
        graph_names.insert(graph.id.as_str(), GraphStage::Shared);
    }
    let mut controls = BTreeMap::new();
    for control in object
        .children
        .values()
        .filter(|child| child.kind == "control")
    {
        controls.insert(
            control.id.clone(),
            lower_control(control, file, &graph_names, &voice, shared_graph.as_ref())?,
        );
    }
    let program = InstrumentProgram {
        id,
        voice,
        shared: shared_graph,
        controls,
        source: ProgramSource {
            file: file.to_owned(),
            object: object.id.clone(),
            span: Some(SourceSpan {
                start: object.span.start,
                end: object.span.end,
            }),
        },
    };
    if program.channels() != declared_channels {
        return Err(object_error(
            DiagnosticCode::PortType,
            file,
            object,
            "instrument channels do not match its output graph",
        ));
    }
    program
        .validate()
        .map_err(|error| plan_diagnostics(error, file, object))?;
    Ok(program)
}

/// Lower an effect declaration: one `shared` graph that processes the audio
/// at the node's `in`, and `control` children that expose its parameters.
fn lower_effect(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    id: String,
    exports: &ResourceExports,
) -> Result<EffectProgram, Diagnostics> {
    exact_fields(object, &["channels", "input"], &["channels"], file)?;
    let declared_channels = integer_field(object, "channels", file, 1, 2)? as u8;
    let input_channels = match object.field("input") {
        Some(_) => integer_field(object, "input", file, 1, 2)? as u8,
        None => declared_channels,
    };
    for child in object.children.values() {
        if !matches!(child.kind.as_str(), "shared" | "control") {
            return Err(object_error(
                DiagnosticCode::UnknownKind,
                file,
                child,
                format!("unexpected effect child `{}`", child.kind),
            ));
        }
    }
    let graphs = object
        .children
        .values()
        .filter(|child| child.kind == "shared")
        .collect::<Vec<_>>();
    if graphs.len() != 1 {
        return Err(object_error(
            DiagnosticCode::Conflict,
            file,
            object,
            "effect requires exactly one shared graph",
        ));
    }
    let graph = lower_graph(bundle, file, graphs[0], GraphStage::Shared, exports)?;
    let graph_names = BTreeMap::from([(graphs[0].id.as_str(), GraphStage::Shared)]);
    let mut controls = BTreeMap::new();
    for control in object
        .children
        .values()
        .filter(|child| child.kind == "control")
    {
        // An effect has no voice graph; its one graph stands in for it, and
        // the graph names map only to the shared stage.
        controls.insert(
            control.id.clone(),
            lower_control(control, file, &graph_names, &graph, Some(&graph))?,
        );
    }
    let program = EffectProgram {
        id,
        input_channels,
        graph,
        controls,
        source: ProgramSource {
            file: file.to_owned(),
            object: object.id.clone(),
            span: Some(SourceSpan {
                start: object.span.start,
                end: object.span.end,
            }),
        },
    };
    if program.channels() != declared_channels {
        return Err(object_error(
            DiagnosticCode::PortType,
            file,
            object,
            "effect channels do not match its graph's output",
        ));
    }
    program
        .validate()
        .map_err(|error| plan_diagnostics(error, file, object))?;
    Ok(program)
}

/// Lower a kit declaration: `piece` children that play other instruments by
/// hit key, and `control` children that expose piece controls.
fn lower_kit(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    id: String,
    program_exports: &BTreeMap<ExportKey, usize>,
    kit_exports: &BTreeMap<ExportKey, usize>,
    programs: &[InstrumentProgram],
) -> Result<KitProgram, Diagnostics> {
    exact_fields(object, &["channels"], &["channels"], file)?;
    let channels = integer_field(object, "channels", file, 1, 2)? as u8;
    for child in object.children.values() {
        if !matches!(child.kind.as_str(), "piece" | "control") {
            return Err(object_error(
                DiagnosticCode::UnknownKind,
                file,
                child,
                format!(
                    "unexpected kit child `{}`; a kit has pieces and controls, not graphs",
                    child.kind
                ),
            ));
        }
    }
    let mut pieces = Vec::new();
    for piece in object
        .children
        .values()
        .filter(|child| child.kind == "piece")
    {
        exact_fields(
            piece,
            &[
                "instrument",
                "preset",
                "params",
                "key",
                "gate",
                "pitch",
                "voices",
                "choke",
            ],
            &["instrument", "key", "gate"],
            file,
        )?;
        no_children(piece, file)?;
        let instrument_field = piece.field("instrument").expect("required field checked");
        let instrument =
            resolve_export_key(bundle, file, reference_field(piece, "instrument", file)?)?;
        if kit_exports.contains_key(&instrument) {
            return Err(field_error(
                DiagnosticCode::Reference,
                file,
                piece,
                instrument_field,
                "a kit piece plays an instrument, not another kit",
            ));
        }
        let program = &programs[*program_exports.get(&instrument).ok_or_else(|| {
            field_error(
                DiagnosticCode::Reference,
                file,
                piece,
                instrument_field,
                "piece instrument reference does not name an instrument export",
            )
        })?];
        let mut params = BTreeMap::new();
        if let Some(field) = piece.field("preset") {
            let preset_key =
                resolve_export_key(bundle, file, expect_reference(field, file, piece)?)?;
            let preset_object = bundle
                .documents
                .get(&preset_key.0)
                .and_then(|document| document.objects.get(&preset_key.1))
                .filter(|object| object.kind == "preset")
                .ok_or_else(|| {
                    field_error(
                        DiagnosticCode::Reference,
                        file,
                        piece,
                        field,
                        "preset reference does not name a preset export",
                    )
                })?;
            let preset = lower_preset(
                bundle,
                &preset_key.0,
                preset_object,
                program_exports,
                programs,
                kit_exports,
                &[],
                &BTreeMap::new(),
                &[],
            )?;
            if preset.instrument != instrument {
                return Err(field_error(
                    DiagnosticCode::Reference,
                    file,
                    piece,
                    field,
                    "preset selects a different instrument",
                ));
            }
            params.extend(preset.params);
        }
        if let Some(field) = piece.field("params") {
            params.extend(lower_control_values(field, file, piece, &|name| {
                program.control_spec(name)
            })?);
        }
        let key_field = piece.field("key").expect("required field checked");
        let key = string_field(piece, "key", file)?;
        if key.is_empty() {
            return Err(field_error(
                DiagnosticCode::Range,
                file,
                piece,
                key_field,
                "a piece key must be nonempty",
            ));
        }
        let gate_field = piece.field("gate").expect("required field checked");
        let gate_seconds =
            source_number(&gate_field.value, GraphUnit::Seconds).map_err(|message| {
                field_error(DiagnosticCode::Unit, file, piece, gate_field, message)
            })?;
        let pitch_hz = match piece.field("pitch") {
            Some(field) => piece_pitch_hz(&field.value).map_err(|message| {
                field_error(DiagnosticCode::Unit, file, piece, field, message)
            })?,
            None => crate::music::Pitch::key(60)
                .frequency_hz()
                .expect("C4 is a valid pitch"),
        };
        let voices = match piece.field("voices") {
            Some(field) => integer_value(&field.value, file, piece, field, 1, MAX_PIECE_VOICES)?,
            None => DEFAULT_PIECE_VOICES,
        };
        let choke = optional_string_field(piece, "choke", file)?;
        pieces.push(KitPiece {
            id: piece.id.clone(),
            key,
            program: program.id.clone(),
            voices,
            pitch_hz,
            gate_seconds,
            choke,
            params,
        });
    }
    if pieces.is_empty() {
        return Err(object_error(
            DiagnosticCode::Range,
            file,
            object,
            "a kit requires at least one piece",
        ));
    }
    let mut controls = BTreeMap::new();
    for control in object
        .children
        .values()
        .filter(|child| child.kind == "control")
    {
        exact_fields(
            control,
            &["default", "target"],
            &["default", "target"],
            file,
        )?;
        no_children(control, file)?;
        let target_field = control.field("target").expect("required field checked");
        let reference = expect_reference(target_field, file, control)?;
        if reference.port.is_some() || reference.path.len() != 3 || reference.path[1] != "params" {
            return Err(field_error(
                DiagnosticCode::Reference,
                file,
                control,
                target_field,
                "kit control target must be &piece.params.control",
            ));
        }
        let piece = pieces
            .iter()
            .find(|piece| piece.id == reference.path[0])
            .ok_or_else(|| {
                field_error(
                    DiagnosticCode::Reference,
                    file,
                    control,
                    target_field,
                    "kit control target piece does not exist",
                )
            })?;
        let spec = programs
            .iter()
            .find(|program| program.id == piece.program)
            .and_then(|program| program.control_spec(&reference.path[2]))
            .ok_or_else(|| {
                field_error(
                    DiagnosticCode::Reference,
                    file,
                    control,
                    target_field,
                    "kit control target is not a public control of the piece instrument",
                )
            })?;
        let default_field = control.field("default").expect("required field checked");
        let default = source_number(&default_field.value, spec.unit).map_err(|message| {
            field_error(DiagnosticCode::Unit, file, control, default_field, message)
        })?;
        validate_against_spec(&default, &spec, file, control, default_field)?;
        controls.insert(
            control.id.clone(),
            KitControl {
                piece: reference.path[0].clone(),
                control: reference.path[2].clone(),
                default,
            },
        );
    }
    let kit = KitProgram {
        id,
        channels,
        pieces,
        controls,
        source: ProgramSource {
            file: file.to_owned(),
            object: object.id.clone(),
            span: Some(SourceSpan {
                start: object.span.start,
                end: object.span.end,
            }),
        },
    };
    kit.validate(programs)
        .map_err(|error| plan_diagnostics(error, file, object))?;
    Ok(kit)
}

/// A kit piece's pitch: a spelled pitch, `key(k)`, or a frequency.
fn piece_pitch_hz(value: &Value) -> Result<f64, &'static str> {
    let pitch = match &value.kind {
        ValueKind::Symbol(text) | ValueKind::String(text) => {
            crate::music::Pitch::parse_spelled(text).map_err(|_| "invalid spelled pitch")?
        }
        ValueKind::Call { function, args } if function == "key" && args.len() == 1 => {
            match &args[0].kind {
                ValueKind::Number(key) if key.is_integer() => crate::music::Pitch::key(
                    key.to_integer().to_i64().ok_or("key() is out of range")?,
                ),
                _ => return Err("key() requires one integer"),
            }
        }
        ValueKind::Quantity { .. } => {
            crate::music::Pitch::hz(source_number(value, GraphUnit::Hertz)?)
                .map_err(|_| "piece frequency must be positive")?
        }
        _ => return Err("piece pitch must be a spelled pitch, key(), or a frequency"),
    };
    pitch
        .frequency_hz()
        .map_err(|_| "piece pitch is outside the representable range")
}

fn lower_graph(
    bundle: &ResolvedBundle,
    file: &str,
    graph: &Object,
    stage: GraphStage,
    exports: &ResourceExports,
) -> Result<GraphProgram, Diagnostics> {
    let allowed = if stage == GraphStage::Voice {
        &["amplitude", "channels", "connected", "output"][..]
    } else {
        &["channels", "output"][..]
    };
    exact_fields(graph, allowed, &["channels", "output"], file)?;
    for child in graph.children.values() {
        if !matches!(child.kind.as_str(), "node" | "connect" | "modulate") {
            return Err(object_error(
                DiagnosticCode::UnknownKind,
                file,
                child,
                format!("unexpected graph child `{}`", child.kind),
            ));
        }
    }
    let channels = integer_field(graph, "channels", file, 1, 2)? as u8;
    let mut nodes = Vec::new();
    for node in graph.children.values().filter(|child| child.kind == "node") {
        nodes.push(lower_graph_node(bundle, file, node, stage, exports)?);
    }
    let mut connections = Vec::new();
    for edge in graph
        .children
        .values()
        .filter(|child| child.kind == "connect")
    {
        exact_fields(edge, &["from", "to"], &["from", "to"], file)?;
        no_children(edge, file)?;
        connections.push(
            Connection::new(
                edge.id.clone(),
                port_field(edge, "from", file)?,
                port_field(edge, "to", file)?,
            )
            .map_err(|error| plan_diagnostics(error, file, edge))?,
        );
    }
    let mut modulations = Vec::new();
    for edge in graph
        .children
        .values()
        .filter(|child| child.kind == "modulate")
    {
        exact_fields(
            edge,
            &["depth", "from", "to"],
            &["depth", "from", "to"],
            file,
        )?;
        no_children(edge, file)?;
        let to = parameter_target_field(edge, "to", file)?;
        let target_node = nodes
            .iter()
            .find(|node| node.id == to.node)
            .ok_or_else(|| {
                object_error(
                    DiagnosticCode::Reference,
                    file,
                    edge,
                    "modulation target node does not exist",
                )
            })?;
        let spec = parameter_descriptor_for_stage(&target_node.processor, &to.parameter, stage)
            .ok_or_else(|| {
                object_error(
                    DiagnosticCode::Reference,
                    file,
                    edge,
                    "modulation target parameter does not exist",
                )
            })?;
        let depth_field = edge.field("depth").expect("required field checked");
        let depth = source_number(&depth_field.value, spec.unit).map_err(|message| {
            field_error(DiagnosticCode::Unit, file, edge, depth_field, message)
        })?;
        modulations.push(Modulation {
            id: edge.id.clone(),
            from: port_field(edge, "from", file)?,
            to,
            depth,
        });
    }
    let output = port_field(graph, "output", file)?;
    let amplitude = match graph.field("amplitude") {
        None => None,
        Some(field) => {
            let reference = expect_reference(field, file, graph)?;
            if reference.port.is_some() || reference.path.len() != 1 {
                return Err(field_error(
                    DiagnosticCode::Reference,
                    file,
                    graph,
                    field,
                    "amplitude must reference one local graph node",
                ));
            }
            Some(reference.path[0].clone())
        }
    };
    let connected = match graph
        .field("connected")
        .map(|field| (field, &field.value.kind))
    {
        None => false,
        Some((_, ValueKind::Boolean(connected))) => *connected,
        Some((field, _)) => {
            return Err(field_error(
                DiagnosticCode::Unit,
                file,
                graph,
                field,
                "connected must be a boolean",
            ))
        }
    };
    Ok(GraphProgram {
        channels,
        nodes,
        connections,
        modulations,
        output,
        amplitude,
        connected,
    })
}

fn lower_graph_node(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    stage: GraphStage,
    exports: &ResourceExports,
) -> Result<GraphNode, Diagnostics> {
    exact_fields(object, &["config", "params", "type"], &["type"], file)?;
    no_children(object, file)?;
    let identity = string_field(object, "type", file)?;
    let config = optional_record(object, "config", file)?;
    let unknown = || {
        object_error(
            DiagnosticCode::UnknownKind,
            file,
            object,
            format!("unknown graph processor `{identity}`"),
        )
    };
    // The declaration says what the config may hold; only building the
    // processor from it is specific to each.
    let declaration = declaration_of(&identity).ok_or_else(unknown)?;
    let config = NodeConfig::read(declaration, config, file, object)?;
    let processor = match identity.as_str() {
        "synth.sine/1" => GraphProcessor::Sine,
        "synth.saw/1" => GraphProcessor::Saw,
        "synth.square/1" => GraphProcessor::Square,
        "synth.triangle/1" => GraphProcessor::Triangle,
        "synth.noise/1" => GraphProcessor::Noise {
            seed: config.integer("seed"),
            per_note: config.boolean("per_note"),
        },
        "synth.pluck/1" => GraphProcessor::Pluck {
            seed: config.integer("seed"),
        },
        "synth.random/1" => GraphProcessor::Random {
            seed: config.integer("seed"),
        },
        "synth.modal/1" => GraphProcessor::Modal {
            modes: config.count("modes"),
            strings: config.count("strings"),
        },
        "synth.bow/1" => GraphProcessor::Bow {
            players: config.count("players"),
            body: config.symbol("body"),
            channels: config.count("channels"),
            seed: config.integer("seed"),
        },
        "synth.wavetable/1" => {
            let field = config.field("table");
            let key = resolve_export_key(bundle, file, expect_reference(field, file, object)?)?;
            let index = exports.tables.get(&key).ok_or_else(|| {
                field_error(
                    DiagnosticCode::Reference,
                    file,
                    object,
                    field,
                    "table reference does not name a wavetable export",
                )
            })?;
            GraphProcessor::Wavetable {
                table: format!("table_{index}"),
            }
        }
        "synth.sample/1" => {
            let channels = config.count("channels");
            GraphProcessor::Sample {
                channels,
                zones: lower_sample_zones(
                    bundle,
                    file,
                    object,
                    config.field("zones"),
                    channels,
                    exports,
                )?,
                fade_shape: config.symbol("fade_shape"),
            }
        }
        "synth.adsr/1" => GraphProcessor::Adsr {
            on_join: config.symbol("on_join"),
        },
        "synth.timbre/1" => GraphProcessor::Timbre,
        "synth.pressure/1" => GraphProcessor::Pressure,
        "synth.velocity/1" => GraphProcessor::Velocity,
        "synth.key/1" => GraphProcessor::Key,
        "synth.lfo/1" => GraphProcessor::Lfo,
        "synth.gain/1" => GraphProcessor::Gain {
            channels: config.count("channels"),
        },
        "synth.onepole/1" => GraphProcessor::OnePole {
            channels: config.count("channels"),
        },
        "synth.highpass/1" => GraphProcessor::HighPass {
            channels: config.count("channels"),
        },
        "synth.svf/1" => GraphProcessor::Svf {
            channels: config.count("channels"),
            mode: config.symbol("mode"),
        },
        "synth.drive/1" => GraphProcessor::Drive {
            channels: config.count("channels"),
        },
        "synth.mix/1" => GraphProcessor::Mix {
            channels: config.count("channels"),
        },
        "synth.pan/1" => GraphProcessor::Pan,
        "synth.balance/1" => GraphProcessor::Balance,
        "synth.width/1" => GraphProcessor::Width,
        "synth.delay/1" => GraphProcessor::Delay {
            channels: config.count("channels"),
            clock: config.symbol("clock"),
            max: config.seconds("max"),
        },
        _ => return Err(unknown()),
    };
    let mut params = BTreeMap::new();
    if let Some(field) = object.field("params") {
        for (name, value_field) in expect_record(field, file, object)? {
            let spec =
                parameter_descriptor_for_stage(&processor, name, stage).ok_or_else(|| {
                    field_error(
                        DiagnosticCode::UnknownField,
                        file,
                        object,
                        value_field,
                        format!("processor `{identity}` has no parameter `{name}`"),
                    )
                })?;
            let value = source_number(&value_field.value, spec.unit).map_err(|message| {
                field_error(DiagnosticCode::Unit, file, object, value_field, message)
            })?;
            validate_against_spec(&value, &spec, file, object, value_field)?;
            params.insert(name.clone(), value);
        }
    }
    Ok(GraphNode {
        id: object.id.clone(),
        processor,
        params,
    })
}

fn lower_control(
    object: &Object,
    file: &str,
    graph_names: &BTreeMap<&str, GraphStage>,
    voice: &GraphProgram,
    shared: Option<&GraphProgram>,
) -> Result<Control, Diagnostics> {
    exact_fields(object, &["default", "target"], &["default", "target"], file)?;
    no_children(object, file)?;
    let target_field = object.field("target").expect("required field checked");
    let reference = expect_reference(target_field, file, object)?;
    if reference.port.is_some() || reference.path.len() != 4 || reference.path[2] != "params" {
        return Err(field_error(
            DiagnosticCode::Reference,
            file,
            object,
            target_field,
            "control target must be &graph.node.params.parameter",
        ));
    }
    let stage = graph_names
        .get(reference.path[0].as_str())
        .copied()
        .ok_or_else(|| {
            field_error(
                DiagnosticCode::Reference,
                file,
                object,
                target_field,
                "control target graph does not exist",
            )
        })?;
    let graph = match stage {
        GraphStage::Voice => voice,
        GraphStage::Shared => shared.expect("shared graph name exists"),
    };
    let node = graph
        .nodes
        .iter()
        .find(|node| node.id == reference.path[1])
        .ok_or_else(|| {
            field_error(
                DiagnosticCode::Reference,
                file,
                object,
                target_field,
                "control target node does not exist",
            )
        })?;
    let spec = parameter_descriptor_for_stage(&node.processor, &reference.path[3], stage)
        .ok_or_else(|| {
            field_error(
                DiagnosticCode::Reference,
                file,
                object,
                target_field,
                "control target parameter does not exist",
            )
        })?;
    let default_field = object.field("default").expect("required field checked");
    let default = source_number(&default_field.value, spec.unit).map_err(|message| {
        field_error(DiagnosticCode::Unit, file, object, default_field, message)
    })?;
    validate_against_spec(&default, &spec, file, object, default_field)?;
    Ok(Control {
        target: ControlTarget {
            graph: stage,
            node: reference.path[1].clone(),
            parameter: reference.path[3].clone(),
        },
        default,
    })
}

#[allow(clippy::too_many_arguments)]
fn lower_preset(
    bundle: &ResolvedBundle,
    file: &str,
    object: &Object,
    program_exports: &BTreeMap<ExportKey, usize>,
    programs: &[InstrumentProgram],
    kit_exports: &BTreeMap<ExportKey, usize>,
    kits: &[KitProgram],
    effect_exports: &BTreeMap<ExportKey, usize>,
    effects: &[EffectProgram],
) -> Result<Preset, Diagnostics> {
    if object.field("effect").is_some() {
        exact_fields(object, &["effect", "params"], &["effect", "params"], file)?;
        no_children(object, file)?;
        let effect = resolve_export_key(bundle, file, reference_field(object, "effect", file)?)?;
        let index = *effect_exports.get(&effect).ok_or_else(|| {
            object_error(
                DiagnosticCode::Reference,
                file,
                object,
                "preset effect reference does not name an effect export",
            )
        })?;
        let field = object.field("params").expect("required field checked");
        let params = lower_control_values(field, file, object, &|name| {
            effects[index].control_spec(name)
        })?;
        return Ok(Preset {
            instrument: effect,
            params,
        });
    }
    exact_fields(
        object,
        &["instrument", "params"],
        &["instrument", "params"],
        file,
    )?;
    no_children(object, file)?;
    let instrument =
        resolve_export_key(bundle, file, reference_field(object, "instrument", file)?)?;
    let field = object.field("params").expect("required field checked");
    let params = if let Some(&index) = kit_exports.get(&instrument) {
        // Pieces resolve their presets before any kit is lowered.
        let kit = kits.get(index).ok_or_else(|| {
            object_error(
                DiagnosticCode::Reference,
                file,
                object,
                "a kit piece preset must select an instrument, not a kit",
            )
        })?;
        lower_control_values(field, file, object, &|name| {
            kit.control_spec(programs, name)
        })?
    } else {
        let index = *program_exports.get(&instrument).ok_or_else(|| {
            object_error(
                DiagnosticCode::Reference,
                file,
                object,
                "preset instrument reference does not name an instrument export",
            )
        })?;
        lower_control_values(field, file, object, &|name| {
            programs[index].control_spec(name)
        })?
    };
    Ok(Preset { instrument, params })
}

fn lower_control_values(
    field: &Field,
    file: &str,
    object: &Object,
    control_spec: &dyn Fn(&str) -> Option<ParameterSpec>,
) -> Result<BTreeMap<String, Rational>, Diagnostics> {
    let mut result = BTreeMap::new();
    for (name, value_field) in expect_record(field, file, object)? {
        let spec = control_spec(name).ok_or_else(|| {
            field_error(
                DiagnosticCode::UnknownField,
                file,
                object,
                value_field,
                format!("instrument has no public control `{name}`"),
            )
        })?;
        let value = source_number(&value_field.value, spec.unit).map_err(|message| {
            field_error(DiagnosticCode::Unit, file, object, value_field, message)
        })?;
        validate_against_spec(&value, &spec, file, object, value_field)?;
        result.insert(name.clone(), value);
    }
    Ok(result)
}

fn lower_instance_config(field: &Field, file: &str, object: &Object) -> Result<u32, Diagnostics> {
    let record = expect_record(field, file, object)?;
    let names = record.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if names != BTreeSet::from(["voices"]) {
        return Err(field_error(
            DiagnosticCode::UnknownField,
            file,
            object,
            field,
            "instance config must contain exactly `voices`",
        ));
    }
    integer_value(
        &record["voices"].value,
        file,
        object,
        &record["voices"],
        1,
        MAX_INSTANCE_VOICES,
    )
}

fn resolve_export_key(
    bundle: &ResolvedBundle,
    file: &str,
    reference: &Reference,
) -> Result<ExportKey, Diagnostics> {
    resolve_export_key_from_maps(file, reference, bundle.imports.get(file))
}

fn resolve_export_key_from_maps(
    file: &str,
    reference: &Reference,
    imports: Option<&BTreeMap<String, String>>,
) -> Result<ExportKey, Diagnostics> {
    if reference.port.is_some() {
        return Err(simple_error(
            DiagnosticCode::Reference,
            "export references cannot contain ports",
        ));
    }
    match reference.path.as_slice() {
        [name] => Ok((file.to_owned(), name.clone())),
        [alias, name] => imports
            .and_then(|map| map.get(alias))
            .cloned()
            .map(|target| (target, name.clone()))
            .ok_or_else(|| {
                simple_error(
                    DiagnosticCode::Reference,
                    format!("unknown direct import alias `{alias}` in `{file}`"),
                )
            }),
        _ => Err(simple_error(
            DiagnosticCode::Reference,
            "exports must be referenced locally or through one direct import alias",
        )),
    }
}

fn source_number(value: &Value, expected: GraphUnit) -> Result<Rational, &'static str> {
    match (expected, &value.kind) {
        (GraphUnit::Dimensionless, ValueKind::Number(number)) => Ok(number.clone()),
        (
            GraphUnit::Seconds,
            ValueKind::Quantity {
                value,
                unit: Unit::S,
            },
        ) => Ok(value.clone()),
        (
            GraphUnit::Seconds,
            ValueKind::Quantity {
                value,
                unit: Unit::Ms,
            },
        ) => Ok(value / Rational::from_integer(BigInt::from(1_000))),
        (
            GraphUnit::Hertz,
            ValueKind::Quantity {
                value,
                unit: Unit::Hz,
            },
        ) => Ok(value.clone()),
        (
            GraphUnit::Hertz,
            ValueKind::Quantity {
                value,
                unit: Unit::KHz,
            },
        ) => Ok(value * Rational::from_integer(BigInt::from(1_000))),
        (
            GraphUnit::Quarter,
            ValueKind::Quantity {
                value,
                unit: Unit::Q,
            },
        ) => Ok(value.clone()),
        (GraphUnit::Dimensionless, _) => Err("parameter requires a dimensionless number"),
        (GraphUnit::Quarter, _) => Err("parameter requires quarter notes using q"),
        (GraphUnit::Seconds, _) => Err("parameter requires seconds using s or ms"),
        (GraphUnit::Hertz, _) => Err("parameter requires frequency using Hz or kHz"),
    }
}

fn validate_against_spec(
    value: &Rational,
    spec: &ParameterSpec,
    file: &str,
    object: &Object,
    field: &Field,
) -> Result<(), Diagnostics> {
    let below = if spec.min_open {
        value <= &spec.min
    } else {
        value < &spec.min
    };
    let above = if spec.max_open {
        value >= &spec.max
    } else {
        value > &spec.max
    };
    if below || above {
        Err(field_error(
            DiagnosticCode::Range,
            file,
            object,
            field,
            "parameter value is outside its allowed range",
        ))
    } else {
        Ok(())
    }
}

/// What a node's config holds at one key.
enum Held<'a> {
    Integer(u32),
    Boolean(bool),
    Symbol(&'static str),
    Seconds(Rational),
    /// A key whose value the processor lowers itself.
    Field(&'a Field),
}

/// A node's config as its declaration reads it. Every key the source wrote
/// is one the declaration has, with a value of the kind and in the range
/// declared, and a key the source left out holds its default.
struct NodeConfig<'a> {
    identity: &'static str,
    held: BTreeMap<&'static str, Held<'a>>,
}

impl<'a> NodeConfig<'a> {
    fn read(
        declaration: &'static Declaration,
        config: Option<&'a BTreeMap<String, Field>>,
        file: &str,
        object: &Object,
    ) -> Result<Self, Diagnostics> {
        let identity = declaration.identity;
        let keys = declaration.config_keys();
        if keys.is_empty() && config.is_some() {
            return Err(object_error(
                DiagnosticCode::UnknownField,
                file,
                object,
                format!("processor `{identity}` does not accept config"),
            ));
        }
        if config.is_none() && keys.iter().any(|key| key.default.is_none()) {
            return Err(object_error(
                DiagnosticCode::UnknownField,
                file,
                object,
                format!("processor `{identity}` requires config"),
            ));
        }
        if let Some((_, field)) = config
            .into_iter()
            .flatten()
            .find(|(name, _)| declaration.config_key(name).is_none())
        {
            let names = keys.iter().map(|key| key.key).collect::<Vec<_>>();
            return Err(field_error(
                DiagnosticCode::UnknownField,
                file,
                object,
                field,
                match names.as_slice() {
                    [name] => format!("processor `{identity}` has one config field, {name}"),
                    _ => format!(
                        "processor `{identity}` config fields are {}",
                        listed(&names, "and")
                    ),
                },
            ));
        }
        let mut held = BTreeMap::new();
        for key in keys {
            let name = key.key;
            let value = match (config.and_then(|config| config.get(name)), key.default) {
                (None, None) => {
                    return Err(object_error(
                        DiagnosticCode::UnknownField,
                        file,
                        object,
                        format!("processor `{identity}` requires config.{name}"),
                    ))
                }
                (None, Some(ConfigValue::Number(value))) => Held::Integer(
                    u32::try_from(value).expect("a declared default is in its key's range"),
                ),
                (None, Some(ConfigValue::Boolean(value))) => Held::Boolean(value),
                (None, Some(ConfigValue::Symbol(value))) => Held::Symbol(value),
                (Some(field), _) => match key.holds {
                    Holds::Integer { min, max } => {
                        Held::Integer(integer_value(&field.value, file, object, field, min, max)?)
                    }
                    Holds::Boolean => match &field.value.kind {
                        ValueKind::Boolean(value) => Held::Boolean(*value),
                        _ => {
                            return Err(field_error(
                                DiagnosticCode::Unit,
                                file,
                                object,
                                field,
                                format!("{name} must be a boolean"),
                            ))
                        }
                    },
                    Holds::Symbol(symbols) => Held::Symbol(
                        field
                            .value
                            .as_symbol()
                            .and_then(|given| symbols.iter().find(|symbol| **symbol == given))
                            .ok_or_else(|| {
                                field_error(
                                    DiagnosticCode::Range,
                                    file,
                                    object,
                                    field,
                                    format!("{name} must be {}", listed(symbols, "or")),
                                )
                            })?,
                    ),
                    Holds::Seconds => Held::Seconds(
                        source_number(&field.value, GraphUnit::Seconds).map_err(|message| {
                            field_error(DiagnosticCode::Unit, file, object, field, message)
                        })?,
                    ),
                    Holds::Table | Holds::Zones => Held::Field(field),
                },
            };
            held.insert(name, value);
        }
        Ok(Self { identity, held })
    }

    fn undeclared(&self, kind: &str, key: &str) -> ! {
        unreachable!("{} declares no {kind} config `{key}`", self.identity)
    }

    fn integer(&self, key: &str) -> u32 {
        match self.held.get(key) {
            Some(Held::Integer(value)) => *value,
            _ => self.undeclared("whole number", key),
        }
    }

    /// A whole number whose declared range fits a byte.
    fn count(&self, key: &str) -> u8 {
        u8::try_from(self.integer(key))
            .unwrap_or_else(|_| self.undeclared("whole number below 256", key))
    }

    fn boolean(&self, key: &str) -> bool {
        match self.held.get(key) {
            Some(Held::Boolean(value)) => *value,
            _ => self.undeclared("boolean", key),
        }
    }

    /// The value of a symbol key, by the name its plan form has.
    fn symbol<T: serde::de::DeserializeOwned>(&self, key: &str) -> T {
        use serde::de::value::{Error, StrDeserializer};
        match self.held.get(key) {
            Some(Held::Symbol(symbol)) => T::deserialize(StrDeserializer::<Error>::new(symbol))
                .unwrap_or_else(|_| self.undeclared(symbol, key)),
            _ => self.undeclared("symbol", key),
        }
    }

    fn seconds(&self, key: &str) -> Rational {
        match self.held.get(key) {
            Some(Held::Seconds(value)) => value.clone(),
            _ => self.undeclared("length", key),
        }
    }

    fn field(&self, key: &str) -> &'a Field {
        match self.held.get(key) {
            Some(Held::Field(field)) => field,
            _ => self.undeclared("own", key),
        }
    }
}

/// `a`, `a or b`, `a, b or c`.
fn listed(names: &[&str], last: &str) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_string(),
        [rest @ .., end] => format!("{} {last} {end}", rest.join(", ")),
    }
}

fn exact_fields(
    object: &Object,
    allowed: &[&str],
    required: &[&str],
    file: &str,
) -> Result<(), Diagnostics> {
    // Every object may carry the optional string `label` (§4).
    optional_string_field(object, "label", file)?;
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    if let Some((name, field)) = object
        .fields
        .iter()
        .find(|(name, _)| *name != "label" && !allowed.contains(name.as_str()))
    {
        return Err(field_error(
            DiagnosticCode::UnknownField,
            file,
            object,
            field,
            format!("unknown field `{name}` on {}", object.kind),
        ));
    }
    if let Some(name) = required
        .iter()
        .find(|name| !object.fields.contains_key(**name))
    {
        return Err(object_error(
            DiagnosticCode::UnknownField,
            file,
            object,
            format!("{} requires field `{name}`", object.kind),
        ));
    }
    Ok(())
}

fn no_children(object: &Object, file: &str) -> Result<(), Diagnostics> {
    if let Some(child) = object.children.values().next() {
        Err(object_error(
            DiagnosticCode::UnknownKind,
            file,
            child,
            format!("{} cannot contain child objects", object.kind),
        ))
    } else {
        Ok(())
    }
}

fn require_kind(object: &Object, expected: &str, file: &str) -> Result<(), Diagnostics> {
    if object.kind == expected {
        Ok(())
    } else {
        Err(object_error(
            DiagnosticCode::UnknownKind,
            file,
            object,
            format!("expected `{expected}`, found `{}`", object.kind),
        ))
    }
}

fn optional_record<'a>(
    object: &'a Object,
    name: &str,
    file: &str,
) -> Result<Option<&'a BTreeMap<String, Field>>, Diagnostics> {
    object
        .field(name)
        .map(|field| expect_record(field, file, object))
        .transpose()
}

fn expect_record<'a>(
    field: &'a Field,
    file: &str,
    object: &Object,
) -> Result<&'a BTreeMap<String, Field>, Diagnostics> {
    match &field.value.kind {
        ValueKind::Record(record) => Ok(record),
        _ => Err(field_error(
            DiagnosticCode::Unit,
            file,
            object,
            field,
            format!("{} requires a record", field.name),
        )),
    }
}

fn reference_field<'a>(
    object: &'a Object,
    name: &str,
    file: &str,
) -> Result<&'a Reference, Diagnostics> {
    expect_reference(
        object.field(name).expect("required field checked"),
        file,
        object,
    )
}

fn expect_reference<'a>(
    field: &'a Field,
    file: &str,
    object: &Object,
) -> Result<&'a Reference, Diagnostics> {
    field.value.reference().ok_or_else(|| {
        field_error(
            DiagnosticCode::Reference,
            file,
            object,
            field,
            format!("{} requires a reference", field.name),
        )
    })
}

fn port_field(object: &Object, name: &str, file: &str) -> Result<PortRef, Diagnostics> {
    let field = object.field(name).expect("required field checked");
    let reference = expect_reference(field, file, object)?;
    if reference.path.len() != 1 || reference.port.is_none() {
        return Err(field_error(
            DiagnosticCode::Reference,
            file,
            object,
            field,
            format!("{name} must be a local node port reference"),
        ));
    }
    PortRef::new(
        reference.path[0].clone(),
        reference.port.clone().expect("port checked"),
    )
    .map_err(|error| plan_diagnostics(error, file, object))
}

fn parameter_target_field(
    object: &Object,
    name: &str,
    file: &str,
) -> Result<ParameterTarget, Diagnostics> {
    let field = object.field(name).expect("required field checked");
    let reference = expect_reference(field, file, object)?;
    if reference.port.is_some() || reference.path.len() != 3 || reference.path[1] != "params" {
        return Err(field_error(
            DiagnosticCode::Reference,
            file,
            object,
            field,
            format!("{name} must be &node.params.parameter"),
        ));
    }
    Ok(ParameterTarget {
        node: reference.path[0].clone(),
        parameter: reference.path[2].clone(),
    })
}

fn string_field(object: &Object, name: &str, file: &str) -> Result<String, Diagnostics> {
    let field = object.field(name).expect("required field checked");
    field.value.as_string().map(str::to_owned).ok_or_else(|| {
        field_error(
            DiagnosticCode::Unit,
            file,
            object,
            field,
            format!("{name} requires a string"),
        )
    })
}

fn optional_string_field(
    object: &Object,
    name: &str,
    file: &str,
) -> Result<Option<String>, Diagnostics> {
    object
        .field(name)
        .map(|field| {
            field.value.as_string().map(str::to_owned).ok_or_else(|| {
                field_error(
                    DiagnosticCode::Unit,
                    file,
                    object,
                    field,
                    format!("{name} requires a string"),
                )
            })
        })
        .transpose()
}

fn integer_field(
    object: &Object,
    name: &str,
    file: &str,
    min: u32,
    max: u32,
) -> Result<u32, Diagnostics> {
    let field = object.field(name).expect("required field checked");
    integer_value(&field.value, file, object, field, min, max)
}

fn integer_value(
    value: &Value,
    file: &str,
    object: &Object,
    field: &Field,
    min: u32,
    max: u32,
) -> Result<u32, Diagnostics> {
    let ValueKind::Number(number) = &value.kind else {
        return Err(field_error(
            DiagnosticCode::Unit,
            file,
            object,
            field,
            format!("{} requires a dimensionless integer", field.name),
        ));
    };
    if number.denom() != &BigInt::one() {
        return Err(field_error(
            DiagnosticCode::Range,
            file,
            object,
            field,
            format!("{} must be an integer", field.name),
        ));
    }
    let Some(number) = number.numer().to_u32() else {
        return Err(field_error(
            DiagnosticCode::Range,
            file,
            object,
            field,
            format!("{} is outside {min} through {max}", field.name),
        ));
    };
    if !(min..=max).contains(&number) {
        return Err(field_error(
            DiagnosticCode::Range,
            file,
            object,
            field,
            format!("{} is outside {min} through {max}", field.name),
        ));
    }
    Ok(number)
}

fn plan_diagnostics(error: PlanError, file: &str, object: &Object) -> Diagnostics {
    let mut diagnostic = error.diagnostic();
    diagnostic.message = format!("source `{file}`: {}", diagnostic.message);
    if diagnostic.span.is_none() {
        diagnostic.span = Some(object.span);
    }
    if diagnostic.object_path.is_empty() {
        diagnostic.object_path = vec![object.id.clone()];
    }
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(diagnostic);
    diagnostics
}

fn object_error(
    code: DiagnosticCode,
    file: &str,
    object: &Object,
    message: impl Into<String>,
) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(
        Diagnostic::error(
            code,
            format!("source `{file}`: {}", message.into()),
            Some(object.span),
        )
        .object_path([object.id.clone()]),
    );
    diagnostics
}

fn field_error(
    code: DiagnosticCode,
    file: &str,
    object: &Object,
    field: &Field,
    message: impl Into<String>,
) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(
        Diagnostic::error(
            code,
            format!("source `{file}`: {}", message.into()),
            Some(field.value.span),
        )
        .object_path([object.id.clone()])
        .field_path([field.name.clone()]),
    );
    diagnostics
}

fn simple_error(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None::<Span>));
    diagnostics
}
