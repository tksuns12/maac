//! Bounded filesystem loading for [`SourceBundle`](crate::bundle::SourceBundle).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use cap_std::fs::{Dir, OpenOptions};
use cap_std::{ambient_authority, AmbientAuthority};

use crate::bundle::{
    count_document_objects, discover_document_references, normalize_file_reference, sha256_digest,
    validate_hash_pin, ImportReference, SourceBundle, MAX_BUNDLE_ASSETS, MAX_BUNDLE_ASSET_BYTES,
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
    load_bundle_in_root(entry_path, &root)
}

/// Load through an already pinned project directory, including the entry.
pub(crate) fn load_bundle_in_root(
    entry_path: &Path,
    root: &ProjectRoot,
) -> Result<SourceBundle, Diagnostics> {
    load_bundle_in_root_mode(entry_path, root, false).map(|(bundle, _)| bundle)
}

pub(crate) fn load_disk_media_bundle_in_root(
    entry_path: &Path,
    root: &ProjectRoot,
) -> Result<
    (
        SourceBundle,
        BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>>,
    ),
    Diagnostics,
> {
    let relative = if entry_path.is_absolute() {
        entry_path
            .strip_prefix(&root.selected)
            .or_else(|_| entry_path.strip_prefix(&root.canonical))
            .map_err(|_| {
                error(
                    DiagnosticCode::Reference,
                    "entry source is outside the project root",
                )
            })?
    } else {
        entry_path
    };
    let resolved =
        resolve_contained_path(root, relative, "entry source", DiagnosticCode::Reference)?;
    load_bundle_in_root_mode(&resolved, root, true)
}

fn load_bundle_in_root_mode(
    entry_path: &Path,
    root: &ProjectRoot,
    disk_media: bool,
) -> Result<
    (
        SourceBundle,
        BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>>,
    ),
    Diagnostics,
