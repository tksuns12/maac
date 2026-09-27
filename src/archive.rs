//! Bounded editable composition snapshots and their exact source/media closure.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cap_std::fs::OpenOptions as CapOpenOptions;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::archive_processors::NativeProcessorContext;
use crate::bundle::{
    sha256_digest, MAX_BUNDLE_ASSETS, MAX_BUNDLE_FILE_BYTES, MAX_BUNDLE_PATH_BYTES,
    MAX_BUNDLE_SOURCES,
};
use crate::bundle_fs::ProjectRoot;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::disk_media::{DiskAsset, DiskMediaProject, MAX_DISK_MEDIA_FILE_BYTES};
use crate::media_import::{self, ImportedWav, RetainedImportPreflight};
use crate::plan::PlanLimits;

pub(crate) const MANIFEST_PATH: &str = "maac-archive.json";
const FORMAT: &str = "maac.editable-archive";
const SNAPSHOT_FORMAT: &str = "maac.archive-snapshot";
const VERSION: u32 = 1;
const MULTI_IMPORT_VERSION: u32 = 2;
const PROCESSOR_CONTEXT_VERSION: u32 = 3;
const PROCESSOR_CONTEXT_PATH: &str = "maac-processors.json";
const MAX_PROCESSOR_CONTEXT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RETAINED_IMPORTS: usize = 16;
const MAX_ORIGINAL_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_TREE_ENTRIES: usize =
    (MAX_BUNDLE_SOURCES + MAX_BUNDLE_ASSETS) * MAX_BUNDLE_PATH_BYTES + 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    entry: String,
    profile: String,
    members: Vec<Member>,
    builtins: Vec<Builtin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import_manifest: Option<Sidecar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_wav: Option<Sidecar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    imports: Vec<ImportRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    processor_context: Option<Sidecar>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sidecar {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRecord {
    directory: String,
    import_manifest: Sidecar,
    original_wav: Sidecar,
    pcm_member: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MemberKind {
    Source,
    Asset,
    Pcm,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    path: String,
    kind: MemberKind,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Builtin {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone)]
enum MemberData {
    Bytes(Vec<u8>),
    Pcm(Arc<DiskAsset>),
}

/// Private, bounded snapshot of the current composition closure.
///
/// All unpack writes come from these captured bytes and private PCM handles.
/// History, freezes, and retained import sidecars are layered around this closure.
#[derive(Clone)]
pub(crate) struct ArchiveSnapshot {
    manifest: Manifest,
    manifest_json: Vec<u8>,
    data: BTreeMap<String, MemberData>,
    retained: Option<RetainedPayload>,
    imports: Vec<ImportPayload>,
    processor_context: Option<Vec<u8>>,
}

#[derive(Clone)]
struct RetainedPayload {
    import_json: Vec<u8>,
    imported: ImportedWav,
}

#[derive(Clone)]
struct ImportPayload {
    directory: String,
    import_json: Vec<u8>,
    imported: ImportedWav,
}

/// Strict snapshot metadata checked before any checkpoint media is loaded.
pub(crate) struct ArchivePreflight {
    manifest: Manifest,
    json: Vec<u8>,
    retained: Option<RetainedImportPreflight>,
    imports: Vec<(String, RetainedImportPreflight)>,
    processor_context: Option<Vec<u8>>,
}

impl ArchivePreflight {
    pub(crate) fn entry(&self) -> &str {
        &self.manifest.entry
    }

    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.json)
    }

    pub(crate) fn resource_bytes(&self) -> (u64, u64, u64, u64) {
        let (source, asset, pcm) = resource_bytes(&self.manifest);
        let original = original_bytes(&self.manifest);
        (source, asset, pcm, original)
    }

    pub(crate) fn is_retained(&self) -> bool {
        self.retained.is_some() || !self.imports.is_empty()
    }

    pub(crate) fn is_multi_import(&self) -> bool {
        !self.imports.is_empty()
    }

    pub(crate) fn has_processor_context(&self) -> bool {
        self.processor_context.is_some()
    }
}

impl ArchiveSnapshot {
    pub(crate) fn capture(entry: &Path, root: &Path, profile: &str) -> Result<Self, Diagnostics> {
        let root = ProjectRoot::open_pinned(root, cap_std::ambient_authority())?;
        Self::capture_in_root(entry, &root, profile, true, &[], false)
    }

    /// Capture the compiled composition without import provenance. Freeze
    /// checks use this only after a previously selected import no longer
    /// matches, so a valid edited composition can be reported as stale.
    pub(crate) fn capture_closure_only(
        entry: &Path,
        root: &Path,
        profile: &str,
    ) -> Result<Self, Diagnostics> {
        let root = ProjectRoot::open_pinned(root, cap_std::ambient_authority())?;
        Self::capture_in_root(entry, &root, profile, false, &[], false)
    }

    pub(crate) fn capture_with_imports(
        entry: &Path,
        root: &Path,
        profile: &str,
        selections: &[PathBuf],
    ) -> Result<Self, Diagnostics> {
        if selections.is_empty() {
            return Self::capture(entry, root, profile);
        }
        let selected = normalize_selections(selections)?;
        let root = ProjectRoot::open_pinned(root, cap_std::ambient_authority())?;
        Self::capture_in_root(entry, &root, profile, false, &selected, false)
    }

    pub(crate) fn capture_with_options(
        entry: &Path,
        root: &Path,
        profile: &str,
        retain_imports: &[PathBuf],
        processor_context: bool,
    ) -> Result<Self, Diagnostics> {
        if !processor_context {
            return Self::capture_with_imports(entry, root, profile, retain_imports);
        }
        let selected = normalize_selections(retain_imports)?;
        let root = ProjectRoot::open_pinned(root, cap_std::ambient_authority())?;
        Self::capture_in_root(
            entry,
            &root,
            profile,
            selected.is_empty(),
            &selected,
            processor_context,
        )
    }

    pub(crate) fn capture_matching_layout(
        entry: &Path,
        root: &Path,
        reference: &Self,
    ) -> Result<Self, Diagnostics> {
        let root = ProjectRoot::open_pinned(root, cap_std::ambient_authority())?;
        let selections: Vec<_> = reference
            .manifest
            .imports
            .iter()
            .map(|record| record.directory.clone())
            .collect();
        Self::capture_in_root(
            entry,
            &root,
            reference.profile(),
            reference.retained.is_some(),
            &selections,
            reference.has_processor_context(),
        )
    }

