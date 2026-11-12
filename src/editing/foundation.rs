//! Production semantic context for the currently implemented core editing slice.
//!
//! This context deliberately covers the source-only foundation that the Rust
//! implementation can validate without executing assets or a renderer. Unknown
//! required extensions, local library/import syntax, native production nodes,
//! and native message performance remain explicit E_CAPABILITY boundaries.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{ToPrimitive, Zero};
use serde_json::{Map, Value};

use crate::diagnostic::Diagnostics;
use crate::semantic::{origin, supplier, Models, Source, Supply, Unplayable};
use crate::syntax::Document;

use crate::compiler::LOCAL_SOURCE;
use crate::normalization::NormalizationContext;

use super::{
    AuthoredDocument, EditContext, EditError, EditImpact, EditResult, Path, RenderInvalidationScope,
};

/// Semantic and normalization context for the implemented source-only core.
///
/// This is intentionally narrower than MaaC/1 Document conformance. It accepts
/// the core source shapes handled by the foundation semantic validator,
/// including core control, kit, audio and warp objects, but refuses capabilities
/// whose full normalization contract is not implemented here.
#[derive(Clone, Copy, Debug, Default)]
pub struct FoundationEditContext;

#[derive(Clone, Debug)]
pub struct BundleEditContext {
    bundle: crate::bundle::SourceBundle,
    source_path: String,
    disk_assets:
        Option<std::collections::BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>>>,
    limits: crate::plan::PlanLimits,
    import_edit: Option<ImportEditMode>,
}

#[derive(Clone, Debug)]
enum ImportEditMode {
    Library {
        importer: String,
        alias: String,
    },
    Importer {
        library: String,
        alternate: String,
        alias: String,
    },
    GroupImporter {
        alternates: BTreeMap<String, (String, String)>,
    },
    Graph {
        imports: BTreeMap<String, BTreeMap<String, String>>,
        overrides: BTreeMap<String, String>,
    },
}

impl BundleEditContext {
    /// Edit the bundle entry source and re-resolve its complete dependency graph
    /// for every candidate. Import path/hash edits therefore use the same pin,
    /// cycle, built-in, path, and resource checks as ordinary bundle loading.
    pub fn new(bundle: &crate::bundle::SourceBundle) -> EditResult<Self> {
        Self::for_source(bundle, &bundle.entry)
    }

    /// Edit one caller-owned source in a bundle. Embedded `@builtin/` sources
    /// are immutable and therefore cannot be selected as Protocol 2 targets.
    pub fn for_source(
        bundle: &crate::bundle::SourceBundle,
        source_path: impl Into<String>,
    ) -> EditResult<Self> {
        let source_path = source_path.into();
        if !bundle.sources.contains_key(&source_path) {
            return Err(EditError::new(
                "E_REFERENCE",
                "editable source is not present in the caller-owned bundle",
            ));
        }
        bundle.resolve().map_err(map_diagnostics)?;
        Ok(Self {
            bundle: bundle.clone(),
            source_path,
            disk_assets: None,
            limits: crate::plan::PlanLimits::default(),
            import_edit: None,
        })
    }

    /// Trusted archive editing context with the exact private native PCM
    /// snapshots used by the archived composition.
    pub(crate) fn new_disk_media(
        bundle: &crate::bundle::SourceBundle,
        assets: &std::collections::BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>>,
        limits: crate::plan::PlanLimits,
    ) -> EditResult<Self> {
        bundle
            .resolve_with_disk_assets(assets)
            .map_err(map_diagnostics)?;
        Ok(Self {
            bundle: bundle.clone(),
            source_path: bundle.entry.clone(),
            disk_assets: Some(assets.clone()),
            limits,
            import_edit: None,
        })
    }

    pub(crate) fn new_disk_media_import_library(
        bundle: &crate::bundle::SourceBundle,
        assets: &std::collections::BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>>,
        limits: crate::plan::PlanLimits,
        library: &str,
        importer: &str,
        alias: &str,
    ) -> EditResult<Self> {
        let mut context = Self::new_disk_media(bundle, assets, limits)?;
        if !bundle.sources.contains_key(library) || !bundle.sources.contains_key(importer) {
            return Err(EditError::new(
                "E_REFERENCE",
                "import edit sources are missing",
            ));
        }
        context.source_path = library.into();
        context.import_edit = Some(ImportEditMode::Library {
            importer: importer.into(),
            alias: alias.into(),
        });
        Ok(context)
    }

    pub(crate) fn new_disk_media_importer(
        bundle: &crate::bundle::SourceBundle,
        assets: &std::collections::BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>>,
        limits: crate::plan::PlanLimits,
        library: &str,
        alternate: &str,
        alias: &str,
    ) -> EditResult<Self> {
        let mut context = Self::new_disk_media(bundle, assets, limits)?;
        if !bundle.sources.contains_key(library) {
            return Err(EditError::new("E_REFERENCE", "import library is missing"));
        }
        context.import_edit = Some(ImportEditMode::Importer {
            library: library.into(),
            alternate: alternate.into(),
            alias: alias.into(),
        });
        Ok(context)
    }

    pub(crate) fn new_disk_media_group_importer(
        bundle: &crate::bundle::SourceBundle,
        assets: &std::collections::BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>>,
        limits: crate::plan::PlanLimits,
        alternates: BTreeMap<String, (String, String)>,
    ) -> EditResult<Self> {
        let mut context = Self::new_disk_media(bundle, assets, limits)?;
        if alternates.is_empty()
            || alternates
                .values()
                .any(|(library, _)| !bundle.sources.contains_key(library))
        {
            return Err(EditError::new(
                "E_REFERENCE",
                "group import libraries are missing",
            ));
        }
        context.import_edit = Some(ImportEditMode::GroupImporter { alternates });
        Ok(context)
    }

    /// Validate a source against the original graph with private candidate
    /// replacements. Every ancestor pin is repaired only in this semantic
    /// context; the caller still has to journal the exact pin edits it emits.
    pub(crate) fn new_disk_media_graph_source(
        bundle: &crate::bundle::SourceBundle,
        assets: &std::collections::BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>>,
        limits: crate::plan::PlanLimits,
        source: &str,
        overrides: BTreeMap<String, String>,
    ) -> EditResult<Self> {
        let resolved = bundle
            .resolve_with_disk_assets(assets)
            .map_err(map_diagnostics)?;
        if !bundle.sources.contains_key(source)
            || overrides
                .keys()
                .any(|path| !bundle.sources.contains_key(path))
        {
            return Err(EditError::new(
                "E_REFERENCE",
                "graph edit source is missing",
            ));
        }
        let imports = resolved
            .imports
            .into_iter()
            .map(|(path, aliases)| {
                let local = aliases
                    .into_iter()
                    .filter(|(_, target)| bundle.sources.contains_key(target))
                    .collect();
                (path, local)
            })
            .collect();
        Ok(Self {
            bundle: bundle.clone(),
            source_path: source.into(),
            disk_assets: Some(assets.clone()),
            limits,
            import_edit: Some(ImportEditMode::Graph { imports, overrides }),
        })
    }

    pub fn source_path(&self) -> &str {
        &self.source_path
    }

    /// Prepare an atomic Protocol 2 copy of one placement's complete pattern
    /// occurrences, including private imported curve and tuning dependencies.
    pub fn prepare_materialize_instance(
        &self,
        document: &AuthoredDocument,
        placement_id: &str,
        new_pattern_id: &str,
    ) -> EditResult<super::MaterializeInstancePlan> {
        self.validate_document(document.tree())?;
        let (_, libraries) = self.resolved_candidate(document.tree())?;
        super::materialize::prepare(
            self,
            document,
            &libraries.resolved_document().to_syntax_json_value(),
            libraries.pattern_source_paths(),
            placement_id,
            new_pattern_id,
        )
    }

