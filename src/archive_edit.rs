//! Immutable Protocol 2 edit evidence for archive checkpoint transitions.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use cap_std::fs::OpenOptions as CapOpenOptions;
use serde::{Deserialize, Serialize};

use crate::archive::ArchiveSnapshot;
use crate::bundle::{sha256_digest, validate_hash_pin};
use crate::bundle_fs::ProjectRoot;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::disk_media::{DiskAsset, DiskMediaProject};
use crate::editing::{
    apply_transaction, AppliedTransaction, BundleEditContext, EditError, Operation, SourceDocument,
    Transaction, MAX_DOCUMENT_BYTES, MAX_OPERATIONS, MAX_TRANSACTION_BYTES, REVISION_ALGORITHM,
};
use crate::plan::PlanLimits;

pub(crate) const EDIT_MANIFEST: &str = "maac-edit.json";
pub(crate) const EDIT_FORWARD: &str = "forward.json";
pub(crate) const EDIT_INVERSE: &str = "inverse.json";
pub(crate) const MAX_EDIT_MANIFEST_BYTES: u64 = 16 * 1024;
pub(crate) const MAX_EDIT_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_EDIT_TRANSACTION_WORK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_SINGLE_TRANSACTION_WORK_BYTES: u64 = 256 * 1024 * 1024;
const FORMAT: &str = "maac.archive-edit";
const VERSION: u32 = 1;
const IMPORT_VERSION: u32 = 2;
const GROUP_VERSION: u32 = 3;
const MAX_GROUP_LEAVES: usize = 16;
const IMPORTER_FORWARD: &str = "importer-forward.json";
const IMPORTER_INVERSE: &str = "importer-inverse.json";
const ENTRY_FORWARD: &str = "entry-forward.json";
const ENTRY_INVERSE: &str = "entry-inverse.json";
const PIN_FORWARD: &str = "pin-forward.json";
const PIN_INVERSE: &str = "pin-inverse.json";
const MAX_GENERATED_PIN_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditManifest {
    format: String,
    version: u32,
    parent: String,
    before: String,
    after: String,
    source: String,
    revision_algorithm: String,
    before_revision: String,
    after_revision: String,
    forward: EditFile,
    inverse: EditFile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    importer: Option<ImporterEdit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<GroupEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupEdit {
    entry_source: String,
    first_alias: String,
    first_pin: String,
    others: Vec<GroupLeafEdit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry: Option<GroupEntryEdit>,
    pin: GroupEntryEdit,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupLeafEdit {
    source: String,
    alias: String,
    before_revision: String,
    after_revision: String,
    pin: String,
    forward: EditFile,
    inverse: EditFile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupEntryEdit {
    before_revision: String,
    after_revision: String,
    forward: EditFile,
    inverse: EditFile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImporterEdit {
    source: String,
    alias: String,
    before_revision: String,
    after_revision: String,
    pin: String,
    forward: EditFile,
    inverse: EditFile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFile {
    path: String,
    bytes: u64,
    sha256: String,
    operations: usize,
}

pub(crate) struct EditPreflight {
    manifest: EditManifest,
    json: Vec<u8>,
}

impl EditPreflight {
    pub(crate) fn source(&self) -> &str {
        if let Some(group) = &self.manifest.group {
            return &group.entry_source;
        }
        self.manifest
            .importer
            .as_ref()
            .map_or(&self.manifest.source, |importer| &importer.source)
    }

    pub(crate) fn is_import_edit(&self) -> bool {
        self.manifest.importer.is_some() || self.manifest.group.is_some()
    }

    pub(crate) fn is_group_edit(&self) -> bool {
        self.manifest.group.is_some()
    }

    pub(crate) fn group_leaf_count(&self) -> Option<usize> {
        self.manifest
            .group
            .as_ref()
            .map(|group| group.others.len() + 1)
    }

    pub(crate) fn tree_entries(&self) -> usize {
        2 + edit_files(&self.manifest).len()
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.json.len() as u64
            + edit_files(&self.manifest)
                .iter()
                .map(|file| file.bytes)
                .sum::<u64>()
    }

    pub(crate) fn transaction_work(&self) -> Result<u64, Diagnostics> {
        transaction_work(&self.manifest)
    }
}

#[derive(Clone)]
pub(crate) struct EditSnapshot {
    manifest: EditManifest,
    json: Vec<u8>,
    forward: Vec<u8>,
    inverse: Vec<u8>,
    importer_forward: Option<Vec<u8>>,
    importer_inverse: Option<Vec<u8>>,
    group_payloads: BTreeMap<String, Vec<u8>>,
}

pub(crate) struct GroupLeafCapture {
    pub(crate) alias: String,
    pub(crate) source: String,
    pub(crate) before_revision: String,
    pub(crate) after_revision: String,
    pub(crate) pin: String,
    pub(crate) applied: AppliedTransaction,
}

pub(crate) struct GroupEditCapture {
    pub(crate) after: ArchiveSnapshot,
    pub(crate) edit: EditSnapshot,
    pub(crate) leaves: Vec<GroupLeafCapture>,
    pub(crate) entry_applied: Option<AppliedTransaction>,
}

pub(crate) struct ImportEditCapture {
    pub(crate) after: ArchiveSnapshot,
    pub(crate) edit: EditSnapshot,
    pub(crate) applied: AppliedTransaction,
    pub(crate) import_source: String,
    pub(crate) before_revision: String,
    pub(crate) after_revision: String,
    pub(crate) pin: String,
    pub(crate) importer_before_revision: String,
    pub(crate) importer_after_revision: String,
}

struct TrustedImport {
    bundle: crate::bundle::SourceBundle,
    assets: BTreeMap<String, std::sync::Arc<DiskAsset>>,
    library: String,
}

struct SourceApplication {
    source: String,
    before_revision: String,
    before_tree: serde_json::Value,
    after_tree: serde_json::Value,
    applied: AppliedTransaction,
}

struct AuthoredApplication {
    before_revision: String,
    before_tree: serde_json::Value,
    after_tree: serde_json::Value,
    applied: AppliedTransaction,
}

struct GroupLeafApplication {
    capture: GroupLeafCapture,
    forward: Vec<u8>,
    inverse: Vec<u8>,
    forward_operations: usize,
    inverse_operations: usize,
}

impl EditSnapshot {
    pub(crate) fn capture_group(
        parent: &str,
        before: &ArchiveSnapshot,
        import_patches: &[(String, Vec<u8>)],
        entry_patch: Option<&[u8]>,
    ) -> Result<GroupEditCapture, Diagnostics> {
        validate_hash_pin(parent, "edit parent checkpoint")?;
        if import_patches.is_empty() || import_patches.len() > MAX_GROUP_LEAVES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "group edit requires 1 to 16 imports",
            ));
        }
        if import_patches.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(fail(
                DiagnosticCode::Syntax,
                "group import aliases must be distinct and sorted",
            ));
        }
        let mut leaves = Vec::with_capacity(import_patches.len());
        let mut replacements = BTreeMap::new();
        let mut alternates = BTreeMap::new();
        let mut pin_operations = Vec::with_capacity(import_patches.len());
        let mut trusted_bundle = None;
        for (alias, patch) in import_patches {
            let trusted = trusted_import(before, alias)?;
            let source_path = trusted.library.clone();
            if replacements.contains_key(&trusted.library) {
                return Err(fail(
                    DiagnosticCode::Capability,
                    "group aliases must target distinct leaves",
                ));
            }
            let original = before.source_text(&trusted.library)?;
            let prior_pin = sha256_digest(original.as_bytes());
            let transaction = Transaction::from_json(patch).map_err(edit_failure)?;
            let forward = transaction.to_json().map_err(edit_failure)?;
            let mut document = SourceDocument::parse(original).map_err(edit_failure)?;
            let before_revision = document.revision().to_owned();
            let context = BundleEditContext::new_disk_media_import_library(
                &trusted.bundle,
                &trusted.assets,
                limits(before.profile())?,
                &trusted.library,
                before.entry(),
                alias,
            )
            .map_err(edit_failure)?;
            let applied = document
                .apply(&transaction, &context)
                .map_err(edit_failure)?;
            let source_after = document.source().to_owned();
            let pin = sha256_digest(source_after.as_bytes());
            let inverse = applied.inverse.to_json().map_err(edit_failure)?;
            let inverse_operations = applied.inverse.operations.len();
            pin_operations.push(Operation::Set {
                object: vec![alias.clone()],
                field: vec!["hash".into()],
                value: serde_json::json!({"t":"string","v":pin}),
                expect: Some(serde_json::json!({"t":"string","v":prior_pin})),
                expect_absent: false,
            });
            alternates.insert(
                alias.clone(),
                (trusted.library.clone(), source_after.clone()),
            );
            replacements.insert(trusted.library.clone(), source_after.clone());
            if trusted_bundle.is_none() {
                trusted_bundle = Some(trusted);
            }
            leaves.push(GroupLeafApplication {
                capture: GroupLeafCapture {
                    alias: alias.clone(),
                    source: source_path,
                    before_revision,
                    after_revision: applied.new_revision.clone(),
                    pin,
                    applied,
                },
                forward,
                inverse,
                forward_operations: transaction.operations.len(),
                inverse_operations,
            });
        }
        let trusted = trusted_bundle.expect("nonempty group");
        let mut entry_document =
            SourceDocument::parse(before.source_text(before.entry())?).map_err(edit_failure)?;
        let mut entry_user = None;
        let mut entry_applied = None;
        let mut entry_forward = None;
        let mut entry_inverse = None;
        if let Some(patch) = entry_patch {
            let tx = Transaction::from_json(patch).map_err(edit_failure)?;
            let forward = tx.to_json().map_err(edit_failure)?;
            let context = BundleEditContext::new_disk_media(
                &trusted.bundle,
                &trusted.assets,
                limits(before.profile())?,
            )
            .map_err(edit_failure)?;
            let before_revision = entry_document.revision().to_owned();
            let applied = entry_document.apply(&tx, &context).map_err(edit_failure)?;
            let inverse = applied.inverse.to_json().map_err(edit_failure)?;
            entry_user = Some(GroupEntryEdit {
                before_revision,
                after_revision: applied.new_revision.clone(),
                forward: edit_file(ENTRY_FORWARD, &forward, tx.operations.len()),
                inverse: edit_file(ENTRY_INVERSE, &inverse, applied.inverse.operations.len()),
            });
            entry_forward = Some(forward);
            entry_inverse = Some(inverse);
            entry_applied = Some(applied);
        }
        let pin_before_revision = entry_document.revision().to_owned();
        let pin_tx =
            Transaction::new(pin_before_revision.clone(), pin_operations).map_err(edit_failure)?;
        let pin_forward = pin_tx.to_json().map_err(edit_failure)?;
        if pin_forward.len() > MAX_GENERATED_PIN_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "generated group pin transaction exceeds 16 KiB",
            ));
        }
        let pin_context = BundleEditContext::new_disk_media_group_importer(
            &trusted.bundle,
            &trusted.assets,
            limits(before.profile())?,
            alternates,
        )
        .map_err(edit_failure)?;
        let pin_applied = entry_document
            .apply(&pin_tx, &pin_context)
            .map_err(edit_failure)?;
        let pin_inverse = pin_applied.inverse.to_json().map_err(edit_failure)?;
        replacements.insert(before.entry().into(), entry_document.source().to_owned());
        let after = before.patched_sources(&replacements)?;
        check_group_import_topology(
            before,
            &after,
            &leaves
                .iter()
                .map(|leaf| (leaf.capture.alias.clone(), leaf.capture.pin.clone()))
                .collect(),
        )?;
        for leaf in &leaves {
            let trusted_after = trusted_import(&after, &leaf.capture.alias)?;
            if trusted_after.library != leaf.capture.source {
                return Err(fail(
                    DiagnosticCode::Capability,
                    "group edit changed a selected direct import",
                ));
            }
        }
        let first = &leaves[0];
        let mut group_payloads = BTreeMap::new();
        let mut others = Vec::new();
        for (index, leaf) in leaves.iter().enumerate().skip(1) {
            let forward_name = format!("leaf-{index:02}-forward.json");
            let inverse_name = format!("leaf-{index:02}-inverse.json");
            others.push(GroupLeafEdit {
                source: leaf.capture.source.clone(),
                alias: leaf.capture.alias.clone(),
                before_revision: leaf.capture.before_revision.clone(),
                after_revision: leaf.capture.after_revision.clone(),
                pin: leaf.capture.pin.clone(),
                forward: edit_file(&forward_name, &leaf.forward, leaf.forward_operations),
                inverse: edit_file(&inverse_name, &leaf.inverse, leaf.inverse_operations),
            });
            group_payloads.insert(forward_name, leaf.forward.clone());
            group_payloads.insert(inverse_name, leaf.inverse.clone());
        }
        if let (Some(forward), Some(inverse)) = (entry_forward, entry_inverse) {
            group_payloads.insert(ENTRY_FORWARD.into(), forward);
            group_payloads.insert(ENTRY_INVERSE.into(), inverse);
        }
        group_payloads.insert(PIN_FORWARD.into(), pin_forward.clone());
        group_payloads.insert(PIN_INVERSE.into(), pin_inverse.clone());
        let manifest = EditManifest {
            format: FORMAT.into(),
            version: GROUP_VERSION,
            parent: parent.into(),
            before: before.digest(),
            after: after.digest(),
            source: first.capture.source.clone(),
            revision_algorithm: REVISION_ALGORITHM.into(),
            before_revision: first.capture.before_revision.clone(),
            after_revision: first.capture.after_revision.clone(),
            forward: edit_file(EDIT_FORWARD, &first.forward, first.forward_operations),
            inverse: edit_file(EDIT_INVERSE, &first.inverse, first.inverse_operations),
            importer: None,
            group: Some(GroupEdit {
                entry_source: before.entry().into(),
                first_alias: first.capture.alias.clone(),
                first_pin: first.capture.pin.clone(),
                others,
                entry: entry_user,
                pin: GroupEntryEdit {
                    before_revision: pin_before_revision,
                    after_revision: pin_applied.new_revision,
                    forward: edit_file(PIN_FORWARD, &pin_forward, pin_tx.operations.len()),
                    inverse: edit_file(
                        PIN_INVERSE,
                        &pin_inverse,
                        pin_applied.inverse.operations.len(),
                    ),
                },
            }),
        };
        validate_manifest(&manifest)?;
        let json = serde_json::to_vec(&manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode edit manifest: {e}"),
            )
        })?;
        if json.len() as u64 > MAX_EDIT_MANIFEST_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "edit manifest exceeds 16 KiB",
            ));
        }
        let edit = Self {
            manifest,
            json,
            forward: first.forward.clone(),
            inverse: first.inverse.clone(),
            importer_forward: None,
            importer_inverse: None,
            group_payloads,
        };
        if edit.total_bytes() > MAX_EDIT_TOTAL_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "group edit exceeds total byte limit",
            ));
        }
        Ok(GroupEditCapture {
            after,
            edit,
            leaves: leaves.into_iter().map(|leaf| leaf.capture).collect(),
            entry_applied,
        })
    }
    pub(crate) fn capture_import(
        parent: &str,
        before: &ArchiveSnapshot,
        alias: &str,
        patch: &[u8],
    ) -> Result<ImportEditCapture, Diagnostics> {
        validate_hash_pin(parent, "edit parent checkpoint")?;
        let trusted = trusted_import(before, alias)?;
        let library_before = before.source_text(&trusted.library)?;
        let prior_pin = sha256_digest(library_before.as_bytes());
        let transaction = Transaction::from_json(patch).map_err(edit_failure)?;
        let forward = transaction.to_json().map_err(edit_failure)?;
        let mut library_document = SourceDocument::parse(library_before).map_err(edit_failure)?;
        let before_revision = library_document.revision().to_owned();
        let library_context = BundleEditContext::new_disk_media_import_library(
            &trusted.bundle,
            &trusted.assets,
            limits(before.profile())?,
            &trusted.library,
            before.entry(),
            alias,
        )
        .map_err(edit_failure)?;
        let applied = library_document
            .apply(&transaction, &library_context)
            .map_err(edit_failure)?;
        let library_after = library_document.source().to_owned();
        let pin = sha256_digest(library_after.as_bytes());
        let mut importer_document =
            SourceDocument::parse(before.source_text(before.entry())?).map_err(edit_failure)?;
        let importer_before_revision = importer_document.revision().to_owned();
        let generated = Transaction::new(
            importer_before_revision.clone(),
            vec![Operation::Set {
                object: vec![alias.into()],
                field: vec!["hash".into()],
                value: serde_json::json!({"t":"string","v":pin}),
                expect: Some(serde_json::json!({"t":"string","v":prior_pin})),
                expect_absent: false,
            }],
        )
        .map_err(edit_failure)?;
        let importer_forward = generated.to_json().map_err(edit_failure)?;
        if importer_forward.len() > MAX_GENERATED_PIN_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "generated import pin transaction exceeds 16 KiB",
            ));
        }
        let importer_context = BundleEditContext::new_disk_media_importer(
            &trusted.bundle,
            &trusted.assets,
            limits(before.profile())?,
            &trusted.library,
            &library_after,
            alias,
        )
        .map_err(edit_failure)?;
        let importer_applied = importer_document
            .apply(&generated, &importer_context)
            .map_err(edit_failure)?;
        let after = before.patched_import_sources(
            &trusted.library,
            &library_after,
            importer_document.source(),
        )?;
        let after_import = trusted_import(&after, alias)?;
        if after_import.library != trusted.library {
            return Err(fail(
                DiagnosticCode::Capability,
                "edited import no longer names the same direct local leaf",
            ));
        }
        let inverse = applied.inverse.to_json().map_err(edit_failure)?;
        let importer_inverse = importer_applied.inverse.to_json().map_err(edit_failure)?;
        let manifest = EditManifest {
            format: FORMAT.into(),
            version: IMPORT_VERSION,
            parent: parent.into(),
            before: before.digest(),
            after: after.digest(),
            source: trusted.library.clone(),
            revision_algorithm: REVISION_ALGORITHM.into(),
            before_revision: before_revision.clone(),
            after_revision: applied.new_revision.clone(),
            forward: edit_file(EDIT_FORWARD, &forward, transaction.operations.len()),
            inverse: edit_file(EDIT_INVERSE, &inverse, applied.inverse.operations.len()),
            importer: Some(ImporterEdit {
                source: before.entry().into(),
                alias: alias.into(),
                before_revision: importer_before_revision.clone(),
                after_revision: importer_applied.new_revision.clone(),
                pin: pin.clone(),
                forward: edit_file(
                    IMPORTER_FORWARD,
                    &importer_forward,
                    generated.operations.len(),
                ),
                inverse: edit_file(
                    IMPORTER_INVERSE,
                    &importer_inverse,
                    importer_applied.inverse.operations.len(),
                ),
            }),
            group: None,
        };
        validate_manifest(&manifest)?;
        let json = serde_json::to_vec(&manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode edit manifest: {e}"),
            )
        })?;
        if json.len() as u64 > MAX_EDIT_MANIFEST_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "edit manifest exceeds 16 KiB",
            ));
        }
        let edit = Self {
            manifest,
            json,
            forward,
            inverse,
            importer_forward: Some(importer_forward),
            importer_inverse: Some(importer_inverse),
            group_payloads: BTreeMap::new(),
        };
        Ok(ImportEditCapture {
            after,
            edit,
            applied,
            import_source: trusted.library,
            before_revision,
            after_revision: library_document.revision().into(),
            pin,
            importer_before_revision,
            importer_after_revision: importer_applied.new_revision,
        })
    }

    pub(crate) fn capture(
        parent: &str,
        before: &ArchiveSnapshot,
        patch: &[u8],
    ) -> Result<(ArchiveSnapshot, Self, AppliedTransaction), Diagnostics> {
        validate_hash_pin(parent, "edit parent checkpoint")?;
        let transaction = Transaction::from_json(patch).map_err(edit_failure)?;
        let forward = transaction.to_json().map_err(edit_failure)?;
        let applied_source = apply_source(before, &transaction)?;
        let after = before.patched_entry_source(&applied_source.source)?;
        let inverse = applied_source
            .applied
            .inverse
            .to_json()
            .map_err(edit_failure)?;
        let manifest = EditManifest {
            format: FORMAT.into(),
            version: VERSION,
            parent: parent.into(),
            before: before.digest(),
            after: after.digest(),
            source: before.entry().into(),
            revision_algorithm: REVISION_ALGORITHM.into(),
            before_revision: applied_source.before_revision,
            after_revision: applied_source.applied.new_revision.clone(),
            forward: EditFile {
                path: EDIT_FORWARD.into(),
                bytes: forward.len() as u64,
                sha256: sha256_digest(&forward),
                operations: transaction.operations.len(),
            },
            inverse: EditFile {
                path: EDIT_INVERSE.into(),
                bytes: inverse.len() as u64,
                sha256: sha256_digest(&inverse),
                operations: applied_source.applied.inverse.operations.len(),
            },
            importer: None,
            group: None,
        };
        validate_manifest(&manifest)?;
        let json = serde_json::to_vec(&manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode edit manifest: {e}"),
            )
        })?;
        if json.len() as u64 > MAX_EDIT_MANIFEST_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "edit manifest exceeds 16 KiB",
            ));
        }
        Ok((
            after,
            Self {
                manifest,
                json,
                forward,
                inverse,
                importer_forward: None,
                importer_inverse: None,
                group_payloads: BTreeMap::new(),
            },
            applied_source.applied,
        ))
    }

    pub(crate) fn preflight(
        root: &ProjectRoot,
        digest: &str,
        parent: &str,
        before: &str,
        after: &str,
    ) -> Result<EditPreflight, Diagnostics> {
        validate_hash_pin(digest, "edit digest")?;
        let json = read_bounded(root, EDIT_MANIFEST, MAX_EDIT_MANIFEST_BYTES)?;
        let manifest: EditManifest = serde_json::from_slice(&json).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("invalid edit manifest: {e}"),
            )
        })?;
        validate_manifest(&manifest)?;
        check_tree(root, &manifest)?;
        if serde_json::to_vec(&manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode edit manifest: {e}"),
            )
        })? != json
        {
            return Err(fail(
                DiagnosticCode::Syntax,
                "edit manifest is not canonical JSON",
            ));
        }
        if sha256_digest(&json) != digest
            || manifest.parent != parent
            || manifest.before != before
            || manifest.after != after
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "edit identity differs from its checkpoint transition",
            ));
        }
        for file in edit_files(&manifest) {
            let meta = root.dir().symlink_metadata(&file.path).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot inspect edit payload: {e}"),
                )
            })?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != file.bytes {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "edit payload metadata differs from manifest",
                ));
            }
        }
        Ok(EditPreflight { manifest, json })
    }

    pub(crate) fn verify_preflighted(
        root: &ProjectRoot,
        preflight: &EditPreflight,
    ) -> Result<Self, Diagnostics> {
        check_tree(root, &preflight.manifest)?;
        let forward = read_bounded(root, EDIT_FORWARD, MAX_TRANSACTION_BYTES as u64)?;
        let inverse = read_bounded(root, EDIT_INVERSE, MAX_TRANSACTION_BYTES as u64)?;
        let importer_forward = if preflight.manifest.importer.is_some() {
            Some(read_bounded(
                root,
                IMPORTER_FORWARD,
                MAX_GENERATED_PIN_BYTES as u64,
            )?)
        } else {
            None
        };
        let importer_inverse = if preflight.manifest.importer.is_some() {
            Some(read_bounded(
                root,
                IMPORTER_INVERSE,
                MAX_TRANSACTION_BYTES as u64,
            )?)
        } else {
            None
        };
        let mut group_payloads = BTreeMap::new();
        if preflight.manifest.group.is_some() {
            for file in edit_files(&preflight.manifest).into_iter().skip(2) {
                let limit = if file.path == PIN_FORWARD {
                    MAX_GENERATED_PIN_BYTES as u64
                } else {
                    MAX_TRANSACTION_BYTES as u64
                };
                group_payloads.insert(file.path.clone(), read_bounded(root, &file.path, limit)?);
            }
        }
        let mut files = vec![
            (&preflight.manifest.forward, &forward),
            (&preflight.manifest.inverse, &inverse),
        ];
        if let Some(importer) = &preflight.manifest.importer {
            files.push((
                &importer.forward,
                importer_forward.as_ref().expect("v2 file"),
            ));
            files.push((
                &importer.inverse,
                importer_inverse.as_ref().expect("v2 file"),
            ));
        }
        if preflight.manifest.group.is_some() {
            for file in edit_files(&preflight.manifest).into_iter().skip(2) {
                files.push((file, group_payloads.get(&file.path).expect("v3 file")));
            }
        }
        for (file, bytes) in files {
            if file.bytes != bytes.len() as u64 || file.sha256 != sha256_digest(bytes) {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "edit payload bytes differ from manifest",
                ));
            }
            let tx = Transaction::from_json(bytes).map_err(edit_failure)?;
            if tx.operations.len() != file.operations
                || tx.to_json().map_err(edit_failure)? != *bytes
            {
                return Err(fail(
                    DiagnosticCode::Syntax,
                    "edit transaction is not canonical JSON",
                ));
            }
        }
        if read_bounded(root, EDIT_MANIFEST, MAX_EDIT_MANIFEST_BYTES)? != preflight.json {
            return Err(fail(
                DiagnosticCode::Hash,
                "edit manifest changed during verification",
            ));
        }
        Ok(Self {
            manifest: preflight.manifest.clone(),
            json: preflight.json.clone(),
            forward,
            inverse,
            importer_forward,
            importer_inverse,
            group_payloads,
        })
    }

    pub(crate) fn verify_transition(
        &self,
        parent: &str,
        before: &ArchiveSnapshot,
        after: &ArchiveSnapshot,
    ) -> Result<(), Diagnostics> {
        if self.manifest.group.is_some() {
            return self.verify_group_transition(parent, before, after);
        }
        if self.manifest.importer.is_some() {
            return self.verify_import_transition(parent, before, after);
        }
        if self.manifest.parent != parent
            || self.manifest.before != before.digest()
            || self.manifest.after != after.digest()
            || self.manifest.source != before.entry()
            || self.manifest.source != after.entry()
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "edit checkpoint transition is inconsistent",
            ));
        }
        let forward = Transaction::from_json(&self.forward).map_err(edit_failure)?;
        let replayed_forward = apply_source(before, &forward)?;
        if replayed_forward.before_revision != self.manifest.before_revision
            || replayed_forward.applied.new_revision != self.manifest.after_revision
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "stored edit revisions differ from forward replay",
            ));
        }
        let replayed = before.patched_entry_source(&replayed_forward.source)?;
        if replayed.digest() != after.digest() {
            return Err(fail(
                DiagnosticCode::Hash,
                "forward edit does not reproduce the after snapshot",
            ));
        }
        let inverse = Transaction::from_json(&self.inverse).map_err(edit_failure)?;
        // An inverse is evidence about the authored tree. Reprojecting it into
        // source text can grow otherwise valid, comment-padded source past the
        // 4 MiB source limit without changing its authored meaning.
        let replayed_inverse = apply_authored(after, &inverse)?;
        if replayed_inverse.before_revision != self.manifest.after_revision
            || replayed_inverse.applied.new_revision != self.manifest.before_revision
            || replayed_inverse.before_tree != replayed_forward.after_tree
            || replayed_inverse.after_tree != replayed_forward.before_tree
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "inverse edit does not restore the parent authored tree",
            ));
        }
        Ok(())
    }

    fn verify_import_transition(
        &self,
        parent: &str,
        before: &ArchiveSnapshot,
        after: &ArchiveSnapshot,
    ) -> Result<(), Diagnostics> {
        let importer = self.manifest.importer.as_ref().expect("v2 importer");
        if self.manifest.parent != parent
            || self.manifest.before != before.digest()
            || self.manifest.after != after.digest()
            || importer.source != before.entry()
            || importer.source != after.entry()
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "import edit checkpoint transition is inconsistent",
            ));
        }
        let trusted_before = trusted_import(before, &importer.alias)?;
        let trusted_after = trusted_import(after, &importer.alias)?;
        if trusted_before.library != self.manifest.source
            || trusted_after.library != self.manifest.source
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "edited import source differs from record",
            ));
        }
        let library_before = before.source_text(&self.manifest.source)?;
        let library_after = after.source_text(&self.manifest.source)?;
        let pin_before = sha256_digest(library_before.as_bytes());
        let pin_after = sha256_digest(library_after.as_bytes());
        if pin_after != importer.pin {
            return Err(fail(
                DiagnosticCode::Hash,
                "generated import pin differs from exact library bytes",
            ));
        }
        let mut library_document = SourceDocument::parse(library_before).map_err(edit_failure)?;
        let library_before_tree = library_document.authored().tree().clone();
        if library_document.revision() != self.manifest.before_revision {
            return Err(fail(
                DiagnosticCode::Hash,
                "library before revision differs",
            ));
        }
        let library_context = BundleEditContext::new_disk_media_import_library(
            &trusted_before.bundle,
            &trusted_before.assets,
            limits(before.profile())?,
            &self.manifest.source,
            before.entry(),
            &importer.alias,
        )
        .map_err(edit_failure)?;
        let forward = Transaction::from_json(&self.forward).map_err(edit_failure)?;
        let library_applied = library_document
            .apply(&forward, &library_context)
            .map_err(edit_failure)?;
        if library_document.source() != library_after
            || library_applied.new_revision != self.manifest.after_revision
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "library forward edit does not reproduce exact child source",
            ));
        }
        let library_after_tree = library_document.authored().tree().clone();
        let mut importer_document =
            SourceDocument::parse(before.source_text(before.entry())?).map_err(edit_failure)?;
        let importer_before_tree = importer_document.authored().tree().clone();
        if importer_document.revision() != importer.before_revision {
            return Err(fail(
                DiagnosticCode::Hash,
                "importer before revision differs",
            ));
        }
        let expected = Transaction::new(
            importer.before_revision.clone(),
            vec![Operation::Set {
                object: vec![importer.alias.clone()],
                field: vec!["hash".into()],
                value: serde_json::json!({"t":"string","v":pin_after}),
                expect: Some(serde_json::json!({"t":"string","v":pin_before})),
                expect_absent: false,
            }],
        )
        .map_err(edit_failure)?;
        let importer_forward =
            Transaction::from_json(self.importer_forward.as_ref().expect("v2 payload"))
                .map_err(edit_failure)?;
        if importer_forward != expected {
            return Err(fail(
                DiagnosticCode::Hash,
                "generated importer transaction differs from exact source pins",
            ));
        }
        let importer_context = BundleEditContext::new_disk_media_importer(
            &trusted_before.bundle,
            &trusted_before.assets,
            limits(before.profile())?,
            &self.manifest.source,
            library_after,
            &importer.alias,
        )
        .map_err(edit_failure)?;
        let importer_applied = importer_document
            .apply(&importer_forward, &importer_context)
            .map_err(edit_failure)?;
        if importer_document.source() != after.source_text(after.entry())?
            || importer_applied.new_revision != importer.after_revision
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "importer forward edit does not reproduce exact child source",
            ));
        }
        let replayed = before.patched_import_sources(
            &self.manifest.source,
            library_document.source(),
            importer_document.source(),
        )?;
        if replayed.digest() != after.digest() {
            return Err(fail(
                DiagnosticCode::Hash,
                "import edit changed closure outside the two authored sources",
            ));
        }
        let importer_after_tree = importer_document.authored().tree().clone();
        let library_inverse = Transaction::from_json(&self.inverse).map_err(edit_failure)?;
        let mut inverse_library = SourceDocument::parse(library_after)
            .map_err(edit_failure)?
            .authored()
            .clone();
        if inverse_library.tree() != &library_after_tree {
            return Err(fail(
                DiagnosticCode::Hash,
                "library forward authored tree differs from child",
            ));
        }
        let inverse_library_context = BundleEditContext::new_disk_media_import_library(
            &trusted_after.bundle,
            &trusted_after.assets,
            limits(after.profile())?,
            &self.manifest.source,
            after.entry(),
            &importer.alias,
        )
        .map_err(edit_failure)?;
        let library_restored = apply_transaction(
            &mut inverse_library,
            &library_inverse,
            &inverse_library_context,
        )
        .map_err(edit_failure)?;
        if library_restored.new_revision != self.manifest.before_revision
            || inverse_library.tree() != &library_before_tree
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "library inverse does not restore parent authored tree",
            ));
        }
        let importer_inverse =
            Transaction::from_json(self.importer_inverse.as_ref().expect("v2 payload"))
                .map_err(edit_failure)?;
        let mut inverse_importer = SourceDocument::parse(after.source_text(after.entry())?)
            .map_err(edit_failure)?
            .authored()
            .clone();
        if inverse_importer.tree() != &importer_after_tree {
            return Err(fail(
                DiagnosticCode::Hash,
                "importer forward authored tree differs from child",
            ));
        }
        let inverse_importer_context = BundleEditContext::new_disk_media_importer(
            &trusted_after.bundle,
            &trusted_after.assets,
            limits(after.profile())?,
            &self.manifest.source,
            library_before,
            &importer.alias,
        )
        .map_err(edit_failure)?;
        let importer_restored = apply_transaction(
            &mut inverse_importer,
            &importer_inverse,
            &inverse_importer_context,
        )
        .map_err(edit_failure)?;
        if importer_restored.new_revision != importer.before_revision
            || inverse_importer.tree() != &importer_before_tree
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "importer inverse does not restore parent authored tree",
            ));
        }
        Ok(())
    }

    fn verify_group_transition(
        &self,
        parent: &str,
        before: &ArchiveSnapshot,
        after: &ArchiveSnapshot,
    ) -> Result<(), Diagnostics> {
        let group = self.manifest.group.as_ref().expect("v3 group");
        if self.manifest.parent != parent
            || self.manifest.before != before.digest()
            || self.manifest.after != after.digest()
            || group.entry_source != before.entry()
            || before.entry() != after.entry()
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "group edit checkpoint transition is inconsistent",
            ));
        }
        let first_before = trusted_import(before, &group.first_alias)?;
        let first_after = trusted_import(after, &group.first_alias)?;
        if first_before.library != self.manifest.source
            || first_after.library != self.manifest.source
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "first group import source differs",
            ));
        }
        let mut leaf_records = Vec::with_capacity(group.others.len() + 1);
        leaf_records.push((
            group.first_alias.as_str(),
            self.manifest.source.as_str(),
            self.manifest.before_revision.as_str(),
            self.manifest.after_revision.as_str(),
            group.first_pin.as_str(),
            &self.forward,
            &self.inverse,
        ));
        for leaf in &group.others {
            leaf_records.push((
                leaf.alias.as_str(),
                leaf.source.as_str(),
                leaf.before_revision.as_str(),
                leaf.after_revision.as_str(),
                leaf.pin.as_str(),
                self.group_payloads
                    .get(&leaf.forward.path)
                    .expect("verified group forward"),
                self.group_payloads
                    .get(&leaf.inverse.path)
                    .expect("verified group inverse"),
            ));
        }
        let mut replacements = BTreeMap::new();
        let mut alternates = BTreeMap::new();
        let mut inverse_alternates = BTreeMap::new();
        let mut pin_operations = Vec::new();
        for (alias, source, before_revision, after_revision, pin, forward_bytes, inverse_bytes) in
            leaf_records
        {
            let trusted_before = trusted_import(before, alias)?;
            let trusted_after = trusted_import(after, alias)?;
            if trusted_before.library != source || trusted_after.library != source {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group import source differs from record",
                ));
            }
            let original = before.source_text(source)?;
            let child = after.source_text(source)?;
            if sha256_digest(child.as_bytes()) != pin {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group pin differs from exact leaf bytes",
                ));
            }
            let mut document = SourceDocument::parse(original).map_err(edit_failure)?;
            let before_tree = document.authored().tree().clone();
            if document.revision() != before_revision {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group leaf before revision differs",
                ));
            }
            let context = BundleEditContext::new_disk_media_import_library(
                &trusted_before.bundle,
                &trusted_before.assets,
                limits(before.profile())?,
                source,
                before.entry(),
                alias,
            )
            .map_err(edit_failure)?;
            let forward = Transaction::from_json(forward_bytes).map_err(edit_failure)?;
            let applied = document.apply(&forward, &context).map_err(edit_failure)?;
            if document.source() != child || applied.new_revision != after_revision {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group leaf forward does not reproduce exact child bytes",
                ));
            }
            let after_tree = document.authored().tree().clone();
            let mut inverse_document = SourceDocument::parse(child)
                .map_err(edit_failure)?
                .authored()
                .clone();
            if inverse_document.tree() != &after_tree {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group leaf child authored tree differs",
                ));
            }
            let inverse_context = BundleEditContext::new_disk_media_import_library(
                &trusted_before.bundle,
                &trusted_before.assets,
                limits(before.profile())?,
                source,
                before.entry(),
                alias,
            )
            .map_err(edit_failure)?;
            let inverse = Transaction::from_json(inverse_bytes).map_err(edit_failure)?;
            let restored = apply_transaction(&mut inverse_document, &inverse, &inverse_context)
                .map_err(edit_failure)?;
            if restored.new_revision != before_revision || inverse_document.tree() != &before_tree {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group leaf inverse does not restore parent tree",
                ));
            }
            let original_pin = sha256_digest(original.as_bytes());
            pin_operations.push(Operation::Set {
                object: vec![alias.into()],
                field: vec!["hash".into()],
                value: serde_json::json!({"t":"string","v":pin}),
                expect: Some(serde_json::json!({"t":"string","v":original_pin})),
                expect_absent: false,
            });
            replacements.insert(source.into(), child.into());
            alternates.insert(alias.into(), (source.into(), child.into()));
            inverse_alternates.insert(alias.into(), (source.into(), original.into()));
        }
        let original_entry = before.source_text(before.entry())?;
        let original_entry_document =
            SourceDocument::parse(original_entry).map_err(edit_failure)?;
        let original_entry_tree = original_entry_document.authored().tree().clone();
        let mut entry_document = original_entry_document;
        if let Some(entry) = &group.entry {
            if entry_document.revision() != entry.before_revision {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group entry before revision differs",
                ));
            }
            let context = BundleEditContext::new_disk_media(
                &first_before.bundle,
                &first_before.assets,
                limits(before.profile())?,
            )
            .map_err(edit_failure)?;
            let forward = Transaction::from_json(
                self.group_payloads
                    .get(ENTRY_FORWARD)
                    .expect("verified entry forward"),
            )
            .map_err(edit_failure)?;
            let applied = entry_document
                .apply(&forward, &context)
                .map_err(edit_failure)?;
            if applied.new_revision != entry.after_revision {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group entry forward revision differs",
                ));
            }
            let mut inverse_document = entry_document.authored().clone();
            let inverse = Transaction::from_json(
                self.group_payloads
                    .get(ENTRY_INVERSE)
                    .expect("verified entry inverse"),
            )
            .map_err(edit_failure)?;
            let restored = apply_transaction(&mut inverse_document, &inverse, &context)
                .map_err(edit_failure)?;
            if restored.new_revision != entry.before_revision
                || inverse_document.tree() != &original_entry_tree
            {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "group entry inverse does not restore parent tree",
                ));
            }
        }
        if entry_document.revision() != group.pin.before_revision {
            return Err(fail(
                DiagnosticCode::Hash,
                "group pin before revision differs",
            ));
        }
        let entry_user_tree = entry_document.authored().tree().clone();
        let expected_pin = Transaction::new(group.pin.before_revision.clone(), pin_operations)
            .map_err(edit_failure)?;
        let pin_forward = Transaction::from_json(
            self.group_payloads
                .get(PIN_FORWARD)
                .expect("verified pin forward"),
        )
        .map_err(edit_failure)?;
        if pin_forward != expected_pin {
            return Err(fail(
                DiagnosticCode::Hash,
                "generated group pin differs from exact source pins",
            ));
        }
        let pin_context = BundleEditContext::new_disk_media_group_importer(
            &first_before.bundle,
            &first_before.assets,
            limits(before.profile())?,
            alternates,
        )
        .map_err(edit_failure)?;
        let applied = entry_document
            .apply(&pin_forward, &pin_context)
            .map_err(edit_failure)?;
        if applied.new_revision != group.pin.after_revision
            || entry_document.source() != after.source_text(after.entry())?
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "group pin forward does not reproduce exact child entry",
            ));
        }
        let child_entry_tree = entry_document.authored().tree().clone();
        let mut inverse_entry = SourceDocument::parse(after.source_text(after.entry())?)
            .map_err(edit_failure)?
            .authored()
            .clone();
        if inverse_entry.tree() != &child_entry_tree {
            return Err(fail(
                DiagnosticCode::Hash,
                "group pin child authored tree differs",
            ));
        }
        let inverse_context = BundleEditContext::new_disk_media_group_importer(
            &first_after.bundle,
            &first_after.assets,
            limits(after.profile())?,
            inverse_alternates,
        )
        .map_err(edit_failure)?;
        let pin_inverse = Transaction::from_json(
            self.group_payloads
                .get(PIN_INVERSE)
                .expect("verified pin inverse"),
        )
        .map_err(edit_failure)?;
        let restored = apply_transaction(&mut inverse_entry, &pin_inverse, &inverse_context)
            .map_err(edit_failure)?;
        if restored.new_revision != group.pin.before_revision
            || inverse_entry.tree() != &entry_user_tree
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "group pin inverse does not restore entry tree",
            ));
        }
        replacements.insert(before.entry().into(), entry_document.source().into());
        let replayed = before.patched_sources(&replacements)?;
        if replayed.digest() != after.digest() {
            return Err(fail(
                DiagnosticCode::Hash,
                "group edit changed closure outside selected sources",
            ));
        }
        let selected: BTreeMap<String, String> = group
            .others
            .iter()
            .map(|leaf| (leaf.alias.clone(), leaf.pin.clone()))
            .chain(std::iter::once((
                group.first_alias.clone(),
                group.first_pin.clone(),
            )))
            .collect();
        check_group_import_topology(before, after, &selected)?;
        Ok(())
    }

    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.json)
    }
    pub(crate) fn total_bytes(&self) -> u64 {
        self.json.len() as u64
            + self.forward.len() as u64
            + self.inverse.len() as u64
            + self.importer_forward.as_ref().map_or(0, Vec::len) as u64
            + self.importer_inverse.as_ref().map_or(0, Vec::len) as u64
            + self.group_payloads.values().map(Vec::len).sum::<usize>() as u64
    }
    pub(crate) fn is_import_edit(&self) -> bool {
        self.manifest.importer.is_some() || self.manifest.group.is_some()
    }
    pub(crate) fn is_group_edit(&self) -> bool {
        self.manifest.group.is_some()
    }

    pub(crate) fn group_leaf_count(&self) -> Option<usize> {
        self.manifest
            .group
            .as_ref()
            .map(|group| group.others.len() + 1)
    }
    pub(crate) fn tree_entries(&self) -> usize {
        2 + edit_files(&self.manifest).len()
    }
    pub(crate) fn transaction_work(&self) -> Result<u64, Diagnostics> {
        transaction_work(&self.manifest)
    }

    pub(crate) fn stage_into(&self, dir: &Path) -> Result<(), Diagnostics> {
        let expected_group: BTreeSet<_> = if self.manifest.group.is_some() {
            edit_files(&self.manifest)
                .into_iter()
                .skip(2)
                .map(|file| file.path.as_str())
                .collect()
        } else {
            BTreeSet::new()
        };
        if expected_group != self.group_payloads.keys().map(String::as_str).collect() {
            return Err(fail(
                DiagnosticCode::Hash,
                "private group edit payload set differs from manifest",
            ));
        }
        let meta = fs::symlink_metadata(dir).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect edit destination: {e}"),
            )
        })?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(fail(
                DiagnosticCode::Reference,
                "edit destination must be a real directory",
            ));
        }
        let mut files = vec![
            (EDIT_MANIFEST, &self.json),
            (EDIT_FORWARD, &self.forward),
            (EDIT_INVERSE, &self.inverse),
        ];
        if let (Some(forward), Some(inverse)) = (&self.importer_forward, &self.importer_inverse) {
            files.push((IMPORTER_FORWARD, forward));
            files.push((IMPORTER_INVERSE, inverse));
        }
        for (name, bytes) in &self.group_payloads {
            files.push((name.as_str(), bytes));
        }
        for (name, bytes) in files {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join(name))
                .map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot create edit file `{name}`: {e}"),
                    )
                })?;
            output.write_all(bytes).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot write edit file `{name}`: {e}"),
                )
            })?;
        }
        if serde_json::to_vec(&self.manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode edit manifest: {e}"),
            )
        })? != self.json
            || sha256_digest(&self.forward) != self.manifest.forward.sha256
            || sha256_digest(&self.inverse) != self.manifest.inverse.sha256
            || self.manifest.importer.as_ref().is_some_and(|importer| {
                self.importer_forward
                    .as_ref()
                    .is_none_or(|bytes| sha256_digest(bytes) != importer.forward.sha256)
                    || self
                        .importer_inverse
                        .as_ref()
                        .is_none_or(|bytes| sha256_digest(bytes) != importer.inverse.sha256)
            })
            || self.manifest.group.as_ref().is_some_and(|_| {
                edit_files(&self.manifest).into_iter().skip(2).any(|file| {
                    self.group_payloads.get(&file.path).is_none_or(|bytes| {
                        bytes.len() as u64 != file.bytes || sha256_digest(bytes) != file.sha256
                    })
                })
            })
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "private edit evidence changed before staging",
            ));
        }
        Ok(())
    }
}

