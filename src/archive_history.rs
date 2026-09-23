//! Immutable, linear archive checkpoints. Each checkpoint remains a complete v1 archive.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::archive::{
    bounded_manifest, ArchivePreflight, ArchiveSnapshot, MANIFEST_PATH, MAX_TREE_ENTRIES,
};
use crate::bundle::{sha256_digest, validate_hash_pin};
use crate::bundle_fs::ProjectRoot;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};

const FORMAT: &str = "maac.editable-archive";
const CHECKPOINT_FORMAT: &str = "maac.archive-checkpoint";
const VERSION: u32 = 2;
const MAX_CHECKPOINTS: usize = 32;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PCM_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryManifest {
    format: String,
    version: u32,
    head: String,
    checkpoints: Vec<CheckpointRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointRecord {
    id: String,
    parent: Option<String>,
    snapshot: String,
}

#[derive(Serialize)]
struct CheckpointIdentity<'a> {
    format: &'static str,
    version: u32,
    parent: Option<&'a str>,
    snapshot: &'a str,
}

enum HistoryState {
    Legacy {
        snapshot: ArchiveSnapshot,
        genesis_id: String,
    },
    Version2 {
        manifest: HistoryManifest,
        json: Vec<u8>,
        snapshots: Vec<ArchiveSnapshot>,
    },
}

/// A checked archive history retaining private snapshots for every checkpoint.
pub(crate) struct ArchiveHistory {
    state: HistoryState,
}

impl ArchiveHistory {
    /// Capture one current composition as a v2 archive with a genesis checkpoint.
    pub(crate) fn capture(entry: &Path, root: &Path, profile: &str) -> Result<Self, Diagnostics> {
        let snapshot = ArchiveSnapshot::capture(entry, root, profile)?;
        Self::from_genesis(snapshot)
    }

    fn from_genesis(snapshot: ArchiveSnapshot) -> Result<Self, Diagnostics> {
        check_aggregate(std::iter::once(snapshot.resource_bytes()))?;
        check_staged_tree(std::iter::once(&snapshot))?;
        let record = checkpoint(None, &snapshot.digest())?;
        let (manifest, json) = encode_history(vec![record])?;
        Ok(Self {
            state: HistoryState::Version2 {
                manifest,
                json,
                snapshots: vec![snapshot],
            },
        })
    }