> {
    let entry = if entry_path.is_absolute() {
        let entry_candidate = entry_path.to_owned();
        let entry_canonical =
            canonicalize(&entry_candidate, "entry source", DiagnosticCode::Reference)?;
        ensure_contained(
            &root.canonical,
            &entry_canonical,
            "entry source",
            DiagnosticCode::Reference,
        )?;
        logical_entry_path(&root.canonical, &entry_candidate, &entry_canonical)?
    } else {
        let entry = os_relative_to_posix(entry_path, "entry source")?;
        if root.pinned_resolution {
            resolve_contained_path(root, entry_path, "entry source", DiagnosticCode::Reference)?;
        } else {
            let candidate = root.canonical.join(entry_path);
            let canonical = canonicalize(&candidate, "entry source", DiagnosticCode::Reference)?;
            ensure_contained(
                &root.canonical,
                &canonical,
                "entry source",
                DiagnosticCode::Reference,
            )?;
        }
        entry
    };

    let mut sources = BTreeMap::new();
    let mut assets: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut disk_assets: BTreeMap<String, std::sync::Arc<crate::disk_media::DiskAsset>> =
        BTreeMap::new();
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
        let file = contained_file(root, &logical_path, "source", DiagnosticCode::Reference)?;
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
            let ImportReference::Local(import) = import else {
                continue;
            };
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
            let target = asset.target_path(&logical_path)?;
            if let Some(snapshot) = disk_assets.get(&target) {
                if snapshot.hash != asset.hash {
                    return Err(error(
                        DiagnosticCode::Hash,
                        format!(
                            "asset `{}` in `{logical_path}` expected `{}`, but `{target}` has `{}`",
                            asset.alias, asset.hash, snapshot.hash
                        ),
                    ));
                }
                continue;
            }
            if let Some(bytes) = assets.get(&target) {
                let actual = sha256_digest(bytes);
                if actual != asset.hash {
                    return Err(error(
                        DiagnosticCode::Hash,
                        format!(
                            "asset `{}` in `{logical_path}` expected `{}`, but `{target}` has `{actual}`",
                            asset.alias, asset.hash
                        ),
                    ));
                }
                continue;
            }
            if assets.len() + disk_assets.len() >= MAX_BUNDLE_ASSETS {
                return Err(resource_error(format!(
                    "bundle contains more than {MAX_BUNDLE_ASSETS} assets"
                )));
            }
            let file = contained_file(root, &target, "asset", DiagnosticCode::Asset)?;
            let is_native_pcm = disk_media
                && asset
                    .alias
                    .strip_prefix("asset:")
                    .and_then(|id| document.objects.get(id))
                    .is_some_and(|object| {
                        object
                            .field("kind")
                            .and_then(|field| field.value.as_symbol())
                            == Some("audio")
                            && object
                                .field("format")
                                .and_then(|field| field.value.as_string())
                                == Some(crate::audio_asset::CORE_AUDIO_FORMAT)
                    });
            if is_native_pcm {
                let snapshot = crate::disk_media::DiskAsset::snapshot(file, &target, &asset.hash)?;
                let total = disk_assets.values().fold(
                    0u64,
                    |n: u64, asset: &std::sync::Arc<crate::disk_media::DiskAsset>| {
                        n.saturating_add(asset.bytes)
                    },
                );
                if total.saturating_add(snapshot.bytes)
                    > crate::disk_media::MAX_DISK_MEDIA_TOTAL_BYTES
                {
                    return Err(resource_error("disk media aggregate exceeds 1 GiB"));
                }
                disk_assets.insert(target, std::sync::Arc::new(snapshot));
                continue;
            }
            let bytes = read_bounded(file, &target, "asset")?;
            asset_bytes = checked_total(asset_bytes, bytes.len(), MAX_BUNDLE_ASSET_BYTES, "asset")?;
            let actual = sha256_digest(&bytes);
            if actual != asset.hash {
                return Err(error(
                    DiagnosticCode::Hash,
                    format!(
                        "asset `{}` in `{logical_path}` expected `{}`, but `{target}` has `{actual}`",
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
    bundle.resolve_with_disk_assets(&disk_assets)?;
    Ok((bundle, disk_assets))
}

/// Match the CLI's canonical-entry behavior without reopening the root path.
pub(crate) fn load_bundle_from_resolved_entry(
    entry_path: &Path,
    root: &ProjectRoot,
) -> Result<SourceBundle, Diagnostics> {
    let resolved =
        resolve_contained_path(root, entry_path, "entry source", DiagnosticCode::Reference)?;
    load_bundle_in_root(&resolved, root)
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

pub(crate) struct ProjectRoot {
    canonical: PathBuf,
    selected: PathBuf,
    dir: Dir,
    pinned_resolution: bool,
}

impl ProjectRoot {
    pub(crate) fn open(path: &Path, authority: AmbientAuthority) -> Result<Self, Diagnostics> {
        let selected = std::path::absolute(path).map_err(|failure| {
            error(
                DiagnosticCode::Reference,
                format!("cannot locate project root `{}`: {failure}", path.display()),
            )
        })?;
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
        Ok(Self {
            canonical,
            selected,
            dir,
            pinned_resolution: false,
        })
    }

    pub(crate) fn open_pinned(
        path: &Path,
        authority: AmbientAuthority,
    ) -> Result<Self, Diagnostics> {
        let mut root = Self::open(path, authority)?;
        root.pinned_resolution = true;
        Ok(root)
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

/// Open a fixed project member through the same containment and handle checks
/// used for authored source and asset references.
pub(crate) fn open_contained_member(root: &ProjectRoot, member: &str) -> Result<File, Diagnostics> {
    contained_file(root, member, "project member", DiagnosticCode::Reference)
}

fn contained_file_with(
    root: &ProjectRoot,
    logical_path: &str,
    label: &str,
    code: DiagnosticCode,
    before_open: impl FnOnce(),
) -> Result<File, Diagnostics> {
    let relative = if root.pinned_resolution {
        resolve_contained_path(root, Path::new(logical_path), label, code)?
    } else {
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
        canonical
            .strip_prefix(&root.canonical)
            .map_err(|_| {
                error(
                    code,
                    format!("{label} `{logical_path}` resolves outside the project root"),
                )
            })?
            .to_path_buf()
    };

    before_open();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = root.dir.open_with(&relative, &options).map_err(|failure| {
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

/// Resolve links against the pinned directory without opening the final node.
/// An authored absolute link is accepted only when it names this project root.
fn resolve_contained_path(
    root: &ProjectRoot,
    path: &Path,
    label: &str,
    code: DiagnosticCode,
) -> Result<PathBuf, Diagnostics> {
    const MAX_LINKS: usize = 40;
    let mut pending = path_parts(path, label, code)?;
    let mut resolved = PathBuf::new();
    let mut links = 0usize;
    while let Some(part) = pending.pop_front() {
        match part {
            PathPart::Parent => {
                if !resolved.pop() {
                    return Err(error(code, format!("{label} path escapes project root")));
                }
            }
            PathPart::Normal(part) => {
                let candidate = resolved.join(&part);
                let metadata = root.dir.symlink_metadata(&candidate).map_err(|failure| {
                    error(
                        code,
                        format!(
                            "cannot resolve {label} `{}` within project root: {failure}",
                            path.display()
                        ),
                    )
                })?;
                if !metadata.file_type().is_symlink() {
                    if !pending.is_empty() && !metadata.is_dir() {
                        return Err(error(
                            code,
                            format!("{label} `{}` is not a directory", candidate.display()),
                        ));
                    }
                    resolved.push(part);
                    continue;
                }
                links += 1;
                if links > MAX_LINKS {
                    return Err(error(
                        code,
                        format!("{label} has too many symlink resolutions"),
                    ));
                }
                let target = root.dir.read_link_contents(&candidate).map_err(|failure| {
                    error(
                        code,
                        format!(
                            "cannot read {label} symlink `{}`: {failure}",
                            candidate.display()
                        ),
                    )
                })?;
                let target = if target.is_absolute() {
                    resolved.clear();
                    target
                        .strip_prefix(&root.canonical)
                        .or_else(|_| target.strip_prefix(&root.selected))
                        .map_err(|_| {
                            error(
                                code,
                                format!(
                                    "{label} symlink `{}` escapes project root",
                                    candidate.display()
                                ),
                            )
                        })?
                        .to_path_buf()
                } else {
                    target
                };
                let mut expansion = path_parts(&target, label, code)?;
                expansion.append(&mut pending);
                pending = expansion;
            }
        }
    }
    if resolved.as_os_str().is_empty() {
        return Err(error(code, format!("{label} path does not name a file")));
    }
    Ok(resolved)
}

enum PathPart {
    Normal(std::ffi::OsString),
    Parent,
}

fn path_parts(
    path: &Path,
    label: &str,
    code: DiagnosticCode,
) -> Result<VecDeque<PathPart>, Diagnostics> {
    let mut parts = VecDeque::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push_back(PathPart::Normal(part.to_os_string())),
            Component::CurDir => {}
            Component::ParentDir => parts.push_back(PathPart::Parent),
            _ => return Err(error(code, format!("{label} path escapes project root"))),
        }
    }
    Ok(parts)
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

        let pinned = ProjectRoot::open_pinned(&project, ambient_authority()).unwrap();
        let diagnostics =
            contained_file(&pinned, "stream.maac", "source", DiagnosticCode::Reference)
                .unwrap_err();
        assert_eq!(diagnostics.first().unwrap().code, DiagnosticCode::Reference);
    }

    #[test]
    fn symlink_parent_components_resolve_after_intervening_links() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(project.join("nested/deeper")).unwrap();
        fs::write(project.join("leaf.maac"), b"wrong").unwrap();
        fs::write(project.join("nested/leaf.maac"), b"right").unwrap();
        symlink("nested/deeper", project.join("jump")).unwrap();
        symlink("jump/../leaf.maac", project.join("alias.maac")).unwrap();
        let root = ProjectRoot::open_pinned(&project, ambient_authority()).unwrap();
        let file =
            contained_file(&root, "alias.maac", "source", DiagnosticCode::Reference).unwrap();
        assert_eq!(
            read_bounded(file, "alias.maac", "source").unwrap(),
            b"right"
        );
    }

    #[test]
    fn ordinary_file_parent_component_is_not_collapsed() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("file"), b"ordinary").unwrap();
        fs::write(project.join("leaf.maac"), b"wrong").unwrap();
        symlink("file/../leaf.maac", project.join("alias.maac")).unwrap();
        let root = ProjectRoot::open_pinned(&project, ambient_authority()).unwrap();
        let failure =
            contained_file(&root, "alias.maac", "source", DiagnosticCode::Reference).unwrap_err();
        assert_eq!(failure.first().unwrap().code, DiagnosticCode::Reference);
    }

    #[test]
    fn pinned_root_accepts_absolute_link_using_selected_root_spelling() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(project.join("real")).unwrap();
        fs::write(project.join("real/data.maac"), b"inside").unwrap();
        symlink(project.join("real/data.maac"), project.join("alias.maac")).unwrap();
        let root = ProjectRoot::open_pinned(&project, ambient_authority()).unwrap();
        let file =
            contained_file(&root, "alias.maac", "source", DiagnosticCode::Reference).unwrap();
        assert_eq!(
            read_bounded(file, "alias.maac", "source").unwrap(),
            b"inside"
        );
    }

    #[test]
    fn ordinary_loader_keeps_absolute_links_through_another_root_alias() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let alias = directory.path().join("project-alias");
        fs::create_dir_all(project.join("real")).unwrap();
        fs::write(project.join("real/data.maac"), b"inside").unwrap();
        symlink(&project, &alias).unwrap();
        symlink(alias.join("real/data.maac"), project.join("alias.maac")).unwrap();
        let root = ProjectRoot::open(&project, ambient_authority()).unwrap();
        let file =
            contained_file(&root, "alias.maac", "source", DiagnosticCode::Reference).unwrap();
        assert_eq!(
            read_bounded(file, "alias.maac", "source").unwrap(),
            b"inside"
        );
    }
}
