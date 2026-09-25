//! Exact internal effect output captured from a private archive snapshot.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use cap_std::fs::OpenOptions as CapOpenOptions;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::archive::ArchiveSnapshot;
use crate::bundle::{sha256_digest, validate_hash_pin};
use crate::bundle_fs::ProjectRoot;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::disk_media::{DiskMediaPlan, DiskMediaProject};
use crate::dsp::{self, RenderError};
use crate::freeze::engine_digest;
use crate::plan::{PlanLimits, PortRef};

pub(crate) const NODE_FREEZE_MANIFEST: &str = "maac-node-freeze.json";
pub(crate) const NODE_FREEZE_OUTPUT: &str = "output.f64le";
pub(crate) const MAX_NODE_FREEZE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const FORMAT: &str = "maac.node-freeze";
const VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    snapshot: String,
    engine: String,
    profile: String,
    source_graph_active: bool,
    freeze_active: bool,
    boundary: Boundary,
    state: State,
    frame_start: u64,
    frame_end: u64,
    sample_rate_hz: u32,
    channels: u8,
    output_channels: u8,
    sample_format: String,
    render_key: String,
    output: Output,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Boundary {
    kind: String,
    node: String,
    port: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    mode: String,
    origin_frame: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Serialize)]
struct RenderKey<'a> {
    format: &'static str,
    version: u32,
    snapshot: &'a str,
    engine: &'a str,
    profile: &'a str,
    boundary: &'a Boundary,
    state: &'a State,
    frame_start: u64,
    frame_end: u64,
    sample_rate_hz: u32,
    channels: u8,
    output_channels: u8,
    sample_format: &'a str,
}

pub(crate) struct NodeFreezePreflight {
    manifest: Manifest,
    json: Vec<u8>,
}

impl NodeFreezePreflight {
    pub(crate) fn output_bytes(&self) -> u64 {
        self.manifest.output.bytes
    }
}

#[derive(Clone)]
pub(crate) struct NodeFreezeSnapshot {
    manifest: Manifest,
    json: Vec<u8>,
    output: Arc<Mutex<File>>,
}