    /// Verify every checkpoint from a single pinned root. A v1 archive is
    /// represented as a virtual genesis while retaining its original digest.
    pub(crate) fn verify(dir: &Path) -> Result<Self, Diagnostics> {
        let meta = fs::symlink_metadata(dir).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect archive root: {e}"),
            )
        })?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(fail(
                DiagnosticCode::Reference,
                "archive root must be a real directory",
            ));
        }
        let root = ProjectRoot::open_pinned(dir, cap_std::ambient_authority())?;
        Self::verify_in_root(&root)
    }

    fn verify_in_root(root: &ProjectRoot) -> Result<Self, Diagnostics> {
        let json = bounded_manifest(root)?;
        let probe: serde_json::Value = serde_json::from_slice(&json).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("invalid archive manifest: {e}"),
            )
        })?;
        match probe.get("version").and_then(serde_json::Value::as_u64) {
            Some(1) => {
                let preflight = ArchiveSnapshot::preflight_in_root(root)?;
                let snapshot =
                    ArchiveSnapshot::verify_preflighted_in_root(root, &preflight, || {})?;
                let genesis_id = checkpoint(None, &snapshot.digest())?.id;
                Ok(Self {
                    state: HistoryState::Legacy {
                        snapshot,
                        genesis_id,
                    },
                })
            }
            Some(2) => Self::verify_v2(root, json),
            _ => Err(fail(
                DiagnosticCode::Version,
                "unsupported editable archive version",
            )),
        }
    }

    fn verify_v2(root: &ProjectRoot, json: Vec<u8>) -> Result<Self, Diagnostics> {
        Self::verify_v2_with_hook(root, json, || {})
    }

    fn verify_v2_with_hook(
        root: &ProjectRoot,
        json: Vec<u8>,
        before_capture: impl FnOnce(),
    ) -> Result<Self, Diagnostics> {
        let manifest: HistoryManifest = serde_json::from_slice(&json).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("invalid v2 archive manifest: {e}"),
            )
        })?;
        validate_history(&manifest)?;
        if canonical_json(&manifest)? != json {
            return Err(fail(
                DiagnosticCode::Syntax,
                "v2 archive manifest is not canonical JSON",
            ));
        }

        // Check the root tree and every small v1 manifest before loading any
        // checkpoint media. Keep child handles so renaming the outer root
        // cannot redirect later closure reads to a different archive.
        let checkpoints_root = check_root_tree(root, &manifest)?;
        let mut children = Vec::with_capacity(manifest.checkpoints.len());
        let mut preflights = Vec::with_capacity(manifest.checkpoints.len());
        let mut tree_entries = 2usize + manifest.checkpoints.len();
        for record in &manifest.checkpoints {
            let child = checkpoints_root.open_child_pinned(&record.id[7..])?;
            let preflight = ArchiveSnapshot::preflight_in_root(&child)?;
            if preflight.digest() != record.snapshot {
                return Err(fail(
                    DiagnosticCode::Hash,
                    format!("checkpoint `{}` snapshot digest differs", record.id),
                ));
            }
            let child_entries = ArchiveSnapshot::tree_entries_in_root(&child, &preflight)?;
            tree_entries = tree_entries.checked_add(child_entries).ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "archive tree entry count overflow",
                )
            })?;
            if tree_entries > MAX_TREE_ENTRIES {
                return Err(fail(
                    DiagnosticCode::ResourceLimit,
                    "archive history contains too many filesystem entries",
                ));
            }
            children.push(child);
            preflights.push(preflight);
        }
        check_aggregate(preflights.iter().map(ArchivePreflight::resource_bytes))?;

        before_capture();
        let mut snapshots = Vec::with_capacity(manifest.checkpoints.len());
        for (child, preflight) in children.iter().zip(&preflights) {
            let snapshot = ArchiveSnapshot::verify_preflighted_in_root(child, preflight, || {})?;
            snapshots.push(snapshot);
        }
        if bounded_manifest(root)? != json {
            return Err(fail(
                DiagnosticCode::Hash,
                "v2 archive manifest changed during verification",
            ));
        }
        check_root_tree(root, &manifest)?;
        Ok(Self {
            state: HistoryState::Version2 {
                manifest,
                json,
                snapshots,
            },
        })
    }

    /// Append a new current composition. Identical head snapshots are a no-op.
    /// Appending to a v1 archive promotes its virtual genesis into v2 history.
    pub(crate) fn append(&mut self, snapshot: ArchiveSnapshot) -> Result<bool, Diagnostics> {
        match &mut self.state {
            HistoryState::Legacy {
                snapshot: old,
                genesis_id,
            } => {
                let changed = old.digest() != snapshot.digest();
                let genesis = checkpoint(None, &old.digest())?;
                debug_assert_eq!(genesis.id, *genesis_id);
                let mut records = vec![genesis];
                if changed {
                    records.push(checkpoint(Some(&records[0].id), &snapshot.digest())?);
                }
                let snapshots = if changed {
                    vec![old.clone(), snapshot]
                } else {
                    vec![old.clone()]
                };
                check_aggregate(snapshots.iter().map(ArchiveSnapshot::resource_bytes))?;
                check_staged_tree(snapshots.iter())?;
                let (manifest, json) = encode_history(records)?;
                self.state = HistoryState::Version2 {
                    manifest,
                    json,
                    snapshots,
                };
                Ok(changed)
            }
            HistoryState::Version2 {
                manifest,
                json,
                snapshots,
            } => {
                if snapshots
                    .last()
                    .is_some_and(|head| head.digest() == snapshot.digest())
                {
                    return Ok(false);
                }
                if snapshots.len() >= MAX_CHECKPOINTS {
                    return Err(fail(
                        DiagnosticCode::ResourceLimit,
                        "archive has reached 32 checkpoints",
                    ));
                }
                check_aggregate(
                    snapshots
                        .iter()
                        .map(ArchiveSnapshot::resource_bytes)
                        .chain(std::iter::once(snapshot.resource_bytes())),
                )?;
                check_staged_tree(snapshots.iter().chain(std::iter::once(&snapshot)))?;
                let mut records = manifest.checkpoints.clone();
                records.push(checkpoint(Some(&manifest.head), &snapshot.digest())?);
                let (new_manifest, new_json) = encode_history(records)?;
                *manifest = new_manifest;
                *json = new_json;
                snapshots.push(snapshot);
                Ok(true)
            }
        }
    }

    pub(crate) fn version(&self) -> u32 {
        match self.state {
            HistoryState::Legacy { .. } => 1,
            HistoryState::Version2 { .. } => VERSION,
        }
    }

    pub(crate) fn digest(&self) -> String {
        match &self.state {
            HistoryState::Legacy { snapshot, .. } => snapshot.digest(),
            HistoryState::Version2 { json, .. } => sha256_digest(json),
        }
    }

    #[cfg(test)]
    pub(crate) fn head_id(&self) -> &str {
        match &self.state {
            HistoryState::Legacy { genesis_id, .. } => genesis_id,
            HistoryState::Version2 { manifest, .. } => &manifest.head,
        }
    }

    #[cfg(test)]
    pub(crate) fn checkpoint_ids(&self) -> Vec<String> {
        match &self.state {
            HistoryState::Legacy { genesis_id, .. } => vec![genesis_id.clone()],
            HistoryState::Version2 { manifest, .. } => manifest
                .checkpoints
                .iter()
                .map(|record| record.id.clone())
                .collect(),
        }
    }

    pub(crate) fn select(&self, id: Option<&str>) -> Result<&ArchiveSnapshot, Diagnostics> {
        if let Some(id) = id {
            validate_hash_pin(id, "checkpoint id")?;
        }
        match &self.state {
            HistoryState::Legacy {
                snapshot,
                genesis_id,
            } => {
                if id.is_none_or(|id| id == genesis_id) {
                    Ok(snapshot)
                } else {
                    Err(fail(
                        DiagnosticCode::Reference,
                        "checkpoint id is not in this archive",
                    ))
                }
            }
            HistoryState::Version2 {
                manifest,
                snapshots,
                ..
            } => {
                let wanted = id.unwrap_or(&manifest.head);
                manifest
                    .checkpoints
                    .iter()
                    .position(|record| record.id == wanted)
                    .map(|index| &snapshots[index])
                    .ok_or_else(|| {
                        fail(
                            DiagnosticCode::Reference,
                            "checkpoint id is not in this archive",
                        )
                    })
            }
        }
    }

    /// Stage a complete archive. Legacy histories preserve their v1 wire form.
    pub(crate) fn stage(&self, dir: &Path) -> Result<(), Diagnostics> {
        match &self.state {
            HistoryState::Legacy { snapshot, .. } => snapshot.stage(dir),
            HistoryState::Version2 {
                manifest,
                json,
                snapshots,
            } => {
                check_staged_tree(snapshots.iter())?;
                check_output_root(dir)?;
                let checkpoints = dir.join("checkpoints");
                fs::create_dir(&checkpoints).map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot create checkpoints directory: {e}"),
                    )
                })?;
                for (record, snapshot) in manifest.checkpoints.iter().zip(snapshots) {
                    let child = checkpoints.join(&record.id[7..]);
                    fs::create_dir(&child).map_err(|e| {
                        fail(
                            DiagnosticCode::Reference,
                            format!("cannot create checkpoint directory: {e}"),
                        )
                    })?;
                    snapshot.stage(&child)?;
                }
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(dir.join(MANIFEST_PATH))
                    .map_err(|e| {
                        fail(
                            DiagnosticCode::Reference,
                            format!("cannot create history manifest: {e}"),
                        )
                    })?;
                file.write_all(json).map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot write history manifest: {e}"),
                    )
                })
            }
        }
    }
}