    fn candidate_bundle(&self, authored: &Value) -> EditResult<crate::bundle::SourceBundle> {
        let mut bundle = self.bundle.clone();
        let source = super::source::authored_source(authored)?;
        bundle.sources.insert(self.source_path.clone(), source);
        match &self.import_edit {
            Some(ImportEditMode::Library { importer, alias }) => {
                let hash =
                    crate::bundle::sha256_digest(bundle.sources[&self.source_path].as_bytes());
                let current = bundle
                    .sources
                    .get(importer)
                    .ok_or_else(|| EditError::new("E_REFERENCE", "importer source is missing"))?;
                let updated = replace_import_hash(current, alias, &hash)?;
                bundle.sources.insert(importer.clone(), updated);
            }
            Some(ImportEditMode::Importer {
                library,
                alternate,
                alias,
            }) => {
                let pin = authored["objects"][alias]["fields"]["hash"]["v"].as_str();
                let alternate_hash = crate::bundle::sha256_digest(alternate.as_bytes());
                if pin == Some(alternate_hash.as_str()) {
                    bundle.sources.insert(library.clone(), alternate.clone());
                }
            }
            Some(ImportEditMode::GroupImporter { alternates }) => {
                for (alias, (library, alternate)) in alternates {
                    let pin = authored["objects"][alias]["fields"]["hash"]["v"].as_str();
                    let alternate_hash = crate::bundle::sha256_digest(alternate.as_bytes());
                    if pin == Some(alternate_hash.as_str()) {
                        bundle.sources.insert(library.clone(), alternate.clone());
                    }
                }
            }
            Some(ImportEditMode::Graph { imports, overrides }) => {
                for (path, source) in overrides {
                    bundle.sources.insert(path.clone(), source.clone());
                }
                // The candidate source takes precedence over any staged copy.
                bundle.sources.insert(
                    self.source_path.clone(),
                    super::source::authored_source(authored)?,
                );
                let entry = bundle.entry.clone();
                repin_graph(&mut bundle, imports, &entry)?;
            }
            None => {}
        }
        Ok(bundle)
    }

    fn resolve_bundle(
        &self,
        bundle: &crate::bundle::SourceBundle,
    ) -> EditResult<crate::bundle::ResolvedBundle> {
        if let Some(assets) = &self.disk_assets {
            bundle
                .resolve_with_disk_assets(assets)
                .map_err(map_diagnostics)
        } else {
            bundle.resolve().map_err(map_diagnostics)
        }
    }

    fn resolved_candidate(
        &self,
        authored: &Value,
    ) -> EditResult<(crate::bundle::ResolvedBundle, crate::library::LibrarySet)> {
        let bundle = self.candidate_bundle(authored)?;
        let resolved = self.resolve_bundle(&bundle)?;
        let libraries = crate::library::LibrarySet::resolve(&resolved).map_err(map_diagnostics)?;
        Ok((resolved, libraries))
    }
}

fn repin_graph(
    bundle: &mut crate::bundle::SourceBundle,
    imports: &BTreeMap<String, BTreeMap<String, String>>,
    path: &str,
) -> EditResult<()> {
    repin_graph_visit(bundle, imports, path, &mut BTreeSet::new())
}

fn repin_graph_visit(
    bundle: &mut crate::bundle::SourceBundle,
    imports: &BTreeMap<String, BTreeMap<String, String>>,
    path: &str,
    visited: &mut BTreeSet<String>,
) -> EditResult<()> {
    if !visited.insert(path.into()) {
        return Ok(());
    }
    if let Some(aliases) = imports.get(path) {
        let mut hashes = BTreeMap::new();
        let mut target_hashes: BTreeMap<String, String> = BTreeMap::new();
        for (alias, target) in aliases {
            repin_graph_visit(bundle, imports, target, visited)?;
            let hash = if let Some(hash) = target_hashes.get(target) {
                hash.clone()
            } else {
                let target_source = bundle.sources.get(target).ok_or_else(|| {
                    EditError::new("E_REFERENCE", "graph edit import target is missing")
                })?;
                let hash = crate::bundle::sha256_digest(target_source.as_bytes());
                target_hashes.insert(target.clone(), hash.clone());
                hash
            };
            hashes.insert(alias.clone(), hash);
        }
        let current = bundle
            .sources
            .get(path)
            .ok_or_else(|| EditError::new("E_REFERENCE", "graph edit importer is missing"))?;
        if let Some(updated) = replace_import_hashes(current, &hashes)? {
            bundle.sources.insert(path.into(), updated);
        }
    }
    Ok(())
}

fn replace_import_hashes(
    source: &str,
    hashes: &BTreeMap<String, String>,
) -> EditResult<Option<String>> {
    if hashes.is_empty() {
        return Ok(None);
    }
    let document = crate::syntax::parse(source).map_err(map_diagnostics)?;
    let mut replacements = Vec::new();
    for (alias, hash) in hashes {
        let object = document
            .objects
            .get(alias)
            .ok_or_else(|| EditError::new("E_REFERENCE", "import alias is missing"))?;
        if object.kind != "import" {
            return Err(EditError::new(
                "E_CAPABILITY",
                "selected alias is not an import",
            ));
        }
        let field = object
            .field("hash")
            .ok_or_else(|| EditError::new("E_REFERENCE", "import hash is missing"))?;
        let previous = field
            .value
            .as_string()
            .ok_or_else(|| EditError::new("E_SYNTAX", "import hash is not a string"))?;
        if previous != hash {
            let replacement = serde_json::to_string(hash)
                .map_err(|e| EditError::new("E_SYNTAX", e.to_string()))?;
            replacements.push((field.value.span.start, field.value.span.end, replacement));
        }
    }
    if replacements.is_empty() {
        return Ok(None);
    }
    replacements.sort_unstable_by_key(|item| std::cmp::Reverse(item.0));
    let mut output = source.to_owned();
    for (start, end, replacement) in replacements {
        output.replace_range(start..end, &replacement);
    }
    Ok(Some(output))
}

fn replace_import_hash(source: &str, alias: &str, hash: &str) -> EditResult<String> {
    let document = crate::syntax::parse(source).map_err(map_diagnostics)?;
    let object = document
        .objects
        .get(alias)
        .ok_or_else(|| EditError::new("E_REFERENCE", "import alias is missing"))?;
    if object.kind != "import" {
        return Err(EditError::new(
            "E_CAPABILITY",
            "selected alias is not an import",
        ));
    }
    let field = object
        .field("hash")
        .ok_or_else(|| EditError::new("E_REFERENCE", "import hash is missing"))?;
    if field.value.as_string().is_none() {
        return Err(EditError::new("E_SYNTAX", "import hash is not a string"));
    }
    let replacement =
        serde_json::to_string(hash).map_err(|e| EditError::new("E_SYNTAX", e.to_string()))?;
    let mut output = source.to_owned();
    output.replace_range(field.value.span.start..field.value.span.end, &replacement);
    Ok(output)
}

impl EditContext for BundleEditContext {
    fn validate_document(&self, authored: &Value) -> EditResult<()> {
        let bundle = self.candidate_bundle(authored)?;
        if let Some(assets) = &self.disk_assets {
            crate::compiler::compile_disk_media_artifact(&bundle, assets, &self.limits)
                .map_err(map_diagnostics)?;
            return Ok(());
        }
        let (resolved, libraries) = self.resolved_candidate(authored)?;
        let target = resolved
            .documents
            .get(&self.source_path)
            .ok_or_else(|| EditError::new("E_REFERENCE", "resolved editable source is missing"))?;
        if target
            .objects
            .values()
            .any(|object| object.kind == "library")
        {
            return Ok(());
        }
        let needs_artifact_validation = crate::takes::declared(target)
            || target.objects.values().any(|object| {
                object.kind == "extension"
                    || (object.kind == "node"
                        && object
                            .field("type")
                            .and_then(|field| field.value.as_string())
                            .is_some_and(|processor| processor.starts_with("fx.")))
            });
        if needs_artifact_validation {
            return crate::compiler::check_bundle_artifact(&bundle).map_err(map_diagnostics);
        }
        let document = libraries.resolved_document();
        let descriptors =
            crate::compiler::instrument_descriptors_for_editing(&resolved, &libraries, &document)
                .map_err(map_diagnostics)?;
        let graph = crate::semantic::validate_source_document_profile(&document, &descriptors)
            .map_err(|diagnostics| {
                if self.source_path == self.bundle.entry {
                    map_local_diagnostics(diagnostics)
                } else {
                    map_diagnostics(diagnostics)
                }
            })?;
        validate_occurrence_targets(&document.to_syntax_json_value(), graph.models()).map_err(
            |error| {
                if self.source_path == self.bundle.entry {
                    error
                } else {
                    error.without_source_mapping()
                }
            },
        )
    }

