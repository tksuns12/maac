//! Immutable, linear archive checkpoints with closure and retained-import leaves.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::archive::{
    bounded_manifest, ArchivePreflight, ArchiveSnapshot, MANIFEST_PATH, MAX_TREE_ENTRIES,
};
use crate::archive_edit::{EditSnapshot, MAX_EDIT_TOTAL_BYTES, MAX_EDIT_TRANSACTION_WORK_BYTES};
use crate::bundle::{sha256_digest, validate_hash_pin};
use crate::bundle_fs::ProjectRoot;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::editing::AppliedTransaction;
use crate::freeze::{engine_digest, FreezeSnapshot};

const FORMAT: &str = "maac.editable-archive";
const CHECKPOINT_FORMAT: &str = "maac.archive-checkpoint";
const VERSION: u32 = 2;
const RETAINED_VERSION: u32 = 3;
const FROZEN_VERSION: u32 = 4;
const EDITED_VERSION: u32 = 5;
const MULTI_IMPORT_VERSION: u32 = 6;
const MAX_CHECKPOINTS: usize = 32;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PCM_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_EDIT_REPLAY_WORK_BYTES: u64 = 8 * 1024 * 1024 * 1024;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    freeze: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edit: Option<String>,
}

#[derive(Serialize)]
struct CheckpointIdentity<'a> {
    format: &'static str,
    version: u32,
    parent: Option<&'a str>,
    snapshot: &'a str,
}

#[derive(Serialize)]
struct FrozenCheckpointIdentity<'a> {
    format: &'static str,
    version: u32,
    parent: Option<&'a str>,
    snapshot: &'a str,
    freeze: &'a str,
}

#[derive(Serialize)]
struct EditedCheckpointIdentity<'a> {
    format: &'static str,
    version: u32,
    parent: Option<&'a str>,
    snapshot: &'a str,
    edit: &'a str,
}

enum HistoryState {
    Legacy {
        snapshot: Box<ArchiveSnapshot>,
        genesis_id: String,
    },
    Versioned {
        manifest: HistoryManifest,
        json: Vec<u8>,
        snapshots: Vec<ArchiveSnapshot>,
        freezes: Vec<Option<FreezeSnapshot>>,
        edits: Vec<Option<EditSnapshot>>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FreezeCheckStatus {
    Current,
    Stale,
}

pub(crate) struct FreezeCheck {
    pub(crate) status: FreezeCheckStatus,
    pub(crate) revision: String,
    pub(crate) source_digest: String,
    pub(crate) frozen_source_digest: String,
    pub(crate) output_digest: String,
    pub(crate) replay_matches: Option<bool>,
}

pub(crate) struct ArchivePatchResult {
    pub(crate) applied: AppliedTransaction,
    pub(crate) checkpoint: String,
    pub(crate) edit_digest: String,
}

/// A checked archive history retaining private snapshots for every checkpoint.
pub(crate) struct ArchiveHistory {
    state: HistoryState,
}

impl ArchiveHistory {
    /// Capture one current composition as v2 or retained-import v3 history.
    pub(crate) fn capture(entry: &Path, root: &Path, profile: &str) -> Result<Self, Diagnostics> {
        let snapshot = ArchiveSnapshot::capture(entry, root, profile)?;
        Self::from_genesis(snapshot, None)
    }

    pub(crate) fn capture_frozen(
        entry: &Path,
        root: &Path,
        profile: &str,
    ) -> Result<Self, Diagnostics> {
        let snapshot = ArchiveSnapshot::capture(entry, root, profile)?;
        let freeze = FreezeSnapshot::capture(&snapshot)?;
        Self::from_genesis(snapshot, Some(freeze))
    }

    pub(crate) fn from_snapshot(
        snapshot: ArchiveSnapshot,
        freeze_output: bool,
    ) -> Result<Self, Diagnostics> {
        let freeze = if freeze_output {
            Some(FreezeSnapshot::capture(&snapshot)?)
        } else {
            None
        };
        Self::from_genesis(snapshot, freeze)
    }

    fn from_genesis(
        snapshot: ArchiveSnapshot,
        freeze: Option<FreezeSnapshot>,
    ) -> Result<Self, Diagnostics> {
        check_aggregate_with_freezes(std::iter::once((
            snapshot.resource_bytes(),
            freeze.as_ref().map_or(0, FreezeSnapshot::output_bytes),
        )))?;
        check_staged_tree_with_freezes(std::iter::once(&snapshot), usize::from(freeze.is_some()))?;
        let record = checkpoint_with_freeze(
            None,
            &snapshot.digest(),
            freeze.as_ref().map(FreezeSnapshot::digest).as_deref(),
        )?;
        let version = if snapshot.is_multi_import() {
            MULTI_IMPORT_VERSION
        } else if freeze.is_some() {
            FROZEN_VERSION
        } else if snapshot.is_retained() {
            RETAINED_VERSION
        } else {
            VERSION
        };
        let (manifest, json) = encode_history(vec![record], version)?;
        Ok(Self {
            state: HistoryState::Versioned {
                manifest,
                json,
                snapshots: vec![snapshot],
                freezes: vec![freeze],
                edits: vec![None],
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
                        snapshot: Box::new(snapshot),
                        genesis_id,
                    },
                })
            }
            Some(2..=6) => Self::verify_versioned(root, json),
            _ => Err(fail(
                DiagnosticCode::Version,
                "unsupported editable archive version",
            )),
        }
    }

    fn verify_versioned(root: &ProjectRoot, json: Vec<u8>) -> Result<Self, Diagnostics> {
        Self::verify_versioned_with_hook(root, json, || {})
    }

