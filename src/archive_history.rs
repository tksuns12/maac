//! Immutable, linear archive checkpoints with closure and retained-import leaves.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::archive::{
    bounded_manifest, ArchivePreflight, ArchiveSnapshot, MANIFEST_PATH, MAX_TREE_ENTRIES,
};
use crate::archive_edit::{
    EditSnapshot, GraphPinCapture, GraphSourceCapture, GroupLeafCapture, MAX_EDIT_TOTAL_BYTES,
    MAX_EDIT_TRANSACTION_WORK_BYTES,
};
use crate::bundle::{sha256_digest, validate_hash_pin};
use crate::bundle_fs::ProjectRoot;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::dsp;
use crate::editing::AppliedTransaction;
use crate::freeze::{engine_digest, FreezeSnapshot};
use crate::node_freeze::{
    NodeFreezeGroup, NodeFreezePreflight, NodeFreezeSnapshot, MAX_NODE_FREEZE_BYTES,
    NODE_FREEZE_MANIFEST,
};
use crate::plan::PortRef;

const FORMAT: &str = "maac.editable-archive";
const CHECKPOINT_FORMAT: &str = "maac.archive-checkpoint";
const VERSION: u32 = 2;
const RETAINED_VERSION: u32 = 3;
const FROZEN_VERSION: u32 = 4;
const EDITED_VERSION: u32 = 5;
const MULTI_IMPORT_VERSION: u32 = 6;
const IMPORT_EDIT_VERSION: u32 = 7;
const NODE_FROZEN_VERSION: u32 = 8;
const NODE_GROUP_VERSION: u32 = 9;
const GROUP_EDIT_VERSION: u32 = 10;
const GRAPH_EDIT_VERSION: u32 = 11;
const PROCESSOR_CONTEXT_VERSION: u32 = 12;
const MAX_CHECKPOINTS: usize = 32;
const MAX_FREEZE_REFERENCES: usize = 64;
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
    node_freezes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edit: Option<String>,
}

impl CheckpointRecord {
    fn freeze_digests(&self) -> impl Iterator<Item = &str> {
        self.freeze
            .as_deref()
            .into_iter()
            .chain(self.node_freezes.iter().flatten().map(String::as_str))
    }
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

#[derive(Serialize)]
struct GroupCheckpointIdentity<'a> {
    format: &'static str,
    version: u32,
    parent: Option<&'a str>,
    snapshot: &'a str,
    node_freezes: &'a [String],
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
        freezes: Vec<Option<StoredFreeze>>,
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

pub(crate) struct FrozenRenderResult {
    pub(crate) revision: String,
    pub(crate) source_digest: String,
    pub(crate) frozen_source_digest: String,
    pub(crate) output_digest: String,
    pub(crate) frames: u64,
}

pub(crate) struct NodeFreezeInfo {
    pub(crate) frames: u64,
    /// Channels in the final project output, which may differ from the node stream.
    pub(crate) channels: u8,
    pub(crate) sample_rate_hz: u32,
    pub(crate) boundary: PortRef,
}

pub(crate) struct NodeGroupLeafCheck {
    pub(crate) boundary: PortRef,
    pub(crate) freeze_digest: String,
    pub(crate) output_digest: String,
    pub(crate) eligible: bool,
    pub(crate) replay_matches: Option<bool>,
}

pub(crate) struct NodeGroupFreezeCheck {
    pub(crate) status: FreezeCheckStatus,
    group_shape_valid: bool,
    pub(crate) revision: String,
    pub(crate) source_digest: String,
    pub(crate) frozen_source_digest: String,
    pub(crate) leaves: Vec<NodeGroupLeafCheck>,
}

pub(crate) struct NodeGroupFreezeInfo {
    pub(crate) frames: u64,
    pub(crate) channels: u8,
    pub(crate) sample_rate_hz: u32,
    pub(crate) leaves: Vec<NodeGroupLeafCheck>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeGroupReuseStatus {
    Current,
    Partial,
}

pub(crate) struct NodeGroupRenderResult {
    pub(crate) revision: String,
    pub(crate) source_digest: String,
    pub(crate) frozen_source_digest: String,
    pub(crate) frames: u64,
    pub(crate) leaves: Vec<NodeGroupLeafCheck>,
    pub(crate) reuse_status: NodeGroupReuseStatus,
    /// Canonical leaf order, matching `leaves`.
    pub(crate) reused: Vec<bool>,
}

pub(crate) struct ArchivePatchResult {
    pub(crate) applied: AppliedTransaction,
    pub(crate) checkpoint: String,
    pub(crate) edit_digest: String,
}

pub(crate) struct ArchiveImportPatchResult {
    pub(crate) checkpoint: String,
    pub(crate) edit_digest: String,
    pub(crate) applied: AppliedTransaction,
    pub(crate) import_source: String,
    pub(crate) before_revision: String,
    pub(crate) after_revision: String,
    pub(crate) pin: String,
    pub(crate) importer_before_revision: String,
    pub(crate) importer_after_revision: String,
}

pub(crate) struct ArchiveGroupPatchResult {
    pub(crate) checkpoint: String,
    pub(crate) edit_digest: String,
    pub(crate) leaves: Vec<GroupLeafCapture>,
    pub(crate) entry_applied: Option<AppliedTransaction>,
}

pub(crate) struct ArchiveGraphPatchResult {
    pub(crate) checkpoint: String,
    pub(crate) edit_digest: String,
    pub(crate) sources: Vec<GraphSourceCapture>,
    pub(crate) generated: Vec<GraphPinCapture>,
    pub(crate) entry_applied: Option<AppliedTransaction>,
}

#[derive(Clone)]
enum StoredFreeze {
    Output(FreezeSnapshot),
    Node(NodeFreezeSnapshot),
    Group(Arc<NodeFreezeGroup>),
}

impl StoredFreeze {
    fn digests(&self) -> Vec<String> {
        match self {
            Self::Output(f) => vec![f.digest()],
            Self::Node(f) => vec![f.digest()],
            Self::Group(group) => group
                .leaves()
                .iter()
                .map(NodeFreezeSnapshot::digest)
                .collect(),
        }
    }
    fn scalar_digest(&self) -> Option<String> {
        match self {
            Self::Output(f) => Some(f.digest()),
            Self::Node(f) => Some(f.digest()),
            Self::Group(_) => None,
        }
    }
    fn output_bytes(&self) -> u64 {
        match self {
            Self::Output(f) => f.output_bytes(),
            Self::Node(f) => f.output_bytes(),
            Self::Group(group) => group
                .leaves()
                .iter()
                .map(NodeFreezeSnapshot::output_bytes)
                .sum(),
        }
    }
    fn source_digest(&self) -> &str {
        match self {
            Self::Output(f) => f.source_digest(),
            Self::Node(f) => f.source_digest(),
            Self::Group(group) => group.leaves()[0].source_digest(),
        }
    }
}

enum StoredPreflight {
    Output(crate::freeze::FreezePreflight),
    Node(NodeFreezePreflight),
}

impl StoredPreflight {
    fn output_bytes(&self) -> u64 {
        match self {
            Self::Output(f) => f.output_bytes(),
            Self::Node(f) => f.output_bytes(),
        }
    }
    fn verify(&self, root: &ProjectRoot) -> Result<StoredFreeze, Diagnostics> {
        match self {
            Self::Output(f) => {
                FreezeSnapshot::verify_preflighted(root, f).map(StoredFreeze::Output)
            }
            Self::Node(f) => {
                NodeFreezeSnapshot::verify_preflighted(root, f).map(StoredFreeze::Node)
            }
        }
    }
}

/// A checked archive history retaining private snapshots for every checkpoint.
pub(crate) struct ArchiveHistory {
    state: HistoryState,
}

impl ArchiveHistory {
    /// Capture one current composition as v2 or retained-import v3 history.
    #[cfg(test)]
    pub(crate) fn capture(entry: &Path, root: &Path, profile: &str) -> Result<Self, Diagnostics> {
        let snapshot = ArchiveSnapshot::capture(entry, root, profile)?;
        Self::from_genesis(snapshot, None)
    }