fn edit_file(path: &str, bytes: &[u8], operations: usize) -> EditFile {
    EditFile {
        path: path.into(),
        bytes: bytes.len() as u64,
        sha256: sha256_digest(bytes),
        operations,
    }
}

fn limits(profile: &str) -> Result<PlanLimits, Diagnostics> {
    match profile {
        "default" => Ok(PlanLimits::default()),
        "song" => Ok(PlanLimits::song()),
        _ => Err(fail(DiagnosticCode::Version, "unsupported archive profile")),
    }
}

fn trusted_import(snapshot: &ArchiveSnapshot, alias: &str) -> Result<TrustedImport, Diagnostics> {
    if alias.is_empty() || alias.len() > 4096 {
        return Err(fail(
            DiagnosticCode::Capability,
            "invalid direct import alias",
        ));
    }
    let staging = tempfile::tempdir().map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot create private edit staging: {e}"),
        )
    })?;
    snapshot.stage_members(staging.path())?;
    let project = DiskMediaProject::load(Path::new(snapshot.entry()), staging.path())?;
    let resolved = project
        .bundle()
        .resolve_with_disk_assets(project.disk_assets())?;
    let selected: Vec<_> = resolved
        .dependencies
        .iter()
        .filter(|edge| edge.source == snapshot.entry() && edge.alias == alias)
        .collect();
    if selected.len() != 1 {
        return Err(fail(
            DiagnosticCode::Capability,
            "selected alias must be one direct local import",
        ));
    }
    let library = &selected[0].path;
    if library.starts_with("@builtin/")
        || !project.bundle().sources.contains_key(library)
        || resolved
            .dependencies
            .iter()
            .filter(|edge| &edge.path == library)
            .count()
            != 1
        || resolved
            .dependencies
            .iter()
            .any(|edge| &edge.source == library)
    {
        return Err(fail(
            DiagnosticCode::Capability,
            "selected import must be a local leaf imported exactly once",
        ));
    }
    snapshot.source_text(library)?;
    Ok(TrustedImport {
        bundle: project.bundle().clone(),
        assets: project.disk_assets().clone(),
        library: library.clone(),
    })
}