    fn verify_versioned_with_hook(
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
        let (checkpoints_root, freezes_root, edits_root) = check_root_tree(root, &manifest)?;
        let mut children = Vec::with_capacity(manifest.checkpoints.len());
        let mut preflights = Vec::with_capacity(manifest.checkpoints.len());
        let mut freeze_children = Vec::with_capacity(manifest.checkpoints.len());
        let mut edit_children = Vec::with_capacity(manifest.checkpoints.len());
        let mut tree_entries = 2usize + manifest.checkpoints.len();
        if manifest
            .checkpoints
            .iter()
            .any(|record| record.freeze.is_some())
        {
            let unique: BTreeSet<&str> = manifest
                .checkpoints
                .iter()
                .filter_map(|record| record.freeze.as_deref())
                .collect();
            tree_entries = tree_entries
                .checked_add(1 + unique.len() * 3)
                .ok_or_else(|| {
                    fail(
                        DiagnosticCode::ResourceLimit,
                        "archive tree entry count overflow",
                    )
                })?;
        }
        if manifest
            .checkpoints
            .iter()
            .any(|record| record.edit.is_some())
        {
            let unique: BTreeSet<&str> = manifest
                .checkpoints
                .iter()
                .filter_map(|record| record.edit.as_deref())
                .collect();
            tree_entries = tree_entries
                .checked_add(1 + unique.len() * 4)
                .ok_or_else(|| {
                    fail(
                        DiagnosticCode::ResourceLimit,
                        "archive tree entry count overflow",
                    )
                })?;
        }
        if tree_entries > MAX_TREE_ENTRIES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive history contains too many filesystem entries",
            ));
        }
        for (index, record) in manifest.checkpoints.iter().enumerate() {
            let child = checkpoints_root.open_child_pinned(&record.id[7..])?;
            let preflight = ArchiveSnapshot::preflight_in_root(&child)?;
            if preflight.digest() != record.snapshot {
                return Err(fail(
                    DiagnosticCode::Hash,
                    format!("checkpoint `{}` snapshot digest differs", record.id),
                ));
            }
            if manifest.version == VERSION && preflight.is_retained() {
                return Err(fail(
                    DiagnosticCode::Version,
                    "v2 history cannot contain a retained-import leaf",
                ));
            }
            if manifest.version < MULTI_IMPORT_VERSION && preflight.is_multi_import() {
                return Err(fail(
                    DiagnosticCode::Version,
                    "older history cannot contain a multi-import leaf",
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
            if let Some(digest) = &record.freeze {
                let freeze_root = freezes_root
                    .as_ref()
                    .ok_or_else(|| {
                        fail(
                            DiagnosticCode::Reference,
                            "archive is missing freezes directory",
                        )
                    })?
                    .open_child_pinned(&digest[7..])?;
                let preflight = FreezeSnapshot::preflight(&freeze_root, digest, &record.snapshot)?;
                freeze_children.push(Some((freeze_root, preflight)));
            } else {
                freeze_children.push(None);
            }
            if let Some(digest) = &record.edit {
                let parent = record.parent.as_deref().ok_or_else(|| {
                    fail(
                        DiagnosticCode::Reference,
                        "genesis checkpoint cannot carry an edit",
                    )
                })?;
                let before = &manifest.checkpoints[index - 1].snapshot;
                let edit_root = edits_root
                    .as_ref()
                    .ok_or_else(|| {
                        fail(
                            DiagnosticCode::Reference,
                            "archive is missing edits directory",
                        )
                    })?
                    .open_child_pinned(&digest[7..])?;
                let preflight =
                    EditSnapshot::preflight(&edit_root, digest, parent, before, &record.snapshot)?;
                if preflight.source() != preflights[index - 1].entry()
                    || preflight.source() != preflights[index].entry()
                {
                    return Err(fail(
                        DiagnosticCode::Hash,
                        "edit source differs from checkpoint entries",
                    ));
                }
                edit_children.push(Some((edit_root, preflight)));
            } else {
                edit_children.push(None);
            }
        }
        if manifest.version == RETAINED_VERSION
            && !preflights.iter().any(ArchivePreflight::is_retained)
        {
            return Err(fail(
                DiagnosticCode::Version,
                "v3 history has no retained-import checkpoint",
            ));
        }
        if manifest.version == MULTI_IMPORT_VERSION
            && !preflights.iter().any(ArchivePreflight::is_multi_import)
        {
            return Err(fail(
                DiagnosticCode::Version,
                "v6 history has no multi-import checkpoint",
            ));
        }
        check_aggregate_with_freezes(preflights.iter().zip(&freeze_children).map(
            |(source, freeze)| {
                (
                    source.resource_bytes(),
                    freeze
                        .as_ref()
                        .map_or(0, |(_, preflight)| preflight.output_bytes()),
                )
            },
        ))?;
        check_edit_total(
            edit_children
                .iter()
                .filter_map(|child| child.as_ref().map(|(_, preflight)| preflight.total_bytes())),
        )?;
        check_edit_transaction_work(edit_children.iter().filter_map(|child| {
            child
                .as_ref()
                .map(|(_, preflight)| preflight.transaction_work())
        }))?;
        check_edit_replay_work(
            &manifest.checkpoints,
            preflights.iter().map(ArchivePreflight::resource_bytes),
        )?;

        before_capture();
        let mut snapshots = Vec::with_capacity(manifest.checkpoints.len());
        for (child, preflight) in children.iter().zip(&preflights) {
            let snapshot = ArchiveSnapshot::verify_preflighted_in_root(child, preflight, || {})?;
            snapshots.push(snapshot);
        }
        let mut freezes = Vec::with_capacity(freeze_children.len());
        for child in &freeze_children {
            freezes.push(match child {
                Some((root, preflight)) => {
                    Some(FreezeSnapshot::verify_preflighted(root, preflight)?)
                }
                None => None,
            });
        }
        let mut edits = Vec::with_capacity(edit_children.len());
        for child in &edit_children {
            edits.push(match child {
                Some((root, preflight)) => Some(EditSnapshot::verify_preflighted(root, preflight)?),
                None => None,
            });
        }
        for (index, edit) in edits.iter().enumerate() {
            if let Some(edit) = edit {
                edit.verify_transition(
                    manifest.checkpoints[index]
                        .parent
                        .as_deref()
                        .expect("validated edit parent"),
                    &snapshots[index - 1],
                    &snapshots[index],
                )?;
            }
        }
        if bounded_manifest(root)? != json {
            return Err(fail(
                DiagnosticCode::Hash,
                "v2 archive manifest changed during verification",
            ));
        }
        check_root_tree(root, &manifest)?;
        Ok(Self {
            state: HistoryState::Versioned {
                manifest,
                json,
                snapshots,
                freezes,
                edits,
            },
        })
    }

    /// Append a new current composition. Identical head snapshots are a no-op.
    /// Appending to a v1 archive promotes its virtual genesis into v2 history.
    pub(crate) fn append(&mut self, snapshot: ArchiveSnapshot) -> Result<bool, Diagnostics> {
        self.append_with_freeze(snapshot, None)
    }

    pub(crate) fn append_frozen(&mut self, snapshot: ArchiveSnapshot) -> Result<bool, Diagnostics> {
        let freeze = FreezeSnapshot::capture(&snapshot)?;
        self.append_with_freeze(snapshot, Some(freeze))
    }

    /// Apply one source-preserving Protocol 2 transaction to the archived head.
    /// An authored no-op still creates an auditable edited checkpoint.
    pub(crate) fn patch_head(
        &mut self,
        patch_bytes: &[u8],
    ) -> Result<ArchivePatchResult, Diagnostics> {
        let (parent, before) = match &self.state {
            HistoryState::Legacy {
                snapshot,
                genesis_id,
            } => (genesis_id.as_str(), snapshot.as_ref()),
            HistoryState::Versioned {
                manifest,
                snapshots,
                ..
            } => (
                manifest.head.as_str(),
                snapshots.last().expect("validated history has a head"),
            ),
        };
        let (after, edit, applied) = EditSnapshot::capture(parent, before, patch_bytes)?;
        let edit_digest = edit.digest();
        let record = checkpoint_with_edit(Some(parent), &after.digest(), &edit_digest)?;
        let checkpoint_id = record.id.clone();
        let (mut records, mut snapshots, mut freezes, mut edits) = match &self.state {
            HistoryState::Legacy { snapshot, .. } => (
                vec![checkpoint(None, &snapshot.digest())?],
                vec![snapshot.as_ref().clone()],
                vec![None],
                vec![None],
            ),
            HistoryState::Versioned {
                manifest,
                snapshots,
                freezes,
                edits,
                ..
            } => (
                manifest.checkpoints.clone(),
                snapshots.clone(),
                freezes.clone(),
                edits.clone(),
            ),
        };
        if snapshots.len() >= MAX_CHECKPOINTS {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive has reached 32 checkpoints",
            ));
        }
        records.push(record);
        snapshots.push(after);
        freezes.push(None);
        edits.push(Some(edit));
        check_aggregate_with_freezes(snapshots.iter().zip(&freezes).map(|(snapshot, freeze)| {
            (
                snapshot.resource_bytes(),
                freeze.as_ref().map_or(0, FreezeSnapshot::output_bytes),
            )
        }))?;
        check_edit_total(
            edits
                .iter()
                .filter_map(|edit| edit.as_ref().map(EditSnapshot::total_bytes)),
        )?;
        check_edit_transaction_work(
            edits
                .iter()
                .filter_map(|edit| edit.as_ref().map(EditSnapshot::transaction_work)),
        )?;
        check_edit_replay_work(
            &records,
            snapshots.iter().map(ArchiveSnapshot::resource_bytes),
        )?;
        check_staged_tree_with_extras(
            snapshots.iter(),
            unique_freeze_count(&freezes, None),
            unique_edit_count(&edits, None),
        )?;
        let version = if snapshots.iter().any(ArchiveSnapshot::is_multi_import) {
            MULTI_IMPORT_VERSION
        } else {
            EDITED_VERSION
        };
        let (manifest, json) = encode_history(records, version)?;
        self.state = HistoryState::Versioned {
            manifest,
            json,
            snapshots,
            freezes,
            edits,
        };
        Ok(ArchivePatchResult {
            applied,
            checkpoint: checkpoint_id,
            edit_digest,
        })
    }

    fn append_with_freeze(
        &mut self,
        snapshot: ArchiveSnapshot,
        freeze: Option<FreezeSnapshot>,
    ) -> Result<bool, Diagnostics> {
        if freeze
            .as_ref()
            .is_some_and(|f| f.source_digest() != snapshot.digest())
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "freeze source does not match appended snapshot",
            ));
        }
        let freeze_digest = freeze.as_ref().map(FreezeSnapshot::digest);
        match &mut self.state {
            HistoryState::Legacy {
                snapshot: old,
                genesis_id,
            } => {
                let changed = old.digest() != snapshot.digest() || freeze.is_some();
                let genesis = checkpoint(None, &old.digest())?;
                debug_assert_eq!(genesis.id, *genesis_id);
                let mut records = vec![genesis];
                if changed {
                    records.push(checkpoint_with_freeze(
                        Some(&records[0].id),
                        &snapshot.digest(),
                        freeze_digest.as_deref(),
                    )?);
                }
                let snapshots = if changed {
                    vec![old.as_ref().clone(), snapshot]
                } else {
                    vec![old.as_ref().clone()]
                };
                let freezes = if changed {
                    vec![None, freeze]
                } else {
                    vec![None]
                };
                let edits = vec![None; snapshots.len()];
                check_aggregate_with_freezes(snapshots.iter().zip(&freezes).map(
                    |(snapshot, freeze)| {
                        (
                            snapshot.resource_bytes(),
                            freeze.as_ref().map_or(0, FreezeSnapshot::output_bytes),
                        )
                    },
                ))?;
                check_staged_tree_with_freezes(
                    snapshots.iter(),
                    unique_freeze_count(&freezes, None),
                )?;
                let version = if snapshots.iter().any(ArchiveSnapshot::is_multi_import) {
                    MULTI_IMPORT_VERSION
                } else if freezes.iter().any(Option::is_some) {
                    FROZEN_VERSION
                } else if snapshots.iter().any(ArchiveSnapshot::is_retained) {
                    RETAINED_VERSION
                } else {
                    VERSION
                };
                let (manifest, json) = encode_history(records, version)?;
                self.state = HistoryState::Versioned {
                    manifest,
                    json,
                    snapshots,
                    freezes,
                    edits,
                };
                Ok(changed)
            }
            HistoryState::Versioned {
                manifest,
                json,
                snapshots,
                freezes,
                edits,
            } => {
                if snapshots
                    .last()
                    .is_some_and(|head| head.digest() == snapshot.digest())
                    && (freeze_digest.is_none()
                        || freezes
                            .last()
                            .and_then(Option::as_ref)
                            .is_some_and(|head| Some(head.digest()) == freeze_digest))
                {
                    return Ok(false);
                }
                if snapshots.len() >= MAX_CHECKPOINTS {
                    return Err(fail(
                        DiagnosticCode::ResourceLimit,
                        "archive has reached 32 checkpoints",
                    ));
                }
                check_aggregate_with_freezes(
                    snapshots
                        .iter()
                        .zip(freezes.iter())
                        .map(|(snapshot, freeze)| {
                            (
                                snapshot.resource_bytes(),
                                freeze.as_ref().map_or(0, FreezeSnapshot::output_bytes),
                            )
                        })
                        .chain(std::iter::once((
                            snapshot.resource_bytes(),
                            freeze.as_ref().map_or(0, FreezeSnapshot::output_bytes),
                        ))),
                )?;
                check_staged_tree_with_extras(
                    snapshots.iter().chain(std::iter::once(&snapshot)),
                    unique_freeze_count(freezes, freeze.as_ref()),
                    unique_edit_count(edits, None),
                )?;
                let mut records = manifest.checkpoints.clone();
                records.push(checkpoint_with_freeze(
                    Some(&manifest.head),
                    &snapshot.digest(),
                    freeze_digest.as_deref(),
                )?);
                let version =
                    if manifest.version == MULTI_IMPORT_VERSION || snapshot.is_multi_import() {
                        MULTI_IMPORT_VERSION
                    } else if manifest.version == EDITED_VERSION {
                        EDITED_VERSION
                    } else if manifest.version == FROZEN_VERSION || freeze.is_some() {
                        FROZEN_VERSION
                    } else if manifest.version == RETAINED_VERSION || snapshot.is_retained() {
                        RETAINED_VERSION
                    } else {
                        VERSION
                    };
                let (new_manifest, new_json) = encode_history(records, version)?;
                *manifest = new_manifest;
                *json = new_json;
                snapshots.push(snapshot);
                freezes.push(freeze);
                edits.push(None);
                Ok(true)
            }
        }
    }

    pub(crate) fn version(&self) -> u32 {
        match self.state {
            HistoryState::Legacy { .. } => 1,
            HistoryState::Versioned { ref manifest, .. } => manifest.version,
        }
    }

    pub(crate) fn digest(&self) -> String {
        match &self.state {
            HistoryState::Legacy { snapshot, .. } => snapshot.digest(),
            HistoryState::Versioned { json, .. } => sha256_digest(json),
        }
    }

    #[cfg(test)]
    pub(crate) fn head_id(&self) -> &str {
        match &self.state {
            HistoryState::Legacy { genesis_id, .. } => genesis_id,
            HistoryState::Versioned { manifest, .. } => &manifest.head,
        }
    }

    #[cfg(test)]
    pub(crate) fn checkpoint_ids(&self) -> Vec<String> {
        match &self.state {
            HistoryState::Legacy { genesis_id, .. } => vec![genesis_id.clone()],
            HistoryState::Versioned { manifest, .. } => manifest
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
            HistoryState::Versioned {
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

    /// Check whether a candidate still names the frozen source and current
    /// executable context. Optional replay verifies its whole WAV output.
    pub(crate) fn freeze_check(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        replay: bool,
    ) -> Result<FreezeCheck, Diagnostics> {
        if let Some(id) = revision {
            validate_hash_pin(id, "checkpoint id")?;
        }
        let (record, freeze) = match &self.state {
            HistoryState::Legacy { .. } => {
                return Err(fail(
                    DiagnosticCode::Reference,
                    "selected checkpoint has no output freeze",
                ))
            }
            HistoryState::Versioned {
                manifest, freezes, ..
            } => {
                let wanted = revision.unwrap_or(&manifest.head);
                let index = manifest
                    .checkpoints
                    .iter()
                    .position(|r| r.id == wanted)
                    .ok_or_else(|| {
                        fail(
                            DiagnosticCode::Reference,
                            "checkpoint id is not in this archive",
                        )
                    })?;
                let freeze = freezes[index].as_ref().ok_or_else(|| {
                    fail(
                        DiagnosticCode::Reference,
                        "selected checkpoint has no output freeze",
                    )
                })?;
                (&manifest.checkpoints[index], freeze)
            }
        };
        let reference = self.select(Some(&record.id))?;
        let (candidate, missing_selected_import) =
            match ArchiveSnapshot::capture_matching_layout(entry, root, reference) {
                Ok(candidate) => (candidate, false),
                Err(_) if reference.is_multi_import() => (
                    ArchiveSnapshot::capture_closure_only(entry, root, reference.profile())?,
                    true,
                ),
                Err(error) => return Err(error),
            };
        let source_digest = candidate.digest();
        let status = if !missing_selected_import
            && source_digest == record.snapshot
            && freeze.engine_digest() == engine_digest()?
            && FreezeSnapshot::candidate_render_key(&candidate, freeze.engine_digest())?
                == freeze.render_key()
        {
            FreezeCheckStatus::Current
        } else {
            FreezeCheckStatus::Stale
        };
        let replay_matches = if replay && status == FreezeCheckStatus::Current {
            Some(FreezeSnapshot::capture(&candidate)?.output_digest() == freeze.output_digest())
        } else {
            None
        };
        Ok(FreezeCheck {
            status,
            revision: record.id.clone(),
            source_digest,
            frozen_source_digest: record.snapshot.clone(),
            output_digest: freeze.output_digest().into(),
            replay_matches,
        })
    }

    /// Stage a complete archive. Legacy histories preserve their v1 wire form.
    pub(crate) fn stage(&self, dir: &Path) -> Result<(), Diagnostics> {
        match &self.state {
            HistoryState::Legacy { snapshot, .. } => snapshot.stage(dir),
            HistoryState::Versioned {
                manifest,
                json,
                snapshots,
                freezes,
                edits,
            } => {
                check_staged_tree_with_extras(
                    snapshots.iter(),
                    unique_freeze_count(freezes, None),
                    unique_edit_count(edits, None),
                )?;
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
                if manifest
                    .checkpoints
                    .iter()
                    .any(|record| record.freeze.is_some())
                {
                    let root = dir.join("freezes");
                    fs::create_dir(&root).map_err(|e| {
                        fail(
                            DiagnosticCode::Reference,
                            format!("cannot create freezes directory: {e}"),
                        )
                    })?;
                    let mut staged = BTreeSet::new();
                    for (record, freeze) in manifest.checkpoints.iter().zip(freezes) {
                        if let (Some(digest), Some(freeze)) = (&record.freeze, freeze) {
                            if staged.insert(digest.clone()) {
                                let child = root.join(&digest[7..]);
                                fs::create_dir(&child).map_err(|e| {
                                    fail(
                                        DiagnosticCode::Reference,
                                        format!("cannot create freeze directory: {e}"),
                                    )
                                })?;
                                freeze.stage_into(&child)?;
                            }
                        }
                    }
                }
                if manifest
                    .checkpoints
                    .iter()
                    .any(|record| record.edit.is_some())
                {
                    let root = dir.join("edits");
                    fs::create_dir(&root).map_err(|e| {
                        fail(
                            DiagnosticCode::Reference,
                            format!("cannot create edits directory: {e}"),
                        )
                    })?;
                    let mut staged = BTreeSet::new();
                    for (record, edit) in manifest.checkpoints.iter().zip(edits) {
                        if let (Some(digest), Some(edit)) = (&record.edit, edit) {
                            if staged.insert(digest.clone()) {
                                let child = root.join(&digest[7..]);
                                fs::create_dir(&child).map_err(|e| {
                                    fail(
                                        DiagnosticCode::Reference,
                                        format!("cannot create edit directory: {e}"),
                                    )
                                })?;
                                edit.stage_into(&child)?;
                            }
                        }
                    }
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
    checkpoint_with_freeze(parent, snapshot, None)
}

fn checkpoint_with_freeze(
    parent: Option<&str>,
    snapshot: &str,
    freeze: Option<&str>,
) -> Result<CheckpointRecord, Diagnostics> {
    validate_hash_pin(snapshot, "checkpoint snapshot hash")?;
    if let Some(parent) = parent {
        validate_hash_pin(parent, "checkpoint parent id")?;
    }
    if let Some(freeze) = freeze {
        validate_hash_pin(freeze, "checkpoint freeze digest")?;
    }
    let bytes = if let Some(freeze) = freeze {
        serde_json::to_vec(&FrozenCheckpointIdentity {
            format: CHECKPOINT_FORMAT,
            version: 2,
            parent,
            snapshot,
            freeze,
        })
    } else {
        serde_json::to_vec(&CheckpointIdentity {
            format: CHECKPOINT_FORMAT,
            version: 1,
            parent,
            snapshot,
        })
    }
    .map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode checkpoint identity: {e}"),
        )
    })?;
    Ok(CheckpointRecord {
        id: sha256_digest(&bytes),
        parent: parent.map(str::to_owned),
        snapshot: snapshot.into(),
        freeze: freeze.map(str::to_owned),
        edit: None,
    })
}

fn checkpoint_with_edit(
    parent: Option<&str>,
    snapshot: &str,
    edit: &str,
) -> Result<CheckpointRecord, Diagnostics> {
    validate_hash_pin(snapshot, "checkpoint snapshot hash")?;
    validate_hash_pin(edit, "checkpoint edit digest")?;
    let parent = parent.ok_or_else(|| {
        fail(
            DiagnosticCode::Reference,
            "genesis checkpoint cannot carry an edit",
        )
    })?;
    validate_hash_pin(parent, "checkpoint parent id")?;
    let bytes = serde_json::to_vec(&EditedCheckpointIdentity {
        format: CHECKPOINT_FORMAT,
        version: 3,
        parent: Some(parent),
        snapshot,
        edit,
    })
    .map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode edited checkpoint identity: {e}"),
        )
    })?;
    Ok(CheckpointRecord {
        id: sha256_digest(&bytes),
        parent: Some(parent.into()),
        snapshot: snapshot.into(),
        freeze: None,
        edit: Some(edit.into()),
    })
}