fn checkpoint(parent: Option<&str>, snapshot: &str) -> Result<CheckpointRecord, Diagnostics> {
    validate_hash_pin(snapshot, "checkpoint snapshot hash")?;
    if let Some(parent) = parent {
        validate_hash_pin(parent, "checkpoint parent id")?;
    }
    let identity = CheckpointIdentity {
        format: CHECKPOINT_FORMAT,
        version: 1,
        parent,
        snapshot,
    };
    let bytes = serde_json::to_vec(&identity).map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode checkpoint identity: {e}"),
        )
    })?;
    Ok(CheckpointRecord {
        id: sha256_digest(&bytes),
        parent: parent.map(str::to_owned),
        snapshot: snapshot.into(),
    })
}

fn encode_history(
    checkpoints: Vec<CheckpointRecord>,
) -> Result<(HistoryManifest, Vec<u8>), Diagnostics> {
    let head = checkpoints
        .last()
        .ok_or_else(|| fail(DiagnosticCode::Reference, "history has no checkpoints"))?
        .id
        .clone();
    let manifest = HistoryManifest {
        format: FORMAT.into(),
        version: VERSION,
        head,
        checkpoints,
    };
    validate_history(&manifest)?;
    let json = canonical_json(&manifest)?;
    if json.len() > MAX_MANIFEST_BYTES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "history manifest exceeds 1 MiB",
        ));
    }
    Ok((manifest, json))
}

