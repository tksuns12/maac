//! Inactive, whole-output WAV freezes bound to an editable archive snapshot.

use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

use cap_std::fs::OpenOptions as CapOpenOptions;
use hound::{SampleFormat, WavReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::archive::ArchiveSnapshot;
use crate::bundle::{sha256_digest, validate_hash_pin};
use crate::bundle_fs::ProjectRoot;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Diagnostics};
use crate::disk_media::DiskMediaProject;
use crate::exact::parse_rational;
use crate::export::{render_wav_to_path_disk_media, WavFormat};
use crate::plan::{OutputSettings, PlanLimits};

pub(crate) const FREEZE_MANIFEST: &str = "maac-freeze.json";
pub(crate) const FREEZE_OUTPUT: &str = "output.wav";
pub(crate) const MAX_FREEZE_MANIFEST_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_FREEZE_OUTPUT_BYTES: u64 = 1024 * 1024 * 1024;
const FORMAT: &str = "maac.output-freeze";
const VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FreezeManifest {
    format: String,
    version: u32,
    snapshot: String,
    engine: String,
    profile: String,
    source_graph_active: bool,
    freeze_active: bool,
    boundary: GraphBoundary,
    state: ResetState,
    score_start_q: String,
    score_end_q: String,
    tail_seconds: String,
    /// Whole-output export applies no offset compensation. Delay processors
    /// in the active source graph still affect their own signal paths.
    output_latency_offset_frames: u64,
    latency_policy: String,
    frame_start: u64,
    frame_end: u64,
    sample_rate_hz: u32,
    channels: u8,
    wav_format: String,
    render_key: String,
    output: FrozenFile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphBoundary {
    kind: String,
    node: String,
    port: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetState {
    mode: String,
    origin_frame: u64,
    score_origin_q: String,
}

#[derive(Serialize)]
struct RenderKeyInput<'a> {
    format: &'static str,
    version: u32,
    snapshot: &'a str,
    engine: &'a str,
    profile: &'a str,
    boundary: &'a GraphBoundary,
    state: &'a ResetState,
    score_start_q: &'a str,
    score_end_q: &'a str,
    tail_seconds: &'a str,
    latency_policy: &'a str,
    output_latency_offset_frames: u64,
    frame_start: u64,
    frame_end: u64,
    sample_rate_hz: u32,
    channels: u8,
    wav_format: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenFile {
    path: String,
    bytes: u64,
    sha256: String,
    pcm_sha256: String,
}

pub(crate) struct FreezePreflight {
    manifest: FreezeManifest,
    json: Vec<u8>,
}

impl FreezePreflight {
    pub(crate) fn output_bytes(&self) -> u64 {
        self.manifest.output.bytes
    }
}

#[derive(Clone)]
pub(crate) struct FreezeSnapshot {
    manifest: FreezeManifest,
    json: Vec<u8>,
    output: Arc<File>,
}

impl FreezeSnapshot {
    pub(crate) fn capture(snapshot: &ArchiveSnapshot) -> Result<Self, Diagnostics> {
        let staged = tempfile::tempdir().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private freeze staging: {e}"),
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
                    "unsupported frozen archive profile",
                ))
            }
        };
        let plan = project.build_with_limits(&limits)?;
        let output_settings = plan.output().clone();
        preflight_output_size(&output_settings)?;
        let rendered = tempfile::tempdir().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private render staging: {e}"),
            )
        })?;
        let output_path = rendered.path().join(FREEZE_OUTPUT);
        render_wav_to_path_disk_media(&plan, &output_path, WavFormat::Float32, false).map_err(
            |e| {
                fail(
                    DiagnosticCode::RenderState,
                    format!("cannot render freeze: {e}"),
                )
            },
        )?;
        let file = File::open(&output_path).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot open private freeze output: {e}"),
            )
        })?;
        let (output, bytes, hash) = snapshot_file(file, MAX_FREEZE_OUTPUT_BYTES, "freeze output")?;
        let mut manifest = manifest_for(snapshot, &output_settings, bytes, hash, engine_digest()?);
        manifest.render_key = render_key(&manifest)?;
        manifest.output.pcm_sha256 = check_wav(&output, &manifest)?;
        validate_manifest(&manifest)?;
        let json = serde_json::to_vec(&manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode freeze manifest: {e}"),
            )
        })?;
        if json.len() as u64 > MAX_FREEZE_MANIFEST_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "freeze manifest exceeds 1 MiB",
            ));
        }
        Ok(Self {
            manifest,
            json,
            output: Arc::new(output),
        })
    }

    pub(crate) fn preflight(
        root: &ProjectRoot,
        digest: &str,
        snapshot: &str,
    ) -> Result<FreezePreflight, Diagnostics> {
        validate_hash_pin(digest, "freeze digest")?;
        check_tree(root)?;
        let json = read_small_file(root, FREEZE_MANIFEST, MAX_FREEZE_MANIFEST_BYTES)?;
        let manifest: FreezeManifest = serde_json::from_slice(&json).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("invalid freeze manifest: {e}"),
            )
        })?;
        validate_manifest(&manifest)?;
        if serde_json::to_vec(&manifest).map_err(|e| {
            fail(
                DiagnosticCode::Syntax,
                format!("cannot encode freeze manifest: {e}"),
            )
        })? != json
        {
            return Err(fail(
                DiagnosticCode::Syntax,
                "freeze manifest is not canonical JSON",
            ));
        }
        if sha256_digest(&json) != digest || manifest.snapshot != snapshot {
            return Err(fail(
                DiagnosticCode::Hash,
                "freeze identity differs from its checkpoint or source snapshot",
            ));
        }
        let metadata = root.dir().symlink_metadata(FREEZE_OUTPUT).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect freeze output: {e}"),
            )
        })?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != manifest.output.bytes
        {
            return Err(fail(
                DiagnosticCode::Hash,
                "freeze output file metadata differs from manifest",
            ));
        }
        Ok(FreezePreflight { manifest, json })
    }

    pub(crate) fn verify_preflighted(
        root: &ProjectRoot,
        preflight: &FreezePreflight,
    ) -> Result<Self, Diagnostics> {
        check_tree(root)?;
        let file = open_regular(root, FREEZE_OUTPUT)?;
        let (output, bytes, hash) = snapshot_file(file, MAX_FREEZE_OUTPUT_BYTES, "freeze output")?;
        if bytes != preflight.manifest.output.bytes || hash != preflight.manifest.output.sha256 {
            return Err(fail(
                DiagnosticCode::Hash,
                "freeze output bytes or hash differ from manifest",
            ));
        }
        if check_wav(&output, &preflight.manifest)? != preflight.manifest.output.pcm_sha256 {
            return Err(fail(
                DiagnosticCode::Hash,
                "freeze decoded PCM hash differs from manifest",
            ));
        }
        if read_small_file(root, FREEZE_MANIFEST, MAX_FREEZE_MANIFEST_BYTES)? != preflight.json {
            return Err(fail(
                DiagnosticCode::Hash,
                "freeze manifest changed during verification",
            ));
        }
        Ok(Self {
            manifest: preflight.manifest.clone(),
            json: preflight.json.clone(),
            output: Arc::new(output),
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
    pub(crate) fn render_key(&self) -> &str {
        &self.manifest.render_key
    }

    pub(crate) fn candidate_render_key(
        snapshot: &ArchiveSnapshot,
        engine: &str,
    ) -> Result<String, Diagnostics> {
        let staged = tempfile::tempdir().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot create private freeze staging: {e}"),
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
                    "unsupported frozen archive profile",
                ))
            }
        };
        let plan = project.build_with_limits(&limits)?;
        let candidate = manifest_for(snapshot, plan.output(), 0, String::new(), engine.into());
        render_key(&candidate)
    }

    /// Stage into a new, real freeze directory. Both files use create_new.
    pub(crate) fn stage_into(&self, dir: &Path) -> Result<(), Diagnostics> {
        let meta = fs::symlink_metadata(dir).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect freeze destination: {e}"),
            )
        })?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(fail(
                DiagnosticCode::Reference,
                "freeze destination must be a real directory",
            ));
        }
        let mut manifest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(FREEZE_MANIFEST))
            .map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot create freeze manifest: {e}"),
                )
            })?;
        manifest.write_all(&self.json).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot write freeze manifest: {e}"),
            )
        })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(FREEZE_OUTPUT))
            .map_err(|e| {
                fail(
                    DiagnosticCode::Reference,
                    format!("cannot create freeze output: {e}"),
                )
            })?;
        let mut source = self.output.as_ref().try_clone().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot clone private freeze output: {e}"),
            )
        })?;
        source.seek(SeekFrom::Start(0)).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot rewind freeze output: {e}"),
            )
        })?;
        let (bytes, hash) = copy_hash(&mut source, &mut output, MAX_FREEZE_OUTPUT_BYTES)?;
        if bytes != self.manifest.output.bytes || hash != self.manifest.output.sha256 {
            return Err(fail(
                DiagnosticCode::Hash,
                "private freeze output changed before staging",
            ));
        }
        Ok(())
    }
}

