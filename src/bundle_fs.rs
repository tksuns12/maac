//! Bounded filesystem loading for [`SourceBundle`](crate::bundle::SourceBundle).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use cap_std::fs::{Dir, OpenOptions};
use cap_std::{ambient_authority, AmbientAuthority};

use crate::bundle::{
    count_document_objects, discover_document_references, normalize_file_reference, sha256_digest,
    validate_hash_pin, SourceBundle, MAX_BUNDLE_ASSETS, MAX_BUNDLE_ASSET_BYTES,
    MAX_BUNDLE_FILE_BYTES, MAX_BUNDLE_SOURCES, MAX_BUNDLE_SOURCE_BYTES, MAX_IMPORT_DEPTH,
    MAX_SYNTAX_OBJECTS,
};
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};

/// Load an entry source and only its explicitly declared local dependencies.
///
/// `entry_path` may be absolute or relative to `project_root`. The loader pins
/// the caller-selected project root as its single explicit ambient-authority
/// directory handle. It resolves authored in-root symlinks, opens each target
/// through that directory capability, verifies the opened handle is a regular
/// file, and performs the bounded read from that same handle.
pub fn load_bundle(entry_path: &Path, project_root: &Path) -> Result<SourceBundle, Diagnostics> {
    let root = ProjectRoot::open(project_root, ambient_authority())?;

    let entry_candidate = if entry_path.is_absolute() {
        entry_path.to_owned()
    } else {
        root.canonical.join(entry_path)
    };
    let entry_canonical =
        canonicalize(&entry_candidate, "entry source", DiagnosticCode::Reference)?;
    ensure_contained(
        &root.canonical,
        &entry_canonical,
        "entry source",
        DiagnosticCode::Reference,
    )?;
    let entry = logical_entry_path(&root.canonical, &entry_candidate, &entry_canonical)?;

    let mut sources = BTreeMap::new();
    let mut assets = BTreeMap::new();
    let mut source_bytes = 0usize;
    let mut asset_bytes = 0usize;
    let mut syntax_objects = 0usize;
    let mut pending = VecDeque::from([(entry.clone(), 0usize)]);
    let mut queued = BTreeSet::from([entry.clone()]);
    let mut expected_source_hashes: BTreeMap<String, Vec<(String, String, String)>> =
        BTreeMap::new();

    while let Some((logical_path, depth)) = pending.pop_front() {
        if sources.len() >= MAX_BUNDLE_SOURCES {
            return Err(resource_error(format!(
                "bundle contains more than {MAX_BUNDLE_SOURCES} source files"
            )));
        }
        let file = contained_file(&root, &logical_path, "source", DiagnosticCode::Reference)?;
        let bytes = read_bounded(file, &logical_path, "source")?;
        source_bytes = checked_total(source_bytes, bytes.len(), MAX_BUNDLE_SOURCE_BYTES, "source")?;
        if let Some(expectations) = expected_source_hashes.get(&logical_path) {
            for (declaring_source, alias, expected) in expectations {
                verify_source_hash(&logical_path, &bytes, declaring_source, alias, expected)?;
            }
        }
        let source = String::from_utf8(bytes).map_err(|invalid| {
            error(
                DiagnosticCode::Syntax,
                format!(
                    "source `{logical_path}` is not UTF-8 at byte {}",
                    invalid.utf8_error().valid_up_to()
                ),
            )
        })?;
        let document = crate::syntax::parse(&source).map_err(|diagnostics| {
            contextualize(diagnostics, &format!("source `{logical_path}`"))
        })?;
        syntax_objects = syntax_objects
            .checked_add(count_document_objects(&document))
            .ok_or_else(|| resource_error("aggregate syntax object count overflowed"))?;
        if syntax_objects > MAX_SYNTAX_OBJECTS {
            return Err(resource_error(format!(
                "bundle contains more than {MAX_SYNTAX_OBJECTS} syntax objects"
            )));
        }
        let references = discover_document_references(&logical_path, &document)?;
        sources.insert(logical_path.clone(), source);

        for import in references.imports {
            validate_hash_pin(&import.hash, "import hash")?;
            let target = normalize_file_reference(&logical_path, &import.path)?;
            if let Some(loaded) = sources.get(&target) {
                verify_source_hash(
                    &target,
                    loaded.as_bytes(),
                    &logical_path,
                    &import.alias,
                    &import.hash,
                )?;
            }
            expected_source_hashes
                .entry(target.clone())
                .or_default()
                .push((logical_path.clone(), import.alias, import.hash));
            if queued.insert(target.clone()) {
                let target_depth = depth + 1;
                if target_depth > MAX_IMPORT_DEPTH {
                    return Err(resource_error(format!(
                        "import depth exceeds {MAX_IMPORT_DEPTH} at `{target}`"
                    )));
                }
                if queued.len() > MAX_BUNDLE_SOURCES {
                    return Err(resource_error(format!(
                        "bundle contains more than {MAX_BUNDLE_SOURCES} source files"
                    )));
                }
                pending.push_back((target, target_depth));
            }
        }

        for asset in references.assets {
            validate_hash_pin(&asset.hash, "asset hash")?;
            let target = normalize_file_reference(&logical_path, &asset.path)?;
            if assets.contains_key(&target) {
                continue;
            }
            if assets.len() >= MAX_BUNDLE_ASSETS {
                return Err(resource_error(format!(
                    "bundle contains more than {MAX_BUNDLE_ASSETS} assets"
                )));
            }
            let file = contained_file(&root, &target, "asset", DiagnosticCode::Asset)?;
            let bytes = read_bounded(file, &target, "asset")?;
            asset_bytes = checked_total(asset_bytes, bytes.len(), MAX_BUNDLE_ASSET_BYTES, "asset")?;
            let actual = sha256_digest(&bytes);
            if actual != asset.hash {
                return Err(error(
                    DiagnosticCode::Hash,
                    format!(
                        "wavetable `{}` in `{logical_path}` expected `{}`, but `{target}` has `{actual}`",
                        asset.alias, asset.hash
                    ),
                ));
            }
            assets.insert(target, bytes);
        }
    }

    let bundle = SourceBundle {
        entry,
        sources,
        assets,
    };
    // Reuse the in-memory boundary for hash pins, cycles, exact import schema,
    // aggregate syntax limits, and all path invariants.
    bundle.resolve()?;
    Ok(bundle)
}

