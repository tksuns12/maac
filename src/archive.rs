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
use crate::plan::PlanLimits;

pub(crate) const MANIFEST_PATH: &str = "maac-archive.json";
const FORMAT: &str = "maac.editable-archive";
const VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_TREE_ENTRIES: usize =
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

enum MemberData {
    Bytes(Vec<u8>),
    Pcm(Arc<DiskAsset>),
}

/// Private, bounded snapshot of the current composition closure.
///
/// All unpack writes come from these captured bytes and private PCM handles.
/// The archive format does not include editing history, freezes, or sidecars.
pub(crate) struct ArchiveSnapshot {
    manifest: Manifest,
    manifest_json: Vec<u8>,
    data: BTreeMap<String, MemberData>,
}

impl ArchiveSnapshot {
    pub(crate) fn capture(entry: &Path, root: &Path, profile: &str) -> Result<Self, Diagnostics> {
        let root = ProjectRoot::open_pinned(root, cap_std::ambient_authority())?;
        Self::capture_in_root(entry, &root, profile)
    }

    fn capture_in_root(
        entry: &Path,
        root: &ProjectRoot,
        profile: &str,
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
        let manifest = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            entry: bundle.entry.clone(),
            profile: profile.into(),
            members,
            builtins,
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
        })
    }

    /// Verify a directory as exactly one current-closure archive, then retain
    /// its own snapshots for later unpack without reopening archive members.
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

    fn verify_in_root(
        root: &ProjectRoot,
        before_capture: impl FnOnce(),
    ) -> Result<Self, Diagnostics> {
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
        check_tree(root, &manifest)?;
        before_capture();
        let captured = Self::capture_in_root(Path::new(&manifest.entry), root, &manifest.profile)?;
        if captured.manifest != manifest {
            return Err(fail(
                DiagnosticCode::Hash,
                "archive members differ from the complete composition closure",
            ));
        }
        Ok(captured)
    }

    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.manifest_json)
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
    if manifest.format != FORMAT || manifest.version != VERSION {
        return Err(fail(
            DiagnosticCode::Version,
            "unsupported editable archive format or version",
        ));
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

fn bounded_manifest(root: &ProjectRoot) -> Result<Vec<u8>, Diagnostics> {
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

fn check_tree(root: &ProjectRoot, manifest: &Manifest) -> Result<(), Diagnostics> {
    let expected: BTreeSet<&str> = manifest
        .members
        .iter()
        .map(|m| m.path.as_str())
        .chain([MANIFEST_PATH])
        .collect();
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
    Ok(())
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