fn manifest_for(
    snapshot: &ArchiveSnapshot,
    output: &OutputSettings,
    bytes: u64,
    hash: String,
    engine: String,
) -> FreezeManifest {
    FreezeManifest {
        format: FORMAT.into(),
        version: VERSION,
        snapshot: snapshot.digest(),
        engine,
        profile: snapshot.profile().into(),
        source_graph_active: true,
        freeze_active: false,
        boundary: GraphBoundary {
            kind: "project_output".into(),
            node: output.output.node.clone(),
            port: output.output.port.clone(),
        },
        state: ResetState {
            mode: "reset".into(),
            origin_frame: 0,
            score_origin_q: output.score_start_q.to_string(),
        },
        score_start_q: output.score_start_q.to_string(),
        score_end_q: output.score_end_q.to_string(),
        tail_seconds: output.tail_seconds.to_string(),
        output_latency_offset_frames: 0,
        latency_policy: "no_output_compensation".into(),
        frame_start: 0,
        frame_end: output.total_frames,
        sample_rate_hz: output.sample_rate_hz,
        channels: output.channels,
        wav_format: "float32".into(),
        render_key: String::new(),
        output: FrozenFile {
            path: FREEZE_OUTPUT.into(),
            bytes,
            sha256: hash,
            pcm_sha256: String::new(),
        },
    }
}