impl NodeFreezeSnapshot {
    pub(crate) fn capture(
        snapshot: &ArchiveSnapshot,
        boundary: &PortRef,
    ) -> Result<Self, Diagnostics> {
        let (_staged, plan) = staged_plan(snapshot)?;
        let channels = plan.node_freeze_channels(boundary).map_err(render_error)?;
        let bytes = output_bytes(plan.output().total_frames, channels)?;
        let mut output = tempfile::tempfile().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private node freeze output: {e}"),
            )
        })?;
        let mut hash = Sha256::new();
        let mut frames = 0u64;
        let mut buffered = BufWriter::new(&mut output);
        plan.render_node_freeze_boundary(boundary, |samples| {
            if samples.len() != channels || frames >= plan.output().total_frames {
                return Err(RenderError::Callback(
                    "node freeze frame shape differs from plan".into(),
                ));
            }
            for &sample in samples {
                if !sample.is_finite() {
                    return Err(RenderError::Nonfinite(
                        "node freeze contains nonfinite PCM".into(),
                    ));
                }
                let bytes = sample.to_le_bytes();
                hash.update(bytes);
                buffered.write_all(&bytes).map_err(|e| {
                    RenderError::Callback(format!("cannot write node freeze output: {e}"))
                })?;
            }
            frames += 1;
            Ok(())
        })
        .map_err(render_error)?;
        buffered.flush().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot flush node freeze output: {e}"),
            )
        })?;
        drop(buffered);
        if frames != plan.output().total_frames
            || output
                .metadata()
                .map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot inspect node freeze output: {e}"),
                    )
                })?
                .len()
                != bytes
        {
            return Err(fail(
                DiagnosticCode::RenderState,
                "node freeze frame count differs from plan",
            ));
        }
        output.seek(SeekFrom::Start(0)).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot rewind node freeze output: {e}"),
            )
        })?;
        let mut manifest = manifest_for(
            snapshot,
            &plan,
            boundary,
            channels,
            bytes,
            format!("sha256:{:x}", hash.finalize()),
            engine_digest()?,
        );
        manifest.render_key = render_key(&manifest)?;
        validate_manifest(&manifest)?;
        let json = serde_json::to_vec(&manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode node freeze manifest: {e}"),
            )
        })?;
        if json.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "node freeze manifest exceeds 1 MiB",
            ));
        }
        Ok(Self {
            manifest,
            json,
            output: Arc::new(Mutex::new(output)),
        })
    }

    pub(crate) fn preflight(
        root: &ProjectRoot,
        digest: &str,
        snapshot: &str,
    ) -> Result<NodeFreezePreflight, Diagnostics> {
        validate_hash_pin(digest, "node freeze digest")?;
        check_tree(root)?;
        let json = read_small(root, NODE_FREEZE_MANIFEST)?;
        let manifest: Manifest = serde_json::from_slice(&json).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("invalid node freeze manifest: {e}"),
            )
        })?;
        validate_manifest(&manifest)?;
        if serde_json::to_vec(&manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode node freeze manifest: {e}"),
            )
        })? != json
        {
            return Err(fail(
                DiagnosticCode::Syntax,
                "node freeze manifest is not canonical JSON",
            ));
        }
        if sha256_digest(&json) != digest || manifest.snapshot != snapshot {
            return Err(fail(
                DiagnosticCode::Hash,
                "node freeze identity differs from checkpoint or snapshot",
            ));
        }
        let metadata = root
            .dir()
            .symlink_metadata(NODE_FREEZE_OUTPUT)
            .map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot inspect node freeze output: {e}"),
                )
            })?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != manifest.output.bytes
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "node freeze output metadata differs from manifest",
            ));
        }
        Ok(NodeFreezePreflight { manifest, json })
    }

    pub(crate) fn verify_preflighted(
        root: &ProjectRoot,
        preflight: &NodeFreezePreflight,
    ) -> Result<Self, Diagnostics> {
        check_tree(root)?;
        let mut source = open_regular(root, NODE_FREEZE_OUTPUT)?;
        let mut output = tempfile::tempfile().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private node freeze output: {e}"),
            )
        })?;
        let (bytes, hash) =
            copy_checked(&mut source, &mut output, preflight.manifest.output.bytes)?;
        if bytes != preflight.manifest.output.bytes || hash != preflight.manifest.output.sha256 {
            return Err(fail(
                DiagnosticCode::Hash,
                "node freeze output differs from manifest",
            ));
        }
        if read_small(root, NODE_FREEZE_MANIFEST)? != preflight.json {
            return Err(fail(
                DiagnosticCode::Hash,
                "node freeze manifest changed during verification",
            ));
        }
        output.seek(SeekFrom::Start(0)).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot rewind node freeze output: {e}"),
            )
        })?;
        Ok(Self {
            manifest: preflight.manifest.clone(),
            json: preflight.json.clone(),
            output: Arc::new(Mutex::new(output)),
        })
    }

    pub(crate) fn digest(&self) -> String {
        sha256_digest(&self.json)
    }
    pub(crate) fn output_bytes(&self) -> u64 {
        self.manifest.output.bytes
    }
    pub(crate) fn source_digest(&self) -> &str {
        &self.manifest.snapshot
    }
    pub(crate) fn engine_digest(&self) -> &str {
        &self.manifest.engine
    }
    pub(crate) fn output_digest(&self) -> &str {
        &self.manifest.output.sha256
    }
    pub(crate) fn frames(&self) -> u64 {
        self.manifest.frame_end
    }
    pub(crate) fn output_channels(&self) -> u8 {
        self.manifest.output_channels
    }
    pub(crate) fn sample_rate_hz(&self) -> u32 {
        self.manifest.sample_rate_hz
    }
    pub(crate) fn render_key(&self) -> &str {
        &self.manifest.render_key
    }

    pub(crate) fn boundary(&self) -> PortRef {
        PortRef::new(&self.manifest.boundary.node, &self.manifest.boundary.port)
            .expect("validated node freeze boundary")
    }

    pub(crate) fn candidate_render_key(
        snapshot: &ArchiveSnapshot,
        boundary: &PortRef,
        engine: &str,
    ) -> Result<String, Diagnostics> {
        let (_staged, plan) = staged_plan(snapshot)?;
        let channels = plan.node_freeze_channels(boundary).map_err(render_error)?;
        let bytes = output_bytes(plan.output().total_frames, channels)?;
        let manifest = manifest_for(
            snapshot,
            &plan,
            boundary,
            channels,
            bytes,
            String::new(),
            engine.into(),
        );
        render_key(&manifest)
    }

    pub(crate) fn stage_into(&self, dir: &Path) -> Result<(), Diagnostics> {
        let meta = fs::symlink_metadata(dir).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect node freeze destination: {e}"),
            )
        })?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(fail(
                DiagnosticCode::Reference,
                "node freeze destination must be a real directory",
            ));
        }
        let mut manifest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(NODE_FREEZE_MANIFEST))
            .map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot create node freeze manifest: {e}"),
                )
            })?;
        manifest.write_all(&self.json).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot write node freeze manifest: {e}"),
            )
        })?;
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(NODE_FREEZE_OUTPUT))
            .map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot create node freeze output: {e}"),
                )
            })?;
        let mut source = self.output.lock().map_err(|_| {
            fail(
                DiagnosticCode::Reference,
                "private node freeze output lock is poisoned",
            )
        })?;
        source.seek(SeekFrom::Start(0)).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot rewind node freeze output: {e}"),
            )
        })?;
        let (bytes, hash) =
            copy_checked(&mut source, &mut destination, self.manifest.output.bytes)?;
        if bytes != self.manifest.output.bytes || hash != self.manifest.output.sha256 {
            return Err(fail(
                DiagnosticCode::Hash,
                "private node freeze output changed before staging",
            ));
        }
        Ok(())
    }

    /// Execute the normal graph while feeding the verified f64 stream into one effect output.
    pub(crate) fn render_replacement<F>(
        &self,
        snapshot: &ArchiveSnapshot,
        callback: F,
    ) -> Result<(), Diagnostics>
    where
        F: FnMut(&[f64]) -> dsp::Result<()>,
    {
        let (_staged, plan) = staged_plan(snapshot)?;
        let boundary = self.boundary();
        let channels = plan.node_freeze_channels(&boundary).map_err(render_error)?;
        if channels != usize::from(self.manifest.channels)
            || plan.output().total_frames != self.frames()
            || plan.output().sample_rate_hz != self.sample_rate_hz()
            || plan.output().channels != self.manifest.output_channels
        {
            return Err(fail(
                DiagnosticCode::RenderState,
                "node freeze boundary differs from archived plan",
            ));
        }
        let mut source = self.output.lock().map_err(|_| {
            fail(
                DiagnosticCode::Reference,
                "private node freeze output lock is poisoned",
            )
        })?;
        source.seek(SeekFrom::Start(0)).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot rewind node freeze output: {e}"),
            )
        })?;
        let mut source = BufReader::new(&mut *source);
        let mut hash = Sha256::new();
        let mut next_frame = 0u64;
        plan.render_with_node_replacement(
            &boundary,
            |frame, samples| {
                if frame != next_frame || samples.len() != channels || frame >= self.frames() {
                    return Err(RenderError::Callback(
                        "node freeze replacement frame shape differs".into(),
                    ));
                }
                for sample in samples {
                    let mut bytes = [0u8; 8];
                    source.read_exact(&mut bytes).map_err(|e| {
                        RenderError::Callback(format!("cannot read node freeze output: {e}"))
                    })?;
                    let value = f64::from_le_bytes(bytes);
                    if !value.is_finite() {
                        return Err(RenderError::Nonfinite(
                            "node freeze contains nonfinite PCM".into(),
                        ));
                    }
                    hash.update(bytes);
                    *sample = value;
                }
                next_frame += 1;
                Ok(())
            },
            callback,
        )
        .map_err(render_error)?;
        if next_frame != self.frames()
            || format!("sha256:{:x}", hash.finalize()) != self.output_digest()
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "private node freeze output changed during activation",
            ));
        }
        Ok(())
    }
}