fn encode_history(
    checkpoints: Vec<CheckpointRecord>,
    version: u32,
) -> Result<(HistoryManifest, Vec<u8>), Diagnostics> {
    let head = checkpoints
        .last()
        .ok_or_else(|| fail(DiagnosticCode::Reference, "history has no checkpoints"))?
        .id
        .clone();
    let manifest = HistoryManifest {
        format: FORMAT.into(),
        version,
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
    if manifest.format != FORMAT
        || !matches!(
            manifest.version,
            VERSION | RETAINED_VERSION | FROZEN_VERSION | EDITED_VERSION | MULTI_IMPORT_VERSION
        )
    {
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
        if record.freeze.is_some() && manifest.version < FROZEN_VERSION {
            return Err(fail(
                DiagnosticCode::Version,
                "v2/v3 history cannot contain freezes",
            ));
        }
        if record.edit.is_some()
            && manifest.version != EDITED_VERSION
            && manifest.version != MULTI_IMPORT_VERSION
        {
            return Err(fail(
                DiagnosticCode::Version,
                "older archive history cannot contain edits",
            ));
        }
        if record.edit.is_some() && (record.parent.is_none() || record.freeze.is_some()) {
            return Err(fail(
                DiagnosticCode::Reference,
                "edited checkpoint requires a parent and cannot inherit a freeze",
            ));
        }
        if record.parent.as_deref() != prior {
            return Err(fail(
                DiagnosticCode::Reference,
                "checkpoint parents do not form a linear history",
            ));
        }
        let expected = if let Some(edit) = &record.edit {
            checkpoint_with_edit(record.parent.as_deref(), &record.snapshot, edit)?
        } else {
            checkpoint_with_freeze(
                record.parent.as_deref(),
                &record.snapshot,
                record.freeze.as_deref(),
            )?
        };
        if expected.id != record.id || !seen.insert(&record.id) {
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
    if manifest.version == FROZEN_VERSION
        && !manifest.checkpoints.iter().any(|r| r.freeze.is_some())
    {
        return Err(fail(
            DiagnosticCode::Version,
            "v4 history has no frozen checkpoint",
        ));
    }
    if manifest.version == EDITED_VERSION && !manifest.checkpoints.iter().any(|r| r.edit.is_some())
    {
        return Err(fail(
            DiagnosticCode::Version,
            "v5 history has no edited checkpoint",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn check_aggregate(budgets: impl Iterator<Item = (u64, u64, u64, u64)>) -> Result<(), Diagnostics> {
    check_aggregate_with_freezes(budgets.map(|budget| (budget, 0)))
}

fn check_aggregate_with_freezes(
    budgets: impl Iterator<Item = ((u64, u64, u64, u64), u64)>,
) -> Result<(), Diagnostics> {
    let mut total = (0u64, 0u64, 0u64);
    for ((source, asset, pcm, original), freeze) in budgets {
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
            .and_then(|n| n.checked_add(original))
            .and_then(|n| n.checked_add(freeze))
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "PCM, original, and freeze total overflow",
                )
            })?;
        if total.0 > MAX_SOURCE_BYTES || total.1 > MAX_ASSET_BYTES || total.2 > MAX_PCM_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive history exceeds aggregate byte limits",
            ));
        }
    }
    Ok(())
}

fn check_staged_tree_with_freezes<'a>(
    snapshots: impl Iterator<Item = &'a ArchiveSnapshot>,
    freeze_count: usize,
) -> Result<(), Diagnostics> {
    check_staged_tree_with_extras(snapshots, freeze_count, 0)
}

