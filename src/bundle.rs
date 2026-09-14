//! Deterministic offline resolution for pinned local and embedded MaaC libraries.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::stdlib::{self, BuiltinSource};
use crate::syntax::{self, Document, Object};

pub const MAX_BUNDLE_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BUNDLE_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_BUNDLE_ASSET_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_BUNDLE_PATH_BYTES: usize = 4_096;
pub const MAX_BUNDLE_SOURCES: usize = 64;
pub const MAX_BUNDLE_ASSETS: usize = 64;
pub const MAX_IMPORT_DEPTH: usize = 32;
pub const MAX_SYNTAX_OBJECTS: usize = 200_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceBundle {
    pub entry: String,
    pub sources: BTreeMap<String, String>,
    pub assets: BTreeMap<String, Vec<u8>>,
}

impl SourceBundle {
    pub fn new(entry: impl Into<String>, source: impl Into<String>) -> Self {
        let entry = entry.into();
        Self {
            sources: BTreeMap::from([(entry.clone(), source.into())]),
            entry,
            assets: BTreeMap::new(),
        }
    }

    pub fn resolve(&self) -> Result<ResolvedBundle, Diagnostics> {
        self.preflight()?;

        let mut parsed = BTreeMap::new();
        let mut object_count = 0usize;
        for (path, source) in &self.sources {
            let document = syntax::parse(source)
                .map_err(|diagnostics| contextualize(diagnostics, &format!("source `{path}`")))?;
            object_count = object_count
                .checked_add(count_document_objects(&document))
                .ok_or_else(|| resource_error("aggregate syntax object count overflowed"))?;
            if object_count > MAX_SYNTAX_OBJECTS {
                return Err(resource_error(format!(
                    "bundle contains more than {MAX_SYNTAX_OBJECTS} syntax objects"
                )));
            }
            parsed.insert(path.clone(), document);
        }

        let entry = normalize_bundle_key(&self.entry, "entry source path")?;
        if !self.sources.contains_key(&entry) {
            return Err(reference_error(format!(
                "entry source `{entry}` is missing from the bundle"
            )));
        }

        // Embedded sources share the ordinary graph and resource accounting,
        // but can only enter through a registry-backed import declaration.
        let mut sources: BTreeMap<String, &str> = self
            .sources
            .iter()
            .map(|(path, source)| (path.clone(), source.as_str()))
            .collect();
        let mut source_bytes: usize = sources.values().map(|source| source.len()).sum();
        let mut pending = vec![entry.clone()];
        let mut discovered = BTreeSet::new();
        while let Some(path) = pending.pop() {
            if !discovered.insert(path.clone()) {
                continue;
            }
            let Some(document) = parsed.get(&path) else {
                continue; // Graph validation supplies the missing-reference context.
            };
            for import in discover_document_references(&path, document)?.imports {
                let target = import.target_path(&path)?;
                if let ImportReference::Builtin { source, .. } = import {
                    if !sources.contains_key(source.path) {
                        if sources.len() >= MAX_BUNDLE_SOURCES {
                            return Err(resource_error(format!(
                                "bundle contains more than {MAX_BUNDLE_SOURCES} source files"
                            )));
                        }
                        if source.source.len() > MAX_BUNDLE_FILE_BYTES {
                            return Err(resource_error(format!(
                                "source `{}` exceeds {MAX_BUNDLE_FILE_BYTES} bytes",
                                source.path
                            )));
                        }
                        source_bytes =
                            source_bytes
                                .checked_add(source.source.len())
                                .ok_or_else(|| {
                                    resource_error("aggregate source byte count overflowed")
                                })?;
                        if source_bytes > MAX_BUNDLE_SOURCE_BYTES {
                            return Err(resource_error(format!(
                                "bundle source bytes exceed {MAX_BUNDLE_SOURCE_BYTES}"
                            )));
                        }
                        let document = syntax::parse(source.source).map_err(|diagnostics| {
                            contextualize(diagnostics, &format!("source `{}`", source.path))
                        })?;
                        object_count = object_count
                            .checked_add(count_document_objects(&document))
                            .ok_or_else(|| {
                                resource_error("aggregate syntax object count overflowed")
                            })?;
                        if object_count > MAX_SYNTAX_OBJECTS {
                            return Err(resource_error(format!(
                                "bundle contains more than {MAX_SYNTAX_OBJECTS} syntax objects"
                            )));
                        }
                        sources.insert(source.path.to_owned(), source.source);
                        parsed.insert(source.path.to_owned(), document);
                    }
                }
                pending.push(target);
            }
        }

        // Validate graph shape before checking content pins. A hash-pinned
        // cycle cannot be constructed without finding a digest fixed point,
        // but it is still invalid input and must receive a cycle diagnostic
        // rather than being hidden behind the first hash mismatch.
        validate_import_graph(&entry, &parsed)?;

        let mut resolver = Resolver {
            bundle: self,
            sources: &sources,
            parsed: &parsed,
            states: BTreeMap::new(),
            stack: Vec::new(),
            documents: BTreeMap::new(),
            imports: BTreeMap::new(),
            resolved_assets: BTreeMap::new(),
            dependencies: Vec::new(),
            source_files: Vec::new(),
        };
        resolver.visit(&entry, 0)?;

        Ok(ResolvedBundle {
            entry,
            documents: resolver.documents,
            imports: resolver.imports,
            assets: resolver.resolved_assets,
            dependencies: resolver.dependencies,
            source_files: resolver.source_files,
        })
    }