    fn capture_in_root(
        entry: &Path,
        root: &ProjectRoot,
        profile: &str,
        detect_import: bool,
        selected_dirs: &[String],
        include_processor_context: bool,
    ) -> Result<Self, Diagnostics> {
        let limits = limits(profile)?;
        let selected_preflights = preflight_selected(root, selected_dirs)?;
        let project = DiskMediaProject::load_in_root(entry, root)?;
        let plan = project.build_with_limits(&limits)?;
        let (processor_context, processor_context_sidecar) = if include_processor_context {
            let context = NativeProcessorContext::capture(&plan)?;
            let sidecar = Sidecar {
                path: PROCESSOR_CONTEXT_PATH.into(),
                bytes: context.bytes_len() as u64,
                sha256: context.digest(),
            };
            (Some(context.bytes().to_vec()), Some(sidecar))
        } else {
            (None, None)
        };
        let bundle = project.bundle();
        let resolved = bundle.resolve_with_disk_assets(project.disk_assets())?;
        let snapshots = bundle.snapshot_resolved_sources(&resolved)?;

        let mut builtins = Vec::new();
        for (path, snapshot) in snapshots {
            if snapshot.builtin {
                builtins.push(Builtin {
                    path,
                    bytes: snapshot.source.len() as u64,
                    sha256: snapshot.hash,
                });
            }
        }

        let mut data = BTreeMap::new();
        let mut members = Vec::new();
        for (path, source) in &bundle.sources {
            add_bytes(
                &mut data,
                &mut members,
                path,
                MemberKind::Source,
                source.as_bytes(),
            )?;
        }
        for (path, bytes) in &bundle.assets {
            add_bytes(&mut data, &mut members, path, MemberKind::Asset, bytes)?;
        }
        for (path, snapshot) in project.disk_assets() {
            check_path(path)?;
            if data
                .insert(path.clone(), MemberData::Pcm(Arc::clone(snapshot)))
                .is_some()
            {
                return Err(fail(
                    DiagnosticCode::Reference,
                    format!("duplicate archive member `{path}`"),
                ));
            }
            members.push(Member {
                path: path.clone(),
                kind: MemberKind::Pcm,
                bytes: snapshot.bytes,
                sha256: snapshot.hash.clone(),
            });
        }
        members.sort_by(|a, b| a.path.cmp(&b.path));
        let retained = if detect_import {
            capture_retained(root, &bundle.entry, &data)?
        } else {
            None
        };
        let (imports, import_records) = capture_selected(root, &selected_preflights, &data)?;
        let (import_manifest, original_wav) = if let Some(payload) = &retained {
            (
                Some(Sidecar {
                    path: "import.json".into(),
                    bytes: payload.import_json.len() as u64,
                    sha256: sha256_digest(&payload.import_json),
                }),
                Some(Sidecar {
                    path: "original.wav".into(),
                    bytes: payload.imported.input_bytes(),
                    sha256: payload.imported.source_hash().into(),
                }),
            )
        } else {
            (None, None)
        };
        let manifest = Manifest {
            format: if retained.is_some() || !imports.is_empty() || include_processor_context {
                SNAPSHOT_FORMAT
            } else {
                FORMAT
            }
            .into(),
            version: if include_processor_context {
                PROCESSOR_CONTEXT_VERSION
            } else if imports.is_empty() {
                VERSION
            } else {
                MULTI_IMPORT_VERSION
            },
            entry: bundle.entry.clone(),
            profile: profile.into(),
            members,
            builtins,
            import_manifest,
            original_wav,
            imports: import_records,
            processor_context: processor_context_sidecar,
        };
        validate_manifest(&manifest)?;
        let manifest_json = canonical_json(&manifest)?;
        if manifest_json.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive manifest exceeds 1 MiB",
            ));
        }
        Ok(Self {
            manifest,
            manifest_json,
            data,
            retained,
            imports,
            processor_context,
        })
    }

    /// Verify a directory as exactly one current-closure archive, then retain
    /// its own snapshots for later unpack without reopening archive members.
    #[cfg(test)]
    pub(crate) fn verify(dir: &Path) -> Result<Self, Diagnostics> {
        let root_meta = fs::symlink_metadata(dir).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect archive directory: {e}"),
            )
        })?;
        if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
            return Err(fail(
                DiagnosticCode::Reference,
                "archive root must be a real directory",
            ));
        }
        let root = ProjectRoot::open_pinned(dir, cap_std::ambient_authority())?;
        Self::verify_in_root(&root, || {})
    }

    #[cfg(test)]
    fn verify_in_root(
        root: &ProjectRoot,
        before_capture: impl FnOnce(),
    ) -> Result<Self, Diagnostics> {
        let preflight = Self::preflight_in_root(root)?;
        Self::verify_preflighted_in_root(root, &preflight, before_capture)
    }

    pub(crate) fn preflight_in_root(root: &ProjectRoot) -> Result<ArchivePreflight, Diagnostics> {
        let bytes = bounded_manifest(root)?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("invalid archive manifest: {e}"),
            )
        })?;
        validate_manifest(&manifest)?;
        if canonical_json(&manifest)? != bytes {
            return Err(fail(
                DiagnosticCode::Syntax,
                "archive manifest is not canonical JSON",
            ));
        }
        let retained = if let (Some(import_manifest), Some(original_wav)) =
            (&manifest.import_manifest, &manifest.original_wav)
        {
            let preflight =
                media_import::preflight_retained_import_in_root(root).map_err(import_failure)?;
            if preflight.json().len() as u64 != import_manifest.bytes
                || sha256_digest(preflight.json()) != import_manifest.sha256
                || preflight.original_bytes() != original_wav.bytes
                || preflight.original_hash() != original_wav.sha256
            {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "retained import sidecar identity differs from snapshot manifest",
                ));
            }
            Some(preflight)
        } else {
            None
        };
        let selected_dirs: Vec<_> = manifest
            .imports
            .iter()
            .map(|record| record.directory.clone())
            .collect();
        let imports = preflight_selected(root, &selected_dirs)?;
        for ((_, preflight), record) in imports.iter().zip(&manifest.imports) {
            if preflight.json().len() as u64 != record.import_manifest.bytes
                || sha256_digest(preflight.json()) != record.import_manifest.sha256
                || preflight.original_bytes() != record.original_wav.bytes
                || preflight.original_hash() != record.original_wav.sha256
            {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "retained import sidecar identity differs from snapshot manifest",
                ));
            }
        }
        let processor_context = if let Some(sidecar) = &manifest.processor_context {
            let bytes = bounded_processor_context(root)?;
            NativeProcessorContext::from_bytes(&bytes)?;
            if bytes.len() as u64 != sidecar.bytes || sha256_digest(&bytes) != sidecar.sha256 {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "processor context sidecar identity differs from snapshot manifest",
                ));
            }
            Some(bytes)
        } else {
            None
        };
        Ok(ArchivePreflight {
            manifest,
            json: bytes,
            retained,
            imports,
            processor_context,
        })
    }

    pub(crate) fn verify_preflighted_in_root(
        root: &ProjectRoot,
        preflight: &ArchivePreflight,
        before_capture: impl FnOnce(),
    ) -> Result<Self, Diagnostics> {
        check_tree(root, &preflight.manifest)?;
        before_capture();
        let captured = Self::capture_in_root(
            Path::new(&preflight.manifest.entry),
            root,
            &preflight.manifest.profile,
            preflight.retained.is_some(),
            &preflight
                .imports
                .iter()
                .map(|(directory, _)| directory.clone())
                .collect::<Vec<_>>(),
            preflight.processor_context.is_some(),
        )?;
        if let Some(expected) = &preflight.processor_context {
            if bounded_processor_context(root)? != *expected {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "processor context sidecar changed during verification",
                ));
            }
        }
        if captured.manifest != preflight.manifest || bounded_manifest(root)? != preflight.json {
            return Err(fail(
                DiagnosticCode::Hash,
                "archive members differ from the complete composition closure",
            ));
        }
        Ok(captured)
    }

    pub(crate) fn tree_entries_in_root(
        root: &ProjectRoot,
        preflight: &ArchivePreflight,
    ) -> Result<usize, Diagnostics> {
        check_tree(root, &preflight.manifest)
    }

    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.manifest_json)
    }

    /// Hash the verified closure with only the selected entry value tokens
    /// replaced by their caller-provided canonical source. All other member
    /// metadata, builtins, and retained provenance remain in the identity.
    pub(crate) fn node_freeze_reuse_identity(
        &self,
        normalized_entry: &[u8],
    ) -> Result<String, Diagnostics> {
        let mut manifest = self.manifest.clone();
        let entry = manifest
            .members
            .iter_mut()
            .find(|member| member.path == manifest.entry && member.kind == MemberKind::Source)
            .ok_or_else(|| fail(DiagnosticCode::Reference, "archive entry source is missing"))?;
        entry.bytes = normalized_entry.len() as u64;
        entry.sha256 = sha256_digest(normalized_entry);
        Ok(sha256_digest(&canonical_json(&manifest)?))
    }

    pub(crate) fn node_freeze_reuse_identity_with_mask(
        &self,
        normalized_entry: &[u8],
        mask: &BTreeMap<String, &'static str>,
    ) -> Result<String, Diagnostics> {
        if !self.has_processor_context() {
            return self.node_freeze_reuse_identity(normalized_entry);
        }
        let mut manifest = self.manifest.clone();
        let entry = manifest
            .members
            .iter_mut()
            .find(|member| member.path == manifest.entry && member.kind == MemberKind::Source)
            .ok_or_else(|| fail(DiagnosticCode::Reference, "archive entry source is missing"))?;
        entry.bytes = normalized_entry.len() as u64;
        entry.sha256 = sha256_digest(normalized_entry);
        let context = self.processor_context.as_ref().ok_or_else(|| {
            fail(
                DiagnosticCode::Reference,
                "private processor context is missing",
            )
        })?;
        let projected = NativeProcessorContext::from_bytes(context)?.projected_for_mask(mask)?;
        let sidecar = manifest
            .processor_context
            .as_mut()
            .expect("context manifest");
        sidecar.bytes = projected.len() as u64;
        sidecar.sha256 = sha256_digest(&projected);
        Ok(sha256_digest(&canonical_json(&manifest)?))
    }

    pub(crate) fn entry(&self) -> &str {
        &self.manifest.entry
    }

    pub(crate) fn profile(&self) -> &str {
        &self.manifest.profile
    }

    pub(crate) fn source_text(&self, path: &str) -> Result<&str, Diagnostics> {
        if !self
            .manifest
            .members
            .iter()
            .any(|m| m.path == path && m.kind == MemberKind::Source)
        {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("`{path}` is not an archived source"),
            ));
        }
        match self.data.get(path) {
            Some(MemberData::Bytes(bytes)) => std::str::from_utf8(bytes)
                .map_err(|_| fail(DiagnosticCode::Syntax, "archived source is not UTF-8")),
            _ => Err(fail(
                DiagnosticCode::Reference,
                "archived source bytes are missing",
            )),
        }
    }

    pub(crate) fn patched_import_sources(
        &self,
        library: &str,
        library_source: &str,
        entry_source: &str,
    ) -> Result<Self, Diagnostics> {
        if library == self.entry()
            || library_source.len() > MAX_BUNDLE_FILE_BYTES
            || entry_source.len() > MAX_BUNDLE_FILE_BYTES
        {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "edited source exceeds bundle file limit",
            ));
        }
        self.source_text(library)?;
        let staging = tempfile::tempdir().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private edit staging: {e}"),
            )
        })?;
        self.stage_members(staging.path())?;
        fs::write(staging.path().join(library), library_source).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot stage library source: {e}"),
            )
        })?;
        fs::write(staging.path().join(self.entry()), entry_source).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot stage importer source: {e}"),
            )
        })?;
        let after = Self::capture_matching_layout(Path::new(self.entry()), staging.path(), self)?;
        let same_members = self
            .manifest
            .members
            .iter()
            .filter(|m| m.path != library && m.path != self.entry())
            .eq(after
                .manifest
                .members
                .iter()
                .filter(|m| m.path != library && m.path != self.entry()));
        if !same_members
            || self.manifest.builtins != after.manifest.builtins
            || self.manifest.import_manifest != after.manifest.import_manifest
            || self.manifest.original_wav != after.manifest.original_wav
            || self.manifest.imports != after.manifest.imports
        {
            return Err(fail(
                DiagnosticCode::Capability,
                "import edit changed the archived non-source closure",
            ));
        }
        Ok(after)
    }

    /// Recompile a coordinated edit of existing archived source members. The
    /// caller supplies the complete final source text for every changed member;
    /// all other closure members and retained provenance must stay identical.
    pub(crate) fn patched_sources(
        &self,
        replacements: &BTreeMap<String, String>,
    ) -> Result<Self, Diagnostics> {
        if replacements.is_empty() {
            return Err(fail(
                DiagnosticCode::Reference,
                "source edit requires at least one replacement",
            ));
        }
        for (path, source) in replacements {
            self.source_text(path)?;
            if source.len() > MAX_BUNDLE_FILE_BYTES {
                return Err(fail(
                    DiagnosticCode::ResourceLimit,
                    "edited source exceeds bundle file limit",
                ));
            }
        }
        let staging = tempfile::tempdir().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private edit staging: {e}"),
            )
        })?;
        self.stage_members(staging.path())?;
        for (path, source) in replacements {
            fs::write(staging.path().join(path), source).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot stage edited source `{path}`: {e}"),
                )
            })?;
        }
        let after = Self::capture_matching_layout(Path::new(self.entry()), staging.path(), self)?;
        let same_members = self
            .manifest
            .members
            .iter()
            .filter(|member| !replacements.contains_key(&member.path))
            .eq(after
                .manifest
                .members
                .iter()
                .filter(|member| !replacements.contains_key(&member.path)));
        if !same_members
            || self.manifest.builtins != after.manifest.builtins
            || self.manifest.import_manifest != after.manifest.import_manifest
            || self.manifest.original_wav != after.manifest.original_wav
            || self.manifest.imports != after.manifest.imports
        {
            return Err(fail(
                DiagnosticCode::Capability,
                "source edit changed the archived non-source closure",
            ));
        }
        Ok(after)
    }

    /// Recompile an edited entry from a private staging of this exact closure.
    /// Every other member, builtin identity, and retained sidecar must remain
    /// identical and reachable in the new complete closure.
    pub(crate) fn patched_entry_source(&self, source: &str) -> Result<Self, Diagnostics> {
        if source.len() > MAX_BUNDLE_FILE_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "edited entry source exceeds bundle file limit",
            ));
        }
        let staging = tempfile::tempdir().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private edit staging: {e}"),
            )
        })?;
        self.stage_members(staging.path())?;
        fs::write(staging.path().join(&self.manifest.entry), source).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot stage edited entry source: {e}"),
            )
        })?;
        let after =
            Self::capture_matching_layout(Path::new(&self.manifest.entry), staging.path(), self)?;
        let same_members = self
            .manifest
            .members
            .iter()
            .filter(|m| m.path != self.manifest.entry)
            .eq(after
                .manifest
                .members
                .iter()
                .filter(|m| m.path != self.manifest.entry));
        if !same_members
            || self.manifest.builtins != after.manifest.builtins
            || self.manifest.import_manifest != after.manifest.import_manifest
            || self.manifest.original_wav != after.manifest.original_wav
            || self.manifest.imports != after.manifest.imports
        {
            return Err(fail(
                DiagnosticCode::Capability,
                "edit changed the archived non-entry closure",
            ));
        }
        Ok(after)
    }

    pub(crate) fn resource_bytes(&self) -> (u64, u64, u64, u64) {
        let (source, asset, pcm) = resource_bytes(&self.manifest);
        let original = original_bytes(&self.manifest);
        (source, asset, pcm, original)
    }

    pub(crate) fn is_retained(&self) -> bool {
        self.retained.is_some() || !self.imports.is_empty()
    }

    pub(crate) fn is_multi_import(&self) -> bool {
        !self.imports.is_empty()
    }

    pub(crate) fn has_processor_context(&self) -> bool {
        self.processor_context.is_some()
    }

    /// Exact file and directory count produced by staging this snapshot.
    pub(crate) fn projected_tree_entries(&self) -> usize {
        let mut directories = BTreeSet::new();
        let sidecars = self.manifest.imports.iter().flat_map(|record| {
            [
                record.import_manifest.path.as_str(),
                record.original_wav.path.as_str(),
            ]
        });
        for path in self
            .manifest
            .members
            .iter()
            .map(|m| m.path.as_str())
            .chain(sidecars)
        {
            for (index, byte) in path.bytes().enumerate() {
                if byte == b'/' {
                    directories.insert(&path[..index]);
                }
            }
        }
        1 + self.manifest.members.len()
            + directories.len()
            + if self.retained.is_some() { 2 } else { 0 }
            + self.imports.len() * 2
            + usize::from(self.processor_context.is_some())
    }

    /// Write only local closure members into an existing output directory.
    /// Every final file uses create_new, so preexisting names never get overwritten.
    pub(crate) fn stage_members(&self, dir: &Path) -> Result<(), Diagnostics> {
        let root_meta = fs::symlink_metadata(dir).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect output directory: {e}"),
            )
        })?;
        if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
            return Err(fail(
                DiagnosticCode::Reference,
                "output root must be a real directory",
            ));
        }
        for member in &self.manifest.members {
            let output = prepare_parent(dir, &member.path)?;
            let data = self.data.get(&member.path).ok_or_else(|| {
                fail(
                    DiagnosticCode::Reference,
                    "archive snapshot is missing a member",
                )
            })?;
            write_member(&output, data, member)?;
        }
        if let Some(retained) = &self.retained {
            let import_path = dir.join("import.json");
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&import_path)
                .map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot create import.json: {e}"),
                    )
                })?;
            output.write_all(&retained.import_json).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot write import.json: {e}"),
                )
            })?;
            if sha256_digest(&retained.import_json)
                != self
                    .manifest
                    .import_manifest
                    .as_ref()
                    .expect("retained manifest")
                    .sha256
            {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "private import.json snapshot changed",
                ));
            }
            retained
                .imported
                .stage_retained_original_to(&dir.join("original.wav"))
                .map_err(import_failure)?;
        }
        for payload in &self.imports {
            let record = self
                .manifest
                .imports
                .iter()
                .find(|record| record.directory == payload.directory)
                .expect("validated import payload");
            let import_path = prepare_parent(dir, &record.import_manifest.path)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&import_path)
                .map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot create import sidecar: {e}"),
                    )
                })?;
            output.write_all(&payload.import_json).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot write import sidecar: {e}"),
                )
            })?;
            if sha256_digest(&payload.import_json) != record.import_manifest.sha256 {
                return Err(fail(DiagnosticCode::Hash, "private import sidecar changed"));
            }
            let original_path = prepare_parent(dir, &record.original_wav.path)?;
            payload
                .imported
                .stage_retained_original_to(&original_path)
                .map_err(import_failure)?;
        }
        if let Some(bytes) = &self.processor_context {
            let sidecar = self
                .manifest
                .processor_context
                .as_ref()
                .expect("context manifest");
            if bytes.len() as u64 != sidecar.bytes || sha256_digest(bytes) != sidecar.sha256 {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "private processor context changed",
                ));
            }
            let output = prepare_parent(dir, PROCESSOR_CONTEXT_PATH)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot create processor context: {e}"),
                    )
                })?;
            file.write_all(bytes).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot write processor context: {e}"),
                )
            })?;
        }
        Ok(())
    }

    /// Write local members and the canonical manifest to a staging directory.
    pub(crate) fn stage(&self, dir: &Path) -> Result<(), Diagnostics> {
        self.stage_members(dir)?;
        let output = prepare_parent(dir, MANIFEST_PATH)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot create manifest `{}`: {e}", output.display()),
                )
            })?;
        file.write_all(&self.manifest_json).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot write manifest: {e}"),
            )
        })
    }
}