fn preflight_output_size(output: &OutputSettings) -> Result<(), Diagnostics> {
    // Hound's float32 RIFF header is small; reserve 64 KiB rather than depend
    // on a crate-specific header layout before spending time rendering.
    const HEADER_RESERVE: u64 = 64 * 1024;
    let samples = output
        .total_frames
        .checked_mul(u64::from(output.channels))
        .ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "freeze sample count overflow",
            )
        })?;
    let data = samples
        .checked_mul(4)
        .ok_or_else(|| fail(DiagnosticCode::ResourceLimit, "freeze output size overflow"))?;
    if data
        .checked_add(HEADER_RESERVE)
        .is_none_or(|bytes| bytes > MAX_FREEZE_OUTPUT_BYTES || bytes > u64::from(u32::MAX))
    {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "predicted float32 freeze WAV exceeds 1 GiB or RIFF length limit",
        ));
    }
    Ok(())
}

fn render_key(m: &FreezeManifest) -> Result<String, Diagnostics> {
    let input = RenderKeyInput {
        format: "maac.freeze-render-key",
        version: 1,
        snapshot: &m.snapshot,
        engine: &m.engine,
        profile: &m.profile,
        boundary: &m.boundary,
        state: &m.state,
        score_start_q: &m.score_start_q,
        score_end_q: &m.score_end_q,
        tail_seconds: &m.tail_seconds,
        latency_policy: &m.latency_policy,
        output_latency_offset_frames: m.output_latency_offset_frames,
        frame_start: m.frame_start,
        frame_end: m.frame_end,
        sample_rate_hz: m.sample_rate_hz,
        channels: m.channels,
        wav_format: &m.wav_format,
    };
    Ok(sha256_digest(&serde_json::to_vec(&input).map_err(|e| {
        fail(
            DiagnosticCode::Syntax,
            format!("cannot encode freeze render key: {e}"),
        )
    })?))
}

