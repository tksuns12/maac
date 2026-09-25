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
const IMPORTER_FORWARD: &str = "importer-forward.json";
const IMPORTER_INVERSE: &str = "importer-inverse.json";
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
        self.manifest
            .importer
            .as_ref()
            .map_or(&self.manifest.source, |importer| &importer.source)
    }

    pub(crate) fn is_import_edit(&self) -> bool {
        self.manifest.importer.is_some()
    }

    pub(crate) fn tree_entries(&self) -> usize {
        if self.is_import_edit() {
            6
        } else {
            4
        }
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.json.len() as u64
            + self.manifest.forward.bytes
            + self.manifest.inverse.bytes
            + self.manifest.importer.as_ref().map_or(0, |importer| {
                importer.forward.bytes + importer.inverse.bytes
            })
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

impl EditSnapshot {
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
        })
    }

    pub(crate) fn verify_transition(
        &self,
        parent: &str,
        before: &ArchiveSnapshot,
        after: &ArchiveSnapshot,
    ) -> Result<(), Diagnostics> {
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

    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.json)
    }
    pub(crate) fn total_bytes(&self) -> u64 {
        self.json.len() as u64
            + self.forward.len() as u64
            + self.inverse.len() as u64
            + self.importer_forward.as_ref().map_or(0, Vec::len) as u64
            + self.importer_inverse.as_ref().map_or(0, Vec::len) as u64
    }
    pub(crate) fn is_import_edit(&self) -> bool {
        self.manifest.importer.is_some()
    }
    pub(crate) fn tree_entries(&self) -> usize {
        if self.is_import_edit() {
            6
        } else {
            4
        }
    }
    pub(crate) fn transaction_work(&self) -> Result<u64, Diagnostics> {
        transaction_work(&self.manifest)
    }

    pub(crate) fn stage_into(&self, dir: &Path) -> Result<(), Diagnostics> {
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
            (m.version, m.importer.is_some()),
            (VERSION, false) | (IMPORT_VERSION, true)
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
    for file in edit_files(m) {
        let name = file.path.as_str();
        if !matches!(
            name,
            EDIT_FORWARD | EDIT_INVERSE | IMPORTER_FORWARD | IMPORTER_INVERSE
        ) || file.bytes == 0
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
    transaction_work(m)?;
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
    files
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