fn add_bytes(
    data: &mut BTreeMap<String, MemberData>,
    members: &mut Vec<Member>,
    path: &str,
    kind: MemberKind,
    bytes: &[u8],
) -> Result<(), Diagnostics> {
    check_path(path)?;
    if data
        .insert(path.into(), MemberData::Bytes(bytes.to_vec()))
        .is_some()
    {
        return Err(fail(
            DiagnosticCode::Reference,
            format!("duplicate archive member `{path}`"),
        ));
    }
    members.push(Member {
        path: path.into(),
        kind,
        bytes: bytes.len() as u64,
        sha256: sha256_digest(bytes),
    });
    Ok(())
}

fn limits(profile: &str) -> Result<PlanLimits, Diagnostics> {
    match profile {
        "default" => Ok(PlanLimits::default()),
        "song" => Ok(PlanLimits::song()),
        _ => Err(fail(
            DiagnosticCode::Version,
            format!("unsupported archive profile `{profile}`"),
        )),
    }
}

fn canonical_json(manifest: &Manifest) -> Result<Vec<u8>, Diagnostics> {
    serde_json::to_vec(manifest).map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode archive manifest: {e}"),
        )
    })
}

fn validate_manifest(manifest: &Manifest) -> Result<(), Diagnostics> {
    let context = manifest.processor_context.as_ref();
    if let Some(sidecar) = context {
        if sidecar.path != PROCESSOR_CONTEXT_PATH || sidecar.bytes > MAX_PROCESSOR_CONTEXT_BYTES {
            return Err(fail(
                DiagnosticCode::Reference,
                "invalid processor context path or size",
            ));
        }
        crate::bundle::validate_hash_pin(&sidecar.sha256, "processor context hash")?;
    }
    let retained = match (
        manifest.format.as_str(),
        manifest.version,
        &manifest.import_manifest,
        &manifest.original_wav,
        context.is_some(),
    ) {
        (FORMAT, VERSION, None, None, false) if manifest.imports.is_empty() => false,
        (
            SNAPSHOT_FORMAT,
            VERSION | PROCESSOR_CONTEXT_VERSION,
            Some(import),
            Some(original),
            has_context,
        ) if (manifest.version == PROCESSOR_CONTEXT_VERSION) == has_context => {
            if !manifest.imports.is_empty() {
                return Err(fail(
                    DiagnosticCode::Version,
                    "root retained snapshot cannot contain multiple imports",
                ));
            }
            if manifest.entry != "main.maac"
                || import.path != "import.json"
                || original.path != "original.wav"
                || import.bytes > 16 * 1024
                || original.bytes > media_import::MAX_MEDIA_IMPORT_INPUT_BYTES
            {
                return Err(fail(
                    DiagnosticCode::Reference,
                    "invalid retained import sidecar paths or sizes",
                ));
            }
            crate::bundle::validate_hash_pin(&import.sha256, "import manifest hash")?;
            crate::bundle::validate_hash_pin(&original.sha256, "original WAV hash")?;
            true
        }
        (
            SNAPSHOT_FORMAT,
            MULTI_IMPORT_VERSION | PROCESSOR_CONTEXT_VERSION,
            None,
            None,
            has_context,
        ) if (manifest.version == PROCESSOR_CONTEXT_VERSION) == has_context
            && !manifest.imports.is_empty()
            && manifest.imports.len() <= MAX_RETAINED_IMPORTS =>
        {
            true
        }
        (SNAPSHOT_FORMAT, PROCESSOR_CONTEXT_VERSION, None, None, true)
            if manifest.imports.is_empty() =>
        {
            false
        }
        _ => {
            return Err(fail(
                DiagnosticCode::Version,
                "unsupported archive snapshot format or incomplete sidecar pair",
            ));
        }
    };
    if manifest.version >= MULTI_IMPORT_VERSION {
        let mut prior_directory: Option<&str> = None;
        let mut sidecar_paths = Vec::with_capacity(manifest.imports.len() * 2);
        let mut total_original = 0u64;
        for record in &manifest.imports {
            if record.directory != "." {
                check_path(&record.directory)?;
            }
            if prior_directory.is_some_and(|prior| prior >= record.directory.as_str()) {
                return Err(fail(
                    DiagnosticCode::Reference,
                    "retained import directories are duplicate or out of order",
                ));
            }
            prior_directory = Some(&record.directory);
            if record.import_manifest.path != import_path(&record.directory, "import.json")
                || record.original_wav.path != import_path(&record.directory, "original.wav")
                || record.pcm_member != import_path(&record.directory, "media.pcm")
                || record.import_manifest.bytes > 16 * 1024
                || record.original_wav.bytes > media_import::MAX_MEDIA_IMPORT_INPUT_BYTES
            {
                return Err(fail(
                    DiagnosticCode::Reference,
                    "invalid retained import paths or sizes",
                ));
            }
            crate::bundle::validate_hash_pin(
                &record.import_manifest.sha256,
                "import manifest hash",
            )?;
            crate::bundle::validate_hash_pin(&record.original_wav.sha256, "original WAV hash")?;
            total_original = total_original
                .checked_add(record.original_wav.bytes)
                .ok_or_else(|| {
                    fail(
                        DiagnosticCode::ResourceLimit,
                        "retained original byte count overflow",
                    )
                })?;
            sidecar_paths.push(record.import_manifest.path.as_str());
            sidecar_paths.push(record.original_wav.path.as_str());
        }
        if total_original > MAX_ORIGINAL_TOTAL_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "retained originals exceed 4 GiB",
            ));
        }
        for (index, path) in sidecar_paths.iter().enumerate() {
            check_path(path)?;
            if paths_overlap(path, MANIFEST_PATH)
                || context.is_some_and(|_| paths_overlap(path, PROCESSOR_CONTEXT_PATH))
                || sidecar_paths[..index]
                    .iter()
                    .any(|prior| paths_overlap(path, prior))
            {
                return Err(fail(
                    DiagnosticCode::Reference,
                    "retained sidecar paths overlap",
                ));
            }
        }
    }
    limits(&manifest.profile)?;
    check_path(&manifest.entry)?;
    if manifest.members.len() > MAX_BUNDLE_SOURCES + MAX_BUNDLE_ASSETS {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "archive has too many members",
        ));
    }
    if manifest.builtins.len() > MAX_BUNDLE_SOURCES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "archive has too many built-ins",
        ));
    }
    let mut prior: Option<&str> = None;
    let mut sources = 0;
    let mut assets = 0;
    let mut source_bytes = 0u64;
    let mut asset_bytes = 0u64;
    let mut pcm_bytes = 0u64;
    for member in &manifest.members {
        check_path(&member.path)?;
        if context.is_some() && paths_overlap(&member.path, PROCESSOR_CONTEXT_PATH) {
            return Err(fail(
                DiagnosticCode::Reference,
                "processor context collides with a closure member",
            ));
        }
        if retained
            && (sidecar_collision(&member.path) && manifest.import_manifest.is_some()
                || manifest.imports.iter().any(|record| {
                    paths_overlap(&member.path, &record.import_manifest.path)
                        || paths_overlap(&member.path, &record.original_wav.path)
                }))
        {
            return Err(fail(
                DiagnosticCode::Reference,
                "retained sidecar collides with a closure member",
            ));
        }
        crate::bundle::validate_hash_pin(&member.sha256, "archive member hash")?;
        if prior
            .is_some_and(|p| p >= member.path.as_str() || member.path.starts_with(&format!("{p}/")))
        {
            return Err(fail(
                DiagnosticCode::Reference,
                "archive member paths are duplicate, overlapping, or out of order",
            ));
        }
        prior = Some(&member.path);
        match member.kind {
            MemberKind::Source => {
                sources += 1;
                if member.bytes > MAX_BUNDLE_FILE_BYTES as u64 {
                    return Err(fail(
                        DiagnosticCode::ResourceLimit,
                        "archive source exceeds file limit",
                    ));
                }
                source_bytes += member.bytes;
            }
            MemberKind::Asset => {
                assets += 1;
                if member.bytes > MAX_BUNDLE_FILE_BYTES as u64 {
                    return Err(fail(
                        DiagnosticCode::ResourceLimit,
                        "archive asset exceeds file limit",
                    ));
                }
                asset_bytes += member.bytes;
            }
            MemberKind::Pcm => {
                assets += 1;
                if member.bytes > MAX_DISK_MEDIA_FILE_BYTES {
                    return Err(fail(
                        DiagnosticCode::ResourceLimit,
                        "archive PCM exceeds file limit",
                    ));
                }
                pcm_bytes += member.bytes;
            }
        }
    }
    if sources > MAX_BUNDLE_SOURCES
        || assets + usize::from(context.is_some()) > MAX_BUNDLE_ASSETS
        || source_bytes > crate::bundle::MAX_BUNDLE_SOURCE_BYTES as u64
        || asset_bytes + context.map_or(0, |sidecar| sidecar.bytes)
            > crate::bundle::MAX_BUNDLE_ASSET_BYTES as u64
        || pcm_bytes > crate::disk_media::MAX_DISK_MEDIA_TOTAL_BYTES
    {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "archive closure exceeds resource limits",
        ));
    }
    if !manifest
        .members
        .iter()
        .any(|m| m.path == manifest.entry && m.kind == MemberKind::Source)
    {
        return Err(fail(
            DiagnosticCode::Reference,
            "archive entry is not a source member",
        ));
    }
    for record in &manifest.imports {
        if !manifest
            .members
            .iter()
            .any(|member| member.path == record.pcm_member && member.kind == MemberKind::Pcm)
        {
            return Err(fail(
                DiagnosticCode::Asset,
                "retained import lacks a native PCM closure member",
            ));
        }
    }
    if pcm_bytes
        .checked_add(original_bytes(manifest))
        .is_none_or(|total| total > MAX_ORIGINAL_TOTAL_BYTES)
    {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "snapshot PCM and originals exceed 4 GiB",
        ));
    }
    let mut prior_builtin: Option<&str> = None;
    for builtin in &manifest.builtins {
        if !builtin.path.starts_with("@builtin/")
            || prior_builtin.is_some_and(|p| p >= builtin.path.as_str())
        {
            return Err(fail(
                DiagnosticCode::Reference,
                "archive built-in identities are invalid or out of order",
            ));
        }
        prior_builtin = Some(&builtin.path);
        crate::bundle::validate_hash_pin(&builtin.sha256, "built-in hash")?;
        let Some(current) = crate::stdlib::lookup_path(&builtin.path) else {
            return Err(fail(
                DiagnosticCode::Reference,
                "archive references an unknown built-in",
            ));
        };
        if builtin.bytes != current.source.len() as u64
            || builtin.sha256 != sha256_digest(current.source.as_bytes())
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "archive built-in identity differs from this runtime",
            ));
        }
    }
    Ok(())
}