fn validate_manifest(m: &FreezeManifest) -> Result<(), Diagnostics> {
    if m.format != FORMAT || m.version != VERSION {
        return Err(fail(
            DiagnosticCode::Version,
            "unsupported freeze format or version",
        ));
    }
    validate_hash_pin(&m.snapshot, "freeze source snapshot")?;
    validate_hash_pin(&m.engine, "freeze engine")?;
    validate_hash_pin(&m.output.sha256, "freeze output")?;
    validate_hash_pin(&m.output.pcm_sha256, "freeze decoded PCM")?;
    validate_hash_pin(&m.render_key, "freeze render key")?;
    if !matches!(m.profile.as_str(), "default" | "song")
        || !m.source_graph_active
        || m.freeze_active
        || m.boundary.kind != "project_output"
        || m.boundary.node.is_empty()
        || m.boundary.port.is_empty()
        || m.state.mode != "reset"
        || m.state.origin_frame != 0
        || m.state.score_origin_q != m.score_start_q
        || m.latency_policy != "no_output_compensation"
        || m.output_latency_offset_frames != 0
        || m.frame_start != 0
        || m.frame_end == 0
        || m.sample_rate_hz == 0
        || !matches!(m.channels, 1 | 2)
        || m.wav_format != "float32"
        || m.output.path != FREEZE_OUTPUT
        || m.output.bytes == 0
        || m.output.bytes > MAX_FREEZE_OUTPUT_BYTES
        || m.frame_end > MAX_FREEZE_OUTPUT_BYTES / 4 / u64::from(m.channels.max(1))
    {
        return Err(fail(
            DiagnosticCode::Syntax,
            "invalid whole-output freeze metadata",
        ));
    }
    let start = parse_rational(&m.score_start_q)
        .map_err(|_| fail(DiagnosticCode::Syntax, "invalid freeze score start"))?;
    let end = parse_rational(&m.score_end_q)
        .map_err(|_| fail(DiagnosticCode::Syntax, "invalid freeze score end"))?;
    let tail = parse_rational(&m.tail_seconds)
        .map_err(|_| fail(DiagnosticCode::Syntax, "invalid freeze tail"))?;
    if start.to_string() != m.score_start_q
        || end.to_string() != m.score_end_q
        || tail.to_string() != m.tail_seconds
        || end <= start
        || tail < num_rational::BigRational::from_integer(0.into())
    {
        return Err(fail(
            DiagnosticCode::Syntax,
            "invalid freeze score interval or tail",
        ));
    }
    if crate::plan::PortRef::new(&m.boundary.node, &m.boundary.port).is_err()
        || render_key(m)? != m.render_key
    {
        return Err(fail(
            DiagnosticCode::Hash,
            "freeze render key or graph boundary is invalid",
        ));
    }
    Ok(())
}

fn check_wav(file: &File, m: &FreezeManifest) -> Result<String, Diagnostics> {
    let source = file.try_clone().map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot clone freeze output: {e}"),
        )
    })?;
    let mut reader = WavReader::new(BufReader::new(source))
        .map_err(|e| fail(DiagnosticCode::Asset, format!("invalid freeze WAV: {e}")))?;
    let spec = reader.spec();
    if spec.sample_format != SampleFormat::Float
        || spec.bits_per_sample != 32
        || spec.channels != u16::from(m.channels)
        || spec.sample_rate != m.sample_rate_hz
        || u64::from(reader.duration()) != m.frame_end
    {
        return Err(fail(
            DiagnosticCode::Asset,
            "freeze WAV shape differs from manifest",
        ));
    }
    let mut samples = 0u64;
    let mut pcm = Sha256::new();
    for sample in reader.samples::<f32>() {
        let sample = sample.map_err(|e| {
            fail(
                DiagnosticCode::Asset,
                format!("invalid freeze WAV sample: {e}"),
            )
        })?;
        if !sample.is_finite() {
            return Err(fail(
                DiagnosticCode::Nonfinite,
                "freeze WAV contains nonfinite PCM",
            ));
        }
        pcm.update(sample.to_le_bytes());
        samples = samples.checked_add(1).ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "freeze WAV sample count overflow",
            )
        })?;
    }
    if samples != m.frame_end * u64::from(m.channels) {
        return Err(fail(
            DiagnosticCode::Asset,
            "freeze WAV sample count differs from manifest",
        ));
    }
    Ok(format!("sha256:{:x}", pcm.finalize()))
}