    #[cfg(test)]
    pub(crate) fn capture_frozen(
        entry: &Path,
        root: &Path,
        profile: &str,
    ) -> Result<Self, Diagnostics> {
        let snapshot = ArchiveSnapshot::capture(entry, root, profile)?;
        let freeze = StoredFreeze::Output(FreezeSnapshot::capture(&snapshot)?);
        Self::from_genesis(snapshot, Some(freeze))
    }

    pub(crate) fn from_snapshot_node_frozen(
        snapshot: ArchiveSnapshot,
        boundary: &PortRef,
    ) -> Result<Self, Diagnostics> {
        let freeze = StoredFreeze::Node(NodeFreezeSnapshot::capture(&snapshot, boundary)?);
        Self::from_genesis(snapshot, Some(freeze))
    }

    pub(crate) fn from_snapshot_node_group(
        snapshot: ArchiveSnapshot,
        boundaries: &[PortRef],
    ) -> Result<Self, Diagnostics> {
        let group = NodeFreezeGroup::capture(&snapshot, boundaries)?;
        Self::from_genesis(snapshot, Some(StoredFreeze::Group(Arc::new(group))))
    }

    pub(crate) fn from_snapshot(
        snapshot: ArchiveSnapshot,
        freeze_output: bool,
    ) -> Result<Self, Diagnostics> {
        let freeze = if freeze_output {
            Some(StoredFreeze::Output(FreezeSnapshot::capture(&snapshot)?))
        } else {
            None
        };
        Self::from_genesis(snapshot, freeze)
    }