type ImportTopology = BTreeMap<(String, String), (String, String)>;

fn import_topology(snapshot: &ArchiveSnapshot) -> Result<ImportTopology, Diagnostics> {
    let staging = tempfile::tempdir().map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot create private edit staging: {e}"),
        )
    })?;
    snapshot.stage_members(staging.path())?;
    let project = DiskMediaProject::load(Path::new(snapshot.entry()), staging.path())?;
    let resolved = project
        .bundle()
        .resolve_with_disk_assets(project.disk_assets())?;
    let mut topology = BTreeMap::new();
    for edge in resolved.dependencies {
        if topology
            .insert((edge.source, edge.alias), (edge.path, edge.hash))
            .is_some()
        {
            return Err(fail(DiagnosticCode::Capability, "duplicate import edge"));
        }
    }
    Ok(topology)
}

fn check_group_import_topology(
    before: &ArchiveSnapshot,
    after: &ArchiveSnapshot,
    selected: &BTreeMap<String, String>,
) -> Result<(), Diagnostics> {
    let original = import_topology(before)?;
    let child = import_topology(after)?;
    if original.len() != child.len() {
        return Err(fail(
            DiagnosticCode::Capability,
            "group edit changed import topology",
        ));
    }
    for ((source, alias), (path, old_pin)) in &original {
        let new = child.get(&(source.clone(), alias.clone()));
        let expected_pin = if source == before.entry() {
            selected.get(alias).unwrap_or(old_pin)
        } else {
            old_pin
        };
        if new != Some(&(path.clone(), expected_pin.clone())) {
            return Err(fail(
                DiagnosticCode::Capability,
                "group edit changed import topology or an unrelated pin",
            ));
        }
    }
    Ok(())
}