fn check_tree(root: &ProjectRoot) -> Result<(), Diagnostics> {
    let mut names = std::collections::BTreeSet::new();
    for entry in root.dir().read_dir(".").map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot list freeze directory: {e}"),
        )
    })? {
        let entry = entry.map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot list freeze directory: {e}"),
            )
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| fail(DiagnosticCode::Reference, "non-UTF-8 freeze member"))?;
        let kind = entry.file_type().map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect freeze member: {e}"),
            )
        })?;
        if !kind.is_file()
            || kind.is_symlink()
            || !matches!(name.as_str(), FREEZE_MANIFEST | FREEZE_OUTPUT)
        {
            return Err(fail(
                DiagnosticCode::Reference,
                format!("unexpected freeze member `{name}`"),
            ));
        }
        names.insert(name);
    }
    if names != std::collections::BTreeSet::from([FREEZE_MANIFEST.into(), FREEZE_OUTPUT.into()]) {
        return Err(fail(
            DiagnosticCode::Reference,
            "freeze directory is missing required files",
        ));
    }
    Ok(())
}

fn open_regular(root: &ProjectRoot, path: &str) -> Result<File, Diagnostics> {
    let meta = root.dir().symlink_metadata(path).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot inspect freeze member `{path}`: {e}"),
        )
    })?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(fail(
            DiagnosticCode::Reference,
            format!("freeze member `{path}` is not a regular file"),
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
                format!("cannot open freeze member `{path}`: {e}"),
            )
        })?
        .into_std();
    if !file
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect opened freeze member: {e}"),
            )
        })?
        .is_file()
    {
        return Err(fail(
            DiagnosticCode::Reference,
            "opened freeze member is not a regular file",
        ));
    }
    Ok(file)
}

fn read_small_file(root: &ProjectRoot, path: &str, max: u64) -> Result<Vec<u8>, Diagnostics> {
    let file = open_regular(root, path)?;
    if file
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect freeze member: {e}"),
            )
        })?
        .len()
        > max
    {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "freeze manifest exceeds 1 MiB",
        ));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot read freeze manifest: {e}"),
        )
    })?;
    if bytes.len() as u64 > max {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "freeze manifest exceeds 1 MiB",
        ));
    }
    Ok(bytes)
}

fn snapshot_file(
    mut source: File,
    max: u64,
    label: &str,
) -> Result<(File, u64, String), Diagnostics> {
    if source
        .metadata()
        .map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot inspect {label}: {e}"),
            )
        })?
        .len()
        > max
    {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            format!("{label} exceeds byte limit"),
        ));
    }
    source.seek(SeekFrom::Start(0)).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot rewind {label}: {e}"),
        )
    })?;
    let mut private = tempfile::tempfile().map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot create private {label}: {e}"),
        )
    })?;
    let (bytes, hash) = copy_hash(&mut source, &mut private, max)?;
    private.seek(SeekFrom::Start(0)).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot rewind private {label}: {e}"),
        )
    })?;
    Ok((private, bytes, hash))
}

fn copy_hash(source: &mut File, dest: &mut File, max: u64) -> Result<(u64, String), Diagnostics> {
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut block = [0u8; 64 * 1024];
    loop {
        let count = source.read(&mut block).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot read freeze output: {e}"),
            )
        })?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "freeze output length overflow",
            )
        })?;
        if total > max {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "freeze output exceeds byte limit",
            ));
        }
        digest.update(&block[..count]);
        dest.write_all(&block[..count]).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot write freeze output: {e}"),
            )
        })?;
    }
    Ok((total, format!("sha256:{:x}", digest.finalize())))
}