    fn from_genesis(
        snapshot: ArchiveSnapshot,
        freeze: Option<StoredFreeze>,
    ) -> Result<Self, Diagnostics> {
        check_aggregate_with_freezes(std::iter::once((
            snapshot.resource_bytes(),
            freeze.as_ref().map_or(0, StoredFreeze::output_bytes),
        )))?;
        check_staged_tree_with_freezes(
            std::iter::once(&snapshot),
            freeze.as_ref().map_or(0, |f| f.digests().len()),
        )?;
        let record = checkpoint_for_stored(None, &snapshot.digest(), freeze.as_ref())?;
        let version = if snapshot.has_processor_context() {
            PROCESSOR_CONTEXT_VERSION
        } else if matches!(freeze, Some(StoredFreeze::Group(_))) {
            NODE_GROUP_VERSION
        } else if matches!(freeze, Some(StoredFreeze::Node(_))) {
            NODE_FROZEN_VERSION
        } else if snapshot.is_multi_import() {
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
            Some(2..=12) => Self::verify_versioned(root, json),
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
        let mut counted_import_edits = BTreeSet::new();
        let mut tree_entries = 2usize + manifest.checkpoints.len();
        if manifest
            .checkpoints
            .iter()
            .any(|record| record.freeze_digests().next().is_some())
        {
            let unique: BTreeSet<&str> = manifest
                .checkpoints
                .iter()
                .flat_map(CheckpointRecord::freeze_digests)
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
            if manifest.version < PROCESSOR_CONTEXT_VERSION && preflight.has_processor_context() {
                return Err(fail(
                    DiagnosticCode::Version,
                    "older history cannot contain processor context",
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
            let mut record_children = Vec::new();
            for digest in record.freeze_digests() {
                let freeze_root = freezes_root
                    .as_ref()
                    .ok_or_else(|| {
                        fail(
                            DiagnosticCode::Reference,
                            "archive is missing freezes directory",
                        )
                    })?
                    .open_child_pinned(&digest[7..])?;
                let node_leaf = record.node_freezes.is_some()
                    || (manifest.version >= NODE_FROZEN_VERSION
                        && freeze_root
                            .dir()
                            .symlink_metadata(NODE_FREEZE_MANIFEST)
                            .is_ok());
                let preflight = if node_leaf {
                    StoredPreflight::Node(NodeFreezeSnapshot::preflight(
                        &freeze_root,
                        digest,
                        &record.snapshot,
                    )?)
                } else {
                    StoredPreflight::Output(FreezeSnapshot::preflight(
                        &freeze_root,
                        digest,
                        &record.snapshot,
                    )?)
                };
                record_children.push((freeze_root, preflight));
            }
            if record.node_freezes.is_some() {
                let group_bytes = record_children.iter().try_fold(0u64, |total, (_, leaf)| {
                    total.checked_add(leaf.output_bytes()).ok_or_else(|| {
                        fail(
                            DiagnosticCode::ResourceLimit,
                            "node freeze group length overflow",
                        )
                    })
                })?;
                if group_bytes > MAX_NODE_FREEZE_BYTES {
                    return Err(fail(
                        DiagnosticCode::ResourceLimit,
                        "node freeze group output exceeds 1 GiB",
                    ));
                }
            }
            freeze_children.push(record_children);
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
                if preflight.is_import_edit()
                    || preflight.is_group_edit()
                    || preflight.is_graph_edit()
                {
                    if preflight.is_import_edit() && manifest.version < IMPORT_EDIT_VERSION {
                        return Err(fail(
                            DiagnosticCode::Version,
                            "older history cannot contain import edits",
                        ));
                    }
                    if preflight.is_group_edit() && manifest.version < GROUP_EDIT_VERSION {
                        return Err(fail(
                            DiagnosticCode::Version,
                            "older history cannot contain grouped source edits",
                        ));
                    }
                    if preflight.is_graph_edit() && manifest.version < GRAPH_EDIT_VERSION {
                        return Err(fail(
                            DiagnosticCode::Version,
                            "older history cannot contain graph source edits",
                        ));
                    }
                    if counted_import_edits.insert(digest.clone()) {
                        tree_entries = tree_entries
                            .checked_add(preflight.tree_entries() - 4)
                            .ok_or_else(|| {
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
                    }
                }
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
        if manifest.version == IMPORT_EDIT_VERSION && counted_import_edits.is_empty() {
            return Err(fail(
                DiagnosticCode::Version,
                "v7 history has no import edit",
            ));
        }
        if manifest.version == GROUP_EDIT_VERSION
            && !edit_children
                .iter()
                .filter_map(Option::as_ref)
                .any(|(_, preflight)| preflight.is_group_edit())
        {
            return Err(fail(
                DiagnosticCode::Version,
                "v10 history has no grouped source edit",
            ));
        }
        if manifest.version == GRAPH_EDIT_VERSION
            && !edit_children
                .iter()
                .filter_map(Option::as_ref)
                .any(|(_, preflight)| preflight.is_graph_edit())
        {
            return Err(fail(
                DiagnosticCode::Version,
                "v11 history has no graph source edit",
            ));
        }
        if manifest.version == PROCESSOR_CONTEXT_VERSION
            && !preflights
                .iter()
                .any(ArchivePreflight::has_processor_context)
        {
            return Err(fail(
                DiagnosticCode::Version,
                "v12 history has no processor-context checkpoint",
            ));
        }
        if manifest.version == NODE_FROZEN_VERSION
            && !freeze_children
                .iter()
                .flatten()
                .any(|(_, preflight)| matches!(preflight, StoredPreflight::Node(_)))
        {
            return Err(fail(
                DiagnosticCode::Version,
                "v8 history has no node freeze",
            ));
        }
        check_aggregate_with_freezes(preflights.iter().zip(&freeze_children).map(
            |(source, leaves)| {
                (
                    source.resource_bytes(),
                    leaves
                        .iter()
                        .map(|(_, preflight)| preflight.output_bytes())
                        .sum(),
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
            edit_children.iter().map(|child| {
                child.as_ref().and_then(|(_, preflight)| {
                    preflight.graph_replay_passes().or_else(|| {
                        preflight
                            .group_leaf_count()
                            .map(|leaves| (leaves as u64 + 3, leaves as u64 + 2))
                    })
                })
            }),
        )?;

        before_capture();
        let mut snapshots = Vec::with_capacity(manifest.checkpoints.len());
        for (child, preflight) in children.iter().zip(&preflights) {
            let snapshot = ArchiveSnapshot::verify_preflighted_in_root(child, preflight, || {})?;
            snapshots.push(snapshot);
        }
        let mut freezes = Vec::with_capacity(freeze_children.len());
        for (record, children) in manifest.checkpoints.iter().zip(&freeze_children) {
            let freeze = if let Some(group_digests) = &record.node_freezes {
                let mut leaves = Vec::with_capacity(children.len());
                for (root, preflight) in children {
                    let StoredFreeze::Node(leaf) = preflight.verify(root)? else {
                        return Err(fail(
                            DiagnosticCode::Reference,
                            "group contains a non-node freeze",
                        ));
                    };
                    leaves.push(leaf);
                }
                let group = NodeFreezeGroup::from_verified(leaves)?;
                if group
                    .leaves()
                    .iter()
                    .map(NodeFreezeSnapshot::digest)
                    .collect::<Vec<_>>()
                    .as_slice()
                    != group_digests.as_slice()
                {
                    return Err(fail(
                        DiagnosticCode::Hash,
                        "group freeze order is not canonical",
                    ));
                }
                Some(StoredFreeze::Group(Arc::new(group)))
            } else {
                children
                    .first()
                    .map(|(root, preflight)| preflight.verify(root))
                    .transpose()?
            };
            freezes.push(freeze);
        }
        for (snapshot, freeze) in snapshots.iter().zip(&freezes) {
            if let Some(freeze) = freeze {
                let matches = match freeze {
                    StoredFreeze::Node(leaf) => leaf.matches_snapshot(snapshot)?,
                    StoredFreeze::Group(group) => group.matches_snapshot(snapshot)?,
                    StoredFreeze::Output(_) => true,
                };
                if !matches {
                    return Err(fail(
                        DiagnosticCode::Hash,
                        "node freeze metadata differs from its private checkpoint plan",
                    ));
                }
            }
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
        let freeze = StoredFreeze::Output(FreezeSnapshot::capture(&snapshot)?);
        self.append_with_freeze(snapshot, Some(freeze))
    }

    pub(crate) fn append_node_frozen(
        &mut self,
        snapshot: ArchiveSnapshot,
        boundary: &PortRef,
    ) -> Result<bool, Diagnostics> {
        let freeze = StoredFreeze::Node(NodeFreezeSnapshot::capture(&snapshot, boundary)?);
        self.append_with_freeze(snapshot, Some(freeze))
    }

    pub(crate) fn append_node_group(
        &mut self,
        snapshot: ArchiveSnapshot,
        boundaries: &[PortRef],
    ) -> Result<bool, Diagnostics> {
        let group = NodeFreezeGroup::capture(&snapshot, boundaries)?;
        self.append_with_freeze(snapshot, Some(StoredFreeze::Group(Arc::new(group))))
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
        self.append_edit_checkpoint(after, edit, applied)
    }

    pub(crate) fn patch_import_head(
        &mut self,
        alias: &str,
        patch: &[u8],
    ) -> Result<ArchiveImportPatchResult, Diagnostics> {
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
        let capture = EditSnapshot::capture_import(parent, before, alias, patch)?;
        let result = self.append_edit_checkpoint(capture.after, capture.edit, capture.applied)?;
        Ok(ArchiveImportPatchResult {
            checkpoint: result.checkpoint,
            edit_digest: result.edit_digest,
            applied: result.applied,
            import_source: capture.import_source,
            before_revision: capture.before_revision,
            after_revision: capture.after_revision,
            pin: capture.pin,
            importer_before_revision: capture.importer_before_revision,
            importer_after_revision: capture.importer_after_revision,
        })
    }

    pub(crate) fn patch_group_head(
        &mut self,
        imports: &[(String, Vec<u8>)],
        entry_patch: Option<&[u8]>,
    ) -> Result<ArchiveGroupPatchResult, Diagnostics> {
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
        let capture = EditSnapshot::capture_group(parent, before, imports, entry_patch)?;
        let applied = capture.leaves[0].applied.clone();
        let result = self.append_edit_checkpoint(capture.after, capture.edit, applied)?;
        Ok(ArchiveGroupPatchResult {
            checkpoint: result.checkpoint,
            edit_digest: result.edit_digest,
            leaves: capture.leaves,
            entry_applied: capture.entry_applied,
        })
    }

    pub(crate) fn patch_graph_head(
        &mut self,
        sources: &[(String, Vec<u8>)],
        entry_patch: Option<&[u8]>,
    ) -> Result<ArchiveGraphPatchResult, Diagnostics> {
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
        let capture = EditSnapshot::capture_graph(parent, before, sources, entry_patch)?;
        let applied = capture.sources[0].applied.clone();
        let result = self.append_edit_checkpoint(capture.after, capture.edit, applied)?;
        Ok(ArchiveGraphPatchResult {
            checkpoint: result.checkpoint,
            edit_digest: result.edit_digest,
            sources: capture.sources,
            generated: capture.generated,
            entry_applied: capture.entry_applied,
        })
    }

    fn append_edit_checkpoint(
        &mut self,
        after: ArchiveSnapshot,
        edit: EditSnapshot,
        applied: AppliedTransaction,
    ) -> Result<ArchivePatchResult, Diagnostics> {
        let edit_digest = edit.digest();
        let parent = match &self.state {
            HistoryState::Legacy { genesis_id, .. } => genesis_id.as_str(),
            HistoryState::Versioned { manifest, .. } => manifest.head.as_str(),
        };
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
                freeze.as_ref().map_or(0, StoredFreeze::output_bytes),
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
            edits.iter().map(|edit| {
                edit.as_ref().and_then(|edit| {
                    edit.graph_replay_passes().or_else(|| {
                        edit.group_leaf_count()
                            .map(|leaves| (leaves as u64 + 3, leaves as u64 + 2))
                    })
                })
            }),
        )?;
        check_staged_tree_with_extras(
            snapshots.iter(),
            unique_freeze_count(&freezes, None),
            unique_edit_entries(&edits),
        )?;
        let version = if snapshots.iter().any(ArchiveSnapshot::has_processor_context) {
            PROCESSOR_CONTEXT_VERSION
        } else if edits
            .iter()
            .filter_map(Option::as_ref)
            .any(EditSnapshot::is_graph_edit)
        {
            GRAPH_EDIT_VERSION
        } else if edits
            .iter()
            .filter_map(Option::as_ref)
            .any(EditSnapshot::is_group_edit)
        {
            GROUP_EDIT_VERSION
        } else if freezes
            .iter()
            .flatten()
            .any(|f| matches!(f, StoredFreeze::Group(_)))
        {
            NODE_GROUP_VERSION
        } else if freezes
            .iter()
            .flatten()
            .any(|f| matches!(f, StoredFreeze::Node(_)))
        {
            NODE_FROZEN_VERSION
        } else if edits
            .iter()
            .filter_map(Option::as_ref)
            .any(EditSnapshot::is_import_edit)
        {
            IMPORT_EDIT_VERSION
        } else if snapshots.iter().any(ArchiveSnapshot::is_multi_import) {
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
        freeze: Option<StoredFreeze>,
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
        if let Some(StoredFreeze::Group(group)) = &freeze {
            if !group.matches_snapshot(&snapshot)? {
                return Err(fail(
                    DiagnosticCode::RenderState,
                    "node freeze group differs from appended snapshot",
                ));
            }
        }
        let freeze_digests = freeze.as_ref().map(StoredFreeze::digests);
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
                    records.push(checkpoint_for_stored(
                        Some(&records[0].id),
                        &snapshot.digest(),
                        freeze.as_ref(),
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
                            freeze.as_ref().map_or(0, StoredFreeze::output_bytes),
                        )
                    },
                ))?;
                check_staged_tree_with_freezes(
                    snapshots.iter(),
                    unique_freeze_count(&freezes, None),
                )?;
                let version = if snapshots.iter().any(ArchiveSnapshot::has_processor_context) {
                    PROCESSOR_CONTEXT_VERSION
                } else if freezes
                    .iter()
                    .flatten()
                    .any(|f| matches!(f, StoredFreeze::Group(_)))
                {
                    NODE_GROUP_VERSION
                } else if freezes
                    .iter()
                    .flatten()
                    .any(|f| matches!(f, StoredFreeze::Node(_)))
                {
                    NODE_FROZEN_VERSION
                } else if snapshots.iter().any(ArchiveSnapshot::is_multi_import) {
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
                    && (freeze_digests.is_none()
                        || freezes
                            .last()
                            .and_then(Option::as_ref)
                            .is_some_and(|head| Some(head.digests()) == freeze_digests))
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
                                freeze.as_ref().map_or(0, StoredFreeze::output_bytes),
                            )
                        })
                        .chain(std::iter::once((
                            snapshot.resource_bytes(),
                            freeze.as_ref().map_or(0, StoredFreeze::output_bytes),
                        ))),
                )?;
                check_staged_tree_with_extras(
                    snapshots.iter().chain(std::iter::once(&snapshot)),
                    unique_freeze_count(freezes, freeze.as_ref()),
                    unique_edit_entries(edits),
                )?;
                let mut records = manifest.checkpoints.clone();
                records.push(checkpoint_for_stored(
                    Some(&manifest.head),
                    &snapshot.digest(),
                    freeze.as_ref(),
                )?);
                let version = if manifest.version == PROCESSOR_CONTEXT_VERSION
                    || snapshot.has_processor_context()
                {
                    PROCESSOR_CONTEXT_VERSION
                } else if manifest.version == GRAPH_EDIT_VERSION {
                    GRAPH_EDIT_VERSION
                } else if manifest.version == GROUP_EDIT_VERSION {
                    GROUP_EDIT_VERSION
                } else if manifest.version == NODE_GROUP_VERSION
                    || matches!(freeze, Some(StoredFreeze::Group(_)))
                {
                    NODE_GROUP_VERSION
                } else if manifest.version == NODE_FROZEN_VERSION
                    || matches!(freeze, Some(StoredFreeze::Node(_)))
                {
                    NODE_FROZEN_VERSION
                } else if manifest.version == IMPORT_EDIT_VERSION {
                    IMPORT_EDIT_VERSION
                } else if manifest.version == MULTI_IMPORT_VERSION || snapshot.is_multi_import() {
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
        let (record, stored) = self.selected_freeze(revision)?;
        let StoredFreeze::Output(freeze) = stored else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no output freeze",
            ));
        };
        let reference = self.select(Some(&record.id))?;
        let (candidate, missing_selected_import) = candidate_snapshot(entry, root, reference)?;
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

    /// Check exact source, executable, and validated effect boundary metadata.
    /// Replay compares the raw interleaved binary64 output hash.
    pub(crate) fn node_freeze_check(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        replay: bool,
    ) -> Result<FreezeCheck, Diagnostics> {
        self.checked_node_candidate(entry, root, revision, replay)
            .map(|(check, _)| check)
    }

    fn checked_node_candidate(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        replay: bool,
    ) -> Result<(FreezeCheck, ArchiveSnapshot), Diagnostics> {
        let (record, stored) = self.selected_freeze(revision)?;
        let StoredFreeze::Node(freeze) = stored else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no node freeze",
            ));
        };
        let reference = self.select(Some(&record.id))?;
        let (candidate, missing_selected_import) = candidate_snapshot(entry, root, reference)?;
        let source_digest = candidate.digest();
        let status = if !missing_selected_import
            && freeze.engine_digest() == engine_digest()?
            && freeze.matches_snapshot(&candidate)?
        {
            FreezeCheckStatus::Current
        } else {
            FreezeCheckStatus::Stale
        };
        let replay_matches = if replay && status == FreezeCheckStatus::Current {
            Some(
                NodeFreezeSnapshot::capture(&candidate, &freeze.boundary())?.output_digest()
                    == freeze.output_digest(),
            )
        } else {
            None
        };
        Ok((
            FreezeCheck {
                status,
                revision: record.id.clone(),
                source_digest,
                frozen_source_digest: record.snapshot.clone(),
                output_digest: freeze.output_digest().into(),
                replay_matches,
            },
            candidate,
        ))
    }

    pub(crate) fn node_freeze_info(
        &self,
        revision: Option<&str>,
    ) -> Result<NodeFreezeInfo, Diagnostics> {
        let (_, StoredFreeze::Node(freeze)) = self.selected_freeze(revision)? else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no node freeze",
            ));
        };
        Ok(NodeFreezeInfo {
            frames: freeze.frames(),
            channels: freeze.output_channels(),
            sample_rate_hz: freeze.sample_rate_hz(),
            boundary: freeze.boundary(),
        })
    }