    fn normalize_object(
        &self,
        base: &Value,
        base_path: &[String],
        object: &Value,
    ) -> EditResult<Value> {
        let libraries = if needs_libraries(base, base_path, object) {
            Some(self.resolved_candidate(base)?.1)
        } else {
            None
        };
        let context = NormalizationContext::new(base, &self.source_path, libraries.as_ref())
            .map_err(edit_error)?;
        crate::normalization::normalize_object(&context, base_path, object).map_err(edit_error)
    }

    fn normalize_value(
        &self,
        base: &Value,
        base_path: &[String],
        field: &[String],
        value: &Value,
    ) -> EditResult<Value> {
        let context =
            NormalizationContext::new(base, &self.source_path, None).map_err(edit_error)?;
        crate::normalization::normalize_value(&context, base_path, field, value).map_err(edit_error)
    }

    fn rewrite_structural_references(
        &self,
        before: &Value,
        candidate: &mut Value,
        old_path: &[String],
        new_path: &[String],
    ) -> EditResult<()> {
        FoundationEditContext.rewrite_structural_references(before, candidate, old_path, new_path)
    }

    fn validate_transition(&self, base: &Value, candidate: &Value) -> EditResult<()> {
        reject_shifted_chord_members(base, candidate)
    }

    fn refine_impact(
        &self,
        base: &Value,
        candidate: &Value,
        impact: &mut EditImpact,
    ) -> EditResult<()> {
        FoundationEditContext.refine_impact(base, candidate, impact)
    }
}

impl FoundationEditContext {
    /// Prepare an atomic Protocol 2 copy of one placement's complete pattern
    /// occurrences. The source document is unchanged until the plan is applied.
    pub fn prepare_materialize_instance(
        &self,
        document: &AuthoredDocument,
        placement_id: &str,
        new_pattern_id: &str,
    ) -> EditResult<super::MaterializeInstancePlan> {
        self.validate_document(document.tree())?;
        super::materialize::prepare(
            self,
            document,
            document.tree(),
            &BTreeMap::new(),
            placement_id,
            new_pattern_id,
        )
    }

    /// Validate and compute the label-retaining N(A) view used by editing
    /// preconditions. This is not the execution-hash projection.
    pub fn normalize_document(&self, document: &AuthoredDocument) -> EditResult<Value> {
        self.validate_document(document.tree())?;
        let context =
            NormalizationContext::new(document.tree(), LOCAL_SOURCE, None).map_err(edit_error)?;
        crate::normalization::normalize_document(&context).map_err(edit_error)
    }
}

impl EditContext for FoundationEditContext {
    fn validate_document(&self, authored: &Value) -> EditResult<()> {
        let document = crate::syntax::document_from_syntax_json_value(authored)
            .map_err(|message| EditError::new("E_SYNTAX", message))?;
        reject_unsupported_capabilities(&document)?;
        let instruments = BTreeMap::new();
        let graph = crate::semantic::validate_source_document_profile(&document, &instruments)
            .map_err(map_local_diagnostics)?;
        validate_occurrence_targets(authored, graph.models())
    }

    fn normalize_object(
        &self,
        base: &Value,
        base_path: &[String],
        object: &Value,
    ) -> EditResult<Value> {
        let context = NormalizationContext::new(base, LOCAL_SOURCE, None).map_err(edit_error)?;
        crate::normalization::normalize_object(&context, base_path, object).map_err(edit_error)
    }

    fn normalize_value(
        &self,
        base: &Value,
        base_path: &[String],
        field: &[String],
        value: &Value,
    ) -> EditResult<Value> {
        let context = NormalizationContext::new(base, LOCAL_SOURCE, None).map_err(edit_error)?;
        crate::normalization::normalize_value(&context, base_path, field, value).map_err(edit_error)
    }

    fn rewrite_structural_references(
        &self,
        before: &Value,
        candidate: &mut Value,
        old_path: &[String],
        new_path: &[String],
    ) -> EditResult<()> {
        rewrite_occurrence_addresses(before, candidate, old_path, new_path)
    }

    fn validate_transition(&self, base: &Value, candidate: &Value) -> EditResult<()> {
        reject_shifted_chord_members(base, candidate)
    }

    fn refine_impact(
        &self,
        base: &Value,
        candidate: &Value,
        impact: &mut EditImpact,
    ) -> EditResult<()> {
        refine_foundation_impact(base, candidate, impact)
    }
}

fn edit_error(error: crate::normalization::NormalizationError) -> EditError {
    let path = (error.object_path.clone(), error.field_path.clone());
    let edit = EditError::new(error.code, error.message);
    if path.0.is_empty() {
        edit
    } else {
        edit.at(&path.0, &path.1)
    }
}

/// Whether normalizing `object` at `base_path` reads the resolved libraries:
/// it sits in an instrument or effect, or declares or names one.
fn needs_libraries(base: &Value, base_path: &[String], object: &Value) -> bool {
    fn declares(object: &Value) -> bool {
        matches!(object["kind"].as_str(), Some("instrument" | "effect"))
            || (object["kind"] == "node"
                && (object["fields"].get("instrument").is_some()
                    || object["fields"].get("effect").is_some()))
            || object["children"]
                .as_object()
                .is_some_and(|children| children.values().any(declares))
    }
    let enclosing = base_path
        .first()
        .and_then(|id| base["objects"].get(id))
        .and_then(|object| object["kind"].as_str());
    matches!(enclosing, Some("instrument" | "effect")) || declares(object)
}

fn map_diagnostics(diagnostics: Diagnostics) -> EditError {
    // Resolution, descriptors and artifact checks can diagnose another source
    // with an unqualified path. No original-source correspondence is proven.
    map_local_diagnostics(diagnostics).without_source_mapping()
}

fn map_local_diagnostics(diagnostics: Diagnostics) -> EditError {
    let diagnostic = diagnostics
        .into_vec()
        .into_iter()
        .find(|diagnostic| diagnostic.is_error())
        .unwrap_or_else(|| {
            crate::Diagnostic::error(
                crate::DiagnosticCode::Range,
                "semantic validation failed",
                None,
            )
        });
    EditError::new(diagnostic.code_str(), diagnostic.message)
        .at(&diagnostic.object_path, &diagnostic.field_path)
}

fn reject_unsupported_capabilities(document: &Document) -> EditResult<()> {
    crate::extensions::check(document).map_err(map_local_diagnostics)?;
    for object in document.objects.values() {
        if matches!(
            object.kind.as_str(),
            "extension"
                | "import"
                | "library"
                | "instrument"
                | "effect"
                | "preset"
                | "wavetable"
                | "sample"
        ) {
            return Err(EditError::new(
                "E_CAPABILITY",
                format!(
                    "{} editing normalization is outside FoundationEditContext",
                    object.kind
                ),
            )
            .at(std::slice::from_ref(&object.id), &[]));
        }
        if object.kind == "node" {
            if object.field("instrument").is_some() || object.field("effect").is_some() {
                return Err(EditError::new(
                    "E_CAPABILITY",
                    "local instrument editing requires a library-aware context",
                )
                .at(std::slice::from_ref(&object.id), &[]));
            }
            if object
                .field("type")
                .and_then(|field| field.value.as_string())
                .is_some_and(|kind| kind.starts_with("fx."))
            {
                return Err(EditError::new(
                    "E_CAPABILITY",
                    "native production node editing requires a production-aware context",
                )
                .at(std::slice::from_ref(&object.id), &[]));
            }
        }
    }
    Ok(())
}