fn staged_plan(
    snapshot: &ArchiveSnapshot,
) -> Result<(tempfile::TempDir, DiskMediaPlan), Diagnostics> {
    let staged = tempfile::tempdir().map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot create private node freeze staging: {e}"),
        )
    })?;
    snapshot.stage_members(staged.path())?;
    let project = DiskMediaProject::load(Path::new(snapshot.entry()), staged.path())?;
    let limits = match snapshot.profile() {
        "default" => PlanLimits::default(),
        "song" => PlanLimits::song(),
        _ => {
            return Err(fail(
                DiagnosticCode::Version,
                "unsupported node freeze profile",
            ));
        }
    };
    Ok((staged, project.build_with_limits(&limits)?))
}

fn manifest_for(
    snapshot: &ArchiveSnapshot,
    plan: &DiskMediaPlan,
    boundary: &PortRef,
    channels: usize,
    bytes: u64,
    hash: String,
    engine: String,
) -> Manifest {
    Manifest {
        format: FORMAT.into(),
        version: VERSION,
        snapshot: snapshot.digest(),
        engine,
        profile: snapshot.profile().into(),
        source_graph_active: true,
        freeze_active: false,
        boundary: Boundary {
            kind: "project_node_output".into(),
            node: boundary.node.clone(),
            port: boundary.port.clone(),
        },
        state: State {
            mode: "reset".into(),
            origin_frame: 0,
        },
        frame_start: 0,
        frame_end: plan.output().total_frames,
        sample_rate_hz: plan.output().sample_rate_hz,
        channels: channels as u8,
        output_channels: plan.output().channels,
        sample_format: "ieee_f64le_interleaved".into(),
        render_key: String::new(),
        output: Output {
            path: NODE_FREEZE_OUTPUT.into(),
            bytes,
            sha256: hash,
        },
    }
}