/// Bound the executable identity read independently of archive byte limits.
pub(crate) fn engine_digest() -> Result<String, Diagnostics> {
    const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
    let path = std::env::current_exe().map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot locate rendering executable: {e}"),
        )
    })?;
    let mut file = File::open(&path).map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot open rendering executable: {e}"),
        )
    })?;
    let metadata = file.metadata().map_err(|e| {
        fail(
            DiagnosticCode::Reference,
            format!("cannot inspect rendering executable: {e}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(fail(
            DiagnosticCode::Reference,
            "rendering executable is not a regular file",
        ));
    }
    if metadata.len() > MAX_EXECUTABLE_BYTES {
        return Err(fail(
            DiagnosticCode::ResourceLimit,
            "rendering executable exceeds 256 MiB identity limit",
        ));
    }
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut block = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut block).map_err(|e| {
            fail(
                DiagnosticCode::Reference,
                format!("cannot read rendering executable: {e}"),
            )
        })?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(|| {
            fail(
                DiagnosticCode::ResourceLimit,
                "rendering executable length overflow",
            )
        })?;
        if total > MAX_EXECUTABLE_BYTES {
            return Err(fail(
                DiagnosticCode::ResourceLimit,
                "rendering executable exceeds 256 MiB identity limit",
            ));
        }
        digest.update(&block[..count]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
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

    #[test]
    fn output_pcm_hash_is_checked_independently_of_file_hash() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("main.maac"), SOURCE).unwrap();
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), &project, "default").unwrap();
        let freeze = FreezeSnapshot::capture(&snapshot).unwrap();
        let dir = temp.path().join("freeze");
        fs::create_dir(&dir).unwrap();
        freeze.stage_into(&dir).unwrap();
        let mut output = fs::read(dir.join(FREEZE_OUTPUT)).unwrap();
        let last_sample = output.len() - 4;
        output[last_sample] ^= 1;
        fs::write(dir.join(FREEZE_OUTPUT), &output).unwrap();
        let mut manifest = freeze.manifest.clone();
        manifest.output.sha256 = sha256_digest(&output);
        let json = serde_json::to_vec(&manifest).unwrap();
        fs::write(dir.join(FREEZE_MANIFEST), &json).unwrap();
        let root = ProjectRoot::open_pinned(&dir, cap_std::ambient_authority()).unwrap();
        let preflight =
            FreezeSnapshot::preflight(&root, &sha256_digest(&json), &snapshot.digest()).unwrap();
        assert!(FreezeSnapshot::verify_preflighted(&root, &preflight).is_err());
    }

    #[test]
    fn freeze_manifest_rejects_noncanonical_and_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("main.maac"), SOURCE).unwrap();
        let snapshot =
            ArchiveSnapshot::capture(Path::new("main.maac"), &project, "default").unwrap();
        let freeze = FreezeSnapshot::capture(&snapshot).unwrap();
        let dir = temp.path().join("freeze");
        fs::create_dir(&dir).unwrap();
        freeze.stage_into(&dir).unwrap();
        let root = ProjectRoot::open_pinned(&dir, cap_std::ambient_authority()).unwrap();
        let pretty = serde_json::to_vec_pretty(&freeze.manifest).unwrap();
        fs::write(dir.join(FREEZE_MANIFEST), &pretty).unwrap();
        assert!(
            FreezeSnapshot::preflight(&root, &sha256_digest(&pretty), &snapshot.digest()).is_err()
        );
        let mut value: serde_json::Value = serde_json::from_slice(&freeze.json).unwrap();
        value["surprise"] = true.into();
        let unknown = serde_json::to_vec(&value).unwrap();
        fs::write(dir.join(FREEZE_MANIFEST), &unknown).unwrap();
        assert!(
            FreezeSnapshot::preflight(&root, &sha256_digest(&unknown), &snapshot.digest()).is_err()
        );
    }
}