fn object_at<'a>(tree: &'a Value, path: &[String]) -> EditResult<&'a Value> {
    let mut objects = tree["objects"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    let mut object = None;
    for id in path {
        let next = objects.get(id).ok_or_else(|| {
            EditError::new("E_REFERENCE", "object path does not exist").at(path, &[])
        })?;
        object = Some(next);
        objects = next["children"]
            .as_object()
            .ok_or_else(|| EditError::new("E_SYNTAX", "children must be a dictionary"))?;
    }
    object.ok_or_else(|| EditError::new("E_REFERENCE", "object path is empty"))
}

fn typed_rational(value: &Value) -> EditResult<BigRational> {
    let numerator = value["n"]
        .as_str()
        .ok_or_else(|| EditError::new("E_SYNTAX", "rational numerator must be a string"))?
        .parse::<BigInt>()
        .map_err(|_| EditError::new("E_SYNTAX", "invalid rational numerator"))?;
    let denominator = value["d"]
        .as_str()
        .ok_or_else(|| EditError::new("E_SYNTAX", "rational denominator must be a string"))?
        .parse::<BigInt>()
        .map_err(|_| EditError::new("E_SYNTAX", "invalid rational denominator"))?;
    if denominator <= BigInt::zero() {
        return Err(EditError::new(
            "E_RANGE",
            "rational denominator must be positive",
        ));
    }
    Ok(BigRational::new(numerator, denominator))
}

fn source_object_id(value: &Value) -> Option<&str> {
    if value["t"] != "ref" || !value["port"].is_null() {
        return None;
    }
    let path = value["path"].as_array()?;
    (path.len() == 1).then(|| path[0].as_str()).flatten()
}

fn positive_count(object: &Value) -> Option<u64> {
    match object["fields"].get("count") {
        None => Some(1),
        Some(value) => {
            let value = typed_rational(value).ok()?;
            if !value.is_integer() || value <= BigRational::zero() {
                return None;
            }
            value.to_integer().to_u64()
        }
    }
}

/// Each source object an occurrence passes through, with the address segment
/// that names it.
type SourceMappings = Vec<(Path, usize)>;

/// A tree with the models that semantic analysis reads of its voicings and
/// harmonies, of what its uses and placements supply, and of the pitches
/// its chords play. A model is missing where the tree is wrong, and what
/// depends on it then does not resolve.
pub(super) struct Reading<'a> {
    objects: &'a Map<String, Value>,
    models: Cow<'a, Models>,
}

impl<'a> Reading<'a> {
    /// Read `tree`. `None` is a tree that is not a document.
    pub(super) fn of(tree: &'a Value) -> Option<Self> {
        let document = crate::syntax::document_from_syntax_json_value(tree).ok()?;
        Some(Self {
            objects: tree["objects"].as_object()?,
            models: Cow::Owned(crate::semantic::models_of(&document)),
        })
    }

    /// `tree` with the `models` that checking it read.
    fn with(tree: &'a Value, models: &'a Models) -> Option<Self> {
        Some(Self {
            objects: tree["objects"].as_object()?,
            models: Cow::Borrowed(models),
        })
    }

    /// Whether the use or placement at object path `path` supplies a
    /// harmony.
    pub(super) fn supplies_harmony(&self, path: &[String]) -> bool {
        matches!(self.models.supplies.get(path), Some(Supply::Harmony { .. }))
    }
}

/// The chord member an occurrence address ends at: member `index` of chord
/// `chord` of pattern `pattern`.
struct Member {
    pattern: String,
    chord: String,
    index: usize,
}

/// Follow `address` from placement `place`, with ID `place_id`, down its
/// uses to its leaf. `enter` is told of the placement and then of each use
/// on the way: its object path, the object, the repetition the address
/// names, and the length of its pattern. Returns the source mappings and,
/// when the leaf is a chord member, that member, which is not checked
/// against the pitches the chord plays.
fn walk_occurrence(
    objects: &Map<String, Value>,
    place_id: &str,
    place: &Value,
    address: &str,
    mut enter: impl FnMut(&[String], &Value, u64, &BigRational) -> Option<()>,
) -> Option<(SourceMappings, Option<Member>)> {
    let segments: Vec<_> = address.split('/').collect();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let place_index = segments[0].parse::<u64>().ok()?;
    if place_index >= positive_count(place)? {
        return None;
    }
    let mut pattern_id = source_object_id(place["fields"].get("pattern")?)?.to_owned();
    let length = typed_rational(&objects.get(&pattern_id)?["fields"]["length"]).ok()?;
    enter(&[place_id.to_owned()], place, place_index, &length)?;
    let mut position = 1usize;
    let mut mappings = Vec::new();

    loop {
        let pattern = objects.get(&pattern_id)?;
        if pattern["kind"] != "pattern" {
            return None;
        }
        let (child_id, member) = split_chord_member(segments.get(position)?)?;
        let child = pattern["children"].get(child_id)?;
        let source_path = vec![pattern_id.clone(), child_id.to_owned()];
        mappings.push((source_path.clone(), position));
        match (child["kind"].as_str()?, member) {
            ("note" | "hit" | "message" | "audio", None) => {
                return (position + 1 == segments.len()).then_some((mappings, None));
            }
            ("chord", Some(index)) => {
                let member = Member {
                    pattern: pattern_id,
                    chord: child_id.to_owned(),
                    index,
                };
                return (position + 1 == segments.len()).then_some((mappings, Some(member)));
            }
            ("use", None) => {
                let repeat_position = position + 1;
                let repeat = segments.get(repeat_position)?.parse::<u64>().ok()?;
                if repeat >= positive_count(child)? {
                    return None;
                }
                pattern_id = source_object_id(child["fields"].get("pattern")?)?.to_owned();
                let length = typed_rational(&objects.get(&pattern_id)?["fields"]["length"]).ok()?;
                enter(&source_path, child, repeat, &length)?;
                position += 2;
            }
            _ => return None,
        }
    }
}

/// The source mappings of the occurrence at `address`, read from the
/// shape of `root` alone: a tree in the middle of a transaction may be
/// wrong, and its occurrences keep their source objects all the same.
fn occurrence_sources(
    root: &Value,
    place_id: &str,
    place: &Value,
    address: &str,
) -> Option<SourceMappings> {
    let objects = root["objects"].as_object()?;
    walk_occurrence(objects, place_id, place, address, |_, _, _, _| Some(()))
        .map(|(mappings, _)| mappings)
}

fn resolve_occurrence(
    reading: &Reading<'_>,
    place_id: &str,
    place: &Value,
    address: &str,
) -> Option<SourceMappings> {
    resolve_occurrence_pitches(reading, place_id, place, address).map(|(mappings, _)| mappings)
}

/// The source mappings of the occurrence at `address` and, for a chord
/// member, the pitches its chord plays there.
fn resolve_occurrence_pitches<'a>(
    reading: &Reading<'a>,
    place_id: &str,
    place: &Value,
    address: &str,
) -> Option<(SourceMappings, Option<Vec<&'a Value>>)> {
    let mut supplied = None;
    let (mappings, member) = walk_occurrence(
        reading.objects,
        place_id,
        place,
        address,
        |path, object, repetition, length| {
            supplied = supplied_into(reading, path, object, supplied.as_ref(), repetition, length)?;
            Some(())
        },
    )?;
    let Some(member) = member else {
        return Some((mappings, None));
    };
    let pitches = chord_pitches(reading, &member.pattern, &member.chord, supplied.as_ref())?;
    (member.index < pitches.len()).then_some((mappings, Some(pitches)))
}

/// Split a leaf address segment into its leaf ID and, for a chord member,
/// its canonical decimal member index (`a.2`).
fn split_chord_member(segment: &str) -> Option<(&str, Option<usize>)> {
    match segment.split_once('.') {
        None => Some((segment, None)),
        Some((leaf, member)) => {
            let index = member.parse::<usize>().ok()?;
            (index.to_string() == member).then_some((leaf, Some(index)))
        }
    }
}