fn paths_overlap(a: &str, b: &str) -> bool {
    a == b
        || a.strip_prefix(b).is_some_and(|tail| tail.starts_with('/'))
        || b.strip_prefix(a).is_some_and(|tail| tail.starts_with('/'))
}

fn resource_bytes(manifest: &Manifest) -> (u64, u64, u64) {
    let mut source = 0;
    let mut asset = 0;
    let mut pcm = 0;
    for member in &manifest.members {
        match member.kind {
            MemberKind::Source => source += member.bytes,
            MemberKind::Asset => asset += member.bytes,
            MemberKind::Pcm => pcm += member.bytes,
        }
    }
    asset += manifest
        .processor_context
        .as_ref()
        .map_or(0, |sidecar| sidecar.bytes);
    (source, asset, pcm)
}

fn original_bytes(manifest: &Manifest) -> u64 {
    manifest
        .original_wav
        .as_ref()
        .map_or(0, |sidecar| sidecar.bytes)
        + manifest
            .imports
            .iter()
            .map(|record| record.original_wav.bytes)
            .sum::<u64>()
}

fn normalize_selections(selections: &[PathBuf]) -> Result<Vec<String>, Diagnostics> {
    if selections.len() > MAX_RETAINED_IMPORTS {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "archive retains at most 16 imports",
        ));
    }
    let mut directories = Vec::with_capacity(selections.len());
    for path in selections {
        let text = path
            .to_str()
            .ok_or_else(|| fail(DiagnosticCode::Reference, "import directory is not UTF-8"))?;
        if text != "." {
            check_path(text)?;
            if text == "import.json" || text == "original.wav" {
                return Err(fail(
                    DiagnosticCode::Reference,
                    "import selection is not a directory",
                ));
            }
        }
        directories.push(text.to_owned());
    }
    directories.sort();
    if directories.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(fail(
            DiagnosticCode::Reference,
            "duplicate retained import directory",
        ));
    }
    Ok(directories)
}