fn output_bytes(frames: u64, channels: usize) -> Result<u64, Diagnostics> {
    let bytes = frames
        .checked_mul(channels as u64)
        .and_then(|v| v.checked_mul(8))
        .ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "node freeze output length overflow",
            )
        })?;
    if frames == 0 || !matches!(channels, 1 | 2) || bytes > MAX_NODE_FREEZE_BYTES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "node freeze output exceeds 1 GiB or has invalid shape",
        ));
    }
    Ok(bytes)
}

fn render_key(m: &Manifest) -> Result<String, Diagnostics> {
    let key = RenderKey {
        format: "maac.node-freeze-render-key",
        version: 1,
        snapshot: &m.snapshot,
        engine: &m.engine,
        profile: &m.profile,
        boundary: &m.boundary,
        state: &m.state,
        frame_start: m.frame_start,
        frame_end: m.frame_end,
        sample_rate_hz: m.sample_rate_hz,
        channels: m.channels,
        output_channels: m.output_channels,
        sample_format: &m.sample_format,
    };
    Ok(sha256_digest(&serde_json::to_vec(&key).map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode node freeze render key: {e}"),
        )
    })?))
}

fn validate_manifest(m: &Manifest) -> Result<(), Diagnostics> {
    if m.format != FORMAT || m.version != VERSION {
        return Err(fail(
            DiagnosticCode::Version,
            "unsupported node freeze format or version",
        ));
    }
    for (value, label) in [
        (&m.snapshot, "snapshot"),
        (&m.engine, "engine"),
        (&m.render_key, "render key"),
        (&m.output.sha256, "output"),
    ] {
        validate_hash_pin(value, label)?;
    }
    if !matches!(m.profile.as_str(), "default" | "song")
        || !m.source_graph_active
        || m.freeze_active
        || m.boundary.kind != "project_node_output"
        || m.boundary.port != "out"
        || PortRef::new(&m.boundary.node, &m.boundary.port).is_err()
        || m.state.mode != "reset"
        || m.state.origin_frame != 0
        || m.frame_start != 0
        || m.sample_rate_hz == 0
        || !matches!(m.channels, 1 | 2)
        || !matches!(m.output_channels, 1 | 2)
        || m.sample_format != "ieee_f64le_interleaved"
        || m.output.path != NODE_FREEZE_OUTPUT
        || output_bytes(m.frame_end, usize::from(m.channels))? != m.output.bytes
    {
        return Err(fail(DiagnosticCode::Syntax, "invalid node freeze metadata"));
    }
    if render_key(m)? != m.render_key {
        return Err(fail(
            DiagnosticCode::Hash,
            "node freeze render key differs from metadata",
        ));
    }
    Ok(())
}