/// What reaches the chords of a pattern from the nearest use or placement
/// that supplies anything (§8.2), which `by` names. `offset` and `scale`
/// map the pattern's positions to the supplier's, where a harmony's chords
/// are looked up.
#[derive(Clone, Debug)]
pub(super) struct Supplied {
    supply: Supply,
    by: String,
    offset: BigRational,
    scale: BigRational,
}

impl Supplied {
    /// The objects a chord playing this supply depends on.
    fn dependencies(&self) -> Vec<String> {
        match &self.supply {
            Supply::Voicing(id) => vec![id.clone()],
            Supply::Harmony { harmony, voicings } => vec![harmony.clone(), voicings.clone()],
        }
    }

    /// Whether every repetition of a pattern under it plays the same: one
    /// voicing does, and a harmony moves on with each.
    fn is_one_voicing(&self) -> bool {
        matches!(self.supply, Supply::Voicing(_))
    }

    /// What tells this from another supply to the same pattern. A harmony
    /// repeats every length, so the pattern's place in one pass is what
    /// counts.
    fn key(&self, reading: &Reading<'_>) -> SuppliedKey {
        let zero = BigRational::zero();
        let (offset, scale) = match &self.supply {
            Supply::Voicing(_) => (zero.clone(), zero),
            Supply::Harmony { harmony, .. } => {
                let offset = match reading.models.harmonies.get(harmony) {
                    Some(harmony) => {
                        &self.offset - (&self.offset / &harmony.length).floor() * &harmony.length
                    }
                    None => self.offset.clone(),
                };
                (offset, self.scale.clone())
            }
        };
        (self.supply.clone(), self.by.clone(), offset, scale)
    }
}

/// What reaches repetition `repetition` of the pattern under `object`, the
/// use or placement at object path `path` whose pattern is `length` long:
/// what it supplies itself, or else what is `inherited` from further out,
/// moved into its pattern. `None` is a supply or a field that did not read.
pub(super) fn supplied_into(
    reading: &Reading<'_>,
    path: &[String],
    object: &Value,
    inherited: Option<&Supplied>,
    repetition: u64,
    length: &BigRational,
) -> Option<Option<Supplied>> {
    let fields = &object["fields"];
    let repeated = length * BigRational::from_integer(repetition.into());
    if let Some(supply) = reading.models.supplies.get(path) {
        return Some(Some(Supplied {
            supply: supply.clone(),
            by: supplier(object["kind"].as_str()?, path.last()?),
            offset: repeated,
            scale: BigRational::from_integer(1.into()),
        }));
    }
    // One that writes a supply that did not read never passes on the one
    // from further out.
    if ["voicing", "harmony", "voicings"]
        .iter()
        .any(|name| fields.get(name).is_some())
    {
        return None;
    }
    let Some(inherited) = inherited else {
        return Some(None);
    };
    if inherited.is_one_voicing() {
        return Some(Some(inherited.clone()));
    }
    // A use moves its pattern by `at` and its repetitions, and stretches
    // it.
    let field = |name: &str, default: i64| match fields.get(name) {
        None => Some(BigRational::from_integer(default.into())),
        Some(value) => typed_rational(value).ok(),
    };
    let (at, stretch) = (field("at", 0)?, field("stretch", 1)?);
    Some(Some(Supplied {
        supply: inherited.supply.clone(),
        by: inherited.by.clone(),
        offset: &inherited.offset + &inherited.scale * (at + &stretch * repeated),
        scale: &inherited.scale * stretch,
    }))
}

/// The voicing a chord is given at one occurrence: its ID and, under a
/// harmony, the name of the chord it voices.
struct Given<'m> {
    voicing: &'m str,
    name: Option<&'m str>,
}

impl Given<'_> {
    /// The words for where the voicing came from.
    fn origin(&self, supplied: &Supplied) -> String {
        origin(&supplied.supply, self.voicing, self.name, &supplied.by)
    }
}

/// The voicing that chord `chord` at position `at` of its pattern plays
/// under `supplied`: the voicing, or the part's voicing for the harmony's
/// chord sounding there. The error is why the chord plays nothing there.
/// `None` is a harmony or voicings that did not read.
fn given_voicing<'m>(
    reading: &'m Reading<'_>,
    chord: &str,
    supplied: Option<&'m Supplied>,
    at: &BigRational,
) -> Option<Result<Given<'m>, Unplayable>> {
    let Some(supplied) = supplied else {
        return Some(Err(Unplayable::no_voicing(chord)));
    };
    Some(Ok(match &supplied.supply {
        Supply::Voicing(id) => Given {
            voicing: id,
            name: None,
        },
        Supply::Harmony { harmony, voicings } => {
            let position = &supplied.offset + &supplied.scale * at;
            let name = reading.models.harmonies.get(harmony)?.chord_at(&position);
            let set = reading.models.voicing_sets.get(voicings)?;
            let Some(id) = set.chords.get(name) else {
                return Some(Err(Unplayable::unmapped(
                    chord,
                    name,
                    harmony,
                    voicings,
                    set,
                    &supplied.by,
                )));
            };
            Given {
                voicing: id,
                name: Some(name),
            }
        }
    }))
}

/// A pattern child's onset in pattern-local q, following `at = after(id,
/// rest)` (§9) back to a written position, with the IDs it follows on the
/// way, nearest first.
pub(super) fn child_onset(pattern: &Value, child: &Value) -> Option<(BigRational, Vec<String>)> {
    let children = pattern["children"].as_object()?;
    let mut onset = BigRational::zero();
    let mut chain = Vec::new();
    let mut current = child;
    loop {
        let at = &current["fields"]["at"];
        if at["t"] != "call" {
            return Some((onset + typed_rational(at).ok()?, chain));
        }
        if at["fn"] != "after" || chain.len() > children.len() {
            return None;
        }
        let args = at["args"].as_array()?;
        let previous = args.first()?["v"].as_str()?;
        if let Some(rest) = args.get(1) {
            onset += typed_rational(rest).ok()?;
        }
        let before = children.get(previous)?;
        onset += typed_rational(&before["fields"]["dur"]).ok()?;
        chain.push(previous.to_owned());
        current = before;
    }
}

/// The pitches that chord `chord` of pattern `pattern` plays (§8.2): its own
/// `pitches`, those of the voicing it names, or else those `supplied` by its
/// nearest use or placement, narrowed by `top`, `bottom` or `pick`.
pub(super) fn chord_pitches<'a>(
    reading: &Reading<'a>,
    pattern: &str,
    chord: &str,
    supplied: Option<&Supplied>,
) -> Option<Vec<&'a Value>> {
    let played = reading
        .models
        .played
        .get([pattern.to_owned(), chord.to_owned()].as_slice())?;
    let pattern = reading.objects.get(pattern)?;
    let chord_id = chord;
    let chord = pattern["children"].get(chord_id)?;
    let of_voicing = |id: &str| -> Option<&'a Value> {
        reading.models.voicings.get(id)?;
        reading.objects.get(id)?["fields"].get("pitches")
    };
    let pitches = match &played.source {
        Source::Own => chord["fields"].get("pitches")?,
        Source::Voicing(id) => of_voicing(id)?,
        Source::Supplied => {
            let at = child_onset(pattern, chord)?.0;
            let given = given_voicing(reading, chord_id, Some(supplied?), &at)?.ok()?;
            of_voicing(given.voicing)?
        }
    };
    played.keep.of(pitches["items"].as_array()?).ok()
}

/// The occurrence a placement child names: an override's `event`, or the
/// `event` of an automation in the placement (§12).
fn named_occurrence(child: &Value) -> Option<&str> {
    match child["kind"].as_str()? {
        "override" | "automation" => child["fields"]["event"]["v"].as_str(),
        _ => None,
    }
}

/// An inserted event's address, `insert_id/leaf_id`, names an insert of
/// the placement and its leaf.
fn names_insert(place: &Value, address: &str) -> bool {
    address.split_once('/').is_some_and(|(insert, leaf)| {
        place["children"]
            .get(insert)
            .is_some_and(|child| child["kind"] == "insert" && child["children"].get(leaf).is_some())
    })
}