fn import_path(directory: &str, name: &str) -> String {
    if directory == "." {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

fn with_import_dir<T>(
    root: &ProjectRoot,
    directory: &str,
    f: impl FnOnce(&ProjectRoot) -> Result<T, Diagnostics>,
) -> Result<T, Diagnostics> {
    if directory == "." {
        return f(root);
    }
    let mut pinned = root.open_child_pinned(directory.split('/').next().expect("nonempty path"))?;
    for part in directory.split('/').skip(1) {
        pinned = pinned.open_child_pinned(part)?;
    }
    f(&pinned)
}

fn preflight_selected(
    root: &ProjectRoot,
    directories: &[String],
) -> Result<Vec<(String, RetainedImportPreflight)>, Diagnostics> {
    let mut result = Vec::with_capacity(directories.len());
    let mut total = 0u64;
    for directory in directories {
        let preflight = with_import_dir(root, directory, |child| {
            media_import::preflight_retained_import_in_root(child).map_err(import_failure)
        })?;
        total = total
            .checked_add(preflight.original_bytes())
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "retained original byte count overflow",
                )
            })?;
        if total > MAX_ORIGINAL_TOTAL_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "retained originals exceed 4 GiB",
            ));
        }
        result.push((directory.clone(), preflight));
    }
    Ok(result)
}