fn apply_source(
    snapshot: &ArchiveSnapshot,
    transaction: &Transaction,
) -> Result<SourceApplication, Diagnostics> {
    let (mut document, context) = trusted_document_and_context(snapshot)?;
    let before_revision = document.revision().to_owned();
    let before_tree = document.authored().tree().clone();
    let applied = document
        .apply(transaction, &context)
        .map_err(edit_failure)?;
    Ok(SourceApplication {
        source: document.source().to_owned(),
        before_revision,
        before_tree,
        after_tree: document.authored().tree().clone(),
        applied,
    })
}

fn apply_authored(
    snapshot: &ArchiveSnapshot,
    transaction: &Transaction,
) -> Result<AuthoredApplication, Diagnostics> {
    let (document, context) = trusted_document_and_context(snapshot)?;
    let mut authored = document.authored().clone();
    let before_revision = authored.revision().to_owned();
    let before_tree = authored.tree().clone();
    let applied = apply_transaction(&mut authored, transaction, &context).map_err(edit_failure)?;
    Ok(AuthoredApplication {
        before_revision,
        before_tree,
        after_tree: authored.tree().clone(),
        applied,
    })
}

fn trusted_document_and_context(
    snapshot: &ArchiveSnapshot,
) -> Result<(SourceDocument, BundleEditContext), Diagnostics> {
    let staged = tempfile::tempdir().map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot create private edit staging: {e}"),
        )
    })?;
    snapshot.stage_members(staged.path())?;
    let project = DiskMediaProject::load(Path::new(snapshot.entry()), staged.path())?;
    let source = project
        .bundle()
        .sources
        .get(snapshot.entry())
        .ok_or_else(|| {
            fail(
                DiagnosticCode::Reference,
                "archived entry source is missing",
            )
        })?;
    let document = SourceDocument::parse(source.clone()).map_err(edit_failure)?;
    let limits = match snapshot.profile() {
        "default" => PlanLimits::default(),
        "song" => PlanLimits::song(),
        _ => {
            return Err(fail(
                DiagnosticCode::Version,
                "unsupported edited archive profile",
            ))
        }
    };
    let context =
        BundleEditContext::new_disk_media(project.bundle(), project.disk_assets(), limits)
            .map_err(edit_failure)?;
    Ok((document, context))
}