/// A chord member is identified by its index. When an edit changes the
/// length of the pitches a chord plays, through its `pitches`, its voicing,
/// or its `top`, `bottom` or `pick`, members after the first changed
/// position shift. An existing override addressing one of them is rejected
/// rather than silently retargeted. Appending keeps every existing index.
fn reject_shifted_chord_members(base: &Value, candidate: &Value) -> EditResult<()> {
    let (Some(base_objects), Some(candidate_objects)) = (
        base["objects"].as_object(),
        candidate["objects"].as_object(),
    ) else {
        return Ok(());
    };
    // An occurrence resolves through placements, patterns, voicings and
    // harmonies. With all of those as they were, no member moved.
    fn resolved_through(objects: &Map<String, Value>) -> impl Iterator<Item = (&String, &Value)> {
        objects.iter().filter(|(_, object)| {
            matches!(
                object["kind"].as_str(),
                Some("place" | "pattern" | "voicing" | "harmony" | "voicings")
            )
        })
    }
    if resolved_through(base_objects).eq(resolved_through(candidate_objects)) {
        return Ok(());
    }
    // Both trees are read once, when the first target that could shift is
    // found.
    let mut readings = None;
    for (place_id, place) in candidate_objects {
        if place["kind"] != "place" {
            continue;
        }
        let Some(children) = place["children"].as_object() else {
            continue;
        };
        for (override_id, override_object) in children {
            let Some(event) = named_occurrence(override_object) else {
                continue;
            };
            let kind = override_object["kind"].as_str().unwrap_or_default();
            let Some(base_place) = base_objects.get(place_id) else {
                continue;
            };
            let existed = base_place["children"]
                .get(override_id.as_str())
                .is_some_and(|before| {
                    before["kind"] == kind && before["fields"]["event"]["v"] == event
                });
            if !existed {
                continue;
            }
            if readings.is_none() {
                readings = Reading::of(base).zip(Reading::of(candidate));
            }
            let Some((before, after)) = &readings else {
                return Ok(());
            };
            // A target that no longer exists is reported as dangling.
            let (Some((mappings, Some(old))), Some((_, Some(new)))) = (
                resolve_occurrence_pitches(before, place_id, base_place, event),
                resolve_occurrence_pitches(after, place_id, place, event),
            ) else {
                continue;
            };
            if old.len() == new.len() {
                continue;
            }
            let first = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
            let (source_path, position) = mappings.last().expect("an occurrence has a leaf");
            let segment = event.split('/').nth(*position).unwrap_or_default();
            let member = split_chord_member(segment).and_then(|(_, member)| member);
            if member.is_some_and(|member| member >= first) {
                return Err(EditError::new(
                    "E_INSTANCE_TARGET",
                    format!(
                        "chord `{}` changed length, so {kind} target `{event}` would address a different member",
                        source_path.join(".")
                    ),
                )
                .at(&[place_id.clone(), override_id.clone()], &["event".into()]));
            }
        }
    }
    Ok(())
}

/// How many chord occurrences and repetitions the check of chord
/// occurrences visits at most. A document past it is past the notes the
/// compiler expands.
const MAX_OCCURRENCE_CHECK_STEPS: usize = 100_000;

/// Every chord of `reading`'s tree plays wherever a placement puts it
/// (§8.2): a chord without pitches has a voicing there, and its `top`,
/// `bottom`, `pick`, per-member lists and tilt fit that voicing. The
/// compiler refuses the same occurrences with the same code and words.
/// What does not read is another check's to report.
fn validate_chord_occurrences(reading: &Reading<'_>) -> EditResult<()> {
    let mut walk = ChordWalk {
        reading,
        steps: MAX_OCCURRENCE_CHECK_STEPS,
        waiting: BTreeMap::new(),
        checked: BTreeSet::new(),
    };
    for (place_id, place) in reading.objects {
        if place["kind"] != "place" {
            continue;
        }
        let path = std::slice::from_ref(place_id);
        if !walk.repetitions_play(path, place, None, 0)? {
            break;
        }
    }
    Ok(())
}

/// The pattern that `object`, a use or a placement, plays, with how many
/// times and the pattern's length.
fn used_pattern<'a>(
    objects: &Map<String, Value>,
    object: &'a Value,
) -> Option<(&'a str, u64, BigRational)> {
    let pattern = source_object_id(object["fields"].get("pattern")?)?;
    let length = typed_rational(&objects.get(pattern)?["fields"]["length"]).ok()?;
    Some((pattern, positive_count(object)?, length))
}

/// The walk of [`validate_chord_occurrences`]. Each of its methods returns
/// whether the walk may go on: it stops when its `steps` run out.
struct ChordWalk<'r, 'a> {
    reading: &'r Reading<'a>,
    steps: usize,
    /// Whether each pattern seen holds a chord that waits for a supplied
    /// voicing, itself or through its uses.
    waiting: BTreeMap<String, bool>,
    /// Each pattern already checked, with what was supplied to it. The
    /// same pattern under the same supply plays the same again.
    checked: BTreeSet<(String, Option<SuppliedKey>)>,
}

/// What tells one supply to a pattern from another: the supply, its
/// supplier, and where the pattern sits in a harmony.
type SuppliedKey = (Supply, String, BigRational, BigRational);

impl ChordWalk<'_, '_> {
    fn step(&mut self) -> bool {
        self.steps = self.steps.saturating_sub(1);
        self.steps > 0
    }

    /// Whether pattern `pattern_id`, or a pattern it uses, holds a chord
    /// that plays a supplied voicing. Most patterns hold none, and the walk
    /// passes them by.
    fn waits(&mut self, pattern_id: &str, depth: usize) -> bool {
        if let Some(waits) = self.waiting.get(pattern_id) {
            return *waits;
        }
        let objects = self.reading.objects;
        let children = objects
            .get(pattern_id)
            .and_then(|pattern| pattern["children"].as_object());
        let waits = depth < 64
            && children.is_some_and(|children| {
                children.iter().any(|(child_id, child)| {
                    let path = [pattern_id.to_owned(), child_id.clone()];
                    match child["kind"].as_str() {
                        Some("chord") => self
                            .reading
                            .models
                            .played
                            .get(path.as_slice())
                            .is_some_and(|played| played.source == Source::Supplied),
                        Some("use") => used_pattern(objects, child)
                            .is_some_and(|(nested, _, _)| self.waits(nested, depth + 1)),
                        _ => false,
                    }
                })
            });
        self.waiting.insert(pattern_id.to_owned(), waits);
        waits
    }

    /// Check each repetition of the pattern that `object`, the use or
    /// placement at `path`, plays, with `inherited` reaching it from
    /// further out.
    fn repetitions_play(
        &mut self,
        path: &[String],
        object: &Value,
        inherited: Option<&Supplied>,
        depth: usize,
    ) -> EditResult<bool> {
        let Some((pattern, count, length)) = used_pattern(self.reading.objects, object) else {
            return Ok(true);
        };
        if !self.waits(pattern, depth) {
            return Ok(true);
        }
        let mut first = None;
        for repetition in 0..count {
            if !self.step() {
                return Ok(false);
            }
            let Some(supplied) =
                supplied_into(self.reading, path, object, inherited, repetition, &length)
            else {
                break;
            };
            // A harmony comes round again: from the repetition that sits
            // where the first did, every one has been checked.
            let key = supplied.as_ref().map(|supplied| supplied.key(self.reading));
            if first.as_ref() == Some(&key) {
                break;
            }
            if !self.pattern_plays(pattern, supplied.as_ref(), depth)? {
                return Ok(false);
            }
            // One voicing, or none, plays the same in every repetition.
            if supplied.as_ref().is_none_or(Supplied::is_one_voicing) {
                break;
            }
            first.get_or_insert(key);
        }
        Ok(true)
    }