fn capture_selected(
    root: &ProjectRoot,
    preflights: &[(String, RetainedImportPreflight)],
    data: &BTreeMap<String, MemberData>,
) -> Result<(Vec<ImportPayload>, Vec<ImportRecord>), Diagnostics> {
    let mut payloads = Vec::with_capacity(preflights.len());
    let mut records = Vec::with_capacity(preflights.len());
    for (directory, preflight) in preflights {
        let imported = with_import_dir(root, directory, |child| {
            media_import::verify_retained_import_snapshot_in_root(child, preflight)
                .map_err(import_failure)
        })?;
        let pcm_path = import_path(directory, "media.pcm");
        let pcm = data.get(&pcm_path).ok_or_else(|| {
            fail(
                DiagnosticCode::Asset,
                format!("selected import `{directory}` is not a closure PCM dependency"),
            )
        })?;
        if !matches!(pcm, MemberData::Pcm(_)) {
            return Err(fail(
                DiagnosticCode::Asset,
                format!("selected import `{directory}` is not native PCM"),
            ));
        }
        if imported.pcm_hash() != preflight.output_hash()
            || !member_matches_bytes(pcm, imported.pcm_bytes())?
        {
            return Err(fail(
                DiagnosticCode::Hash,
                format!("retained WAV crop differs from `{pcm_path}`"),
            ));
        }
        records.push(ImportRecord {
            directory: directory.clone(),
            import_manifest: Sidecar {
                path: import_path(directory, "import.json"),
                bytes: preflight.json().len() as u64,
                sha256: sha256_digest(preflight.json()),
            },
            original_wav: Sidecar {
                path: import_path(directory, "original.wav"),
                bytes: imported.input_bytes(),
                sha256: imported.source_hash().to_owned(),
            },
            pcm_member: pcm_path,
        });
        payloads.push(ImportPayload {
            directory: directory.clone(),
            import_json: preflight.json().to_vec(),
            imported,
        });
    }
    Ok((payloads, records))
}

fn capture_retained(
    root: &ProjectRoot,
    entry: &str,
    data: &BTreeMap<String, MemberData>,
) -> Result<Option<RetainedPayload>, Diagnostics> {
    if entry != "main.maac" {
        if let Some((parent, _)) = entry.rsplit_once('/') {
            let nested_import = sidecar_exists(root, &format!("{parent}/import.json"))?;
            let nested_original = sidecar_exists(root, &format!("{parent}/original.wav"))?;
            if nested_import || nested_original {
                return Err(fail(
                    DiagnosticCode::Reference,
                    "nested import sidecars require their directory as the project root",
                ));
            }
        }
    }
    let import = sidecar_exists(root, "import.json")?;
    let original = sidecar_exists(root, "original.wav")?;
    if !import && !original {
        return Ok(None);
    }
    if entry != "main.maac" {
        return Err(fail(
            DiagnosticCode::Reference,
            "retained import sidecars require project-root `main.maac`",
        ));
    }
    if data.keys().any(|path| sidecar_collision(path)) {
        return Err(fail(
            DiagnosticCode::Reference,
            "import sidecar collides with a composition member",
        ));
    }
    if !import {
        return Err(fail(
            DiagnosticCode::Reference,
            "original.wav has no import.json",
        ));
    }
    if !original {
        media_import::preflight_legacy_import_in_root(root).map_err(import_failure)?;
        return Ok(None); // v1 imports contain no retained original.
    }
    let preflight =
        media_import::preflight_retained_import_in_root(root).map_err(import_failure)?;
    let imported = media_import::verify_retained_import_snapshot_in_root(root, &preflight)
        .map_err(import_failure)?;
    let pcm = data.get("media.pcm").ok_or_else(|| {
        fail(
            DiagnosticCode::Asset,
            "retained import has no media.pcm closure member",
        )
    })?;
    if !matches!(pcm, MemberData::Pcm(_)) {
        return Err(fail(
            DiagnosticCode::Asset,
            "retained import media.pcm is not a native PCM dependency",
        ));
    }
    if imported.pcm_hash() != preflight.output_hash()
        || !member_matches_bytes(pcm, imported.pcm_bytes())?
    {
        return Err(fail(
            DiagnosticCode::Hash,
            "retained WAV crop differs from captured media.pcm",
        ));
    }
    Ok(Some(RetainedPayload {
        import_json: preflight.json().to_vec(),
        imported,
    }))
}

fn sidecar_collision(path: &str) -> bool {
    path == "import.json"
        || path.starts_with("import.json/")
        || path == "original.wav"
        || path.starts_with("original.wav/")
}

fn sidecar_exists(root: &ProjectRoot, path: &str) -> Result<bool, Diagnostics> {
    match root.dir().symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(fail(
            DiagnosticCode::Reference,
            format!("sidecar `{path}` is not a regular file"),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fail(
            DiagnosticCode::Reference,
            format!("cannot inspect sidecar `{path}`: {error}"),
        )),
    }
}

fn member_matches_bytes(data: &MemberData, expected: &[u8]) -> Result<bool, Diagnostics> {
    match data {
        MemberData::Bytes(bytes) => Ok(bytes == expected),
        MemberData::Pcm(asset) => {
            if asset.bytes != expected.len() as u64 {
                return Ok(false);
            }
            let mut block = [0u8; 64 * 1024];
            let mut offset = 0usize;
            while offset < expected.len() {
                let wanted = block.len().min(expected.len() - offset);
                let n = read_snapshot_at(&asset.file, &mut block[..wanted], offset as u64)
                    .map_err(|e| {
                        fail(
                            DiagnosticCode::Asset,
                            format!("cannot compare private PCM snapshot: {e}"),
                        )
                    })?;
                if n == 0 || block[..n] != expected[offset..offset + n] {
                    return Ok(false);
                }
                offset += n;
            }
            Ok(true)
        }
    }
}

fn import_failure(error: media_import::MediaImportError) -> Diagnostics {
    let code = match error.code() {
        "E_RESOURCE_LIMIT" => DiagnosticCode::ResourceLimit,
        "E_IMPORT_MANIFEST" => DiagnosticCode::Syntax,
        "E_REFERENCE" => DiagnosticCode::Reference,
        "E_IMPORT_MISMATCH" => DiagnosticCode::Hash,
        _ => DiagnosticCode::Asset,
    };
    fail(code, error.to_string())
}

fn check_path(path: &str) -> Result<(), Diagnostics> {
    let mut parts = path.split('/');
    let first = parts.next().unwrap_or_default();
    let bad = path.is_empty()
        || path.len() > MAX_BUNDLE_PATH_BYTES
        || path == MANIFEST_PATH
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || path == "@builtin"
        || path.starts_with("@builtin/")
        || (first.len() >= 2
            && first.as_bytes()[0].is_ascii_alphabetic()
            && first.as_bytes()[1] == b':');
    if bad {
        Err(fail(
            DiagnosticCode::Reference,
            format!("unsafe or reserved archive path `{path}`"),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn bounded_manifest(root: &ProjectRoot) -> Result<Vec<u8>, Diagnostics> {
    let metadata = root.dir().symlink_metadata(MANIFEST_PATH).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot inspect archive manifest: {e}"),
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(fail(
            DiagnosticCode::Reference,
            "archive manifest is not a regular file",
        ));
    }
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = root
        .dir()
        .open_with(MANIFEST_PATH, &options)
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot open archive manifest: {e}"),
            )
        })?
        .into_std();
    if !file
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect archive manifest: {e}"),
            )
        })?
        .is_file()
    {
        return Err(fail(
            DiagnosticCode::Reference,
            "archive manifest is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot read archive manifest: {e}"),
            )
        })?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "archive manifest exceeds 1 MiB",
        ));
    }
    Ok(bytes)
}