fn canonical_json(manifest: &HistoryManifest) -> Result<Vec<u8>, Diagnostics> {
    serde_json::to_vec(manifest).map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode history manifest: {e}"),
        )
    })
}

fn validate_history(manifest: &HistoryManifest) -> Result<(), Diagnostics> {
    if manifest.format != FORMAT || manifest.version != VERSION {
        return Err(fail(
            DiagnosticCode::Version,
            "unsupported archive history format or version",
        ));
    }
    if manifest.checkpoints.is_empty() || manifest.checkpoints.len() > MAX_CHECKPOINTS {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "archive history requires 1 to 32 checkpoints",
        ));
    }
    let mut prior: Option<&str> = None;
    let mut seen = BTreeSet::new();
    for record in &manifest.checkpoints {
        validate_hash_pin(&record.id, "checkpoint id")?;
        validate_hash_pin(&record.snapshot, "checkpoint snapshot hash")?;
        if record.parent.as_deref() != prior {
            return Err(fail(
                DiagnosticCode::Reference,
                "checkpoint parents do not form a linear history",
            ));
        }
        if checkpoint(record.parent.as_deref(), &record.snapshot)?.id != record.id
            || !seen.insert(&record.id)
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "checkpoint id does not match its canonical identity",
            ));
        }
        prior = Some(&record.id);
    }
    if Some(manifest.head.as_str()) != prior {
        return Err(fail(
            DiagnosticCode::Reference,
            "archive head is not the final checkpoint",
        ));
    }
    Ok(())
}

fn check_aggregate(budgets: impl Iterator<Item = (u64, u64, u64)>) -> Result<(), Diagnostics> {
    let mut total = (0u64, 0u64, 0u64);
    for (source, asset, pcm) in budgets {
        total.0 = total
            .0
            .checked_add(source)
            .ok_or_else(|| fail(DiagnosticCode::ResourceLimit, "source total overflow"))?;
        total.1 = total
            .1
            .checked_add(asset)
            .ok_or_else(|| fail(DiagnosticCode::ResourceLimit, "asset total overflow"))?;
        total.2 = total
            .2
            .checked_add(pcm)
            .ok_or_else(|| fail(DiagnosticCode::ResourceLimit, "PCM total overflow"))?;
        if total.0 > MAX_SOURCE_BYTES || total.1 > MAX_ASSET_BYTES || total.2 > MAX_PCM_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive history exceeds aggregate byte limits",
            ));
        }
    }
    Ok(())
}

fn check_staged_tree<'a>(
    snapshots: impl Iterator<Item = &'a ArchiveSnapshot>,
) -> Result<(), Diagnostics> {
    check_staged_tree_with_limit(snapshots, MAX_TREE_ENTRIES)
}

fn check_staged_tree_with_limit<'a>(
    snapshots: impl Iterator<Item = &'a ArchiveSnapshot>,
    limit: usize,
) -> Result<(), Diagnostics> {
    let mut entries = 2usize; // root manifest and checkpoints directory
    for snapshot in snapshots {
        entries = entries
            .checked_add(1) // checkpoint directory
            .and_then(|n| n.checked_add(snapshot.projected_tree_entries()))
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "archive tree entry count overflow",
                )
            })?;
        if entries > limit {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive history contains too many filesystem entries",
            ));
        }
    }
    Ok(())
}