fn verify_source_hash(
    target: &str,
    bytes: &[u8],
    declaring_source: &str,
    alias: &str,
    expected: &str,
) -> Result<(), Diagnostics> {
    let actual = sha256_digest(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(error(
            DiagnosticCode::Hash,
            format!(
                "import `{alias}` in `{declaring_source}` expected `{expected}`, but `{target}` has `{actual}`"
            ),
        ))
    }
}

fn logical_entry_path(
    root: &Path,
    candidate: &Path,
    canonical: &Path,
) -> Result<String, Diagnostics> {
    // Preserve an in-root symlink's project-relative spelling. For paths whose
    // lexical form is outside the root, containment was already checked using
    // the canonical target, so use that target's relative spelling.
    let relative = candidate
        .strip_prefix(root)
        .ok()
        .or_else(|| canonical.strip_prefix(root).ok())
        .ok_or_else(|| {
            error(
                DiagnosticCode::Reference,
                format!(
                    "entry source `{}` is outside the project root",
                    candidate.display()
                ),
            )
        })?;
    os_relative_to_posix(relative, "entry source")
}

fn os_relative_to_posix(path: &Path, label: &str) -> Result<String, Diagnostics> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_str().ok_or_else(|| {
                    error(
                        DiagnosticCode::Reference,
                        format!("{label} path is not valid UTF-8"),
                    )
                })?;
                if value.contains('\\') || value.contains('/') || value.is_empty() {
                    return Err(error(
                        DiagnosticCode::Reference,
                        format!("{label} path must use project-relative POSIX components"),
                    ));
                }
                parts.push(value);
            }
            Component::CurDir => {}
            _ => {
                return Err(error(
                    DiagnosticCode::Reference,
                    format!("{label} path must be normalized beneath the project root"),
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(error(
            DiagnosticCode::Reference,
            format!("{label} path must name a file"),
        ));
    }
    Ok(parts.join("/"))
}

struct ProjectRoot {
    canonical: PathBuf,
    dir: Dir,
}

impl ProjectRoot {
    fn open(path: &Path, authority: AmbientAuthority) -> Result<Self, Diagnostics> {
        let canonical = canonicalize(path, "project root", DiagnosticCode::Reference)?;
        let dir = Dir::open_ambient_dir(&canonical, authority).map_err(|failure| {
            error(
                DiagnosticCode::Reference,
                format!(
                    "cannot open project root `{}` as a directory: {failure}",
                    path.display()
                ),
            )
        })?;
        Ok(Self { canonical, dir })
    }
}

fn contained_file(
    root: &ProjectRoot,
    logical_path: &str,
    label: &str,
    code: DiagnosticCode,
) -> Result<File, Diagnostics> {
    contained_file_with(root, logical_path, label, code, || {})
}

fn contained_file_with(
    root: &ProjectRoot,
    logical_path: &str,
    label: &str,
    code: DiagnosticCode,
    before_open: impl FnOnce(),
) -> Result<File, Diagnostics> {
    let candidate = logical_path
        .split('/')
        .fold(root.canonical.clone(), |path, component| {
            path.join(component)
        });
    let canonical = canonicalize(&candidate, &format!("{label} `{logical_path}`"), code)?;
    ensure_contained(
        &root.canonical,
        &canonical,
        &format!("{label} `{logical_path}`"),
        code,
    )?;
    let relative = canonical.strip_prefix(&root.canonical).map_err(|_| {
        error(
            code,
            format!("{label} `{logical_path}` resolves outside the project root"),
        )
    })?;

    before_open();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = root.dir.open_with(relative, &options).map_err(|failure| {
        error(
            code,
            format!("cannot open {label} `{logical_path}`: {failure}"),
        )
    })?;
    let metadata = file.metadata().map_err(|failure| {
        error(
            code,
            format!("cannot inspect {label} `{logical_path}`: {failure}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(error(
            code,
            format!("{label} `{logical_path}` is not a regular file"),
        ));
    }
    Ok(file.into_std())
}

fn canonicalize(path: &Path, label: &str, code: DiagnosticCode) -> Result<PathBuf, Diagnostics> {
    std::fs::canonicalize(path).map_err(|failure| {
        error(
            code,
            format!("cannot resolve {label} `{}`: {failure}", path.display()),
        )
    })
}

fn ensure_contained(
    root: &Path,
    target: &Path,
    label: &str,
    code: DiagnosticCode,
) -> Result<(), Diagnostics> {
    if target.starts_with(root) {
        Ok(())
    } else {
        Err(error(
            code,
            format!("{label} resolves outside project root `{}`", root.display()),
        ))
    }
}

fn read_bounded(file: File, logical_path: &str, label: &str) -> Result<Vec<u8>, Diagnostics> {
    let mut bytes = Vec::new();
    file.take(MAX_BUNDLE_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|failure| {
            error(
                if label == "asset" {
                    DiagnosticCode::Asset
                } else {
                    DiagnosticCode::Reference
                },
                format!("cannot read {label} `{logical_path}`: {failure}"),
            )
        })?;
    if bytes.len() > MAX_BUNDLE_FILE_BYTES {
        return Err(resource_error(format!(
            "{label} `{logical_path}` exceeds {MAX_BUNDLE_FILE_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

fn checked_total(
    current: usize,
    added: usize,
    limit: usize,
    label: &str,
) -> Result<usize, Diagnostics> {
    let total = current
        .checked_add(added)
        .ok_or_else(|| resource_error(format!("aggregate {label} byte count overflowed")))?;
    if total > limit {
        Err(resource_error(format!(
            "bundle {label} bytes exceed {limit}"
        )))
    } else {
        Ok(total)
    }
}

fn contextualize(diagnostics: Diagnostics, context: &str) -> Diagnostics {
    let mut result = Diagnostics::new();
    for mut diagnostic in diagnostics {
        diagnostic.message = format!("{context}: {}", diagnostic.message);
        result.push(diagnostic);
    }
    result
}

fn error(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}

fn resource_error(message: impl Into<String>) -> Diagnostics {
    error(DiagnosticCode::ResourceLimit, message)
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::CString;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn a_validated_file_is_not_reopened_through_replaced_ancestors() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let outside = directory.path().join("outside");
        fs::create_dir_all(project.join("nested")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(project.join("nested/data.maac"), b"inside").unwrap();
        fs::write(outside.join("data.maac"), b"outside").unwrap();

        let root = ProjectRoot::open(&project, ambient_authority()).unwrap();
        let opened = contained_file(
            &root,
            "nested/data.maac",
            "source",
            DiagnosticCode::Reference,
        )
        .unwrap();
        fs::rename(project.join("nested"), project.join("nested-original")).unwrap();
        symlink(&outside, project.join("nested")).unwrap();

        let bytes = read_bounded(opened, "nested/data.maac", "source").unwrap();
        assert_eq!(bytes, b"inside");
    }

    #[test]
    fn capability_open_rejects_ancestor_replacement_after_resolution() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let outside = directory.path().join("outside");
        fs::create_dir_all(project.join("nested")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(project.join("nested/data.maac"), b"inside").unwrap();
        fs::write(outside.join("data.maac"), b"outside").unwrap();
        let root = ProjectRoot::open(&project, ambient_authority()).unwrap();

        let diagnostics = contained_file_with(
            &root,
            "nested/data.maac",
            "source",
            DiagnosticCode::Reference,
            || {
                fs::rename(project.join("nested"), project.join("nested-original")).unwrap();
                symlink(&outside, project.join("nested")).unwrap();
            },
        )
        .unwrap_err();
        assert_eq!(diagnostics.first().unwrap().code, DiagnosticCode::Reference);
    }

    #[test]
    fn capability_open_rejects_final_replacement_after_resolution() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let outside = directory.path().join("outside.maac");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("data.maac"), b"inside").unwrap();
        fs::write(&outside, b"outside").unwrap();
        let root = ProjectRoot::open(&project, ambient_authority()).unwrap();

        let diagnostics = contained_file_with(
            &root,
            "data.maac",
            "source",
            DiagnosticCode::Reference,
            || {
                fs::rename(
                    project.join("data.maac"),
                    project.join("data-original.maac"),
                )
                .unwrap();
                symlink(&outside, project.join("data.maac")).unwrap();
            },
        )
        .unwrap_err();
        assert_eq!(diagnostics.first().unwrap().code, DiagnosticCode::Reference);
    }

    #[test]
    fn capability_open_rejects_a_fifo_without_waiting_for_a_writer() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir(&project).unwrap();
        let fifo = project.join("stream.maac");
        let fifo = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo` is a live, NUL-terminated pathname and the mode is a
        // valid permission bitmask. The return value is checked immediately.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let root = ProjectRoot::open(&project, ambient_authority()).unwrap();

        let diagnostics =
            contained_file(&root, "stream.maac", "source", DiagnosticCode::Reference).unwrap_err();
        let diagnostic = diagnostics.first().unwrap();
        assert_eq!(diagnostic.code, DiagnosticCode::Reference);
        assert!(diagnostic.message.contains("not a regular file"));
    }
}