fn check_staged_tree_with_extras<'a>(
    snapshots: impl Iterator<Item = &'a ArchiveSnapshot>,
    freeze_count: usize,
    edit_count: usize,
) -> Result<(), Diagnostics> {
    let freeze_overhead = if freeze_count == 0 {
        0
    } else {
        1usize
            .checked_add(freeze_count.checked_mul(3).ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "archive tree entry count overflow",
                )
            })?)
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "archive tree entry count overflow",
                )
            })?
    };
    let edit_overhead = if edit_count == 0 {
        0
    } else {
        1usize
            .checked_add(edit_count.checked_mul(4).ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "archive tree entry count overflow",
                )
            })?)
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "archive tree entry count overflow",
                )
            })?
    };
    let overhead = freeze_overhead.checked_add(edit_overhead).ok_or_else(|| {
        fail(
            DiagnosticCode::ResourceLimit,
            "archive tree entry count overflow",
        )
    })?;
    let limit = MAX_TREE_ENTRIES.checked_sub(overhead).ok_or_else(|| {
        fail(
            DiagnosticCode::ResourceLimit,
            "archive history contains too many filesystem entries",
        )
    })?;
    check_staged_tree_with_limit(snapshots, limit)
}

fn unique_edit_count(edits: &[Option<EditSnapshot>], extra: Option<&EditSnapshot>) -> usize {
    edits
        .iter()
        .filter_map(Option::as_ref)
        .chain(extra)
        .map(EditSnapshot::digest)
        .collect::<BTreeSet<_>>()
        .len()
}