fn check_root_tree(
    root: &ProjectRoot,
    manifest: &HistoryManifest,
) -> Result<ProjectRoot, Diagnostics> {
    let mut names = BTreeSet::new();
    for entry in root.dir().read_dir(".").map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot list archive root: {e}"),
        )
    })? {
        let entry = entry.map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot list archive root: {e}"),
            )
        })?;
        let name = entry.file_name().into_string().map_err(|_| {
            fail(
                DiagnosticCode::Reference,
                "archive root has non-UTF-8 member",
            )
        })?;
        let kind = entry.file_type().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect archive root member: {e}"),
            )
        })?;
        if (name == MANIFEST_PATH && !kind.is_file())
            || (name == "checkpoints" && !kind.is_dir())
            || (name != MANIFEST_PATH && name != "checkpoints")
            || kind.is_symlink()
        {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("unexpected archive root member `{name}`"),
            ));
        }
        names.insert(name);
    }
    if names != BTreeSet::from([MANIFEST_PATH.to_owned(), "checkpoints".to_owned()]) {
        return Err(fail(
            DiagnosticCode::Reference,
            "archive root is missing required members",
        ));
    }
    let checkpoints_root = root.open_child_pinned("checkpoints")?;
    let expected: BTreeSet<String> = manifest
        .checkpoints
        .iter()
        .map(|record| record.id[7..].to_owned())
        .collect();
    let mut found = BTreeSet::new();
    for entry in checkpoints_root.dir().read_dir(".").map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot list checkpoints: {e}"),
        )
    })? {
        let entry = entry.map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot list checkpoints: {e}"),
            )
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| fail(DiagnosticCode::Reference, "checkpoint has non-UTF-8 name"))?;
        let kind = entry.file_type().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect checkpoint directory: {e}"),
            )
        })?;
        if !kind.is_dir() || kind.is_symlink() || !expected.contains(&name) {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("unexpected checkpoint directory `{name}`"),
            ));
        }
        found.insert(name);
    }
    if found != expected {
        return Err(fail(
            DiagnosticCode::Reference,
            "archive is missing a checkpoint directory",
        ));
    }
    Ok(checkpoints_root)
}

fn check_output_root(dir: &Path) -> Result<(), Diagnostics> {
    let meta = fs::symlink_metadata(dir).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot inspect output directory: {e}"),
        )
    })?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(fail(
            DiagnosticCode::Reference,
            "output root must be a real directory",
        ));
    }
    Ok(())
}