fn check_tree(root: &ProjectRoot) -> Result<(), Diagnostics> {
    let mut names = BTreeSet::new();
    for entry in root.dir().read_dir(".").map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot list node freeze directory: {e}"),
        )
    })? {
        let entry = entry.map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot list node freeze directory: {e}"),
            )
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| fail(DiagnosticCode::Reference, "non-UTF-8 node freeze member"))?;
        let kind = entry.file_type().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect node freeze member: {e}"),
            )
        })?;
        if !kind.is_file()
            || kind.is_symlink()
            || !matches!(name.as_str(), NODE_FREEZE_MANIFEST | NODE_FREEZE_OUTPUT)
        {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("unexpected node freeze member `{name}`"),
            ));
        }
        names.insert(name);
    }
    if names != BTreeSet::from([NODE_FREEZE_MANIFEST.into(), NODE_FREEZE_OUTPUT.into()]) {
        return Err(fail(
            DiagnosticCode::Reference,
            "node freeze directory is missing required files",
        ));
    }
    Ok(())
}

fn open_regular(root: &ProjectRoot, path: &str) -> Result<File, Diagnostics> {
    let meta = root.dir().symlink_metadata(path).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot inspect node freeze member: {e}"),
        )
    })?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(fail(
            DiagnosticCode::Reference,
            "node freeze member is not a regular file",
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
                format!("cannot open node freeze member: {e}"),
            )
        })?
        .into_std();
    if !file
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect opened node freeze member: {e}"),
            )
        })?
        .is_file()
    {
        return Err(fail(
            DiagnosticCode::Reference,
            "opened node freeze member is not a regular file",
        ));
    }
    Ok(file)
}

fn read_small(root: &ProjectRoot, path: &str) -> Result<Vec<u8>, Diagnostics> {
    let file = open_regular(root, path)?;
    if file
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect node freeze manifest: {e}"),
            )
        })?
        .len()
        > MAX_MANIFEST_BYTES
    {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "node freeze manifest exceeds 1 MiB",
        ));
    }
    let mut json = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut json)
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot read node freeze manifest: {e}"),
            )
        })?;
    if json.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "node freeze manifest exceeds 1 MiB",
        ));
    }
    Ok(json)
}

fn copy_checked(
    source: &mut File,
    destination: &mut File,
    expected: u64,
) -> Result<(u64, String), Diagnostics> {
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut block = [0u8; 64 * 1024];
    let mut pending = [0u8; 8];
    let mut pending_len = 0usize;
    loop {
        let count = source.read(&mut block).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot read node freeze output: {e}"),
            )
        })?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "node freeze output length overflow",
            )
        })?;
        if bytes > expected || bytes > MAX_NODE_FREEZE_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "node freeze output exceeds declared length",
            ));
        }
        hash.update(&block[..count]);
        destination.write_all(&block[..count]).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot write node freeze output: {e}"),
            )
        })?;
        let mut offset = 0usize;
        if pending_len != 0 {
            let copied = (8 - pending_len).min(count);
            pending[pending_len..pending_len + copied].copy_from_slice(&block[..copied]);
            pending_len += copied;
            offset = copied;
            if pending_len == 8 {
                if !f64::from_le_bytes(pending).is_finite() {
                    return Err(fail(
                        DiagnosticCode::Nonfinite,
                        "node freeze output contains nonfinite PCM",
                    ));
                }
            } else {
                continue;
            }
        }
        let mut chunks = block[offset..count].chunks_exact(8);
        for sample in &mut chunks {
            if !f64::from_le_bytes(sample.try_into().expect("eight bytes")).is_finite() {
                return Err(fail(
                    DiagnosticCode::Nonfinite,
                    "node freeze output contains nonfinite PCM",
                ));
            }
        }
        let remainder = chunks.remainder();
        pending[..remainder.len()].copy_from_slice(remainder);
        pending_len = remainder.len();
    }
    if bytes != expected || pending_len != 0 {
        return Err(fail(
            DiagnosticCode::Hash,
            "node freeze output length differs from manifest",
        ));
    }
    Ok((bytes, format!("sha256:{:x}", hash.finalize())))
}