fn validate_manifest(m: &EditManifest) -> Result<(), Diagnostics> {
    if m.format != FORMAT
        || !matches!(
            (m.version, m.importer.is_some(), m.group.is_some()),
            (VERSION, false, false) | (IMPORT_VERSION, true, false) | (GROUP_VERSION, false, true)
        )
    {
        return Err(fail(
            DiagnosticCode::Version,
            "unsupported archive edit format or version",
        ));
    }
    for (value, label) in [
        (&m.parent, "edit parent"),
        (&m.before, "edit before snapshot"),
        (&m.after, "edit after snapshot"),
        (&m.before_revision, "edit before revision"),
        (&m.after_revision, "edit after revision"),
        (&m.forward.sha256, "forward transaction"),
        (&m.inverse.sha256, "inverse transaction"),
    ] {
        validate_hash_pin(value, label)?;
    }
    if m.revision_algorithm != REVISION_ALGORITHM
        || m.source.is_empty()
        || m.source.len() > 4096
        || m.source.starts_with('/')
        || m.source
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || m.source.contains('\\')
        || m.source.contains('\0')
    {
        return Err(fail(
            DiagnosticCode::Syntax,
            "invalid edit source or revision algorithm",
        ));
    }
    if let Some(importer) = &m.importer {
        for (value, label) in [
            (&importer.before_revision, "importer before revision"),
            (&importer.after_revision, "importer after revision"),
            (&importer.pin, "generated import pin"),
        ] {
            validate_hash_pin(value, label)?;
        }
        if importer.source.is_empty()
            || importer.source == m.source
            || importer.alias.is_empty()
            || importer.alias.len() > 4096
            || importer.source.starts_with('/')
            || importer.source.contains('\\')
            || importer
                .source
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(fail(
                DiagnosticCode::Syntax,
                "invalid importer source or alias",
            ));
        }
    }
    if let Some(group) = &m.group {
        if group.others.len() >= MAX_GROUP_LEAVES
            || !valid_source_path(&group.entry_source)
            || group.entry_source == m.source
            || group.first_alias.is_empty()
            || group.first_alias.len() > 4096
            || group
                .others
                .iter()
                .any(|leaf| leaf.alias.is_empty() || leaf.alias.len() > 4096)
        {
            return Err(fail(DiagnosticCode::Syntax, "invalid group import aliases"));
        }
        validate_hash_pin(&group.first_pin, "first group import pin")?;
        let mut aliases = BTreeSet::from([group.first_alias.as_str()]);
        let mut sources = BTreeSet::from([m.source.as_str()]);
        let mut previous = group.first_alias.as_str();
        for (index, leaf) in group.others.iter().enumerate() {
            if leaf.alias.as_str() <= previous
                || !aliases.insert(&leaf.alias)
                || !sources.insert(&leaf.source)
                || leaf.source == group.entry_source
                || !valid_source_path(&leaf.source)
                || leaf.forward.path != format!("leaf-{:02}-forward.json", index + 1)
                || leaf.inverse.path != format!("leaf-{:02}-inverse.json", index + 1)
            {
                return Err(fail(
                    DiagnosticCode::Syntax,
                    "group leaves are not canonical",
                ));
            }
            previous = &leaf.alias;
            for (pin, label) in [
                (&leaf.before_revision, "group before revision"),
                (&leaf.after_revision, "group after revision"),
                (&leaf.pin, "group source pin"),
            ] {
                validate_hash_pin(pin, label)?;
            }
        }
        if let Some(entry) = &group.entry {
            validate_group_entry(entry, ENTRY_FORWARD, ENTRY_INVERSE)?;
        }
        validate_group_entry(&group.pin, PIN_FORWARD, PIN_INVERSE)?;
        if group.pin.forward.bytes > MAX_GENERATED_PIN_BYTES as u64
            || group.pin.forward.operations != group.others.len() + 1
        {
            return Err(fail(
                DiagnosticCode::Syntax,
                "invalid group pin transaction metadata",
            ));
        }
        if group
            .entry
            .as_ref()
            .is_some_and(|entry| entry.after_revision != group.pin.before_revision)
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "entry and pin revisions do not join",
            ));
        }
    }
    for file in edit_files(m) {
        let name = file.path.as_str();
        if (name.contains('/') || name.contains('\\') || name.is_empty())
            || file.bytes == 0
            || file.bytes > MAX_TRANSACTION_BYTES as u64
            || file.operations == 0
            || file.operations > MAX_OPERATIONS
        {
            return Err(fail(
                DiagnosticCode::Syntax,
                "invalid edit transaction metadata",
            ));
        }
    }
    if let Some(importer) = &m.importer {
        if importer.forward.path != IMPORTER_FORWARD
            || importer.inverse.path != IMPORTER_INVERSE
            || importer.forward.bytes > MAX_GENERATED_PIN_BYTES as u64
            || importer.forward.operations != 1
        {
            return Err(fail(
                DiagnosticCode::Syntax,
                "invalid generated import pin metadata",
            ));
        }
    }
    if m.forward.path != EDIT_FORWARD || m.inverse.path != EDIT_INVERSE {
        return Err(fail(
            DiagnosticCode::Syntax,
            "invalid edit transaction paths",
        ));
    }
    if m.group.is_some()
        && edit_files(m).iter().map(|file| file.bytes).sum::<u64>() > MAX_EDIT_TOTAL_BYTES
    {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "group edit payloads exceed 64 MiB",
        ));
    }
    let work = transaction_work(m)?;
    if m.group.is_some() && work > MAX_EDIT_TRANSACTION_WORK_BYTES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "group edit exceeds 2 GiB transaction work preflight",
        ));
    }
    Ok(())
}

