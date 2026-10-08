//! Exact internal effect output captured from a private archive snapshot.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
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
use crate::syntax::{self, ValueKind};

pub(crate) const NODE_FREEZE_MANIFEST: &str = "maac-node-freeze.json";
pub(crate) const NODE_FREEZE_OUTPUT: &str = "output.f64le";
pub(crate) const MAX_NODE_FREEZE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const FORMAT: &str = "maac.node-freeze";
const VERSION: u32 = 3;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reuse_identity: Option<String>,
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

#[derive(Serialize)]
struct SelectiveRenderKey<'a> {
    format: &'static str,
    version: u32,
    reuse_identity: &'a str,
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

/// A set of independent leaf freezes. Each member remains a normal, separately
/// stageable node-freeze archive with the existing manifest and payload format.
pub(crate) struct NodeFreezeGroup {
    leaves: Vec<NodeFreezeSnapshot>,
}

pub(crate) struct NodeFreezeGroupInfo {
    pub(crate) frames: u64,
    pub(crate) channels: u8,
    pub(crate) sample_rate_hz: u32,
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
        manifest.reuse_identity = Some(reuse_identity(snapshot, &plan, boundary, VERSION)?);
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
    pub(crate) fn boundary(&self) -> PortRef {
        PortRef::new(&self.manifest.boundary.node, &self.manifest.boundary.port)
            .expect("validated node freeze boundary")
    }

    pub(crate) fn matches_snapshot(&self, snapshot: &ArchiveSnapshot) -> Result<bool, Diagnostics> {
        let (_staged, plan) = staged_plan(snapshot)?;
        self.matches_plan(snapshot, &plan)
    }