    fn preflight(&self) -> Result<(), Diagnostics> {
        if self.sources.len() > MAX_BUNDLE_SOURCES {
            return Err(resource_error(format!(
                "bundle contains more than {MAX_BUNDLE_SOURCES} source files"
            )));
        }
        if self.assets.len() > MAX_BUNDLE_ASSETS {
            return Err(resource_error(format!(
                "bundle contains more than {MAX_BUNDLE_ASSETS} assets"
            )));
        }

        normalize_bundle_key(&self.entry, "entry source path")?;
        reject_reserved_path(&self.entry)?;
        let mut source_bytes = 0usize;
        for (path, source) in &self.sources {
            normalize_bundle_key(path, "source path")?;
            reject_reserved_path(path)?;
            if source.len() > MAX_BUNDLE_FILE_BYTES {
                return Err(resource_error(format!(
                    "source `{path}` exceeds {MAX_BUNDLE_FILE_BYTES} bytes"
                )));
            }
            source_bytes = source_bytes
                .checked_add(source.len())
                .ok_or_else(|| resource_error("aggregate source byte count overflowed"))?;
        }
        if source_bytes > MAX_BUNDLE_SOURCE_BYTES {
            return Err(resource_error(format!(
                "bundle source bytes exceed {MAX_BUNDLE_SOURCE_BYTES}"
            )));
        }

        let mut asset_bytes = 0usize;
        for (path, bytes) in &self.assets {
            normalize_bundle_key(path, "asset path")?;
            reject_reserved_path(path)?;
            if bytes.len() > MAX_BUNDLE_FILE_BYTES {
                return Err(resource_error(format!(
                    "asset `{path}` exceeds {MAX_BUNDLE_FILE_BYTES} bytes"
                )));
            }
            asset_bytes = asset_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| resource_error("aggregate asset byte count overflowed"))?;
        }
        if asset_bytes > MAX_BUNDLE_ASSET_BYTES {
            return Err(resource_error(format!(
                "bundle asset bytes exceed {MAX_BUNDLE_ASSET_BYTES}"
            )));
        }
        Ok(())
    }

    pub(crate) fn snapshot_resolved_sources(
        &self,
        resolved: &ResolvedBundle,
    ) -> Result<BTreeMap<String, ResolvedSourceSnapshot>, Diagnostics> {
        let mut snapshot = BTreeMap::new();
        for identity in &resolved.source_files {
            let (source, builtin) = match self.sources.get(&identity.path) {
                Some(source) => (source.clone(), false),
                None => {
                    let builtin = stdlib::lookup_path(&identity.path).ok_or_else(|| {
                        reference_error(format!(
                            "resolved source `{}` has no source snapshot",
                            identity.path
                        ))
                    })?;
                    (builtin.source.to_owned(), true)
                }
            };
            let actual = sha256_digest(source.as_bytes());
            if actual != identity.hash {
                return Err(hash_error(format!(
                    "resolved source `{}` expected `{}`, but its snapshot has `{actual}`",
                    identity.path, identity.hash
                )));
            }
            snapshot.insert(
                identity.path.clone(),
                ResolvedSourceSnapshot {
                    hash: identity.hash.clone(),
                    source,
                    builtin,
                },
            );
        }
        Ok(snapshot)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedSourceSnapshot {
    pub hash: String,
    pub source: String,
    pub builtin: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedBundle {
    pub entry: String,
    pub documents: BTreeMap<String, Document>,
    pub imports: BTreeMap<String, BTreeMap<String, String>>,
    pub assets: BTreeMap<String, Vec<u8>>,
    pub dependencies: Vec<DependencyIdentity>,
    pub source_files: Vec<SourceIdentity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub path: String,
    pub hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyIdentity {
    pub source: String,
    pub alias: String,
    pub path: String,
    pub hash: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PinnedReference {
    pub base: ReferenceBase,
    pub alias: String,
    pub path: String,
    pub hash: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ReferenceBase {
    DeclaringSource,
    PackageRoot,
}

impl PinnedReference {
    pub(crate) fn target_path(&self, declaring_source: &str) -> Result<String, Diagnostics> {
        match self.base {
            ReferenceBase::DeclaringSource => {
                normalize_file_reference(declaring_source, &self.path)
            }
            ReferenceBase::PackageRoot => normalize_file_reference("package.maac", &self.path),
        }
    }
}

pub(crate) struct DocumentReferences {
    pub imports: Vec<ImportReference>,
    pub assets: Vec<PinnedReference>,
}

pub(crate) enum ImportReference {
    Local(PinnedReference),
    Builtin {
        alias: String,
        source: BuiltinSource,
    },
}

impl ImportReference {
    fn target_path(&self, declaring_source: &str) -> Result<String, Diagnostics> {
        match self {
            Self::Local(reference) => reference.target_path(declaring_source),
            Self::Builtin { source, .. } => Ok(source.path.to_owned()),
        }
    }

    fn pin(self, declaring_source: &str) -> Result<PinnedReference, Diagnostics> {
        let path = self.target_path(declaring_source)?;
        match self {
            Self::Local(reference) => {
                validate_hash_pin(&reference.hash, "import hash")?;
                Ok(PinnedReference { path, ..reference })
            }
            Self::Builtin { alias, source } => Ok(PinnedReference {
                base: ReferenceBase::PackageRoot,
                alias,
                path,
                hash: sha256_digest(source.source.as_bytes()),
            }),
        }
    }
}

struct Resolver<'a> {
    bundle: &'a SourceBundle,
    sources: &'a BTreeMap<String, &'a str>,
    parsed: &'a BTreeMap<String, Document>,
    states: BTreeMap<String, VisitState>,
    stack: Vec<String>,
    documents: BTreeMap<String, Document>,
    imports: BTreeMap<String, BTreeMap<String, String>>,
    resolved_assets: BTreeMap<String, Vec<u8>>,
    dependencies: Vec<DependencyIdentity>,
    source_files: Vec<SourceIdentity>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum VisitState {
    Visiting,
    Complete,
}

fn validate_import_graph(
    entry: &str,
    parsed: &BTreeMap<String, Document>,
) -> Result<(), Diagnostics> {
    fn visit(
        path: &str,
        depth: usize,
        parsed: &BTreeMap<String, Document>,
        states: &mut BTreeMap<String, VisitState>,
        greatest_depth: &mut BTreeMap<String, usize>,
        stack: &mut Vec<String>,
    ) -> Result<(), Diagnostics> {
        if depth > MAX_IMPORT_DEPTH {
            return Err(resource_error(format!(
                "import depth exceeds {MAX_IMPORT_DEPTH} at `{path}`"
            )));
        }
        match states.get(path) {
            Some(VisitState::Complete)
                if greatest_depth.get(path).is_some_and(|seen| *seen >= depth) =>
            {
                return Ok(())
            }
            Some(VisitState::Visiting) => {
                let start = stack.iter().position(|item| item == path).unwrap_or(0);
                let mut cycle = stack[start..].to_vec();
                cycle.push(path.to_owned());
                return Err(reference_error(format!(
                    "import cycle detected: {}",
                    cycle.join(" -> ")
                )));
            }
            Some(VisitState::Complete) | None => {}
        }

        let document = parsed.get(path).ok_or_else(|| {
            reference_error(format!(
                "imported source `{path}` is missing from the bundle"
            ))
        })?;
        states.insert(path.to_owned(), VisitState::Visiting);
        stack.push(path.to_owned());
        for import in discover_document_references(path, document)?.imports {
            let import = import.pin(path)?;
            let target = import.path;
            if !parsed.contains_key(&target) {
                return Err(reference_error(format!(
                    "import `{}` in `{path}` refers to missing source `{target}`",
                    import.alias
                )));
            }
            visit(&target, depth + 1, parsed, states, greatest_depth, stack)?;
        }
        stack.pop();
        states.insert(path.to_owned(), VisitState::Complete);
        greatest_depth.insert(path.to_owned(), depth);
        Ok(())
    }

    visit(
        entry,
        0,
        parsed,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        &mut Vec::new(),
    )
}

impl Resolver<'_> {
    fn visit(&mut self, path: &str, depth: usize) -> Result<(), Diagnostics> {
        if depth > MAX_IMPORT_DEPTH {
            return Err(resource_error(format!(
                "import depth exceeds {MAX_IMPORT_DEPTH} at `{path}`"
            )));
        }
        match self.states.get(path) {
            Some(VisitState::Complete) => return Ok(()),
            Some(VisitState::Visiting) => {
                let start = self.stack.iter().position(|item| item == path).unwrap_or(0);
                let mut cycle = self.stack[start..].to_vec();
                cycle.push(path.to_owned());
                return Err(reference_error(format!(
                    "import cycle detected: {}",
                    cycle.join(" -> ")
                )));
            }
            None => {}
        }

        let document = self.parsed.get(path).ok_or_else(|| {
            reference_error(format!(
                "imported source `{path}` is missing from the bundle"
            ))
        })?;
        let references = discover_document_references(path, document)?;
        self.states.insert(path.to_owned(), VisitState::Visiting);
        self.stack.push(path.to_owned());
        self.documents.insert(path.to_owned(), document.clone());
        self.source_files.push(SourceIdentity {
            path: path.to_owned(),
            hash: sha256_digest(self.sources[path].as_bytes()),
        });

        let mut aliases = BTreeMap::new();
        for import in references.imports {
            let import = import.pin(path)?;
            let target = import.path;
            let source = self.sources.get(&target).ok_or_else(|| {
                reference_error(format!(
                    "import `{}` in `{path}` refers to missing source `{target}`",
                    import.alias
                ))
            })?;
            let actual = sha256_digest(source.as_bytes());
            if actual != import.hash {
                return Err(hash_error(format!(
                    "import `{}` in `{path}` expected `{}`, but `{target}` has `{actual}`",
                    import.alias, import.hash
                )));
            }
            aliases.insert(import.alias.clone(), target.clone());
            self.dependencies.push(DependencyIdentity {
                source: path.to_owned(),
                alias: import.alias,
                path: target.clone(),
                hash: import.hash,
            });
            self.visit(&target, depth + 1)?;
        }

        for asset in references.assets {
            let target = asset.target_path(path)?;
            validate_hash_pin(&asset.hash, "asset hash")?;
            let bytes = self.bundle.assets.get(&target).ok_or_else(|| {
                asset_error(format!(
                    "{} `{}` in `{path}` refers to missing asset `{target}`",
                    asset.alias.split(':').next().unwrap_or("asset"),
                    asset.alias.split(':').nth(1).unwrap_or(&asset.alias)
                ))
            })?;
            let actual = sha256_digest(bytes);
            if actual != asset.hash {
                return Err(hash_error(format!(
                    "asset `{}` in `{path}` expected `{}`, but `{target}` has `{actual}`",
                    asset.alias, asset.hash
                )));
            }
            self.resolved_assets.insert(target, bytes.clone());
        }

        self.imports.insert(path.to_owned(), aliases);
        self.stack.pop();
        self.states.insert(path.to_owned(), VisitState::Complete);
        Ok(())
    }
}

pub(crate) fn discover_document_references(
    source_path: &str,
    document: &Document,
) -> Result<DocumentReferences, Diagnostics> {
    let mut imports = Vec::new();
    let mut assets = Vec::new();
    for object in document.objects.values() {
        match object.kind.as_str() {
            "import" => imports.push(exact_import_reference(source_path, object)?),
            "wavetable" => {
                assets.push(asset_reference(source_path, object)?);
            }
            "asset"
                if object
                    .field("kind")
                    .and_then(|field| field.value.as_symbol())
                    == Some("audio") =>
            {
                let mut reference = asset_reference(source_path, object)?;
                reference.base = ReferenceBase::PackageRoot;
                assets.push(reference);
            }
            _ => {}
        }
    }
    // Only the descriptor explicitly referenced by a recognized production
    // extension is an executable dependency. Other non-audio core assets remain for the
    // ordinary semantic validator to reject, without loading their bytes.
    for extension in document
        .objects
        .values()
        .filter(|object| object.kind == "extension")
    {
        if extension
            .field("namespace")
            .and_then(|field| field.value.as_string())
            != Some(crate::production_data::CAPABILITY)
        {
            continue;
        }
        let reference = extension
            .field("schema")
            .and_then(|field| field.value.reference())
            .ok_or_else(|| reference_error("production schema requires a descriptor reference"))?;
        if reference.path.len() != 1 || reference.port.is_some() {
            return Err(reference_error(
                "production schema must reference a top-level descriptor",
            ));
        }
        let descriptor = document
            .objects
            .get(&reference.path[0])
            .ok_or_else(|| reference_error("production schema descriptor does not exist"))?;
        if descriptor.kind != "asset"
            || descriptor
                .field("kind")
                .and_then(|field| field.value.as_symbol())
                != Some("descriptor")
        {
            return Err(asset_error(
                "production schema must reference a descriptor asset",
            ));
        }
        let mut reference = pinned_fields(source_path, descriptor, DiagnosticCode::Asset)?;
        reference.base = ReferenceBase::PackageRoot;
        assets.push(reference);
    }
    Ok(DocumentReferences { imports, assets })
}

fn exact_import_reference(
    source_path: &str,
    object: &Object,
) -> Result<ImportReference, Diagnostics> {
    let fields: BTreeSet<&str> = object.fields.keys().map(String::as_str).collect();
    let expected = BTreeSet::from(["hash", "path"]);
    let builtin = BTreeSet::from(["builtin"]);
    if (fields != expected && fields != builtin) || !object.children.is_empty() {
        return Err(reference_error(format!(
            "import `{}` in `{source_path}` must contain exactly string fields `path` and `hash`, or only string field `builtin`, and no children",
            object.id
        )));
    }
    if fields == builtin {
        let id = object
            .field("builtin")
            .and_then(|field| field.value.as_string())
            .ok_or_else(|| {
                reference_error(format!(
                    "import `{}` in `{source_path}` requires a string `builtin` field",
                    object.id
                ))
            })?;
        validate_path_length(id, "built-in identifier")?;
        let source = stdlib::lookup(id).ok_or_else(|| {
            reference_error(format!(
                "import `{}` in `{source_path}` refers to unknown built-in library `{id}`",
                object.id
            ))
        })?;
        Ok(ImportReference::Builtin {
            alias: object.id.clone(),
            source,
        })
    } else {
        pinned_fields(source_path, object, DiagnosticCode::Reference).map(ImportReference::Local)
    }
}

fn asset_reference(source_path: &str, object: &Object) -> Result<PinnedReference, Diagnostics> {
    pinned_fields(source_path, object, DiagnosticCode::Asset)
}

fn pinned_fields(
    source_path: &str,
    object: &Object,
    code: DiagnosticCode,
) -> Result<PinnedReference, Diagnostics> {
    let path = object
        .field("path")
        .and_then(|field| field.value.as_string())
        .ok_or_else(|| {
            one_error(
                code,
                format!(
                    "{} `{}` in `{source_path}` requires a string `path` field",
                    object.kind, object.id
                ),
            )
        })?;
    validate_path_length(
        path,
        if object.kind == "import" {
            "import path"
        } else {
            "wavetable path"
        },
    )?;
    let hash = object
        .field("hash")
        .and_then(|field| field.value.as_string())
        .ok_or_else(|| {
            one_error(
                code,
                format!(
                    "{} `{}` in `{source_path}` requires a string `hash` field",
                    object.kind, object.id
                ),
            )
        })?;
    Ok(PinnedReference {
        base: ReferenceBase::DeclaringSource,
        alias: if object.kind == "import" {
            object.id.clone()
        } else {
            format!("{}:{}", object.kind, object.id)
        },
        path: path.to_owned(),
        hash: hash.to_owned(),
    })
}

/// Normalize a file reference relative to the source that declares it.
///
/// Both inputs use project-relative POSIX spelling. The result never contains
/// empty, `.` or `..` components and never escapes the project root.
pub fn normalize_file_reference(
    declaring_source: &str,
    reference: &str,
) -> Result<String, Diagnostics> {
    validate_path_length(reference, "file reference")?;
    let base = normalize_bundle_key(declaring_source, "declaring source path")?;
    validate_reference_spelling(reference)?;
    let mut components: Vec<&str> = base.split('/').collect();
    components.pop();
    for component in reference.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err(reference_error(format!(
                        "file reference `{reference}` from `{declaring_source}` escapes the project root"
                    )));
                }
            }
            value => components.push(value),
        }
    }
    if components.is_empty() {
        return Err(reference_error(format!(
            "file reference `{reference}` from `{declaring_source}` does not name a file"
        )));
    }
    let normalized_len = components.iter().try_fold(0usize, |length, component| {
        length
            .checked_add(usize::from(length != 0))
            .and_then(|length| length.checked_add(component.len()))
    });
    if normalized_len.is_none_or(|length| length > MAX_BUNDLE_PATH_BYTES) {
        return Err(resource_error(format!(
            "normalized file reference exceeds {MAX_BUNDLE_PATH_BYTES} bytes"
        )));
    }
    let normalized = components.join("/");
    reject_reserved_path(&normalized)?;
    Ok(normalized)
}