    pub(crate) fn node_group_check(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        replay: bool,
    ) -> Result<NodeGroupFreezeCheck, Diagnostics> {
        self.checked_node_group_candidate(entry, root, revision, replay)
            .map(|(check, _)| check)
    }

    fn checked_node_group_candidate(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        replay: bool,
    ) -> Result<(NodeGroupFreezeCheck, ArchiveSnapshot), Diagnostics> {
        let (record, stored) = self.selected_freeze(revision)?;
        let StoredFreeze::Group(group) = stored else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no node freeze group",
            ));
        };
        let reference = self.select(Some(&record.id))?;
        let (candidate, missing_selected_import) = candidate_snapshot(entry, root, reference)?;
        let source_digest = candidate.digest();
        // Candidate edits can remove or rewire a selected effect. Keep a
        // per-leaf stale result while requiring the group shape to remain valid.
        let (group_shape_valid, leaf_matches) = group.candidate_eligibility(&candidate)?;
        let engine_current = group.leaves()[0].engine_digest() == engine_digest()?;
        let eligibility: Vec<_> = leaf_matches
            .into_iter()
            .map(|eligible| eligible && !missing_selected_import && engine_current)
            .collect();
        let status = if group_shape_valid && eligibility.iter().all(|eligible| *eligible) {
            FreezeCheckStatus::Current
        } else {
            FreezeCheckStatus::Stale
        };
        let replay_matches = if replay && status == FreezeCheckStatus::Current {
            Some(group.replay_matches(&candidate)?)
        } else {
            None
        };
        let check = NodeGroupFreezeCheck {
            status,
            group_shape_valid,
            revision: record.id.clone(),
            source_digest,
            frozen_source_digest: record.snapshot.clone(),
            leaves: group_leaf_checks(group, Some(&eligibility), replay_matches.as_deref()),
        };
        Ok((check, candidate))
    }

    pub(crate) fn node_group_info(
        &self,
        revision: Option<&str>,
    ) -> Result<NodeGroupFreezeInfo, Diagnostics> {
        let (_, StoredFreeze::Group(group)) = self.selected_freeze(revision)? else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no node freeze group",
            ));
        };
        let info = group.info();
        Ok(NodeGroupFreezeInfo {
            frames: info.frames,
            channels: info.channels,
            sample_rate_hz: info.sample_rate_hz,
            leaves: group_leaf_checks(group, None, None),
        })
    }

    pub(crate) fn render_from_node_group<F>(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        callback: F,
    ) -> Result<NodeGroupRenderResult, Diagnostics>
    where
        F: FnMut(&[f64]) -> dsp::Result<()>,
    {
        let (check, candidate) = self.checked_node_group_candidate(entry, root, revision, false)?;
        if check.status != FreezeCheckStatus::Current {
            let stale = check
                .leaves
                .iter()
                .filter(|leaf| !leaf.eligible)
                .map(|leaf| leaf.boundary.node.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(fail(
                DiagnosticCode::RenderState,
                format!("stored node freeze group is stale for this source: {stale}"),
            ));
        }
        let (_, StoredFreeze::Group(group)) = self.selected_freeze(Some(&check.revision))? else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no node freeze group",
            ));
        };
        group.render_replacements(&candidate, callback)?;
        let reused = vec![true; check.leaves.len()];
        Ok(NodeGroupRenderResult {
            revision: check.revision,
            source_digest: check.source_digest,
            frozen_source_digest: check.frozen_source_digest,
            frames: group.info().frames,
            leaves: check.leaves,
            reuse_status: NodeGroupReuseStatus::Current,
            reused,
        })
    }

    /// Opt in to reusing every still eligible leaf of a valid independent
    /// group. Unselected effects execute normally in the candidate graph.
    pub(crate) fn render_from_node_group_partial<F>(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        callback: F,
    ) -> Result<NodeGroupRenderResult, Diagnostics>
    where
        F: FnMut(&[f64]) -> dsp::Result<()>,
    {
        let (check, candidate) = self.checked_node_group_candidate(entry, root, revision, false)?;
        if !check.group_shape_valid {
            return Err(fail(
                DiagnosticCode::RenderState,
                "stored node freeze group has changed boundaries for this source",
            ));
        }
        let reused: Vec<_> = check.leaves.iter().map(|leaf| leaf.eligible).collect();
        if !reused.iter().any(|reused| *reused) {
            return Err(fail(
                DiagnosticCode::RenderState,
                "stored node freeze group has no eligible leaves for this source",
            ));
        }
        let (_, StoredFreeze::Group(group)) = self.selected_freeze(Some(&check.revision))? else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no node freeze group",
            ));
        };
        group.render_selected_replacements(&candidate, &reused, callback)?;
        Ok(NodeGroupRenderResult {
            revision: check.revision,
            source_digest: check.source_digest,
            frozen_source_digest: check.frozen_source_digest,
            frames: group.info().frames,
            leaves: check.leaves,
            reuse_status: if reused.iter().all(|reused| *reused) {
                NodeGroupReuseStatus::Current
            } else {
                NodeGroupReuseStatus::Partial
            },
            reused,
        })
    }

    /// Render downstream graph frames through the verified node replacement.
    /// The caller encodes and publishes the resulting final output WAV.
    pub(crate) fn render_from_node_freeze<F>(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        callback: F,
    ) -> Result<FrozenRenderResult, Diagnostics>
    where
        F: FnMut(&[f64]) -> dsp::Result<()>,
    {
        let (check, candidate) = self.checked_node_candidate(entry, root, revision, false)?;
        if check.status != FreezeCheckStatus::Current {
            return Err(fail(
                DiagnosticCode::RenderState,
                "stored node freeze is stale for this source",
            ));
        }
        let (_, StoredFreeze::Node(freeze)) = self.selected_freeze(Some(&check.revision))? else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no node freeze",
            ));
        };
        freeze.render_replacement(&candidate, callback)?;
        Ok(FrozenRenderResult {
            revision: check.revision,
            source_digest: check.source_digest,
            frozen_source_digest: check.frozen_source_digest,
            output_digest: check.output_digest,
            frames: freeze.frames(),
        })
    }

    /// Publish a whole-output freeze only while it remains eligible for the
    /// supplied source. The WAV comes from the verified private snapshot,
    /// independent of subsequent changes to the archive pathname.
    pub(crate) fn render_from_freeze(
        &self,
        entry: &Path,
        root: &Path,
        revision: Option<&str>,
        destination: &Path,
    ) -> Result<FrozenRenderResult, Diagnostics> {
        let check = self.freeze_check(entry, root, revision, false)?;
        if check.status != FreezeCheckStatus::Current {
            return Err(fail(
                DiagnosticCode::RenderState,
                "stored freeze is stale for this source",
            ));
        }
        let (_, StoredFreeze::Output(freeze)) = self.selected_freeze(Some(&check.revision))? else {
            return Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no output freeze",
            ));
        };
        freeze.publish_output(destination)?;
        Ok(FrozenRenderResult {
            revision: check.revision,
            source_digest: check.source_digest,
            frozen_source_digest: check.frozen_source_digest,
            output_digest: check.output_digest,
            frames: freeze.frames(),
        })
    }

    fn selected_freeze(
        &self,
        revision: Option<&str>,
    ) -> Result<(&CheckpointRecord, &StoredFreeze), Diagnostics> {
        if let Some(id) = revision {
            validate_hash_pin(id, "checkpoint id")?;
        }
        match &self.state {
            HistoryState::Legacy { .. } => Err(fail(
                DiagnosticCode::Reference,
                "selected checkpoint has no output freeze",
            )),
            HistoryState::Versioned {
                manifest, freezes, ..
            } => {
                let wanted = revision.unwrap_or(&manifest.head);
                let index = manifest
                    .checkpoints
                    .iter()
                    .position(|record| record.id == wanted)
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
                Ok((&manifest.checkpoints[index], freeze))
            }
        }
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
                    unique_edit_entries(edits),
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
                    .any(|record| record.freeze_digests().next().is_some())
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
                        let Some(freeze) = freeze else { continue };
                        if record.freeze_digests().collect::<Vec<_>>()
                            != freeze
                                .digests()
                                .iter()
                                .map(String::as_str)
                                .collect::<Vec<_>>()
                        {
                            return Err(fail(
                                DiagnosticCode::Hash,
                                "freeze record differs from private media",
                            ));
                        }
                        match freeze {
                            StoredFreeze::Output(leaf) => {
                                stage_freeze_leaf(&root, &mut staged, &leaf.digest(), |dir| {
                                    leaf.stage_into(dir)
                                })?
                            }
                            StoredFreeze::Node(leaf) => {
                                stage_freeze_leaf(&root, &mut staged, &leaf.digest(), |dir| {
                                    leaf.stage_into(dir)
                                })?
                            }
                            StoredFreeze::Group(group) => {
                                for leaf in group.leaves() {
                                    stage_freeze_leaf(&root, &mut staged, &leaf.digest(), |dir| {
                                        leaf.stage_into(dir)
                                    })?;
                                }
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

fn stage_freeze_leaf(
    root: &Path,
    staged: &mut BTreeSet<String>,
    digest: &str,
    stage: impl FnOnce(&Path) -> Result<(), Diagnostics>,
) -> Result<(), Diagnostics> {
    if staged.insert(digest.to_owned()) {
        let child = root.join(&digest[7..]);
        fs::create_dir(&child).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create freeze directory: {e}"),
            )
        })?;
        stage(&child)?;
    }
    Ok(())
}