fn transaction_work(m: &EditManifest) -> Result<u64, Diagnostics> {
    let mut total = 0u64;
    for file in edit_files(m) {
        let work = (MAX_DOCUMENT_BYTES as u64)
            .checked_add(file.bytes)
            .and_then(|n| n.checked_mul(file.operations as u64 + 4))
            .ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "edit transaction work overflow",
                )
            })?;
        if work > MAX_SINGLE_TRANSACTION_WORK_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "edit transaction exceeds 256 MiB work preflight",
            ));
        }
        total = total.checked_add(work).ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "edit transaction work overflow",
            )
        })?;
    }
    Ok(total)
}

fn edit_files(manifest: &EditManifest) -> Vec<&EditFile> {
    let mut files = vec![&manifest.forward, &manifest.inverse];
    if let Some(importer) = &manifest.importer {
        files.push(&importer.forward);
        files.push(&importer.inverse);
    }
    if let Some(group) = &manifest.group {
        for leaf in &group.others {
            files.push(&leaf.forward);
            files.push(&leaf.inverse);
        }
        if let Some(entry) = &group.entry {
            files.push(&entry.forward);
            files.push(&entry.inverse);
        }
        files.push(&group.pin.forward);
        files.push(&group.pin.inverse);
    }
    files
}

fn valid_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn validate_group_entry(
    entry: &GroupEntryEdit,
    forward: &str,
    inverse: &str,
) -> Result<(), Diagnostics> {
    validate_hash_pin(&entry.before_revision, "group entry before revision")?;
    validate_hash_pin(&entry.after_revision, "group entry after revision")?;
    if entry.forward.path != forward || entry.inverse.path != inverse {
        return Err(fail(
            DiagnosticCode::Syntax,
            "invalid group entry transaction paths",
        ));
    }
    Ok(())
}

fn check_tree(root: &ProjectRoot, manifest: &EditManifest) -> Result<(), Diagnostics> {
    let expected: BTreeSet<String> = std::iter::once(EDIT_MANIFEST.to_owned())
        .chain(
            edit_files(manifest)
                .into_iter()
                .map(|file| file.path.clone()),
        )
        .collect();
    let mut names = BTreeSet::new();
    for entry in root.dir().read_dir(".").map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot list edit directory: {e}"),
        )
    })? {
        let entry = entry.map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot list edit directory: {e}"),
            )
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| fail(DiagnosticCode::Reference, "non-UTF-8 edit member"))?;
        let kind = entry.file_type().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect edit member: {e}"),
            )
        })?;
        if !kind.is_file() || kind.is_symlink() || !expected.contains(&name) {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("unexpected edit member `{name}`"),
            ));
        }
        names.insert(name);
    }
    if names != expected {
        return Err(fail(
            DiagnosticCode::Reference,
            "edit directory is missing required files",
        ));
    }
    Ok(())
}

fn open_regular(root: &ProjectRoot, path: &str) -> Result<File, Diagnostics> {
    let meta = root.dir().symlink_metadata(path).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot inspect edit file `{path}`: {e}"),
        )
    })?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(fail(
            DiagnosticCode::Reference,
            format!("edit file `{path}` is not regular"),
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
        .open_with(path, &options)
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot open edit file `{path}`: {e}"),
            )
        })?
        .into_std();
    if !file
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect opened edit file: {e}"),
            )
        })?
        .is_file()
    {
        return Err(fail(
            DiagnosticCode::Reference,
            "opened edit file is not regular",
        ));
    }
    Ok(file)
}