    fn matches_plan(
        &self,
        snapshot: &ArchiveSnapshot,
        plan: &DiskMediaPlan,
    ) -> Result<bool, Diagnostics> {
        let boundary = self.boundary();
        let identity_matches = if matches!(self.manifest.version, 2 | 3) {
            self.manifest.reuse_identity.as_deref()
                == Some(reuse_identity(snapshot, plan, &boundary, self.manifest.version)?.as_str())
        } else {
            self.manifest.snapshot == snapshot.digest()
        };
        if !identity_matches {
            return Ok(false);
        }
        let channels = plan.node_freeze_channels(&boundary).map_err(render_error)?;
        let mut candidate = manifest_for(
            snapshot,
            plan,
            &boundary,
            channels,
            output_bytes(plan.output().total_frames, channels)?,
            String::new(),
            self.manifest.engine.clone(),
        );
        candidate.version = self.manifest.version;
        candidate.reuse_identity = self.manifest.reuse_identity.clone();
        Ok(render_key(&candidate)? == self.manifest.render_key)
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

struct CapturedGroupStream {
    writer: BufWriter<File>,
    hash: Sha256,
    frames: u64,
    channels: usize,
    bytes: u64,
}

impl NodeFreezeGroup {
    /// Capture all independent effects in one reset-origin execution. The DSP
    /// validator fixes canonical node-ID order before any private output exists.
    pub(crate) fn capture(
        snapshot: &ArchiveSnapshot,
        boundaries: &[PortRef],
    ) -> Result<Self, Diagnostics> {
        let (_staged, plan) = staged_plan(snapshot)?;
        let ordered = plan
            .validate_node_freeze_group(boundaries)
            .map_err(render_error)?;
        let mut total_bytes = 0u64;
        let mut streams = Vec::with_capacity(ordered.len());
        for (_, channels) in &ordered {
            let bytes = output_bytes(plan.output().total_frames, *channels)?;
            total_bytes = total_bytes.checked_add(bytes).ok_or_else(|| {
                fail(
                    DiagnosticCode::ResourceLimit,
                    "node freeze group length overflow",
                )
            })?;
            if total_bytes > MAX_NODE_FREEZE_BYTES {
                return Err(fail(
                    DiagnosticCode::ResourceLimit,
                    "node freeze group output exceeds 1 GiB",
                ));
            }
            let output = tempfile::tempfile().map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot create private node freeze output: {e}"),
                )
            })?;
            streams.push(CapturedGroupStream {
                writer: BufWriter::new(output),
                hash: Sha256::new(),
                frames: 0,
                channels: *channels,
                bytes,
            });
        }
        let ports: Vec<_> = ordered.iter().map(|(port, _)| port.clone()).collect();
        plan.render_node_freeze_group(&ports, |outputs| {
            if outputs.len() != streams.len() {
                return Err(RenderError::Callback(
                    "node freeze group frame shape differs from plan".into(),
                ));
            }
            for (samples, stream) in outputs.iter().zip(&mut streams) {
                if samples.len() != stream.channels || stream.frames >= plan.output().total_frames {
                    return Err(RenderError::Callback(
                        "node freeze group frame shape differs from plan".into(),
                    ));
                }
                for &sample in samples {
                    if !sample.is_finite() {
                        return Err(RenderError::Nonfinite(
                            "node freeze contains nonfinite PCM".into(),
                        ));
                    }
                    let bytes = sample.to_le_bytes();
                    stream.hash.update(bytes);
                    stream.writer.write_all(&bytes).map_err(|e| {
                        RenderError::Callback(format!("cannot write node freeze output: {e}"))
                    })?;
                }
                stream.frames += 1;
            }
            Ok(())
        })
        .map_err(render_error)?;
        let engine = engine_digest()?;
        let mut leaves = Vec::with_capacity(ordered.len());
        for ((boundary, channels), mut stream) in ordered.into_iter().zip(streams) {
            stream.writer.flush().map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot flush node freeze output: {e}"),
                )
            })?;
            let mut output = stream.writer.into_inner().map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot finish node freeze output: {e}"),
                )
            })?;
            if stream.frames != plan.output().total_frames
                || output
                    .metadata()
                    .map_err(|e| {
                        fail(
                            DiagnosticCode::Reference,
                            format!("cannot inspect node freeze output: {e}"),
                        )
                    })?
                    .len()
                    != stream.bytes
            {
                return Err(fail(
                    DiagnosticCode::RenderState,
                    "node freeze group frame count differs from plan",
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
                &boundary,
                channels,
                stream.bytes,
                format!("sha256:{:x}", stream.hash.finalize()),
                engine.clone(),
            );
            manifest.reuse_identity = Some(reuse_identity(snapshot, &plan, &boundary, VERSION)?);
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
            leaves.push(NodeFreezeSnapshot {
                manifest,
                json,
                output: Arc::new(Mutex::new(output)),
            });
        }
        Ok(Self { leaves })
    }

    /// Retain already verified archive leaves, sorted by canonical node ID.
    pub(crate) fn from_verified(mut leaves: Vec<NodeFreezeSnapshot>) -> Result<Self, Diagnostics> {
        if !(2..=16).contains(&leaves.len()) {
            return Err(fail(
                DiagnosticCode::RenderState,
                "node freeze group must contain 2..16 boundaries",
            ));
        }
        leaves.sort_by(|left, right| {
            left.manifest
                .boundary
                .node
                .as_bytes()
                .cmp(right.manifest.boundary.node.as_bytes())
        });
        let first = &leaves[0].manifest;
        let mut total_bytes = 0u64;
        let mut previous: Option<&str> = None;
        for leaf in &leaves {
            let manifest = &leaf.manifest;
            validate_manifest(manifest)?;
            if previous == Some(manifest.boundary.node.as_str())
                || manifest.engine != first.engine
                || manifest.profile != first.profile
                || manifest.frame_end != first.frame_end
                || manifest.sample_rate_hz != first.sample_rate_hz
                || manifest.output_channels != first.output_channels
            {
                return Err(fail(
                    DiagnosticCode::RenderState,
                    "node freeze group leaves have incompatible metadata",
                ));
            }
            previous = Some(&manifest.boundary.node);
            total_bytes = total_bytes
                .checked_add(manifest.output.bytes)
                .ok_or_else(|| {
                    fail(
                        DiagnosticCode::ResourceLimit,
                        "node freeze group length overflow",
                    )
                })?;
            if total_bytes > MAX_NODE_FREEZE_BYTES {
                return Err(fail(
                    DiagnosticCode::ResourceLimit,
                    "node freeze group output exceeds 1 GiB",
                ));
            }
        }
        Ok(Self { leaves })
    }

    pub(crate) fn leaves(&self) -> &[NodeFreezeSnapshot] {
        &self.leaves
    }

    pub(crate) fn info(&self) -> NodeFreezeGroupInfo {
        let first = &self.leaves[0];
        NodeFreezeGroupInfo {
            frames: first.frames(),
            channels: first.output_channels(),
            sample_rate_hz: first.sample_rate_hz(),
        }
    }

    pub(crate) fn matches_snapshot(&self, snapshot: &ArchiveSnapshot) -> Result<bool, Diagnostics> {
        let (_staged, plan) = staged_plan(snapshot)?;
        let ordered = plan
            .validate_node_freeze_group(&self.boundaries())
            .map_err(render_error)?;
        for ((boundary, channels), leaf) in ordered.iter().zip(&self.leaves) {
            if boundary != &leaf.boundary()
                || *channels != usize::from(leaf.manifest.channels)
                || !leaf.matches_plan(snapshot, &plan)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Candidate edits may remove or rewire a boundary. Report those as stale
    /// leaves while archive integrity continues to use strict validation.
    pub(crate) fn candidate_eligibility(
        &self,
        snapshot: &ArchiveSnapshot,
    ) -> Result<(bool, Vec<bool>), Diagnostics> {
        let (_staged, plan) = staged_plan(snapshot)?;
        let group_shape_valid = plan.validate_node_freeze_group(&self.boundaries()).is_ok();
        let mut leaves = Vec::with_capacity(self.leaves.len());
        for leaf in &self.leaves {
            let boundary = leaf.boundary();
            let eligible = if plan.node_freeze_channels(&boundary).is_ok() {
                leaf.matches_plan(snapshot, &plan)?
            } else {
                false
            };
            leaves.push(eligible);
        }
        Ok((group_shape_valid, leaves))
    }

    /// Re-render every selected output once and return its leaf PCM digests in
    /// the same canonical order returned by `leaves`.
    pub(crate) fn replay_output_digests(
        &self,
        candidate: &ArchiveSnapshot,
    ) -> Result<Vec<String>, Diagnostics> {
        let replay = Self::capture(candidate, &self.boundaries())?;
        Ok(replay
            .leaves()
            .iter()
            .map(|leaf| leaf.output_digest().to_owned())
            .collect())
    }

    /// Compare all stored leaf digests using one candidate render.
    pub(crate) fn replay_matches(
        &self,
        candidate: &ArchiveSnapshot,
    ) -> Result<Vec<bool>, Diagnostics> {
        let digests = self.replay_output_digests(candidate)?;
        Ok(self
            .leaves
            .iter()
            .zip(digests)
            .map(|(stored, digest)| stored.output_digest() == digest)
            .collect())
    }

    /// Run the graph once with all verified private streams. No final-output
    /// frame reaches the caller until replacement and graph rendering complete.
    pub(crate) fn render_replacements<F>(
        &self,
        candidate: &ArchiveSnapshot,
        callback: F,
    ) -> Result<(), Diagnostics>
    where
        F: FnMut(&[f64]) -> dsp::Result<()>,
    {
        self.render_selected_replacements(candidate, &vec![true; self.leaves.len()], callback)
    }

    /// Replace a canonical subset while verifying the full group still has a
    /// valid independent shape. Selected streams and final output remain private
    /// until every hash and frame count has been checked.
    pub(crate) fn render_selected_replacements<F>(
        &self,
        candidate: &ArchiveSnapshot,
        selected: &[bool],
        mut callback: F,
    ) -> Result<(), Diagnostics>
    where
        F: FnMut(&[f64]) -> dsp::Result<()>,
    {
        if selected.len() != self.leaves.len() || !selected.iter().any(|selected| *selected) {
            return Err(fail(
                DiagnosticCode::RenderState,
                "node freeze group requires at least one selected leaf",
            ));
        }
        if self.leaves[0].engine_digest() != engine_digest()? {
            return Err(fail(
                DiagnosticCode::RenderState,
                "node freeze group engine differs from current executable",
            ));
        }
        let (_staged, plan) = staged_plan(candidate)?;
        let boundaries = self.boundaries();
        let ordered = plan
            .validate_node_freeze_group(&boundaries)
            .map_err(render_error)?;
        let mut leaves = Vec::new();
        for ((boundary, channels), (leaf, selected)) in
            ordered.iter().zip(self.leaves.iter().zip(selected))
        {
            if !selected {
                continue;
            }
            if boundary != &leaf.boundary()
                || *channels != usize::from(leaf.manifest.channels)
                || !leaf.matches_plan(candidate, &plan)?
            {
                return Err(fail(
                    DiagnosticCode::RenderState,
                    "node freeze group differs from candidate plan",
                ));
            }
            leaves.push(leaf);
        }
        let boundaries: Vec<_> = leaves.iter().map(|leaf| leaf.boundary()).collect();
        let mut sources = leaves
            .iter()
            .map(|leaf| {
                leaf.output.lock().map_err(|_| {
                    fail(
                        DiagnosticCode::Reference,
                        "private node freeze output lock is poisoned",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (leaf, source) in leaves.iter().zip(&mut sources) {
            source.seek(SeekFrom::Start(0)).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot rewind node freeze output: {e}"),
                )
            })?;
            let (bytes, hash) = copy_checked(&mut *source, &mut io::sink(), leaf.output_bytes())?;
            if bytes != leaf.output_bytes() || hash != leaf.output_digest() {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "private node freeze output changed before activation",
                ));
            }
            source.seek(SeekFrom::Start(0)).map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot rewind node freeze output: {e}"),
                )
            })?;
        }
        let mut readers: Vec<_> = sources
            .iter_mut()
            .map(|source| BufReader::new(&mut **source))
            .collect();
        let mut hashes = vec![Sha256::new(); leaves.len()];
        let mut next_frames = vec![0u64; leaves.len()];
        let output_bytes = output_bytes(
            plan.output().total_frames,
            usize::from(plan.output().channels),
        )?;
        let final_output = tempfile::tempfile().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private node freeze render: {e}"),
            )
        })?;
        let mut rendered_frames = 0u64;
        let mut writer = BufWriter::new(final_output);
        let mut replace = |index: usize, frame: u64, samples: &mut [f64]| {
            let leaf = leaves[index];
            if frame != next_frames[index]
                || frame >= leaf.frames()
                || samples.len() != usize::from(leaf.manifest.channels)
            {
                return Err(RenderError::Callback(
                    "node freeze replacement frame shape differs".into(),
                ));
            }
            for sample in samples {
                let mut bytes = [0u8; 8];
                readers[index].read_exact(&mut bytes).map_err(|e| {
                    RenderError::Callback(format!("cannot read node freeze output: {e}"))
                })?;
                let value = f64::from_le_bytes(bytes);
                if !value.is_finite() {
                    return Err(RenderError::Nonfinite(
                        "node freeze contains nonfinite PCM".into(),
                    ));
                }
                hashes[index].update(bytes);
                *sample = value;
            }
            next_frames[index] += 1;
            Ok(())
        };
        let mut spool = |samples: &[f64]| {
            if samples.len() != usize::from(plan.output().channels)
                || rendered_frames >= plan.output().total_frames
            {
                return Err(RenderError::Callback(
                    "node freeze final frame shape differs from plan".into(),
                ));
            }
            for &sample in samples {
                if !sample.is_finite() {
                    return Err(RenderError::Nonfinite(
                        "node freeze render contains nonfinite PCM".into(),
                    ));
                }
                writer.write_all(&sample.to_le_bytes()).map_err(|e| {
                    RenderError::Callback(format!("cannot write node freeze render: {e}"))
                })?;
            }
            rendered_frames += 1;
            Ok(())
        };
        if boundaries.len() == 1 {
            plan.render_with_node_replacement(
                &boundaries[0],
                |frame, samples| replace(0, frame, samples),
                &mut spool,
            )
            .map_err(render_error)?;
        } else {
            plan.render_with_node_replacements(&boundaries, &mut replace, &mut spool)
                .map_err(render_error)?;
        }
        for (index, leaf) in leaves.iter().enumerate() {
            if next_frames[index] != leaf.frames()
                || format!("sha256:{:x}", hashes[index].clone().finalize()) != leaf.output_digest()
            {
                return Err(fail(
                    DiagnosticCode::Hash,
                    "private node freeze output changed during activation",
                ));
            }
        }
        writer.flush().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot flush node freeze render: {e}"),
            )
        })?;
        let mut final_output = writer.into_inner().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot finish node freeze render: {e}"),
            )
        })?;
        if rendered_frames != plan.output().total_frames
            || final_output
                .metadata()
                .map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot inspect node freeze render: {e}"),
                    )
                })?
                .len()
                != output_bytes
        {
            return Err(fail(
                DiagnosticCode::RenderState,
                "node freeze final frame count differs from plan",
            ));
        }
        final_output.seek(SeekFrom::Start(0)).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot rewind node freeze render: {e}"),
            )
        })?;
        let mut reader = BufReader::new(final_output);
        let mut frame = vec![0.0; usize::from(plan.output().channels)];
        for _ in 0..rendered_frames {
            for sample in &mut frame {
                let mut bytes = [0u8; 8];
                reader.read_exact(&mut bytes).map_err(|e| {
                    fail(
                        DiagnosticCode::Reference,
                        format!("cannot read node freeze render: {e}"),
                    )
                })?;
                *sample = f64::from_le_bytes(bytes);
            }
            callback(&frame).map_err(render_error)?;
        }
        Ok(())
    }

    fn boundaries(&self) -> Vec<PortRef> {
        self.leaves
            .iter()
            .map(NodeFreezeSnapshot::boundary)
            .collect()
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
        reuse_identity: None,
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

fn reuse_identity(
    snapshot: &ArchiveSnapshot,
    plan: &DiskMediaPlan,
    boundary: &PortRef,
    version: u32,
) -> Result<String, Diagnostics> {
    let source = snapshot.source_text(snapshot.entry())?;
    let document = syntax::parse(source)?;
    let allowed = match version {
        2 => plan.selective_node_params(boundary),
        3 => plan.selective_node_params_v3(boundary),
        _ => {
            return Err(fail(
                DiagnosticCode::Version,
                "unsupported node freeze version",
            ))
        }
    };
    let mut spans = Vec::new();
    for (id, parameter) in &allowed {
        let Some(object) = document.object(id) else {
            continue;
        };
        if object.kind != "node" {
            continue;
        }
        let Some(params) = object.field("params") else {
            continue;
        };
        let ValueKind::Record(fields) = &params.value.kind else {
            continue;
        };
        let Some(field) = fields.get(*parameter) else {
            continue;
        };
        if matches!(field.value.kind, ValueKind::Number(_)) {
            spans.push(field.value.span);
        }
    }
    spans.sort_by_key(|span| span.start);
    let mut normalized = Vec::with_capacity(source.len());
    let mut cursor = 0;
    for span in spans {
        if span.start < cursor || span.end > source.len() || span.is_empty() {
            return Err(fail(
                DiagnosticCode::Syntax,
                "invalid selective node parameter span",
            ));
        }
        normalized.extend_from_slice(&source.as_bytes()[cursor..span.start]);
        normalized.extend_from_slice(b"<node-freeze-param>");
        cursor = span.end;
    }
    normalized.extend_from_slice(&source.as_bytes()[cursor..]);
    snapshot.node_freeze_reuse_identity_with_mask(&normalized, &allowed)
}

fn render_key(m: &Manifest) -> Result<String, Diagnostics> {
    if matches!(m.version, 2 | 3) {
        let key = SelectiveRenderKey {
            format: "maac.node-freeze-render-key",
            version: m.version,
            reuse_identity: m.reuse_identity.as_deref().ok_or_else(|| {
                fail(DiagnosticCode::Syntax, "missing node freeze reuse identity")
            })?,
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
        return Ok(sha256_digest(&serde_json::to_vec(&key).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode node freeze render key: {e}"),
            )
        })?));
    }
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
    if m.format != FORMAT || !matches!(m.version, 1..=3) {
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
    match (m.version, &m.reuse_identity) {
        (1, None) => {}
        (2 | 3, Some(identity)) => validate_hash_pin(identity, "reuse identity")?,
        _ => {
            return Err(fail(
                DiagnosticCode::Syntax,
                "invalid node freeze reuse identity",
            ))
        }
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
    destination: &mut impl Write,
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
        let (samples, remainder) = block[offset..count].as_chunks::<8>();
        for sample in samples {
            if !f64::from_le_bytes(*sample).is_finite() {
                return Err(fail(
                    DiagnosticCode::Nonfinite,
                    "node freeze output contains nonfinite PCM",
                ));
            }
        }
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

    fn project_with_source(source: &str) -> (tempfile::TempDir, ArchiveSnapshot, PortRef) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.maac"), source).unwrap();
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), dir.path(), "default").unwrap();
        let boundary = PortRef::new("room", "out").unwrap();
        (dir, snapshot, boundary)
    }

    fn project() -> (tempfile::TempDir, ArchiveSnapshot, PortRef) {
        project_with_source(SOURCE)
    }

    fn sibling_source() -> String {
        SOURCE
            .replace("output=&pan:out", "output=&bus:out")
            .replace(
                "node pan {",
                "node sidegain { type=\"core.gain/1\"; config={channels=1;}; params={gain=1/3;}; }\nnode sidepan { type=\"core.pan/1\"; params={pan=-0.3;}; }\nnode bus { type=\"core.sum/1\"; config={channels=2;}; }\nnode pan {",
            )
            .replace(
                "connect c { from=&gain:out; to=&pan:in; }",
                "connect c { from=&gain:out; to=&pan:in; }\nconnect side_input { from=&tone:out; to=&sidegain:in; }\nconnect side_pan { from=&sidegain:out; to=&sidepan:in; }\nconnect main_bus { from=&pan:out; to=&bus:in; }\nconnect side_bus { from=&sidepan:out; to=&bus:in; }",
            )
    }

    fn independent_effects_source() -> String {
        sibling_source()
            .replace(
                "node sidegain {",
                "node sideroom { type=\"fx.reverb/1\"; config={channels=1;}; params={decay=1/3s;mix=0.25;}; }\nnode sidegain {",
            )
            .replace(
                "connect side_pan { from=&sidegain:out; to=&sidepan:in; }",
                "connect side_room { from=&sidegain:out; to=&sideroom:in; }\nconnect side_pan { from=&sideroom:out; to=&sidepan:in; }",
            )
    }

    fn three_independent_effects_source() -> String {
        independent_effects_source()
            .replace(
                "node bus {",
                "node thirdpan { type=\"core.pan/1\"; params={pan=0.2;}; }\nnode thirdfx { type=\"fx.reverb/1\"; config={channels=2;}; params={decay=1/4s;mix=0.3;}; }\nnode bus {",
            )
            .replace(
                "connect side_bus { from=&sidepan:out; to=&bus:in; }",
                "connect side_bus { from=&sidepan:out; to=&bus:in; }\nconnect third_input { from=&tone:out; to=&thirdpan:in; }\nconnect third_room { from=&thirdpan:out; to=&thirdfx:in; }\nconnect third_bus { from=&thirdfx:out; to=&bus:in; }",
            )
    }

    fn as_v2(
        mut freeze: NodeFreezeSnapshot,
        snapshot: &ArchiveSnapshot,
        boundary: &PortRef,
    ) -> NodeFreezeSnapshot {
        let (_staged, plan) = staged_plan(snapshot).unwrap();
        freeze.manifest.version = 2;
        freeze.manifest.reuse_identity =
            Some(reuse_identity(snapshot, &plan, boundary, 2).unwrap());
        freeze.manifest.render_key = render_key(&freeze.manifest).unwrap();
        freeze.json = serde_json::to_vec(&freeze.manifest).unwrap();
        freeze
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
    fn group_capture_keeps_leaf_bytes_and_replays_in_canonical_order() {
        let (_project, snapshot, room) = project_with_source(&independent_effects_source());
        let side = PortRef::new("sideroom", "out").unwrap();
        let group = NodeFreezeGroup::capture(&snapshot, &[side.clone(), room.clone()]).unwrap();
        assert_eq!(
            group
                .leaves()
                .iter()
                .map(NodeFreezeSnapshot::boundary)
                .collect::<Vec<_>>(),
            vec![room.clone(), side.clone()]
        );
        assert_eq!(group.info().channels, 2);
        assert_eq!(group.info().sample_rate_hz, 48_000);
        assert!(group.info().frames > 0);
        for leaf in group.leaves() {
            let single = NodeFreezeSnapshot::capture(&snapshot, &leaf.boundary()).unwrap();
            assert_eq!(leaf.digest(), single.digest());
            assert_eq!(leaf.output_digest(), single.output_digest());
        }
        assert_eq!(group.replay_matches(&snapshot).unwrap(), vec![true, true]);
        let verified = group
            .leaves()
            .iter()
            .rev()
            .map(|leaf| {
                let directory = tempfile::tempdir().unwrap();
                leaf.stage_into(directory.path()).unwrap();
                let root = ProjectRoot::open_pinned(directory.path(), cap_std::ambient_authority())
                    .unwrap();
                let preflight =
                    NodeFreezeSnapshot::preflight(&root, &leaf.digest(), &snapshot.digest())
                        .unwrap();
                NodeFreezeSnapshot::verify_preflighted(&root, &preflight).unwrap()
            })
            .collect();
        let restored = NodeFreezeGroup::from_verified(verified).unwrap();
        assert_eq!(
            restored
                .leaves()
                .iter()
                .map(NodeFreezeSnapshot::boundary)
                .collect::<Vec<_>>(),
            vec![room, side]
        );
        assert!(restored.matches_snapshot(&snapshot).unwrap());
        let (_staged, plan) = staged_plan(&snapshot).unwrap();
        let mut normal = Vec::new();
        plan.render(|frame| {
            normal.extend(frame.iter().map(|sample| sample.to_bits()));
            Ok(())
        })
        .unwrap();
        let mut replaced = Vec::new();
        restored
            .render_replacements(&snapshot, |frame| {
                replaced.extend(frame.iter().map(|sample| sample.to_bits()));
                Ok(())
            })
            .unwrap();
        assert_eq!(replaced, normal);
    }

    #[test]
    fn group_partial_replacement_uses_one_verified_leaf_and_spools_output() {
        let (project, snapshot, room) = project_with_source(&independent_effects_source());
        let side = PortRef::new("sideroom", "out").unwrap();
        let group = NodeFreezeGroup::capture(&snapshot, &[room, side]).unwrap();
        let source = fs::read_to_string(project.path().join("main.maac")).unwrap();
        fs::write(
            project.path().join("main.maac"),
            source.replace("gain=1/3;", "gain=1/4;"),
        )
        .unwrap();
        let candidate =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        assert_eq!(
            group.candidate_eligibility(&candidate).unwrap(),
            (true, vec![true, false])
        );
        let (_staged, plan) = staged_plan(&candidate).unwrap();
        let mut normal = Vec::new();
        plan.render(|frame| {
            normal.extend(frame.iter().map(|sample| sample.to_bits()));
            Ok(())
        })
        .unwrap();
        let mut replaced = Vec::new();
        group
            .render_selected_replacements(&candidate, &[true, false], |frame| {
                replaced.extend(frame.iter().map(|sample| sample.to_bits()));
                Ok(())
            })
            .unwrap();
        assert_eq!(replaced, normal);

        let mut output = group.leaves()[0].output.lock().unwrap();
        output.seek(SeekFrom::Start(0)).unwrap();
        output.write_all(&0.25f64.to_le_bytes()).unwrap();
        drop(output);
        let mut called = false;
        assert!(group
            .render_selected_replacements(&candidate, &[true, false], |_| {
                called = true;
                Ok(())
            })
            .is_err());
        assert!(!called);
    }

    #[test]
    fn group_partial_replacement_uses_mixed_width_canonical_subset() {
        let (project, snapshot, room) = project_with_source(&three_independent_effects_source());
        let side = PortRef::new("sideroom", "out").unwrap();
        let third = PortRef::new("thirdfx", "out").unwrap();
        let group = NodeFreezeGroup::capture(&snapshot, &[third, side, room]).unwrap();
        assert_eq!(
            group
                .leaves()
                .iter()
                .map(|leaf| leaf.manifest.channels)
                .collect::<Vec<_>>(),
            vec![1, 1, 2]
        );
        let source = fs::read_to_string(project.path().join("main.maac")).unwrap();
        fs::write(
            project.path().join("main.maac"),
            source.replace("gain=1/3;", "gain=1/4;"),
        )
        .unwrap();
        let candidate =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        let selected = [true, false, true];
        assert_eq!(
            group.candidate_eligibility(&candidate).unwrap(),
            (true, selected.to_vec())
        );
        let (_staged, plan) = staged_plan(&candidate).unwrap();
        let mut normal = Vec::new();
        plan.render(|frame| {
            normal.extend(frame.iter().map(|sample| sample.to_bits()));
            Ok(())
        })
        .unwrap();
        let mut replaced = Vec::new();
        group
            .render_selected_replacements(&candidate, &selected, |frame| {
                replaced.extend(frame.iter().map(|sample| sample.to_bits()));
                Ok(())
            })
            .unwrap();
        assert_eq!(replaced, normal);
    }

    #[test]
    fn group_integrity_survives_a_new_executable_identity() {
        let (_project, snapshot, room) = project_with_source(&independent_effects_source());
        let side = PortRef::new("sideroom", "out").unwrap();
        let mut group = NodeFreezeGroup::capture(&snapshot, &[room, side]).unwrap();
        for leaf in &mut group.leaves {
            leaf.manifest.engine = sha256_digest(b"a different executable");
            leaf.manifest.render_key = render_key(&leaf.manifest).unwrap();
            leaf.json = serde_json::to_vec(&leaf.manifest).unwrap();
        }
        assert!(group.matches_snapshot(&snapshot).unwrap());
        assert!(group.render_replacements(&snapshot, |_| Ok(())).is_err());
    }

    #[test]
    fn group_rejects_duplicates_and_corrupt_private_stream_before_callback() {
        let (_project, snapshot, room) = project_with_source(&independent_effects_source());
        let side = PortRef::new("sideroom", "out").unwrap();
        assert!(NodeFreezeGroup::capture(&snapshot, std::slice::from_ref(&room)).is_err());
        assert!(NodeFreezeGroup::capture(&snapshot, &[room.clone(), room.clone()]).is_err());
        let group = NodeFreezeGroup::capture(&snapshot, &[room, side]).unwrap();
        {
            let mut output = group.leaves()[1].output.lock().unwrap();
            output.seek(SeekFrom::Start(0)).unwrap();
            output.write_all(&0.25f64.to_le_bytes()).unwrap();
        }
        let mut called = 0usize;
        assert!(group
            .render_replacements(&snapshot, |_| {
                called += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(called, 0);
    }

    #[test]
    fn group_rejects_effects_with_a_signal_path_between_them() {
        let source = SOURCE
            .replace(
                "node gain {",
                "node after_room { type=\"fx.reverb/1\"; config={channels=1;}; params={decay=1/3s;mix=0.25;}; }\nnode gain {",
            )
            .replace(
                "connect b { from=&room:out; to=&gain:in; }",
                "connect b { from=&room:out; to=&after_room:in; }\nconnect after { from=&after_room:out; to=&gain:in; }",
            );
        let (_project, snapshot, room) = project_with_source(&source);
        let downstream = PortRef::new("after_room", "out").unwrap();
        assert!(NodeFreezeGroup::capture(&snapshot, &[room, downstream]).is_err());
    }

    #[test]
    fn v3_allows_downstream_numeric_params_and_renders_candidate() {
        let (project, snapshot, boundary) = project();
        let freeze = NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap();
        assert_eq!(freeze.manifest.version, 3);
        let edited = SOURCE
            .replace("gain=3/4", "gain=1/2")
            .replace("pan=0.2", "pan=-0.1");
        fs::write(project.path().join("main.maac"), &edited).unwrap();
        let candidate =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        assert_ne!(candidate.digest(), snapshot.digest());
        assert!(freeze.matches_snapshot(&candidate).unwrap());
        let (_staged, plan) = staged_plan(&candidate).unwrap();
        let mut normal = Vec::new();
        plan.render(|frame| {
            normal.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
        let mut reused = Vec::new();
        freeze
            .render_replacement(&candidate, |frame| {
                reused.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            normal.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            reused.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        for stale in [
            SOURCE.replace("mix=0.4", "mix=0.3"),
            SOURCE.replace("level=0.2", "level=0.1"),
            format!("{SOURCE}// unrelated byte\n"),
        ] {
            fs::write(project.path().join("main.maac"), stale).unwrap();
            let candidate =
                ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default")
                    .unwrap();
            assert!(!freeze.matches_snapshot(&candidate).unwrap());
        }
    }

    #[test]
    fn v3_reuses_sibling_gain_and_pan_and_renders_candidate() {
        let source = sibling_source();
        let (project, snapshot, boundary) = project_with_source(&source);
        let freeze = NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap();
        assert_eq!(freeze.manifest.version, 3);
        let edited = source
            .replace("gain=1/3", "gain=2/3")
            .replace("pan=-0.3", "pan=0.4");
        fs::write(project.path().join("main.maac"), edited).unwrap();
        let candidate =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        assert!(freeze.matches_snapshot(&candidate).unwrap());
        let (_staged, plan) = staged_plan(&candidate).unwrap();
        let mut normal = Vec::new();
        plan.render(|frame| {
            normal.extend_from_slice(frame);
            Ok(())
        })
        .unwrap();
        let mut reused = Vec::new();
        freeze
            .render_replacement(&candidate, |frame| {
                reused.extend_from_slice(frame);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            normal.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            reused.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn v2_rejects_sibling_edits_but_accepts_downstream_edits() {
        let source = sibling_source();
        let (project, snapshot, boundary) = project_with_source(&source);
        let freeze = as_v2(
            NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap(),
            &snapshot,
            &boundary,
        );
        assert_eq!(freeze.manifest.version, 2);
        fs::write(
            project.path().join("main.maac"),
            source.replace("gain=1/3", "gain=2/3"),
        )
        .unwrap();
        let sibling =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        assert!(!freeze.matches_snapshot(&sibling).unwrap());
        fs::write(
            project.path().join("main.maac"),
            source
                .replace("gain=3/4", "gain=1/2")
                .replace("pan=0.2", "pan=-0.1"),
        )
        .unwrap();
        let downstream =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        assert!(freeze.matches_snapshot(&downstream).unwrap());
    }

    #[test]
    fn delayed_feedback_excludes_downstream_gain() {
        let source = SOURCE
            .replace(
                "node room {",
                "node sum { type=\"core.sum/1\"; config={channels=1;}; }\nnode delay { type=\"core.delay/1\"; config={channels=1;frames=1;}; }\nnode room {",
            )
            .replace(
                "connect a { from=&tone:out; to=&room:in; }",
                "connect a { from=&tone:out; to=&sum:in; }\nconnect feedback { from=&gain:out; to=&delay:in; }\nconnect delayed { from=&delay:out; to=&sum:in; }\nconnect before_room { from=&sum:out; to=&room:in; }",
            );
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("main.maac"), &source).unwrap();
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        let (_staged, plan) = staged_plan(&snapshot).unwrap();
        let boundary = PortRef::new("room", "out").unwrap();
        let allowed = plan.selective_node_params(&boundary);
        assert!(!allowed.contains_key("gain"));
        assert_eq!(allowed.get("pan"), Some(&"pan"));
        let allowed_v3 = plan.selective_node_params_v3(&boundary);
        assert!(!allowed_v3.contains_key("gain"));
        assert_eq!(allowed_v3.get("pan"), Some(&"pan"));
        let freeze = NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap();
        fs::write(
            project.path().join("main.maac"),
            source.replace("gain=3/4", "gain=1/2"),
        )
        .unwrap();
        let feedback_edit =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        assert!(!freeze.matches_snapshot(&feedback_edit).unwrap());
    }

    #[test]
    fn compressor_sidechain_gain_cannot_change_under_v3_freeze() {
        let source = SOURCE
            .replace(
                "node room { type=\"fx.reverb/1\"; config={channels=1;}; params={decay=1/2s;mix=0.4;}; }",
                "node room { type=\"fx.compressor/1\"; config={channels=1;detector=external;sidechain_channels=1;}; params={threshold=-30dB;ratio=2;attack=0s;release=0s;}; }\nnode detector_gain { type=\"core.gain/1\"; config={channels=1;}; params={gain=1/3;}; }",
            )
            .replace(
                "connect a { from=&tone:out; to=&room:in; }",
                "connect a { from=&tone:out; to=&room:in; }\nconnect detector_input { from=&tone:out; to=&detector_gain:in; }\nconnect sidechain { from=&detector_gain:out; to=&room:sidechain; }",
            );
        let (project, snapshot, boundary) = project_with_source(&source);
        let (_staged, plan) = staged_plan(&snapshot).unwrap();
        assert!(!plan
            .selective_node_params_v3(&boundary)
            .contains_key("detector_gain"));
        let freeze = NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap();
        fs::write(
            project.path().join("main.maac"),
            source.replace("gain=1/3", "gain=2/3"),
        )
        .unwrap();
        let edited =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        assert!(!freeze.matches_snapshot(&edited).unwrap());
    }

    #[test]
    fn v1_manifest_keeps_exact_source_policy() {
        let (project, snapshot, boundary) = project();
        let mut freeze = NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap();
        freeze.manifest.version = 1;
        freeze.manifest.reuse_identity = None;
        freeze.manifest.render_key = render_key(&freeze.manifest).unwrap();
        freeze.json = serde_json::to_vec(&freeze.manifest).unwrap();
        let directory = tempfile::tempdir().unwrap();
        freeze.stage_into(directory.path()).unwrap();
        let root =
            ProjectRoot::open_pinned(directory.path(), cap_std::ambient_authority()).unwrap();
        let preflight =
            NodeFreezeSnapshot::preflight(&root, &freeze.digest(), &snapshot.digest()).unwrap();
        let verified = NodeFreezeSnapshot::verify_preflighted(&root, &preflight).unwrap();
        assert!(verified.matches_snapshot(&snapshot).unwrap());
        fs::write(
            project.path().join("main.maac"),
            SOURCE.replace("gain=3/4", "gain=1/2"),
        )
        .unwrap();
        let edited =
            ArchiveSnapshot::capture(Path::new("main.maac"), project.path(), "default").unwrap();
        assert!(!verified.matches_snapshot(&edited).unwrap());
    }

    #[test]
    fn v3_manifest_identity_is_recomputed_from_archived_source() {
        let (_project, snapshot, boundary) = project();
        let mut freeze = NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap();
        freeze.manifest.reuse_identity = Some(sha256_digest(b"forged identity"));
        freeze.manifest.render_key = render_key(&freeze.manifest).unwrap();
        freeze.json = serde_json::to_vec(&freeze.manifest).unwrap();
        assert!(!freeze.matches_snapshot(&snapshot).unwrap());
    }

    #[test]
    fn v2_manifest_identity_is_recomputed_from_archived_source() {
        let (_project, snapshot, boundary) = project();
        let mut freeze = as_v2(
            NodeFreezeSnapshot::capture(&snapshot, &boundary).unwrap(),
            &snapshot,
            &boundary,
        );
        freeze.manifest.reuse_identity = Some(sha256_digest(b"forged identity"));
        freeze.manifest.render_key = render_key(&freeze.manifest).unwrap();
        freeze.json = serde_json::to_vec(&freeze.manifest).unwrap();
        assert!(!freeze.matches_snapshot(&snapshot).unwrap());
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