fn group_leaf_checks(
    group: &NodeFreezeGroup,
    eligibility: Option<&[bool]>,
    replay: Option<&[bool]>,
) -> Vec<NodeGroupLeafCheck> {
    group
        .leaves()
        .iter()
        .enumerate()
        .map(|(index, leaf)| NodeGroupLeafCheck {
            boundary: leaf.boundary(),
            freeze_digest: leaf.digest(),
            output_digest: leaf.output_digest().to_owned(),
            eligible: eligibility.is_some_and(|values| values[index]),
            replay_matches: replay.map(|values| values[index]),
        })
        .collect()
}

fn checkpoint(parent: Option<&str>, snapshot: &str) -> Result<CheckpointRecord, Diagnostics> {
    checkpoint_with_freeze(parent, snapshot, None)
}

fn candidate_snapshot(
    entry: &Path,
    root: &Path,
    reference: &ArchiveSnapshot,
) -> Result<(ArchiveSnapshot, bool), Diagnostics> {
    match ArchiveSnapshot::capture_matching_layout(entry, root, reference) {
        Ok(candidate) => Ok((candidate, false)),
        Err(_) if reference.is_multi_import() => Ok((
            ArchiveSnapshot::capture_closure_only(entry, root, reference.profile())?,
            true,
        )),
        Err(error) => Err(error),
    }
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
        node_freezes: None,
        edit: None,
    })
}