fn bounded_processor_context(root: &ProjectRoot) -> Result<Vec<u8>, Diagnostics> {
    let metadata = root
        .dir()
        .symlink_metadata(PROCESSOR_CONTEXT_PATH)
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect processor context: {e}"),
            )
        })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(fail(
            DiagnosticCode::Reference,
            "processor context is not a regular file",
        ));
    }
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = root
        .dir()
        .open_with(PROCESSOR_CONTEXT_PATH, &options)
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot open processor context: {e}"),
            )
        })?
        .into_std();
    if !file
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect processor context: {e}"),
            )
        })?
        .is_file()
    {
        return Err(fail(
            DiagnosticCode::Reference,
            "processor context is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_PROCESSOR_CONTEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot read processor context: {e}"),
            )
        })?;
    if bytes.len() as u64 > MAX_PROCESSOR_CONTEXT_BYTES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "processor context exceeds 4 MiB",
        ));
    }
    Ok(bytes)
}

fn check_tree(root: &ProjectRoot, manifest: &Manifest) -> Result<usize, Diagnostics> {
    let mut expected: BTreeSet<&str> = manifest
        .members
        .iter()
        .map(|m| m.path.as_str())
        .chain([MANIFEST_PATH])
        .collect();
    if manifest.import_manifest.is_some() {
        expected.insert("import.json");
        expected.insert("original.wav");
    }
    for record in &manifest.imports {
        expected.insert(&record.import_manifest.path);
        expected.insert(&record.original_wav.path);
    }
    if manifest.processor_context.is_some() {
        expected.insert(PROCESSOR_CONTEXT_PATH);
    }
    let mut found = BTreeSet::new();
    let mut pending = vec![(
        root.dir().try_clone().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot pin archive directory: {e}"),
            )
        })?,
        String::new(),
    )];
    let mut count = 0usize;
    while let Some((directory, prefix)) = pending.pop() {
        for entry in directory.read_dir(".").map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot list archive directory: {e}"),
            )
        })? {
            let entry = entry.map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot list archive member: {e}"),
                )
            })?;
            count += 1;
            if count > MAX_TREE_ENTRIES {
                return Err(fail(
                    DiagnosticCode::ResourceLimit,
                    "archive directory contains too many entries",
                ));
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| fail(DiagnosticCode::Reference, "archive has a non-UTF-8 name"))?;
            let logical = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry.file_type().map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot inspect archive member: {e}"),
                )
            })?;
            if kind.is_symlink() {
                return Err(fail(
                    DiagnosticCode::Reference,
                    format!("archive contains symlink `{logical}`"),
                ));
            }
            if kind.is_dir() {
                if !expected
                    .iter()
                    .any(|p| p.starts_with(&format!("{logical}/")))
                {
                    return Err(fail(
                        DiagnosticCode::Reference,
                        format!("archive contains unexpected directory `{logical}`"),
                    ));
                }
                let child = entry.open_dir().map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot open archive directory `{logical}`: {e}"),
                    )
                })?;
                pending.push((child, logical));
            } else if kind.is_file() && expected.contains(logical.as_str()) {
                found.insert(logical);
            } else {
                return Err(fail(
                    DiagnosticCode::Reference,
                    format!("archive contains unexpected member `{logical}`"),
                ));
            }
        }
    }
    if found.len() != expected.len() {
        return Err(fail(
            DiagnosticCode::Reference,
            "archive is missing a declared member",
        ));
    }
    Ok(count)
}

fn prepare_parent(root: &Path, logical: &str) -> Result<PathBuf, Diagnostics> {
    let mut parent = root.to_path_buf();
    let mut parts = logical.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return Ok(parent.join(part));
        }
        let child = parent.join(part);
        match fs::create_dir(&child) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let exact = fs::read_dir(&parent)
                    .map_err(|e| {
                        fail(
                            DiagnosticCode::Reference,
                            format!("cannot inspect output directory: {e}"),
                        )
                    })?
                    .filter_map(Result::ok)
                    .any(|e| e.file_name() == part);
                let meta = fs::symlink_metadata(&child).map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot inspect output path: {e}"),
                    )
                })?;
                if !exact || !meta.is_dir() || meta.file_type().is_symlink() {
                    return Err(fail(
                        DiagnosticCode::Reference,
                        format!("output path collision at `{}`", child.display()),
                    ));
                }
            }
            Err(e) => {
                return Err(fail(
                    DiagnosticCode::Reference,
                    format!("cannot create output directory: {e}"),
                ));
            }
        }
        parent = child;
    }
    Err(fail(DiagnosticCode::Reference, "empty archive member path"))
}