    /// Check the chords of pattern `pattern_id`, and of the patterns it
    /// uses, under `supplied`.
    fn pattern_plays(
        &mut self,
        pattern_id: &str,
        supplied: Option<&Supplied>,
        depth: usize,
    ) -> EditResult<bool> {
        let reading = self.reading;
        let Some(pattern) = reading.objects.get(pattern_id) else {
            return Ok(true);
        };
        let Some(children) = pattern["children"].as_object() else {
            return Ok(true);
        };
        if depth >= 64 {
            return Ok(false);
        }
        let key = supplied.map(|supplied| supplied.key(reading));
        if !self.checked.insert((pattern_id.to_owned(), key)) {
            return Ok(true);
        }
        for (child_id, child) in children {
            if !matches!(child["kind"].as_str(), Some("chord" | "use")) {
                continue;
            }
            let path = [pattern_id.to_owned(), child_id.clone()];
            match child["kind"].as_str() {
                Some("chord") => {
                    let waits = reading
                        .models
                        .played
                        .get(path.as_slice())
                        .is_some_and(|played| played.source == Source::Supplied);
                    let Some((at, _)) = child_onset(pattern, child).filter(|_| waits) else {
                        continue;
                    };
                    if !self.step() {
                        return Ok(false);
                    }
                    let refuse = |error: Unplayable| {
                        let field: Vec<String> =
                            error.field.into_iter().map(str::to_owned).collect();
                        EditError::new(error.code.as_str(), error.message).at(&path, &field)
                    };
                    let Some(given) = given_voicing(reading, child_id, supplied, &at) else {
                        continue;
                    };
                    let given = given.map_err(refuse)?;
                    let Some(voicing) = reading.models.voicings.get(given.voicing) else {
                        continue;
                    };
                    if let (Some(Err(error)), Some(supplied)) = (
                        reading.models.members_given(&path, voicing.pitches),
                        supplied,
                    ) {
                        return Err(refuse(error.under(&given.origin(supplied))));
                    }
                }
                Some("use") => {
                    let goes_on = self.repetitions_play(&path, child, supplied, depth + 1)?;
                    if !goes_on {
                        return Ok(false);
                    }
                }
                _ => {}
            }
        }
        Ok(true)
    }
}

/// Each override and automation of a placement in `root` names an
/// occurrence that exists, after each chord plays wherever it is placed.
/// `models` are those that checking `root` read.
fn validate_occurrence_targets(root: &Value, models: &Models) -> EditResult<()> {
    let reading = Reading::with(root, models)
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    validate_chord_occurrences(&reading)?;
    for (place_id, place) in reading.objects {
        if place["kind"] != "place" {
            continue;
        }
        let mut targets = BTreeSet::new();
        let Some(children) = place["children"].as_object() else {
            continue;
        };
        // An automation may name any event of the placement, an inserted
        // one included, that no override deletes.
        let deleted: BTreeMap<&str, &str> = children
            .iter()
            .filter(|(_, child)| {
                child["kind"] == "override" && child["fields"]["delete"]["v"] == true
            })
            .filter_map(|(id, child)| Some((child["fields"]["event"]["v"].as_str()?, id.as_str())))
            .collect();
        for (automation_id, automation) in children {
            if automation["kind"] != "automation" {
                continue;
            }
            let Some(event) = named_occurrence(automation) else {
                continue;
            };
            let at = |error: EditError| {
                error.at(
                    &[place_id.clone(), automation_id.clone()],
                    &["event".into()],
                )
            };
            if let Some(by) = deleted.get(event) {
                return Err(at(EditError::new(
                    "E_INSTANCE_TARGET",
                    format!("automation event `{event}` is deleted by override `{by}`"),
                )));
            }
            if resolve_occurrence(&reading, place_id, place, event).is_none()
                && !names_insert(place, event)
            {
                return Err(at(EditError::new(
                    "E_INSTANCE_TARGET",
                    format!("automation event `{event}` does not exist"),
                )));
            }
        }
        for (override_id, override_object) in children {
            if override_object["kind"] != "override" {
                continue;
            }
            let event = override_object["fields"]["event"]["v"]
                .as_str()
                .ok_or_else(|| {
                    EditError::new("E_UNIT", "override.event must be a string")
                        .at(&[place_id.clone(), override_id.clone()], &["event".into()])
                })?;
            if !targets.insert(event.to_owned()) {
                return Err(EditError::new(
                    "E_INSTANCE_TARGET",
                    "an occurrence may have at most one override",
                )
                .at(&[place_id.clone(), override_id.clone()], &["event".into()]));
            }
            if resolve_occurrence(&reading, place_id, place, event).is_none() {
                return Err(EditError::new(
                    "E_INSTANCE_TARGET",
                    format!("override target `{event}` does not exist"),
                )
                .at(&[place_id.clone(), override_id.clone()], &["event".into()]));
            }
        }
    }
    Ok(())
}

fn rewrite_occurrence_addresses(
    before: &Value,
    candidate: &mut Value,
    old_path: &[String],
    new_path: &[String],
) -> EditResult<()> {
    if old_path.len() != 2 || new_path.len() != 2 {
        return Ok(());
    }
    let parent = object_at(before, &old_path[..1])?;
    if parent["kind"] != "pattern" {
        return Ok(());
    }

    let before_objects = before["objects"]
        .as_object()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    let candidate_objects = candidate["objects"]
        .as_object_mut()
        .ok_or_else(|| EditError::new("E_SYNTAX", "objects must be a dictionary"))?;
    for (place_id, before_place) in before_objects {
        if before_place["kind"] != "place" {
            continue;
        }
        let Some(before_children) = before_place["children"].as_object() else {
            continue;
        };
        for (override_id, before_override) in before_children {
            let Some(event) = named_occurrence(before_override) else {
                continue;
            };
            let Some(mappings) = occurrence_sources(before, place_id, before_place, event) else {
                continue;
            };
            let Some((_, segment_index)) = mappings
                .iter()
                .find(|(source_path, _)| source_path == old_path)
            else {
                continue;
            };
            let mut segments: Vec<_> = event.split('/').map(str::to_owned).collect();
            // A chord member keeps its index when its chord is renamed.
            let member = segments[*segment_index]
                .split_once('.')
                .map(|(_, member)| format!(".{member}"))
                .unwrap_or_default();
            segments[*segment_index] = format!("{}{member}", new_path[1]);
            let rewritten = segments.join("/");
            let target = candidate_objects
                .get_mut(place_id)
                .and_then(|place| place["children"].get_mut(override_id))
                .and_then(|object| object["fields"].get_mut("event"))
                .ok_or_else(|| {
                    EditError::new(
                        "E_REFERENCE",
                        "override moved while rewriting occurrence address",
                    )
                })?;
            target["v"] = Value::String(rewritten);
        }
    }
    Ok(())
}

const MAX_REPORTED_EVENT_ADDRESSES: usize = 1024;

#[derive(Clone, Debug)]
struct EventOccurrence {
    address: String,
    dependencies: Vec<Path>,
}

fn refine_foundation_impact(
    base: &Value,
    candidate: &Value,
    impact: &mut EditImpact,
) -> EditResult<()> {
    if execution_relevant_view(base) == execution_relevant_view(candidate) {
        impact.affected_expanded_event_addresses.clear();
        impact.total_affected_expanded_event_addresses = 0;
        impact.event_addresses_truncated = false;
        impact.all_expanded_events_may_be_affected = false;
        impact.render_invalidation_scope = RenderInvalidationScope::None;
        impact.full_render_invalidated = false;
        return Ok(());
    }

    let changed = directly_changed_paths(base, candidate);
    let Some(addresses) = bounded_event_impact(base, candidate, &changed) else {
        return Ok(());
    };
    let total = addresses.len();
    impact.affected_expanded_event_addresses = addresses
        .into_iter()
        .take(MAX_REPORTED_EVENT_ADDRESSES)
        .collect();
    impact.total_affected_expanded_event_addresses = total;
    impact.event_addresses_truncated = total > impact.affected_expanded_event_addresses.len();
    impact.all_expanded_events_may_be_affected = false;
    impact.render_invalidation_scope = RenderInvalidationScope::AffectedEventsAndDependents;
    impact.full_render_invalidated = false;
    Ok(())
}