fn checkpoint_with_node_freezes(
    parent: Option<&str>,
    snapshot: &str,
    node_freezes: &[String],
) -> Result<CheckpointRecord, Diagnostics> {
    validate_hash_pin(snapshot, "checkpoint snapshot hash")?;
    if let Some(parent) = parent {
        validate_hash_pin(parent, "checkpoint parent id")?;
    }
    if !(2..=16).contains(&node_freezes.len()) {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "node freeze group requires 2 to 16 leaves",
        ));
    }
    let mut unique = BTreeSet::new();
    for digest in node_freezes {
        validate_hash_pin(digest, "checkpoint node freeze digest")?;
        if !unique.insert(digest) {
            return Err(fail(
                DiagnosticCode::Reference,
                "node freeze group repeats a leaf digest",
            ));
        }
    }
    let bytes = serde_json::to_vec(&GroupCheckpointIdentity {
        format: CHECKPOINT_FORMAT,
        version: 4,
        parent,
        snapshot,
        node_freezes,
    })
    .map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode grouped checkpoint identity: {e}"),
        )
    })?;
    Ok(CheckpointRecord {
        id: sha256_digest(&bytes),
        parent: parent.map(str::to_owned),
        snapshot: snapshot.into(),
        freeze: None,
        node_freezes: Some(node_freezes.to_vec()),
        edit: None,
    })
}

