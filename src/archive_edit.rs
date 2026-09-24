//! Immutable Protocol 2 edit evidence for archive checkpoint transitions.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use cap_std::fs::OpenOptions as CapOpenOptions;
use serde::{Deserialize, Serialize};

use crate::archive::ArchiveSnapshot;
use crate::bundle::{sha256_digest, validate_hash_pin};
use crate::bundle_fs::ProjectRoot;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::disk_media::DiskMediaProject;
use crate::editing::{
    apply_transaction, AppliedTransaction, BundleEditContext, EditError, SourceDocument,
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
        &self.manifest.source
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.json.len() as u64 + self.manifest.forward.bytes + self.manifest.inverse.bytes
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
        check_tree(root)?;
        let json = read_bounded(root, EDIT_MANIFEST, MAX_EDIT_MANIFEST_BYTES)?;
        let manifest: EditManifest = serde_json::from_slice(&json).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("invalid edit manifest: {e}"),
            )
        })?;
        validate_manifest(&manifest)?;
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
        for file in [&manifest.forward, &manifest.inverse] {
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
        check_tree(root)?;
        let forward = read_bounded(root, EDIT_FORWARD, MAX_TRANSACTION_BYTES as u64)?;
        let inverse = read_bounded(root, EDIT_INVERSE, MAX_TRANSACTION_BYTES as u64)?;
        for (file, bytes) in [
            (&preflight.manifest.forward, &forward),
            (&preflight.manifest.inverse, &inverse),
        ] {
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
        })
    }

    pub(crate) fn verify_transition(
        &self,
        parent: &str,
        before: &ArchiveSnapshot,
        after: &ArchiveSnapshot,
    ) -> Result<(), Diagnostics> {
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

    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.json)
    }
    pub(crate) fn total_bytes(&self) -> u64 {
        self.json.len() as u64 + self.forward.len() as u64 + self.inverse.len() as u64
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
        for (name, bytes) in [
            (EDIT_MANIFEST, &self.json),
            (EDIT_FORWARD, &self.forward),
            (EDIT_INVERSE, &self.inverse),
        ] {
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
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "private edit evidence changed before staging",
            ));
        }
        Ok(())
    }
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
    if m.format != FORMAT || m.version != VERSION {
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
    for (file, name) in [(&m.forward, EDIT_FORWARD), (&m.inverse, EDIT_INVERSE)] {
        if file.path != name
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
    transaction_work(m)?;
    Ok(())
}

fn transaction_work(m: &EditManifest) -> Result<u64, Diagnostics> {
    let mut total = 0u64;
    for file in [&m.forward, &m.inverse] {
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

fn check_tree(root: &ProjectRoot) -> Result<(), Diagnostics> {
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
        if !kind.is_file()
            || kind.is_symlink()
            || !matches!(name.as_str(), EDIT_MANIFEST | EDIT_FORWARD | EDIT_INVERSE)
        {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("unexpected edit member `{name}`"),
            ));
        }
        names.insert(name);
    }
    if names
        != BTreeSet::from([
            EDIT_MANIFEST.into(),
            EDIT_FORWARD.into(),
            EDIT_INVERSE.into(),
        ])
    {
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
}
