//! First-slice editable composition archive: current source and media closure only.
//! It deliberately does not retain edit history, freezes, or original sidecars.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cap_std::fs::OpenOptions as CapOpenOptions;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sidecar {
    path: String,
    bytes: u64,
    sha256: String,
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
/// The archive format does not include editing history, freezes, or sidecars.
#[derive(Clone)]
pub(crate) struct ArchiveSnapshot {
    manifest: Manifest,
    manifest_json: Vec<u8>,
    data: BTreeMap<String, MemberData>,
    retained: Option<RetainedPayload>,
}

#[derive(Clone)]
struct RetainedPayload {
    import_json: Vec<u8>,
    imported: ImportedWav,
}

/// Strict v1 metadata checked before any checkpoint media is loaded.
pub(crate) struct ArchivePreflight {
    manifest: Manifest,
    json: Vec<u8>,
    retained: Option<RetainedImportPreflight>,
}

impl ArchivePreflight {
    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.json)
    }

    pub(crate) fn resource_bytes(&self) -> (u64, u64, u64, u64) {
        let (source, asset, pcm) = resource_bytes(&self.manifest);
        let original = self
            .manifest
            .original_wav
            .as_ref()
            .map_or(0, |sidecar| sidecar.bytes);
        (source, asset, pcm, original)
    }

    pub(crate) fn is_retained(&self) -> bool {
        self.retained.is_some()
    }
}

impl ArchiveSnapshot {
    pub(crate) fn capture(entry: &Path, root: &Path, profile: &str) -> Result<Self, Diagnostics> {
        let root = ProjectRoot::open_pinned(root, cap_std::ambient_authority())?;
        Self::capture_in_root(entry, &root, profile, true)
    }

    fn capture_in_root(
        entry: &Path,
        root: &ProjectRoot,
        profile: &str,
        detect_import: bool,
    ) -> Result<Self, Diagnostics> {
        let limits = limits(profile)?;
        let project = DiskMediaProject::load_in_root(entry, root)?;
        project.build_with_limits(&limits)?;
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
            format: if retained.is_some() {
                SNAPSHOT_FORMAT
            } else {
                FORMAT
            }
            .into(),
            version: VERSION,
            entry: bundle.entry.clone(),
            profile: profile.into(),
            members,
            builtins,
            import_manifest,
            original_wav,
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
        Ok(ArchivePreflight {
            manifest,
            json: bytes,
            retained,
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
        )?;
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

    pub(crate) fn resource_bytes(&self) -> (u64, u64, u64, u64) {
        let (source, asset, pcm) = resource_bytes(&self.manifest);
        let original = self
            .manifest
            .original_wav
            .as_ref()
            .map_or(0, |sidecar| sidecar.bytes);
        (source, asset, pcm, original)
    }

    pub(crate) fn is_retained(&self) -> bool {
        self.retained.is_some()
    }

    /// Exact file and directory count produced by staging this v1 snapshot.
    pub(crate) fn projected_tree_entries(&self) -> usize {
        let mut directories = BTreeSet::new();
        for member in &self.manifest.members {
            for (index, byte) in member.path.bytes().enumerate() {
                if byte == b'/' {
                    directories.insert(&member.path[..index]);
                }
            }
        }
        1 + self.manifest.members.len()
            + directories.len()
            + if self.retained.is_some() { 2 } else { 0 }
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
    let retained = match (
        manifest.format.as_str(),
        manifest.version,
        &manifest.import_manifest,
        &manifest.original_wav,
    ) {
        (FORMAT, VERSION, None, None) => false,
        (SNAPSHOT_FORMAT, VERSION, Some(import), Some(original)) => {
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
        _ => {
            return Err(fail(
                DiagnosticCode::Version,
                "unsupported archive snapshot format or incomplete sidecar pair",
            ));
        }
    };
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
        if retained && sidecar_collision(&member.path) {
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
        || assets > MAX_BUNDLE_ASSETS
        || source_bytes > crate::bundle::MAX_BUNDLE_SOURCE_BYTES as u64
        || asset_bytes > crate::bundle::MAX_BUNDLE_ASSET_BYTES as u64
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
    (source, asset, pcm)
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
                ))
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
        let mut text = format!("maac 1;\nproject p {{ score=[0q,1/4q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&{output}:out; }}\ntempo clock {{ points=[(0q,120bpm,step)]; }}\nmeter metre {{ points=[(0q,4,4)]; }}\n");
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