fn reject_reserved_path(path: &str) -> Result<(), Diagnostics> {
    if path == "@builtin" || path.starts_with("@builtin/") {
        Err(reference_error(format!(
            "path `{path}` uses the reserved `@builtin/` namespace"
        )))
    } else {
        Ok(())
    }
}

fn normalize_bundle_key(path: &str, label: &str) -> Result<String, Diagnostics> {
    validate_path_length(path, label)?;
    validate_reference_spelling(path)?;
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err(reference_error(format!(
                        "{label} `{path}` escapes the project root"
                    )));
                }
            }
            value => components.push(value),
        }
    }
    if components.is_empty() {
        return Err(reference_error(format!("{label} must name a file")));
    }
    let normalized = components.join("/");
    if normalized != path {
        return Err(reference_error(format!(
            "{label} `{path}` is not normalized; use `{normalized}`"
        )));
    }
    Ok(normalized)
}

fn validate_path_length(path: &str, label: &str) -> Result<(), Diagnostics> {
    if path.len() > MAX_BUNDLE_PATH_BYTES {
        return Err(resource_error(format!(
            "{label} exceeds {MAX_BUNDLE_PATH_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_reference_spelling(path: &str) -> Result<(), Diagnostics> {
    if path.is_empty() {
        return Err(reference_error("file path cannot be empty"));
    }
    if path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
        || path.contains('\\')
        || path.contains('\0')
    {
        return Err(reference_error(format!(
            "file path `{path}` must be project-relative POSIX syntax"
        )));
    }
    let first = path.split('/').next().unwrap_or_default().as_bytes();
    if first.len() >= 2 && first[0].is_ascii_alphabetic() && first[1] == b':' {
        return Err(reference_error(format!(
            "file path `{path}` must not use a platform path prefix"
        )));
    }
    Ok(())
}

pub(crate) fn validate_hash_pin(pin: &str, label: &str) -> Result<(), Diagnostics> {
    let valid = pin.len() == 71
        && pin.starts_with("sha256:")
        && pin[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if valid {
        Ok(())
    } else {
        Err(hash_error(format!(
            "{label} must be `sha256:` followed by 64 lowercase hexadecimal digits"
        )))
    }
}

pub(crate) fn count_document_objects(document: &Document) -> usize {
    fn count(object: &Object) -> usize {
        1 + object.children.values().map(count).sum::<usize>()
    }
    document.objects.values().map(count).sum()
}

fn contextualize(diagnostics: Diagnostics, context: &str) -> Diagnostics {
    let mut result = Diagnostics::new();
    for mut diagnostic in diagnostics {
        diagnostic.message = format!("{context}: {}", diagnostic.message);
        result.push(diagnostic);
    }
    result
}

fn one_error(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}

fn reference_error(message: impl Into<String>) -> Diagnostics {
    one_error(DiagnosticCode::Reference, message)
}

fn asset_error(message: impl Into<String>) -> Diagnostics {
    one_error(DiagnosticCode::Asset, message)
}

fn hash_error(message: impl Into<String>) -> Diagnostics {
    one_error(DiagnosticCode::Hash, message)
}

fn resource_error(message: impl Into<String>) -> Diagnostics {
    one_error(DiagnosticCode::ResourceLimit, message)
}

/// Return the canonical lowercase SHA-256 identity of `bytes`.
pub fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