fn fail(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = include_str!("../tests/fixtures/archive_v1/main.maac");
    const V1_DIGEST: &str =
        "sha256:15d422b07aeb803b3f488c42a9b07d307f0cfdb78f09bd1173c4face9bb9842a";

    fn write_project(dir: &Path, source: &str) {
        fs::create_dir(dir).unwrap();
        fs::write(dir.join("main.maac"), source).unwrap();
    }

    fn capture(dir: &Path) -> ArchiveSnapshot {
        ArchiveSnapshot::capture(Path::new("main.maac"), dir, "default").unwrap()
    }

    #[test]
    fn fixed_v1_archive_keeps_digest_and_virtual_genesis() {
        let fixture = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/archive_v1"
        ));
        let mut history = ArchiveHistory::verify(fixture).unwrap();
        assert_eq!(history.version(), 1);
        assert_eq!(history.digest(), V1_DIGEST);
        let genesis = history.head_id().to_owned();
        assert_eq!(history.checkpoint_ids(), vec![genesis.clone()]);
        assert_eq!(history.select(Some(&genesis)).unwrap().digest(), V1_DIGEST);

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        write_project(&project, SOURCE);
        assert!(!history.append(capture(&project)).unwrap());
        assert_eq!(history.version(), 2);
        assert_eq!(history.checkpoint_ids(), vec![genesis]);
        let output = temp.path().join("v2");
        fs::create_dir(&output).unwrap();
        history.stage(&output).unwrap();
        assert_eq!(
            ArchiveHistory::verify(&output).unwrap().digest(),
            history.digest()
        );
    }

    #[test]
    fn append_select_noop_and_revert_are_linear() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        write_project(&a, SOURCE);
        let changed_source = format!("{SOURCE}// changed arrangement\n");
        write_project(&b, &changed_source);
        let mut history = ArchiveHistory::capture(Path::new("main.maac"), &a, "default").unwrap();
        assert_eq!(history.version(), 2);
        let first_id = history.head_id().to_owned();
        let original_digest = history.digest();
        assert!(!history.append(capture(&a)).unwrap());
        assert_eq!(history.digest(), original_digest);
        assert_eq!(history.checkpoint_ids().len(), 1);

        assert!(history.append(capture(&b)).unwrap());
        let second_id = history.head_id().to_owned();
        assert_ne!(first_id, second_id);
        assert_eq!(history.checkpoint_ids().len(), 2);
        let old = temp.path().join("old");
        fs::create_dir(&old).unwrap();
        history
            .select(Some(&first_id))
            .unwrap()
            .stage_members(&old)
            .unwrap();
        assert_eq!(fs::read_to_string(old.join("main.maac")).unwrap(), SOURCE);
        assert_eq!(history.select(None).unwrap().digest(), capture(&b).digest());

        assert!(history.append(capture(&a)).unwrap());
        assert_eq!(history.checkpoint_ids().len(), 3);
        assert_ne!(history.head_id(), first_id);
        let output = temp.path().join("history");
        fs::create_dir(&output).unwrap();
        history.stage(&output).unwrap();
        let verified = ArchiveHistory::verify(&output).unwrap();
        assert_eq!(verified.digest(), history.digest());
        assert_eq!(verified.checkpoint_ids(), history.checkpoint_ids());
        assert_eq!(
            verified.select(Some(&second_id)).unwrap().digest(),
            capture(&b).digest()
        );
    }

    #[test]
    fn tampered_non_head_checkpoint_fails_verification() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        write_project(&a, SOURCE);
        write_project(&b, &format!("{SOURCE}// changed\n"));
        let mut history = ArchiveHistory::capture(Path::new("main.maac"), &a, "default").unwrap();
        history.append(capture(&b)).unwrap();
        let first_id = history.checkpoint_ids()[0].clone();
        let output = temp.path().join("history");
        fs::create_dir(&output).unwrap();
        history.stage(&output).unwrap();
        fs::write(
            output
                .join("checkpoints")
                .join(&first_id[7..])
                .join("main.maac"),
            b"maac 1;\n",
        )
        .unwrap();
        assert!(ArchiveHistory::verify(&output).is_err());
    }

    #[test]
    fn child_roots_remain_pinned_when_outer_path_is_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        write_project(&a, SOURCE);
        write_project(&b, &format!("{SOURCE}// other archive\n"));
        let first = ArchiveHistory::capture(Path::new("main.maac"), &a, "default").unwrap();
        let second = ArchiveHistory::capture(Path::new("main.maac"), &b, "default").unwrap();
        let archive = temp.path().join("archive");
        let replacement = temp.path().join("replacement");
        let moved = temp.path().join("moved");
        fs::create_dir(&archive).unwrap();
        fs::create_dir(&replacement).unwrap();
        first.stage(&archive).unwrap();
        second.stage(&replacement).unwrap();
        let pinned = ProjectRoot::open_pinned(&archive, cap_std::ambient_authority()).unwrap();
        let json = bounded_manifest(&pinned).unwrap();
        let verified = ArchiveHistory::verify_v2_with_hook(&pinned, json, || {
            fs::rename(&archive, &moved).unwrap();
            fs::rename(&replacement, &archive).unwrap();
        })
        .unwrap();
        assert_eq!(verified.digest(), first.digest());
        assert_eq!(
            ArchiveHistory::verify(&archive).unwrap().digest(),
            second.digest()
        );
    }

    #[test]
    fn staged_tree_budget_counts_wrapper_and_unique_parent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::create_dir(project.join("nested")).unwrap();
        fs::write(project.join("nested/main.maac"), SOURCE).unwrap();
        let snapshot =
            ArchiveSnapshot::capture(Path::new("nested/main.maac"), &project, "default").unwrap();
        assert_eq!(snapshot.projected_tree_entries(), 3); // manifest, source, nested/
        check_staged_tree_with_limit(std::iter::once(&snapshot), 6).unwrap();
        assert!(check_staged_tree_with_limit(std::iter::once(&snapshot), 5).is_err());
        check_staged_tree_with_limit([&snapshot, &snapshot].into_iter(), 10).unwrap();
        assert!(check_staged_tree_with_limit([&snapshot, &snapshot].into_iter(), 9).is_err());
    }
}