fn render_error(error: RenderError) -> Diagnostics {
    let code = match error {
        RenderError::Nonfinite(_) => DiagnosticCode::Nonfinite,
        _ => DiagnosticCode::RenderState,
    };
    fail(code, error.to_string())
}

fn fail(code: DiagnosticCode, message: impl Into<String>) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    diagnostics.push(Diagnostic::error(code, message, None));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"maac 1;
project p { score=[0q,1/10q]; tail=1/5s; rate=48000Hz; tempo=&clock; meter=&metre; output=&pan:out; requires=["maac.production/1"]; }
tempo clock { points=[(0q,120bpm,step)]; }
meter metre { points=[(0q,4,4)]; }
node tone { type="core.sine/1"; params={attack=0s;release=0s;level=0.2;}; }
node room { type="fx.reverb/1"; config={channels=1;}; params={decay=1/2s;mix=0.4;}; }
node gain { type="core.gain/1"; config={channels=1;}; params={gain=3/4;}; }
node pan { type="core.pan/1"; params={pan=0.2;}; }
connect a { from=&tone:out; to=&room:in; }
connect b { from=&room:out; to=&gain:in; }
connect c { from=&gain:out; to=&pan:in; }
pattern phrase { length=1/10q; note n { at=0q; dur=1/10q; pitch=A4; velocity=1; } }
track t { target=&tone:events; }
place notes { pattern=&phrase; track=&t; at=0q; }
"#;

    fn project() -> (tempfile::TempDir, ArchiveSnapshot, PortRef) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.maac"), SOURCE).unwrap();
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), dir.path(), "default").unwrap();
        let boundary = PortRef::new("room", "out").unwrap();
        (dir, snapshot, boundary)
    }

    #[test]
    fn captured_node_stream_roundtrips_and_replaces_only_the_effect() {
        let (_source, snapshot, boundary) = project();
        let freeze = NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap();
        assert_eq!(freeze.manifest.channels, 1);
        assert_eq!(freeze.output_channels(), 2);
        let archive = tempfile::tempdir().unwrap();
        freeze.stage_into(archive.path()).unwrap();
        let root = ProjectRoot::open_pinned(archive.path(), cap_std::ambient_authority()).unwrap();
        let preflight =
            NodeFreezeSnapshot::preflight(&root, &freeze.digest(), &snapshot.digest()).unwrap();
        let verified = NodeFreezeSnapshot::verify_preflighted(&root, &preflight).unwrap();
        let (_staged, plan) = staged_plan(&snapshot).unwrap();
        let mut normal = Vec::new();
        plan.render(|frame| {
            normal.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
        let mut replaced = Vec::new();
        verified
            .render_replacement(&snapshot, |frame| {
                replaced.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        assert_eq!(normal.len(), replaced.len());
        assert_eq!(
            normal.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            replaced.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn verified_stream_rejects_nonfinite_even_with_matching_hash() {
        let (_source, snapshot, boundary) = project();
        let freeze = NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap();
        let archive = tempfile::tempdir().unwrap();
        freeze.stage_into(archive.path()).unwrap();
        let output = archive.path().join(NODE_FREEZE_OUTPUT);
        let mut bytes = fs::read(&output).unwrap();
        bytes[..8].copy_from_slice(&f64::NAN.to_le_bytes());
        fs::write(&output, &bytes).unwrap();
        let mut manifest = freeze.manifest.clone();
        manifest.output.sha256 = sha256_digest(&bytes);
        let json = serde_json::to_vec(&manifest).unwrap();
        fs::write(archive.path().join(NODE_FREEZE_MANIFEST), &json).unwrap();
        let root = ProjectRoot::open_pinned(archive.path(), cap_std::ambient_authority()).unwrap();
        let preflight =
            NodeFreezeSnapshot::preflight(&root, &sha256_digest(&json), &snapshot.digest())
                .unwrap();
        assert!(NodeFreezeSnapshot::verify_preflighted(&root, &preflight).is_err());
    }
}