fn check_edit_total(bytes: impl Iterator<Item = u64>) -> Result<(), Diagnostics> {
    let mut total = 0u64;
    for amount in bytes {
        total = total.checked_add(amount).ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "archive edit file total overflow",
            )
        })?;
        if total > MAX_EDIT_TOTAL_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive edits exceed 64 MiB",
            ));
        }
    }
    Ok(())
}

fn check_edit_transaction_work(
    amounts: impl Iterator<Item = Result<u64, Diagnostics>>,
) -> Result<(), Diagnostics> {
    let mut total = 0u64;
    for amount in amounts {
        total = total.checked_add(amount?).ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "archive edit transaction work overflow",
            )
        })?;
        if total > MAX_EDIT_TRANSACTION_WORK_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive edits exceed 2 GiB aggregate transaction work preflight",
            ));
        }
    }
    Ok(())
}

fn check_edit_replay_work(
    records: &[CheckpointRecord],
    budgets: impl Iterator<Item = (u64, u64, u64, u64)>,
) -> Result<(), Diagnostics> {
    let budgets: Vec<_> = budgets.collect();
    if budgets.len() != records.len() {
        return Err(fail(
            DiagnosticCode::Reference,
            "edit replay budget has mismatched checkpoints",
        ));
    }
    let mut total = 0u64;
    for (index, record) in records.iter().enumerate().skip(1) {
        if record.edit.is_none() {
            continue;
        }
        let before = resource_sum(budgets[index - 1])?;
        let after = resource_sum(budgets[index])?;
        let work = before
            .checked_mul(2)
            .and_then(|n| n.checked_add(after))
            .ok_or_else(|| fail(DiagnosticCode::ResourceLimit, "edit replay work overflow"))?;
        total = total
            .checked_add(work)
            .ok_or_else(|| fail(DiagnosticCode::ResourceLimit, "edit replay work overflow"))?;
        if total > MAX_EDIT_REPLAY_WORK_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive edits exceed 8 GiB replay work budget",
            ));
        }
    }
    Ok(())
}