fn read_bounded(root: &ProjectRoot, path: &str, limit: u64) -> Result<Vec<u8>, Diagnostics> {
    let file = open_regular(root, path)?;
    if file
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect edit file: {e}"),
            )
        })?
        .len()
        > limit
    {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "edit file exceeds byte limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot read edit file: {e}"),
        )
    })?;
    if bytes.len() as u64 > limit {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "edit file exceeds byte limit",
        ));
    }
    Ok(bytes)
}

fn edit_failure(error: EditError) -> Diagnostics {
    let code = match error.code.as_str() {
        "E_SYNTAX" => DiagnosticCode::Syntax,
        "E_DUPLICATE_ID" => DiagnosticCode::DuplicateId,
        "E_DUPLICATE_FIELD" => DiagnosticCode::DuplicateField,
        "E_UNKNOWN_FIELD" => DiagnosticCode::UnknownField,
        "E_UNKNOWN_KIND" => DiagnosticCode::UnknownKind,
        "E_REFERENCE" => DiagnosticCode::Reference,
        "E_UNIT" => DiagnosticCode::Unit,
        "E_RANGE" => DiagnosticCode::Range,
        "E_TEMPO" => DiagnosticCode::Tempo,
        "E_METER_BOUNDARY" => DiagnosticCode::MeterBoundary,
        "E_INTERVAL" => DiagnosticCode::Interval,
        "E_TIME_PRECISION" => DiagnosticCode::TimePrecision,
        "E_SUBSAMPLE_NOTE" => DiagnosticCode::SubsampleNote,
        "E_PATTERN_CYCLE" => DiagnosticCode::PatternCycle,
        "E_INSTANCE_TARGET" => DiagnosticCode::InstanceTarget,
        "E_AUTOMATION_WRITER" => DiagnosticCode::AutomationWriter,
        "E_CAPABILITY" => DiagnosticCode::Capability,
        "E_PORT_TYPE" => DiagnosticCode::PortType,
        "E_ALGEBRAIC_LOOP" => DiagnosticCode::AlgebraicLoop,
        "E_ASSET" => DiagnosticCode::Asset,
        "E_HASH" => DiagnosticCode::Hash,
        "E_VOICE_LIMIT" => DiagnosticCode::VoiceLimit,
        "E_RENDER_STATE" => DiagnosticCode::RenderState,
        "E_NONFINITE" => DiagnosticCode::Nonfinite,
        "E_CONFLICT" => DiagnosticCode::Conflict,
        "E_RESOURCE_LIMIT" => DiagnosticCode::ResourceLimit,
        "E_VERSION" => DiagnosticCode::Version,
        _ => DiagnosticCode::Syntax,
    };
    let mut diagnostics = Diagnostics::new();
    let mut diagnostic = Diagnostic::error(code, error.message, None);
    diagnostic.object_path = error.object_path;
    diagnostic.field_path = error.field_path;
    diagnostics.push(diagnostic);
    diagnostics
}