fn execution_relevant_view(tree: &Value) -> Value {
    fn strip(objects: &mut Value) {
        let Some(objects) = objects.as_object_mut() else {
            return;
        };
        for object in objects.values_mut() {
            if let Some(fields) = object.get_mut("fields").and_then(Value::as_object_mut) {
                fields.remove("label");
            }
            if let Some(children) = object.get_mut("children") {
                strip(children);
            }
        }
    }
    let mut copy = tree.clone();
    strip(&mut copy["objects"]);
    copy
}

fn directly_changed_paths(base: &Value, candidate: &Value) -> Vec<Path> {
    fn own(value: &Value) -> Option<(&Value, &Value)> {
        Some((value.get("kind")?, value.get("fields")?))
    }
    let before = foundation_object_paths(base);
    let after = foundation_object_paths(candidate);
    before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| match (before.get(path), after.get(path)) {
            (Some(left), Some(right)) => own(left) != own(right),
            _ => true,
        })
        .collect()
}

fn foundation_object_paths(tree: &Value) -> BTreeMap<Path, &Value> {
    fn visit<'a>(objects: &'a Value, path: &mut Path, out: &mut BTreeMap<Path, &'a Value>) {
        let Some(objects) = objects.as_object() else {
            return;
        };
        for (id, object) in objects {
            path.push(id.clone());
            out.insert(path.clone(), object);
            visit(&object["children"], path, out);
            path.pop();
        }
    }
    let mut out = BTreeMap::new();
    visit(&tree["objects"], &mut Vec::new(), &mut out);
    out
}

fn bounded_event_impact(
    base: &Value,
    candidate: &Value,
    changed: &[Path],
) -> Option<BTreeSet<String>> {
    if changed.is_empty() {
        return Some(BTreeSet::new());
    }
    for path in changed {
        let kind = top_level_kind(base, path).or_else(|| top_level_kind(candidate, path))?;
        if !matches!(
            kind,
            "pattern" | "place" | "voicing" | "harmony" | "voicings"
        ) {
            return None;
        }
    }

    let mut affected = BTreeSet::new();
    for tree in [base, candidate] {
        for event in enumerate_event_occurrences(tree)? {
            if changed.iter().any(|path| {
                event
                    .dependencies
                    .iter()
                    .any(|dependency| dependency == path)
                    || path.first().is_some_and(|id| {
                        event.dependencies.iter().any(|dependency| {
                            dependency.first() == Some(id) && kind_is_place(tree, id)
                        })
                    })
            }) {
                affected.insert(event.address);
            }
        }
    }
    Some(affected)
}

fn top_level_kind<'a>(tree: &'a Value, path: &[String]) -> Option<&'a str> {
    let id = path.first()?;
    tree["objects"].get(id)?["kind"].as_str()
}

fn kind_is_place(tree: &Value, id: &str) -> bool {
    tree["objects"]
        .get(id)
        .and_then(|object| object["kind"].as_str())
        == Some("place")
}

fn enumerate_event_occurrences(tree: &Value) -> Option<Vec<EventOccurrence>> {
    let reading = Reading::of(tree)?;
    let objects = reading.objects;
    let mut out = Vec::new();
    for (place_id, place) in objects {
        if place["kind"] != "place" {
            continue;
        }
        let pattern = source_object_id(place["fields"].get("pattern")?)?;
        let count = positive_count(place)?;
        let length = typed_rational(&objects.get(pattern)?["fields"]["length"]).ok()?;
        for repetition in 0..count {
            let mut address = vec![place_id.clone(), repetition.to_string()];
            let dependencies = vec![vec![place_id.clone()]];
            let supplied = supplied_into(
                &reading,
                std::slice::from_ref(place_id),
                place,
                None,
                repetition,
                &length,
            )?;
            enumerate_pattern_occurrences(
                &reading,
                pattern,
                supplied.as_ref(),
                &mut address,
                &dependencies,
                &mut out,
                0,
            )?;
        }
        enumerate_insert_occurrences(place_id, place, &mut out)?;
    }
    Some(out)
}

fn enumerate_pattern_occurrences(
    reading: &Reading<'_>,
    pattern_id: &str,
    supplied: Option<&Supplied>,
    address: &mut Vec<String>,
    inherited_dependencies: &[Path],
    out: &mut Vec<EventOccurrence>,
    depth: usize,
) -> Option<()> {
    if depth >= 64 {
        return None;
    }
    let objects = reading.objects;
    let pattern = objects.get(pattern_id)?;
    if pattern["kind"] != "pattern" {
        return None;
    }
    let children = pattern["children"].as_object()?;
    // A phrase may shape any event its pattern expands (§9.3).
    let mut inherited_dependencies = inherited_dependencies.to_vec();
    inherited_dependencies.extend(
        children
            .iter()
            .filter(|(_, child)| child["kind"] == "phrase")
            .map(|(child_id, _)| vec![pattern_id.to_owned(), child_id.clone()]),
    );
    for (child_id, child) in children {
        let mut dependencies = inherited_dependencies.clone();
        dependencies.push(vec![pattern_id.to_owned()]);
        dependencies.push(vec![pattern_id.to_owned(), child_id.clone()]);
        // A note in succession moves with each note it follows (§9).
        if let Some((_, chain)) = child_onset(pattern, child) {
            dependencies.extend(chain.into_iter().map(|id| vec![pattern_id.to_owned(), id]));
        }
        match child["kind"].as_str()? {
            "note" | "hit" | "message" | "audio" => {
                address.push(child_id.clone());
                out.push(EventOccurrence {
                    address: address.join("/"),
                    dependencies,
                });
                address.pop();
            }
            "chord" => {
                let played = reading
                    .models
                    .played
                    .get([pattern_id.to_owned(), child_id.clone()].as_slice())?;
                match &played.source {
                    Source::Own => {}
                    Source::Voicing(id) => dependencies.push(vec![id.clone()]),
                    Source::Supplied => {
                        let supplied = supplied?;
                        dependencies.extend(supplied.dependencies().into_iter().map(|id| vec![id]));
                        let at = child_onset(pattern, child)?.0;
                        if let Some(Ok(given)) =
                            given_voicing(reading, child_id, Some(supplied), &at)
                        {
                            dependencies.push(vec![given.voicing.to_owned()]);
                        }
                    }
                }
                for member in 0..chord_pitches(reading, pattern_id, child_id, supplied)?.len() {
                    address.push(format!("{child_id}.{member}"));
                    out.push(EventOccurrence {
                        address: address.join("/"),
                        dependencies: dependencies.clone(),
                    });
                    address.pop();
                }
            }
            "use" => {
                let nested = source_object_id(child["fields"].get("pattern")?)?;
                let count = positive_count(child)?;
                let length = typed_rational(&objects.get(nested)?["fields"]["length"]).ok()?;
                for repetition in 0..count {
                    let nested_supplied = supplied_into(
                        reading,
                        &[pattern_id.to_owned(), child_id.clone()],
                        child,
                        supplied,
                        repetition,
                        &length,
                    )?;
                    address.push(child_id.clone());
                    address.push(repetition.to_string());
                    enumerate_pattern_occurrences(
                        reading,
                        nested,
                        nested_supplied.as_ref(),
                        address,
                        &dependencies,
                        out,
                        depth + 1,
                    )?;
                    address.pop();
                    address.pop();
                }
            }
            _ => {}
        }
    }
    Some(())
}

fn enumerate_insert_occurrences(
    place_id: &str,
    place: &Value,
    out: &mut Vec<EventOccurrence>,
) -> Option<()> {
    for (insert_id, insert) in place["children"].as_object()? {
        if insert["kind"] != "insert" {
            continue;
        }
        for (leaf_id, leaf) in insert["children"].as_object()? {
            if !matches!(
                leaf["kind"].as_str(),
                Some("note" | "hit" | "message" | "audio")
            ) {
                continue;
            }
            out.push(EventOccurrence {
                address: format!("{place_id}/{insert_id}/{leaf_id}"),
                dependencies: vec![
                    vec![place_id.to_owned()],
                    vec![place_id.to_owned(), insert_id.clone()],
                    vec![place_id.to_owned(), insert_id.clone(), leaf_id.clone()],
                ],
            });
        }
    }
    Some(())
}

#[cfg(test)]
mod plan_agreement;