fn write_member(path: &Path, data: &MemberData, member: &Member) -> Result<(), Diagnostics> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create archive member `{}`: {e}", path.display()),
            )
        })?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    match data {
        MemberData::Bytes(bytes) => {
            output.write_all(bytes).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot write archive member: {e}"),
                )
            })?;
            digest.update(bytes);
            total = bytes.len() as u64;
        }
        MemberData::Pcm(asset) => {
            let mut block = [0u8; 64 * 1024];
            while total < asset.bytes {
                let wanted = block.len().min((asset.bytes - total) as usize);
                let n =
                    read_snapshot_at(&asset.file, &mut block[..wanted], total).map_err(|e| {
                        fail(
                            DiagnosticCode::Asset,
                            format!("cannot read private PCM snapshot: {e}"),
                        )
                    })?;
                if n == 0 {
                    return Err(fail(
                        DiagnosticCode::Asset,
                        "private PCM snapshot ended early",
                    ));
                }
                output.write_all(&block[..n]).map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot write PCM member: {e}"),
                    )
                })?;
                digest.update(&block[..n]);
                total += n as u64;
            }
        }
    }
    let actual = format!("sha256:{:x}", digest.finalize());
    if total != member.bytes || actual != member.sha256 {
        return Err(fail(
            DiagnosticCode::Hash,
            format!("captured member `{}` changed", member.path),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn read_snapshot_at(file: &File, block: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(block, offset)
}

#[cfg(windows)]
fn read_snapshot_at(file: &File, block: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(block, offset)
}

fn fail(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(asset: Option<(&str, usize)>) -> String {
        let output = if asset.is_some() { "clip" } else { "tone" };
        let mut text = format!(
            "maac 1;\nproject p {{ score=[0q,1/4q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&{output}:out; }}\ntempo clock {{ points=[(0q,120bpm,step)]; }}\nmeter metre {{ points=[(0q,4,4)]; }}\n"
        );
        if let Some((hash, frames)) = asset {
            text.push_str(&format!("asset sample {{ kind=audio; path=\"sample.pcm\"; hash=\"{hash}\"; format=\"pcm_f32le_interleaved/1\"; rate=48000Hz; channels=1; frames={frames}; }}\naudio clip {{ asset=&sample; at=0q; source=[0frame,1frame]; mode=rate; }}\n"));
        } else {
            text.push_str("node tone { type=\"core.sine/1\"; config={ voices=2; }; params={ attack=0s; release=0s; level=0.2; }; }\n");
        }
        text
    }

    fn project(root: &Path) {
        fs::write(root.join("main.maac"), source(None)).unwrap();
    }

    #[test]
    fn legacy_options_preserve_snapshot_identity() {
        let source_dir = tempfile::tempdir().unwrap();
        project(source_dir.path());
        let legacy =
            ArchiveSnapshot::capture(Path::new("main.maac"), source_dir.path(), "default").unwrap();
        let options = ArchiveSnapshot::capture_with_options(
            Path::new("main.maac"),
            source_dir.path(),
            "default",
            &[],
            false,
        )
        .unwrap();
        assert_eq!(legacy.manifest_json, options.manifest_json);
        assert_eq!(legacy.digest(), options.digest());
        assert!(!options.has_processor_context());
    }

    #[test]
    fn processor_context_round_trip_rejects_rehashed_foreign_context() {
        let source_dir = tempfile::tempdir().unwrap();
        project(source_dir.path());
        let snapshot = ArchiveSnapshot::capture_with_options(
            Path::new("main.maac"),
            source_dir.path(),
            "default",
            &[],
            true,
        )
        .unwrap();
        assert_eq!(snapshot.manifest.version, PROCESSOR_CONTEXT_VERSION);
        assert!(snapshot.has_processor_context());
        let archive = tempfile::tempdir().unwrap();
        snapshot.stage(archive.path()).unwrap();
        let verified = ArchiveSnapshot::verify(archive.path()).unwrap();
        assert_eq!(verified.digest(), snapshot.digest());
        assert_eq!(
            verified.resource_bytes().1,
            snapshot.processor_context.as_ref().unwrap().len() as u64
        );

        let other_source = tempfile::tempdir().unwrap();
        fs::write(
            other_source.path().join("main.maac"),
            source(None).replace("level=0.2", "level=0.4"),
        )
        .unwrap();
        let other = ArchiveSnapshot::capture_with_options(
            Path::new("main.maac"),
            other_source.path(),
            "default",
            &[],
            true,
        )
        .unwrap();
        let foreign = other.processor_context.as_ref().unwrap();
        assert_ne!(foreign, snapshot.processor_context.as_ref().unwrap());
        fs::write(archive.path().join(PROCESSOR_CONTEXT_PATH), foreign).unwrap();
        let mut forged = snapshot.manifest.clone();
        let sidecar = forged.processor_context.as_mut().unwrap();
        sidecar.bytes = foreign.len() as u64;
        sidecar.sha256 = sha256_digest(foreign);
        fs::write(
            archive.path().join(MANIFEST_PATH),
            canonical_json(&forged).unwrap(),
        )
        .unwrap();
        assert!(ArchiveSnapshot::verify(archive.path()).is_err());
    }

    #[test]
    fn round_trip_uses_captured_bytes_and_rejects_extra_members() {
        let source_dir = tempfile::tempdir().unwrap();
        project(source_dir.path());
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), source_dir.path(), "default").unwrap();
        let digest = snapshot.digest();
        fs::write(source_dir.path().join("main.maac"), "replaced").unwrap();

        let archive = tempfile::tempdir().unwrap();
        snapshot.stage(archive.path()).unwrap();
        assert_eq!(
            fs::read(archive.path().join("main.maac")).unwrap(),
            source(None).as_bytes()
        );
        let verified = ArchiveSnapshot::verify(archive.path()).unwrap();
        assert_eq!(verified.digest(), digest);
        assert_eq!(verified.manifest.entry, "main.maac");
        assert_eq!(verified.manifest_json, snapshot.manifest_json);
        let unpacked = tempfile::tempdir().unwrap();
        verified.stage_members(unpacked.path()).unwrap();
        assert_eq!(
            fs::read(unpacked.path().join("main.maac")).unwrap(),
            source(None).as_bytes()
        );
        assert!(!unpacked.path().join(MANIFEST_PATH).exists());

        fs::write(archive.path().join("extra.txt"), b"extra").unwrap();
        assert!(ArchiveSnapshot::verify(archive.path()).is_err());
    }

    #[test]
    fn verification_keeps_one_root_when_path_is_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let first_source = temp.path().join("first-source");
        let second_source = temp.path().join("second-source");
        let archive = temp.path().join("archive");
        let replacement = temp.path().join("replacement");
        let moved = temp.path().join("moved");
        for path in [&first_source, &second_source, &archive, &replacement] {
            fs::create_dir(path).unwrap();
        }
        project(&first_source);
        let second_text = format!("{}// replacement\n", source(None));
        fs::write(second_source.join("main.maac"), &second_text).unwrap();
        let first =
            ArchiveSnapshot::capture(Path::new("main.maac"), &first_source, "default").unwrap();
        let second =
            ArchiveSnapshot::capture(Path::new("main.maac"), &second_source, "default").unwrap();
        first.stage(&archive).unwrap();
        second.stage(&replacement).unwrap();

        let pinned = ProjectRoot::open_pinned(&archive, cap_std::ambient_authority()).unwrap();
        let verified = ArchiveSnapshot::verify_in_root(&pinned, || {
            fs::rename(&archive, &moved).unwrap();
            fs::rename(&replacement, &archive).unwrap();
        })
        .unwrap();
        assert_eq!(verified.digest(), first.digest());
        assert_eq!(
            ArchiveSnapshot::verify(&archive).unwrap().digest(),
            second.digest()
        );
        let unpacked = temp.path().join("unpacked");
        fs::create_dir(&unpacked).unwrap();
        verified.stage_members(&unpacked).unwrap();
        assert_eq!(
            fs::read_to_string(unpacked.join("main.maac")).unwrap(),
            source(None)
        );
    }

    #[test]
    fn strict_manifest_and_closure_reject_tampering() {
        let source_dir = tempfile::tempdir().unwrap();
        project(source_dir.path());
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), source_dir.path(), "song").unwrap();
        let archive = tempfile::tempdir().unwrap();
        snapshot.stage(archive.path()).unwrap();

        let manifest_path = archive.path().join(MANIFEST_PATH);
        let canonical = fs::read(&manifest_path).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
        value["unknown"] = serde_json::json!(true);
        fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(ArchiveSnapshot::verify(archive.path()).is_err());

        fs::write(&manifest_path, &canonical).unwrap();
        fs::write(archive.path().join("main.maac"), "maac 1;").unwrap();
        assert!(ArchiveSnapshot::verify(archive.path()).is_err());

        fs::write(archive.path().join("main.maac"), source(None)).unwrap();
        let text = String::from_utf8(canonical).unwrap();
        let duplicate = text.replacen(
            "\"format\":",
            "\"format\":\"maac.editable-archive\",\"format\":",
            1,
        );
        fs::write(&manifest_path, duplicate).unwrap();
        assert!(ArchiveSnapshot::verify(archive.path()).is_err());

        let mut phantom = snapshot.manifest.clone();
        phantom.members.push(Member {
            path: "z.txt".into(),
            kind: MemberKind::Asset,
            bytes: 1,
            sha256: sha256_digest(b"z"),
        });
        fs::write(&manifest_path, canonical_json(&phantom).unwrap()).unwrap();
        fs::write(archive.path().join("z.txt"), b"z").unwrap();
        assert!(ArchiveSnapshot::verify(archive.path()).is_err());
    }

    #[test]
    fn reserved_manifest_name_and_existing_output_are_rejected() {
        let source_dir = tempfile::tempdir().unwrap();
        fs::write(source_dir.path().join(MANIFEST_PATH), source(None)).unwrap();
        assert!(
            ArchiveSnapshot::capture(Path::new(MANIFEST_PATH), source_dir.path(), "default")
                .is_err()
        );

        project(source_dir.path());
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), source_dir.path(), "default").unwrap();
        let output = tempfile::tempdir().unwrap();
        fs::write(output.path().join("main.maac"), b"keep").unwrap();
        assert!(snapshot.stage_members(output.path()).is_err());
        assert_eq!(fs::read(output.path().join("main.maac")).unwrap(), b"keep");
    }

    #[test]
    fn nested_import_sidecars_require_nested_project_root() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("main.maac"), source(None)).unwrap();
        fs::write(nested.join("import.json"), b"{}").unwrap();
        assert!(
            ArchiveSnapshot::capture(Path::new("nested/main.maac"), temp.path(), "default")
                .is_err()
        );
    }

    #[test]
    fn pcm_larger_than_four_mib_is_retained_from_private_snapshot() {
        let source_dir = tempfile::tempdir().unwrap();
        let pcm = vec![0u8; MAX_BUNDLE_FILE_BYTES + 4];
        fs::write(source_dir.path().join("sample.pcm"), &pcm).unwrap();
        fs::write(
            source_dir.path().join("main.maac"),
            source(Some((&sha256_digest(&pcm), pcm.len() / 4))),
        )
        .unwrap();
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), source_dir.path(), "default").unwrap();
        fs::remove_file(source_dir.path().join("sample.pcm")).unwrap();
        let archive = tempfile::tempdir().unwrap();
        snapshot.stage(archive.path()).unwrap();
        assert_eq!(
            fs::metadata(archive.path().join("sample.pcm"))
                .unwrap()
                .len(),
            pcm.len() as u64
        );
        ArchiveSnapshot::verify(archive.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_in_archive_are_rejected() {
        use std::os::unix::fs::symlink;
        let source_dir = tempfile::tempdir().unwrap();
        project(source_dir.path());
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), source_dir.path(), "default").unwrap();
        let archive = tempfile::tempdir().unwrap();
        snapshot.stage(archive.path()).unwrap();
        symlink("main.maac", archive.path().join("link.maac")).unwrap();
        assert!(ArchiveSnapshot::verify(archive.path()).is_err());
    }
}