fn fail(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_history::ArchiveHistory;
    use crate::bundle::MAX_BUNDLE_FILE_BYTES;
    use crate::editing::Operation;

    const SOURCE: &str = include_str!("../tests/fixtures/archive_v1/main.maac");

    #[test]
    fn group_edit_replays_two_leaves_and_entry_with_exact_pins() {
        let temp = tempfile::tempdir().unwrap();
        let sounds = "maac 1;\nlibrary sounds { version=\"1\"; }\n";
        let drums = "maac 1;\nlibrary drums { version=\"1\"; }\n";
        let entry = format!(
            "{SOURCE}import drums {{ path=\"drums.maac\"; hash=\"{}\"; }}\nimport sounds {{ path=\"sounds.maac\"; hash=\"{}\"; }}\n",
            sha256_digest(drums.as_bytes()), sha256_digest(sounds.as_bytes()),
        );
        fs::write(temp.path().join("main.maac"), &entry).unwrap();
        fs::write(temp.path().join("drums.maac"), drums).unwrap();
        fs::write(temp.path().join("sounds.maac"), sounds).unwrap();
        let before =
            ArchiveSnapshot::capture(Path::new("main.maac"), temp.path(), "default").unwrap();
        let patch = |source: &str, alias: &str| {
            Transaction::new(
                SourceDocument::parse(source).unwrap().revision().to_owned(),
                vec![Operation::Set {
                    object: vec![alias.into()],
                    field: vec!["version".into()],
                    value: serde_json::json!({"t":"string","v":"2"}),
                    expect: Some(serde_json::json!({"t":"string","v":"1"})),
                    expect_absent: false,
                }],
            )
            .unwrap()
            .to_json()
            .unwrap()
        };
        let entry_patch = Transaction::new(
            SourceDocument::parse(&entry).unwrap().revision().to_owned(),
            vec![Operation::Set {
                object: vec!["p".into()],
                field: vec!["tail".into()],
                value: serde_json::json!({"t":"quantity","n":"1","d":"1","u":"s"}),
                expect: Some(serde_json::json!({"t":"quantity","n":"0","d":"1","u":"s"})),
                expect_absent: false,
            }],
        )
        .unwrap()
        .to_json()
        .unwrap();
        let parent = format!("sha256:{}", "1".repeat(64));
        let capture = EditSnapshot::capture_group(
            &parent,
            &before,
            &[
                ("drums".into(), patch(drums, "drums")),
                ("sounds".into(), patch(sounds, "sounds")),
            ],
            Some(&entry_patch),
        )
        .unwrap();
        assert_eq!(capture.leaves.len(), 2);
        assert!(capture.entry_applied.is_some());
        assert_eq!(capture.edit.tree_entries(), 10);
        capture
            .edit
            .verify_transition(&parent, &before, &capture.after)
            .unwrap();
        let edit_dir = temp.path().join("edit");
        fs::create_dir(&edit_dir).unwrap();
        capture.edit.stage_into(&edit_dir).unwrap();
        let root = ProjectRoot::open(&edit_dir, cap_std::ambient_authority()).unwrap();
        let preflight = EditSnapshot::preflight(
            &root,
            &capture.edit.digest(),
            &parent,
            &before.digest(),
            &capture.after.digest(),
        )
        .unwrap();
        assert!(preflight.is_group_edit());
        let verified = EditSnapshot::verify_preflighted(&root, &preflight).unwrap();
        verified
            .verify_transition(&parent, &before, &capture.after)
            .unwrap();
        fs::write(edit_dir.join(PIN_INVERSE), b"{}").unwrap();
        assert!(EditSnapshot::verify_preflighted(&root, &preflight).is_err());
    }

    #[test]
    fn group_edit_accepts_one_leaf_without_entry_and_rejects_duplicate_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let library = "maac 1;\nlibrary sounds { version=\"1\"; }\n";
        let entry = format!(
            "{SOURCE}import sounds {{ path=\"sounds.maac\"; hash=\"{}\"; }}\n",
            sha256_digest(library.as_bytes()),
        );
        fs::write(temp.path().join("main.maac"), entry).unwrap();
        fs::write(temp.path().join("sounds.maac"), library).unwrap();
        let before =
            ArchiveSnapshot::capture(Path::new("main.maac"), temp.path(), "default").unwrap();
        let patch = Transaction::new(
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
        let parent = format!("sha256:{}", "1".repeat(64));
        let inputs = [("sounds".into(), patch.clone())];
        let captured = EditSnapshot::capture_group(&parent, &before, &inputs, None).unwrap();
        assert!(captured.entry_applied.is_none());
        assert_eq!(captured.edit.tree_entries(), 6);
        captured
            .edit
            .verify_transition(&parent, &before, &captured.after)
            .unwrap();
        let duplicates = [("sounds".into(), patch.clone()), ("sounds".into(), patch)];
        assert!(EditSnapshot::capture_group(&parent, &before, &duplicates, None).is_err());
    }

    #[test]
    fn group_leaf_inverse_uses_original_bundle_when_partial_reverse_exceeds_limit() {
        let temp = tempfile::tempdir().unwrap();
        let instrument = |id: &str| {
            format!(
            "instrument {id} {{ channels=1; voice v {{ channels=1; amplitude=&amp; output=&osc:out; node amp {{ type=\"synth.adsr/1\"; }} node osc {{ type=\"synth.sine/1\"; }} }} }}\n"
        )
        };
        let library = |alias: &str, count: usize| {
            let mut source = format!("maac 1;\nlibrary {alias} {{ version=\"1\"; }}\n");
            for index in 0..count {
                source.push_str(&instrument(&format!("{alias}_{index}")));
            }
            source
        };
        let alpha = library("alpha", 42);
        let beta = library("beta", 43);
        let gamma = library("gamma", 42);
        let entry = format!(
            "{SOURCE}import alpha {{ path=\"alpha.maac\"; hash=\"{}\"; }}\nimport beta {{ path=\"beta.maac\"; hash=\"{}\"; }}\nimport gamma {{ path=\"gamma.maac\"; hash=\"{}\"; }}\n",
            sha256_digest(alpha.as_bytes()), sha256_digest(beta.as_bytes()),
            sha256_digest(gamma.as_bytes()),
        );
        fs::write(temp.path().join("main.maac"), entry).unwrap();
        for (alias, source) in [("alpha", &alpha), ("beta", &beta), ("gamma", &gamma)] {
            fs::write(temp.path().join(format!("{alias}.maac")), source).unwrap();
        }
        let before =
            ArchiveSnapshot::capture(Path::new("main.maac"), temp.path(), "default").unwrap();
        let inserted = SourceDocument::parse(format!("maac 1;\n{}", instrument("candidate")))
            .unwrap()
            .authored()
            .tree()["objects"]["candidate"]
            .clone();
        let patch = |source: &str, operation: Operation| {
            Transaction::new(
                SourceDocument::parse(source).unwrap().revision().to_owned(),
                vec![operation],
            )
            .unwrap()
            .to_json()
            .unwrap()
        };
        let inputs = [
            (
                "alpha".into(),
                patch(
                    &alpha,
                    Operation::InsertObject {
                        parent: vec![],
                        id: "alpha_added".into(),
                        object_value: inserted.clone(),
                    },
                ),
            ),
            (
                "beta".into(),
                patch(
                    &beta,
                    Operation::DeleteObject {
                        object: vec!["beta_42".into()],
                        expect_object: None,
                    },
                ),
            ),
            (
                "gamma".into(),
                patch(
                    &gamma,
                    Operation::InsertObject {
                        parent: vec![],
                        id: "gamma_added".into(),
                        object_value: inserted,
                    },
                ),
            ),
        ];
        let parent = format!("sha256:{}", "1".repeat(64));
        let captured = EditSnapshot::capture_group(&parent, &before, &inputs, None).unwrap();
        let group = captured.edit.manifest.group.as_ref().unwrap();
        let beta_inverse = Transaction::from_json(
            captured
                .edit
                .group_payloads
                .get(&group.others[0].inverse.path)
                .unwrap(),
        )
        .unwrap();
        let trusted_child = trusted_import(&captured.after, "beta").unwrap();
        let final_context = BundleEditContext::new_disk_media_import_library(
            &trusted_child.bundle,
            &trusted_child.assets,
            limits(captured.after.profile()).unwrap(),
            "beta.maac",
            captured.after.entry(),
            "beta",
        )
        .unwrap();
        let mut partial_reverse =
            SourceDocument::parse(captured.after.source_text("beta.maac").unwrap())
                .unwrap()
                .authored()
                .clone();
        let partial_error =
            apply_transaction(&mut partial_reverse, &beta_inverse, &final_context).unwrap_err();
        assert_eq!(partial_error.code, "E_RESOURCE_LIMIT");
        captured
            .edit
            .verify_transition(&parent, &before, &captured.after)
            .unwrap();
    }

    #[test]
    fn equivalent_inverse_encoding_is_verified_by_behavior() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("main.maac"), SOURCE).unwrap();
        let before =
            ArchiveSnapshot::capture(Path::new("main.maac"), temp.path(), "default").unwrap();
        let revision = SourceDocument::parse(SOURCE).unwrap().revision().to_owned();
        let no_op = || {
            vec![Operation::Set {
                object: vec!["p".into()],
                field: vec!["tail".into()],
                value: serde_json::json!({"t":"quantity","n":"0","d":"1","u":"s"}),
                expect: None,
                expect_absent: false,
            }]
        };
        let patch = Transaction::new(revision.clone(), no_op())
            .unwrap()
            .to_json()
            .unwrap();
        let parent = format!("sha256:{}", "1".repeat(64));
        let (after, mut edit, _) = EditSnapshot::capture(&parent, &before, &patch).unwrap();
        edit.inverse = Transaction::new(revision, no_op())
            .unwrap()
            .to_json()
            .unwrap();
        edit.manifest.inverse.bytes = edit.inverse.len() as u64;
        edit.manifest.inverse.sha256 = sha256_digest(&edit.inverse);
        edit.manifest.inverse.operations = no_op().len();
        edit.json = serde_json::to_vec(&edit.manifest).unwrap();
        edit.verify_transition(&parent, &before, &after).unwrap();
        let mut excessive = edit.manifest.clone();
        excessive.forward.operations = MAX_OPERATIONS;
        assert!(validate_manifest(&excessive).is_err());
    }

    #[test]
    fn comment_padded_source_publishes_and_verifies_noop_edit() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let prefix = format!("{SOURCE}//");
        let source = format!(
            "{prefix}{}\n",
            "x".repeat(MAX_BUNDLE_FILE_BYTES - prefix.len() - 1)
        );
        assert_eq!(source.len(), MAX_BUNDLE_FILE_BYTES);
        fs::write(project.join("main.maac"), &source).unwrap();
        let mut history =
            ArchiveHistory::capture(Path::new("main.maac"), &project, "default").unwrap();
        let revision = SourceDocument::parse(source).unwrap().revision().to_owned();
        let patch = Transaction::new(
            revision,
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
        history.patch_head(&patch).unwrap();
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        assert_eq!(
            ArchiveHistory::verify(&archive).unwrap().digest(),
            history.digest()
        );
    }

    #[test]
    fn bad_import_hash_keeps_hash_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let library = "maac 1;\nlibrary sounds { version=\"1\"; }\n";
        let pin = sha256_digest(library.as_bytes());
        let source = format!("{SOURCE}import sounds {{ path=\"sounds.maac\"; hash=\"{pin}\"; }}\n");
        fs::write(temp.path().join("main.maac"), &source).unwrap();
        fs::write(temp.path().join("sounds.maac"), library).unwrap();
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), temp.path(), "default").unwrap();
        let revision = SourceDocument::parse(source).unwrap().revision().to_owned();
        let patch = Transaction::new(
            revision,
            vec![Operation::Set {
                object: vec!["sounds".into()],
                field: vec!["hash".into()],
                value: serde_json::json!({"t":"string","v":format!("sha256:{}", "0".repeat(64))}),
                expect: None,
                expect_absent: false,
            }],
        )
        .unwrap()
        .to_json()
        .unwrap();
        let parent = format!("sha256:{}", "1".repeat(64));
        let error = EditSnapshot::capture(&parent, &snapshot, &patch)
            .err()
            .expect("bad import hash must be rejected");
        assert_eq!(error.first().unwrap().code, DiagnosticCode::Hash);
    }

    #[test]
    fn library_patch_updates_exact_pin_and_verifies_both_source_replays() {
        let temp = tempfile::tempdir().unwrap();
        let library = "maac 1;\nlibrary sounds { version=\"1\"; }\n";
        let pin = sha256_digest(library.as_bytes());
        let entry = format!("{SOURCE}import sounds {{ path=\"sounds.maac\"; hash=\"{pin}\"; }}\n");
        fs::write(temp.path().join("main.maac"), &entry).unwrap();
        fs::write(temp.path().join("sounds.maac"), library).unwrap();
        let mut history =
            ArchiveHistory::capture(Path::new("main.maac"), temp.path(), "default").unwrap();
        let revision = SourceDocument::parse(library)
            .unwrap()
            .revision()
            .to_owned();
        let patch = Transaction::new(
            revision,
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
        let result = history.patch_import_head("sounds", &patch).unwrap();
        assert_eq!(history.version(), 7);
        assert_eq!(
            result.pin,
            sha256_digest(
                history
                    .select(None)
                    .unwrap()
                    .source_text("sounds.maac")
                    .unwrap()
                    .as_bytes()
            )
        );
        let archive = temp.path().join("archive");
        assert!(history
            .append(
                ArchiveSnapshot::capture(Path::new("main.maac"), temp.path(), "default").unwrap(),
            )
            .unwrap());
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        assert_eq!(
            ArchiveHistory::verify(&archive).unwrap().digest(),
            history.digest()
        );
        fs::write(
            archive
                .join("edits")
                .join(&result.edit_digest[7..])
                .join(IMPORTER_INVERSE),
            b"{}",
        )
        .unwrap();
        assert!(ArchiveHistory::verify(&archive).is_err());
    }

    #[test]
    fn unicode_comment_padded_library_keeps_exact_pin_and_authored_inverse() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = "maac 1;\nlibrary sounds { version=\"1\"; }\n// 한글🎼 ";
        let library = format!(
            "{prefix}{}\n",
            "x".repeat(MAX_BUNDLE_FILE_BYTES - prefix.len() - 1)
        );
        assert_eq!(library.len(), MAX_BUNDLE_FILE_BYTES);
        let pin = sha256_digest(library.as_bytes());
        let entry = format!("{SOURCE}import sounds {{ path=\"sounds.maac\"; hash=\"{pin}\"; }}\n");
        fs::write(temp.path().join("main.maac"), entry).unwrap();
        fs::write(temp.path().join("sounds.maac"), &library).unwrap();
        let before =
            ArchiveSnapshot::capture(Path::new("main.maac"), temp.path(), "default").unwrap();
        let patch = Transaction::new(
            SourceDocument::parse(&library)
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
        let parent = format!("sha256:{}", "1".repeat(64));
        let captured = EditSnapshot::capture_import(&parent, &before, "sounds", &patch).unwrap();
        let edited = captured.after.source_text("sounds.maac").unwrap();
        assert_eq!(edited.len(), MAX_BUNDLE_FILE_BYTES);
        assert!(edited.contains("한글🎼"));
        assert_eq!(captured.pin, sha256_digest(edited.as_bytes()));
        captured
            .edit
            .verify_transition(&parent, &before, &captured.after)
            .unwrap();
    }

    #[test]
    fn patch_import_rejects_new_edge_to_existing_closure_library() {
        let temp = tempfile::tempdir().unwrap();
        let sounds = "maac 1;\nlibrary sounds { version=\"1\"; }\n";
        let other = "maac 1;\nlibrary other { version=\"1\"; }\n";
        let sounds_pin = sha256_digest(sounds.as_bytes());
        let other_pin = sha256_digest(other.as_bytes());
        let entry = format!(
            "{SOURCE}import sounds {{ path=\"sounds.maac\"; hash=\"{sounds_pin}\"; }}\nimport other {{ path=\"other.maac\"; hash=\"{other_pin}\"; }}\n"
        );
        fs::write(temp.path().join("main.maac"), entry).unwrap();
        fs::write(temp.path().join("sounds.maac"), sounds).unwrap();
        fs::write(temp.path().join("other.maac"), other).unwrap();
        let mut history =
            ArchiveHistory::capture(Path::new("main.maac"), temp.path(), "default").unwrap();
        let original_digest = history.digest();
        let patch = Transaction::new(
            SourceDocument::parse(sounds).unwrap().revision().to_owned(),
            vec![Operation::InsertObject {
                parent: vec![],
                id: "nested".into(),
                object_value: serde_json::json!({
                    "kind":"import",
                    "fields":{
                        "path":{"t":"string","v":"other.maac"},
                        "hash":{"t":"string","v":other_pin}
                    },
                    "children":{}
                }),
            }],
        )
        .unwrap()
        .to_json()
        .unwrap();
        let error = history.patch_import_head("sounds", &patch).err().unwrap();
        assert_eq!(error.first().unwrap().code, DiagnosticCode::Capability);
        assert_eq!(history.digest(), original_digest);
        assert_eq!(history.version(), 2);
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        history.stage(&archive).unwrap();
        assert_eq!(
            ArchiveHistory::verify(&archive).unwrap().digest(),
            original_digest
        );
    }
}