fn resource_sum((source, asset, pcm, original): (u64, u64, u64, u64)) -> Result<u64, Diagnostics> {
    source
        .checked_add(asset)
        .and_then(|n| n.checked_add(pcm))
        .and_then(|n| n.checked_add(original))
        .ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "archive resource total overflow",
            )
        })
}

fn unique_freeze_count(
    freezes: &[Option<FreezeSnapshot>],
    extra: Option<&FreezeSnapshot>,
) -> usize {
    freezes
        .iter()
        .filter_map(Option::as_ref)
        .chain(extra)
        .map(FreezeSnapshot::digest)
        .collect::<BTreeSet<_>>()
        .len()
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
) -> Result<(ProjectRoot, Option<ProjectRoot>, Option<ProjectRoot>), Diagnostics> {
    let has_freezes = manifest
        .checkpoints
        .iter()
        .any(|record| record.freeze.is_some());
    let has_edits = manifest
        .checkpoints
        .iter()
        .any(|record| record.edit.is_some());
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
            || (name == "freezes" && (!has_freezes || !kind.is_dir()))
            || (name == "edits" && (!has_edits || !kind.is_dir()))
            || (name != MANIFEST_PATH
                && name != "checkpoints"
                && name != "freezes"
                && name != "edits")
            || kind.is_symlink()
        {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("unexpected archive root member `{name}`"),
            ));
        }
        names.insert(name);
    }
    let mut required = BTreeSet::from([MANIFEST_PATH.to_owned(), "checkpoints".to_owned()]);
    if has_freezes {
        required.insert("freezes".to_owned());
    }
    if has_edits {
        required.insert("edits".to_owned());
    }
    if names != required {
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
    let freezes_root = if has_freezes {
        let freezes_root = root.open_child_pinned("freezes")?;
        let expected: BTreeSet<String> = manifest
            .checkpoints
            .iter()
            .filter_map(|record| record.freeze.as_ref().map(|id| id[7..].to_owned()))
            .collect();
        let mut found = BTreeSet::new();
        for entry in freezes_root.dir().read_dir(".").map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot list freezes: {e}"),
            )
        })? {
            let entry = entry.map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot list freezes: {e}"),
                )
            })?;
            let name = entry.file_name().into_string().map_err(|_| {
                fail(
                    DiagnosticCode::Reference,
                    "freeze directory has non-UTF-8 name",
                )
            })?;
            let kind = entry.file_type().map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot inspect freeze directory: {e}"),
                )
            })?;
            if !kind.is_dir() || kind.is_symlink() || !expected.contains(&name) {
                return Err(fail(
                    DiagnosticCode::Reference,
                    format!("unexpected freeze directory `{name}`"),
                ));
            }
            found.insert(name);
        }
        if found != expected {
            return Err(fail(
                DiagnosticCode::Reference,
                "archive is missing a freeze directory",
            ));
        }
        Some(freezes_root)
    } else {
        None
    };
    let edits_root = if has_edits {
        let edits_root = root.open_child_pinned("edits")?;
        let expected: BTreeSet<String> = manifest
            .checkpoints
            .iter()
            .filter_map(|record| record.edit.as_ref().map(|id| id[7..].to_owned()))
            .collect();
        let mut found = BTreeSet::new();
        for entry in edits_root
            .dir()
            .read_dir(".")
            .map_err(|e| fail(DiagnosticCode::Reference, format!("cannot list edits: {e}")))?
        {
            let entry = entry
                .map_err(|e| fail(DiagnosticCode::Reference, format!("cannot list edits: {e}")))?;
            let name = entry.file_name().into_string().map_err(|_| {
                fail(
                    DiagnosticCode::Reference,
                    "edit directory has non-UTF-8 name",
                )
            })?;
            let kind = entry.file_type().map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot inspect edit directory: {e}"),
                )
            })?;
            if !kind.is_dir() || kind.is_symlink() || !expected.contains(&name) {
                return Err(fail(
                    DiagnosticCode::Reference,
                    format!("unexpected edit directory `{name}`"),
                ));
            }
            found.insert(name);
        }
        if found != expected {
            return Err(fail(
                DiagnosticCode::Reference,
                "archive is missing an edit directory",
            ));
        }
        Some(edits_root)
    } else {
        None
    };
    Ok((checkpoints_root, freezes_root, edits_root))
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
    use crate::editing::{Operation, SourceDocument, Transaction};
    use crate::media_import::{import_wav_file_with_retention, WavImportRange};
    use hound::{SampleFormat, WavSpec, WavWriter};

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

    fn retained_project(root: &Path) -> std::path::PathBuf {
        let input = root.join("input.wav");
        let mut writer = WavWriter::create(
            &input,
            WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [1_000i16, 2_000, -3_000, 4_000] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let imported =
            import_wav_file_with_retention(&input, Some(WavImportRange::new(1, 3)), true).unwrap();
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        let bundle = imported.source_bundle();
        fs::write(project.join("main.maac"), &bundle.sources["main.maac"]).unwrap();
        fs::write(project.join("media.pcm"), &bundle.assets["media.pcm"]).unwrap();
        fs::write(
            project.join("import.json"),
            imported.manifest_json().unwrap(),
        )
        .unwrap();
        imported
            .copy_original_to(&project.join("original.wav"))
            .unwrap();
        project
    }

    #[test]
    fn selected_nested_import_is_preserved_and_verified_as_v6() {
        let temp = tempfile::tempdir().unwrap();
        let project = retained_project(temp.path());
        let nested = project.join("imports/drums");
        fs::create_dir_all(&nested).unwrap();
        for name in ["media.pcm", "import.json", "original.wav"] {
            fs::rename(project.join(name), nested.join(name)).unwrap();
        }
        let source = fs::read_to_string(project.join("main.maac")).unwrap();
        let rewritten =
            source.replace("path = \"media.pcm\"", "path = \"imports/drums/media.pcm\"");
        assert_ne!(source, rewritten);
        fs::write(project.join("main.maac"), rewritten).unwrap();
        let snapshot = ArchiveSnapshot::capture_with_imports(
            Path::new("main.maac"),
            &project,
            "default",
            &["imports/drums".into()],
        )
        .unwrap();
        assert!(snapshot.is_multi_import());
        let history = ArchiveHistory::from_snapshot(snapshot, false).unwrap();
        assert_eq!(history.version(), 6);
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        let verified = ArchiveHistory::verify(&archive).unwrap();
        assert_eq!(verified.digest(), history.digest());
        assert_eq!(
            verified.select(None).unwrap().digest(),
            history.select(None).unwrap().digest()
        );
        fs::write(
            archive
                .join("checkpoints")
                .join(&history.head_id()[7..])
                .join("imports/drums/original.wav"),
            b"tampered",
        )
        .unwrap();
        assert!(ArchiveHistory::verify(&archive).is_err());
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
        let verified = ArchiveHistory::verify_versioned_with_hook(&pinned, json, || {
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

    #[test]
    fn aggregate_media_budget_combines_pcm_and_retained_original() {
        check_aggregate([(0, 0, MAX_PCM_BYTES - 1, 1)].into_iter()).unwrap();
        assert!(check_aggregate([(0, 0, MAX_PCM_BYTES, 1)].into_iter()).is_err());
        assert!(check_aggregate(
            [(0, 0, MAX_PCM_BYTES / 2, MAX_PCM_BYTES / 2), (0, 0, 1, 0)].into_iter()
        )
        .is_err());
    }

    #[test]
    fn retained_leaf_preserves_sidecars_and_v2_rejects_it() {
        let temp = tempfile::tempdir().unwrap();
        let project = retained_project(temp.path());
        let history = ArchiveHistory::capture(Path::new("main.maac"), &project, "default").unwrap();
        assert_eq!(history.version(), 3);
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        let verified = ArchiveHistory::verify(&archive).unwrap();
        assert_eq!(verified.digest(), history.digest());
        let unpacked = temp.path().join("unpacked");
        fs::create_dir(&unpacked).unwrap();
        verified
            .select(None)
            .unwrap()
            .stage_members(&unpacked)
            .unwrap();
        for name in ["main.maac", "media.pcm", "import.json", "original.wav"] {
            assert_eq!(
                fs::read(project.join(name)).unwrap(),
                fs::read(unpacked.join(name)).unwrap()
            );
        }
        let mut downgraded: HistoryManifest =
            serde_json::from_slice(&fs::read(archive.join(MANIFEST_PATH)).unwrap()).unwrap();
        downgraded.version = 2;
        fs::write(
            archive.join(MANIFEST_PATH),
            canonical_json(&downgraded).unwrap(),
        )
        .unwrap();
        assert!(ArchiveHistory::verify(&archive).is_err());
    }

    #[test]
    fn retained_original_change_outside_crop_invalidates_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let project = retained_project(temp.path());
        let history = ArchiveHistory::capture(Path::new("main.maac"), &project, "default").unwrap();
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        let checkpoint = &history.checkpoint_ids()[0][7..];
        let original = archive
            .join("checkpoints")
            .join(checkpoint)
            .join("original.wav");
        let mut bytes = fs::read(&original).unwrap();
        bytes[44] ^= 1; // first sample lies before the declared [1,3) crop.
        fs::write(original, bytes).unwrap();
        assert!(ArchiveHistory::verify(&archive).is_err());
    }

    #[test]
    fn frozen_checkpoint_is_v4_and_plain_append_preserves_old_freeze() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        write_project(&a, SOURCE);
        write_project(&b, &format!("{SOURCE}// later source\n"));
        let mut history =
            ArchiveHistory::capture_frozen(Path::new("main.maac"), &a, "default").unwrap();
        assert_eq!(history.version(), 4);
        let frozen_id = history.head_id().to_owned();
        assert!(!history.append_frozen(capture(&a)).unwrap());
        assert!(history.append(capture(&b)).unwrap());
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        let verified = ArchiveHistory::verify(&archive).unwrap();
        assert_eq!(verified.digest(), history.digest());
        assert_eq!(verified.checkpoint_ids()[0], frozen_id);
        assert_eq!(
            verified
                .freeze_check(Path::new("main.maac"), &a, Some(&frozen_id), true)
                .unwrap()
                .status,
            FreezeCheckStatus::Current
        );
        assert_eq!(
            verified
                .freeze_check(Path::new("main.maac"), &b, Some(&frozen_id), false)
                .unwrap()
                .status,
            FreezeCheckStatus::Stale
        );
        assert!(verified
            .freeze_check(Path::new("main.maac"), &a, None, false)
            .is_err());
    }

    #[test]
    fn edit_promotes_v1_and_records_even_an_authored_noop() {
        let fixture = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/archive_v1"
        ));
        let mut history = ArchiveHistory::verify(fixture).unwrap();
        let old_id = history.head_id().to_owned();
        let revision = SourceDocument::parse(SOURCE).unwrap().revision().to_owned();
        let patch = Transaction::new(
            revision.clone(),
            vec![Operation::Set {
                object: vec!["p".into()],
                field: vec!["tail".into()],
                value: serde_json::json!({"t":"quantity","n":"0","d":"1","u":"s"}),
                expect: None,
                expect_absent: false,
            }],
        )
        .unwrap()
        .to_json()
        .unwrap();
        let result = history.patch_head(&patch).unwrap();
        assert_eq!(history.version(), 5);
        assert_ne!(result.checkpoint, old_id);
        assert_eq!(result.applied.new_revision, revision);
        assert_eq!(history.checkpoint_ids().len(), 2);
        assert_eq!(
            history.select(Some(&old_id)).unwrap().digest(),
            history.select(None).unwrap().digest()
        );
        let temp = tempfile::tempdir().unwrap();
        history.stage(temp.path()).unwrap();
        assert_eq!(
            ArchiveHistory::verify(temp.path()).unwrap().digest(),
            history.digest()
        );
    }
}