fn checkpoint_for_stored(
    parent: Option<&str>,
    snapshot: &str,
    freeze: Option<&StoredFreeze>,
) -> Result<CheckpointRecord, Diagnostics> {
    match freeze {
        Some(StoredFreeze::Group(group)) => checkpoint_with_node_freezes(
            parent,
            snapshot,
            &group
                .leaves()
                .iter()
                .map(NodeFreezeSnapshot::digest)
                .collect::<Vec<_>>(),
        ),
        _ => checkpoint_with_freeze(
            parent,
            snapshot,
            freeze.and_then(StoredFreeze::scalar_digest).as_deref(),
        ),
    }
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
        node_freezes: None,
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
            VERSION
                | RETAINED_VERSION
                | FROZEN_VERSION
                | EDITED_VERSION
                | MULTI_IMPORT_VERSION
                | IMPORT_EDIT_VERSION
                | NODE_FROZEN_VERSION
                | NODE_GROUP_VERSION
                | GROUP_EDIT_VERSION
                | GRAPH_EDIT_VERSION
                | PROCESSOR_CONTEXT_VERSION
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
    let mut freeze_references = 0usize;
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
            && manifest.version != IMPORT_EDIT_VERSION
            && manifest.version != NODE_FROZEN_VERSION
            && manifest.version != NODE_GROUP_VERSION
            && manifest.version != GROUP_EDIT_VERSION
            && manifest.version != GRAPH_EDIT_VERSION
            && manifest.version != PROCESSOR_CONTEXT_VERSION
        {
            return Err(fail(
                DiagnosticCode::Version,
                "older archive history cannot contain edits",
            ));
        }
        if record.node_freezes.is_some()
            && manifest.version != NODE_GROUP_VERSION
            && manifest.version != GROUP_EDIT_VERSION
            && manifest.version != GRAPH_EDIT_VERSION
            && manifest.version != PROCESSOR_CONTEXT_VERSION
        {
            return Err(fail(
                DiagnosticCode::Version,
                "older archive history cannot contain grouped node freezes",
            ));
        }
        if record.node_freezes.is_some() && (record.freeze.is_some() || record.edit.is_some()) {
            return Err(fail(
                DiagnosticCode::Reference,
                "grouped node freeze cannot share a checkpoint with scalar freeze or edit",
            ));
        }
        if record.edit.is_some() && (record.parent.is_none() || record.freeze.is_some()) {
            return Err(fail(
                DiagnosticCode::Reference,
                "edited checkpoint requires a parent and cannot inherit a freeze",
            ));
        }
        freeze_references = freeze_references
            .checked_add(usize::from(record.freeze.is_some()))
            .and_then(|n| n.checked_add(record.node_freezes.as_ref().map_or(0, Vec::len)))
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "freeze reference count overflow",
                )
            })?;
        if freeze_references > MAX_FREEZE_REFERENCES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "archive history exceeds 64 freeze references",
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
        } else if let Some(group) = &record.node_freezes {
            checkpoint_with_node_freezes(record.parent.as_deref(), &record.snapshot, group)?
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
    if manifest.version == NODE_GROUP_VERSION
        && !manifest
            .checkpoints
            .iter()
            .any(|r| r.node_freezes.is_some())
    {
        return Err(fail(
            DiagnosticCode::Version,
            "v9 history has no grouped node freeze",
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
    edit_entries: usize,
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
    let edit_overhead = if edit_entries == 0 {
        0
    } else {
        1usize.checked_add(edit_entries).ok_or_else(|| {
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

fn unique_edit_entries(edits: &[Option<EditSnapshot>]) -> usize {
    let mut seen = BTreeSet::new();
    edits
        .iter()
        .filter_map(Option::as_ref)
        .filter(|edit| seen.insert(edit.digest()))
        .map(EditSnapshot::tree_entries)
        .sum()
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
    replay_passes: impl Iterator<Item = Option<(u64, u64)>>,
) -> Result<(), Diagnostics> {
    let budgets: Vec<_> = budgets.collect();
    let replay_passes: Vec<_> = replay_passes.collect();
    if budgets.len() != records.len() || replay_passes.len() != records.len() {
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
        let (before_passes, after_passes) = replay_passes[index].unwrap_or((2, 1));
        let work = before
            .checked_mul(before_passes)
            .and_then(|n| {
                after
                    .checked_mul(after_passes)
                    .and_then(|after| n.checked_add(after))
            })
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

fn unique_freeze_count(freezes: &[Option<StoredFreeze>], extra: Option<&StoredFreeze>) -> usize {
    freezes
        .iter()
        .filter_map(Option::as_ref)
        .chain(extra)
        .flat_map(StoredFreeze::digests)
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
        .any(|record| record.freeze_digests().next().is_some());
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
            .flat_map(CheckpointRecord::freeze_digests)
            .map(|id| id[7..].to_owned())
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
    fn v6_import_patch_promotes_to_v7_and_keeps_later_entry_edits() {
        let temp = tempfile::tempdir().unwrap();
        let project = retained_project(temp.path());
        let nested = project.join("imports/drums");
        fs::create_dir_all(&nested).unwrap();
        for name in ["media.pcm", "import.json", "original.wav"] {
            fs::rename(project.join(name), nested.join(name)).unwrap();
        }
        let library = "maac 1;\nlibrary sounds { version=\"1\"; }\n";
        fs::write(project.join("sounds.maac"), library).unwrap();
        let pin = sha256_digest(library.as_bytes());
        let source = fs::read_to_string(project.join("main.maac")).unwrap();
        let source = source.replace("path = \"media.pcm\"", "path = \"imports/drums/media.pcm\"");
        let source =
            format!("{source}\nimport sounds {{ path=\"sounds.maac\"; hash=\"{pin}\"; }}\n");
        fs::write(project.join("main.maac"), source).unwrap();
        let snapshot = ArchiveSnapshot::capture_with_imports(
            Path::new("main.maac"),
            &project,
            "default",
            &["imports/drums".into()],
        )
        .unwrap();
        let mut history = ArchiveHistory::from_snapshot(snapshot, false).unwrap();
        assert_eq!(history.version(), 6);
        let genesis = history.head_id().to_owned();
        let transaction = Transaction::new(
            SourceDocument::parse(library)
                .unwrap()
                .revision()
                .to_owned(),
            vec![Operation::Set {
                object: vec!["sounds".into()],
                field: vec!["version".into()],
                value: serde_json::json!({"t":"string","v":"2"}),
                expect: Some(serde_json::json!({"t":"string","v":"1"})),
                expect_absent: false,
            }],
        )
        .unwrap()
        .to_json()
        .unwrap();
        let result = history.patch_import_head("sounds", &transaction).unwrap();
        assert_eq!(history.version(), 7);
        let entry = history
            .select(None)
            .unwrap()
            .source_text("main.maac")
            .unwrap();
        let entry_transaction = Transaction::new(
            SourceDocument::parse(entry).unwrap().revision().to_owned(),
            vec![Operation::Set {
                object: vec!["sounds".into()],
                field: vec!["hash".into()],
                value: serde_json::json!({"t":"string","v":result.pin}),
                expect: None,
                expect_absent: false,
            }],
        )
        .unwrap()
        .to_json()
        .unwrap();
        history.patch_head(&entry_transaction).unwrap();
        assert_eq!(history.version(), 7);
        assert!(history
            .append(
                ArchiveSnapshot::capture_with_imports(
                    Path::new("main.maac"),
                    &project,
                    "default",
                    &["imports/drums".into()],
                )
                .unwrap()
            )
            .unwrap());
        assert_eq!(history.version(), 7);
        let archive = temp.path().join("archive-v7");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        let verified = ArchiveHistory::verify(&archive).unwrap();
        assert_eq!(verified.digest(), history.digest());
        assert_eq!(verified.checkpoint_ids()[0], genesis);
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
    fn frozen_render_uses_verified_private_bytes_and_never_clobbers() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current");
        let stale = temp.path().join("stale");
        write_project(&current, SOURCE);
        write_project(&stale, &format!("{SOURCE}// changed\n"));
        let history =
            ArchiveHistory::capture_frozen(Path::new("main.maac"), &current, "default").unwrap();
        let revision = history.head_id().to_owned();
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        let verified = ArchiveHistory::verify(&archive).unwrap();
        let archived_output = archive
            .join("freezes")
            .join(
                &verified
                    .selected_freeze(None)
                    .unwrap()
                    .0
                    .freeze
                    .as_ref()
                    .unwrap()[7..],
            )
            .join("output.wav");
        let expected = fs::read(&archived_output).unwrap();
        fs::write(
            &archived_output,
            b"archive pathname changed after verification",
        )
        .unwrap();

        let destination = temp.path().join("reused.wav");
        let result = verified
            .render_from_freeze(
                Path::new("main.maac"),
                &current,
                Some(&revision),
                &destination,
            )
            .unwrap();
        assert_eq!(result.revision, revision);
        assert_eq!(
            result.source_digest,
            verified.select(None).unwrap().digest()
        );
        assert_eq!(result.output_digest, sha256_digest(&expected));
        assert!(result.frames > 0);
        assert_eq!(fs::read(&destination).unwrap(), expected);

        assert!(verified
            .render_from_freeze(Path::new("main.maac"), &current, None, &destination)
            .is_err());
        assert_eq!(fs::read(&destination).unwrap(), expected);

        let missing = temp.path().join("stale.wav");
        assert!(verified
            .render_from_freeze(Path::new("main.maac"), &stale, None, &missing)
            .is_err());
        assert!(!missing.exists());
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

    #[test]
    fn v8_retains_mixed_freezes_and_replays_node_payload() {
        const EFFECT_SOURCE: &str = r#"maac 1;
project p { score=[0q,1/100q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&gain:out; requires=["maac.production/1"]; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node tone { type="core.sine/1"; params={attack=0s;release=0s;level=0.2;}; }
node room { type="fx.reverb/1"; config={channels=1;}; params={decay=1/2s;mix=0.4;}; }
node gain { type="core.gain/1"; config={channels=1;}; params={gain=3/4;}; }
connect a { from=&tone:out; to=&room:in; }
connect b { from=&room:out; to=&gain:in; }
pattern phrase { length=1/100q; note n { at=0q; dur=1/100q; pitch=A4; velocity=1; } }
track t { target=&tone:events; }
place notes { pattern=&phrase; track=&t; at=0q; }
"#;
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        write_project(&project, EFFECT_SOURCE);
        let mut history =
            ArchiveHistory::capture_frozen(Path::new("main.maac"), &project, "default").unwrap();
        let output_revision = history.head_id().to_owned();
        let boundary = PortRef::new("room", "out").unwrap();
        assert!(history
            .append_node_frozen(capture(&project), &boundary)
            .unwrap());
        assert_eq!(history.version(), 8);
        let node_revision = history.head_id().to_owned();
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        let verified = ArchiveHistory::verify(&archive).unwrap();
        assert_eq!(verified.version(), 8);
        assert_eq!(
            verified
                .freeze_check(
                    Path::new("main.maac"),
                    &project,
                    Some(&output_revision),
                    true
                )
                .unwrap()
                .replay_matches,
            Some(true)
        );
        let check = verified
            .node_freeze_check(Path::new("main.maac"), &project, Some(&node_revision), true)
            .unwrap();
        assert_eq!(check.status, FreezeCheckStatus::Current);
        assert_eq!(check.replay_matches, Some(true));
        let info = verified.node_freeze_info(Some(&node_revision)).unwrap();
        let mut frames = 0u64;
        verified
            .render_from_node_freeze(
                Path::new("main.maac"),
                &project,
                Some(&node_revision),
                |samples| {
                    assert_eq!(samples.len(), usize::from(info.channels));
                    frames += 1;
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(frames, info.frames);
        let mut extended = verified;
        fs::write(
            project.join("main.maac"),
            format!("{EFFECT_SOURCE}\n// edited\n"),
        )
        .unwrap();
        assert!(extended.append(capture(&project)).unwrap());
        assert_eq!(extended.version(), 8);
        let extended_archive = temp.path().join("extended");
        fs::create_dir(&extended_archive).unwrap();
        extended.stage(&extended_archive).unwrap();
        assert_eq!(
            ArchiveHistory::verify(&extended_archive).unwrap().version(),
            8
        );
    }

    const GROUP_SOURCE: &str = r#"maac 1;
project p { score=[0q,1/100q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&sum:out; requires=["maac.production/1"]; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node tone_a { type="core.sine/1"; params={attack=0s;release=0s;level=0.2;}; }
node tone_b { type="core.sine/1"; params={attack=0s;release=0s;level=0.1;}; }
node trim { type="core.gain/1"; config={channels=1;}; params={gain=1;}; }
node eq { type="fx.eq/1"; config={channels=1;mode=low_pass;}; params={frequency=12000Hz;q=1/2;}; }
node room { type="fx.reverb/1"; config={channels=1;}; params={decay=1/2s;mix=0.4;}; }
node sum { type="core.sum/1"; config={channels=1;}; }
connect a { from=&tone_a:out; to=&trim:in; }
connect trim_to_eq { from=&trim:out; to=&eq:in; }
connect b { from=&tone_b:out; to=&room:in; }
connect c { from=&eq:out; to=&sum:in; }
connect d { from=&room:out; to=&sum:in; }
pattern phrase { length=1/100q; note n { at=0q; dur=1/100q; pitch=A4; velocity=1; } }
track ta { target=&tone_a:events; }
track tb { target=&tone_b:events; }
place pa { pattern=&phrase; track=&ta; at=0q; }
place pb { pattern=&phrase; track=&tb; at=0q; }
"#;

    fn group_boundaries() -> [PortRef; 2] {
        [
            PortRef::new("room", "out").unwrap(),
            PortRef::new("eq", "out").unwrap(),
        ]
    }

    #[test]
    fn v9_group_round_trip_mixed_history_replay_and_partial_eligibility() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        write_project(&project, GROUP_SOURCE);
        let mut history =
            ArchiveHistory::capture_frozen(Path::new("main.maac"), &project, "default").unwrap();
        let scalar_id = history.head_id().to_owned();
        assert!(history
            .append_node_group(capture(&project), &group_boundaries())
            .unwrap());
        assert_eq!(history.version(), NODE_GROUP_VERSION);
        let group_id = history.head_id().to_owned();
        assert!(!history
            .append_node_group(capture(&project), &group_boundaries())
            .unwrap());
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        let mut verified = ArchiveHistory::verify(&archive).unwrap();
        assert_eq!(verified.checkpoint_ids()[0], scalar_id);
        assert_eq!(verified.head_id(), group_id);
        let check = verified
            .node_group_check(Path::new("main.maac"), &project, None, true)
            .unwrap();
        assert_eq!(check.status, FreezeCheckStatus::Current);
        assert_eq!(check.leaves.len(), 2);
        assert_eq!(check.leaves[0].boundary.node, "eq");
        assert_eq!(check.leaves[1].boundary.node, "room");
        assert!(check
            .leaves
            .iter()
            .all(|leaf| leaf.eligible && leaf.replay_matches == Some(true)));
        let info = verified.node_group_info(None).unwrap();
        let mut rendered = Vec::new();
        let render = verified
            .render_from_node_group(Path::new("main.maac"), &project, None, |frame| {
                rendered.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        assert_eq!(render.frames, info.frames);
        assert_eq!(render.leaves.len(), 2);
        assert_eq!(render.reuse_status, NodeGroupReuseStatus::Current);
        assert_eq!(render.reused, vec![true, true]);
        let mut normal = Vec::new();
        crate::disk_media::DiskMediaProject::load(Path::new("main.maac"), &project)
            .unwrap()
            .build_with_limits(&crate::plan::PlanLimits::default())
            .unwrap()
            .render(|frame| {
                normal.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            rendered.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            normal.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );

        fs::write(
            project.join("main.maac"),
            GROUP_SOURCE.replace("gain=1;", "gain=1/2;"),
        )
        .unwrap();
        let partial = verified
            .node_group_check(Path::new("main.maac"), &project, None, true)
            .unwrap();
        assert_eq!(partial.status, FreezeCheckStatus::Stale);
        assert!(!partial.leaves[0].eligible);
        assert!(partial.leaves[1].eligible);
        assert!(partial
            .leaves
            .iter()
            .all(|leaf| leaf.replay_matches.is_none()));
        assert!(verified
            .render_from_node_group(Path::new("main.maac"), &project, None, |_| Ok(()))
            .is_err());
        let mut partially_rendered = Vec::new();
        let partial_render = verified
            .render_from_node_group_partial(Path::new("main.maac"), &project, None, |frame| {
                partially_rendered.extend(frame.iter().map(|sample| sample.to_bits()));
                Ok(())
            })
            .unwrap();
        assert_eq!(partial_render.reuse_status, NodeGroupReuseStatus::Partial);
        assert_eq!(partial_render.reused, vec![false, true]);
        let mut current_render = Vec::new();
        crate::disk_media::DiskMediaProject::load(Path::new("main.maac"), &project)
            .unwrap()
            .build_with_limits(&crate::plan::PlanLimits::default())
            .unwrap()
            .render(|frame| {
                current_render.extend(frame.iter().map(|sample| sample.to_bits()));
                Ok(())
            })
            .unwrap();
        assert_eq!(partially_rendered, current_render);
        assert!(verified.append(capture(&project)).unwrap());
        assert_eq!(verified.version(), NODE_GROUP_VERSION);
        let source = fs::read_to_string(project.join("main.maac")).unwrap();
        let source_revision = SourceDocument::parse(&source)
            .unwrap()
            .revision()
            .to_owned();
        let patch = Transaction::new(
            source_revision,
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
        verified.patch_head(&patch).unwrap();
        assert_eq!(verified.version(), NODE_GROUP_VERSION);
        let extended = temp.path().join("extended");
        fs::create_dir(&extended).unwrap();
        verified.stage(&extended).unwrap();
        assert_eq!(
            ArchiveHistory::verify(&extended).unwrap().version(),
            NODE_GROUP_VERSION
        );
        let damaged_leaf = archive
            .join("freezes")
            .join(&check.leaves[1].freeze_digest[7..])
            .join(crate::node_freeze::NODE_FREEZE_OUTPUT);
        fs::write(damaged_leaf, b"damaged").unwrap();
        assert!(ArchiveHistory::verify(&archive).is_err());
    }

    #[test]
    fn grouped_checkpoint_identity_and_reference_bound_are_enforced() {
        let snapshot = sha256_digest(b"group snapshot");
        let mut records = Vec::new();
        for index in 0..5 {
            let digests: Vec<_> = (0..16)
                .map(|leaf| sha256_digest(format!("{index}-{leaf}").as_bytes()))
                .collect();
            records.push(
                checkpoint_with_node_freezes(
                    records.last().map(|r: &CheckpointRecord| r.id.as_str()),
                    &snapshot,
                    &digests,
                )
                .unwrap(),
            );
        }
        assert!(encode_history(records, NODE_GROUP_VERSION).is_err());
        assert!(checkpoint_with_node_freezes(
            None,
            &snapshot,
            &[sha256_digest(b"same"), sha256_digest(b"same")]
        )
        .is_err());
        let legacy =
            checkpoint_with_freeze(None, &snapshot, Some(&sha256_digest(b"single"))).unwrap();
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(!json.contains("node_freezes"));
    }

    #[test]
    fn grouped_edit_replay_budget_charges_each_member_closure_pass() {
        let snapshot = sha256_digest(b"snapshot");
        let first = checkpoint(None, &snapshot).unwrap();
        let second =
            checkpoint_with_edit(Some(&first.id), &snapshot, &sha256_digest(b"group edit"))
                .unwrap();
        let records = [first, second];
        let budget = (0, 0, 256 * 1024 * 1024, 0);
        assert!(check_edit_replay_work(
            &records,
            [budget, budget].into_iter(),
            [None, Some((19, 18))].into_iter(),
        )
        .is_err());
        assert!(check_edit_replay_work(
            &records,
            [budget, budget].into_iter(),
            [None, None].into_iter(),
        )
        .is_ok());
    }
}
