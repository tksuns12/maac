//! Named deliveries: one complete graph execution, bounded disk spools, final
//! artifact analysis, and atomic publication of each file independently.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use num_traits::ToPrimitive;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::dsp::{DspEngine, RenderError};
use crate::exact::Rational;
use crate::plan::{Plan, PlanLimits, PlanView, PortRef};
use crate::plan_v3::{PlanV3, TimingContext, VersionedPlan};
use crate::production_analysis::{self as analysis, Analysis, AnalyzerLimits};
use crate::production_convert::{self as convert, ConversionError, ConversionLimits, Converter};
use crate::production_data::{self as data, Target};
use crate::PlanArtifact;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct DeliveryLimits {
    pub max_targets: usize,
    pub max_spool_bytes: u64,
    pub max_disk_bytes: u64,
    pub max_work: u64,
    pub max_output_frames: u64,
}

impl Default for DeliveryLimits {
    fn default() -> Self {
        Self {
            max_targets: 256,
            max_spool_bytes: 2 * 1024_u64.pow(3),
            max_disk_bytes: 4 * 1024_u64.pow(3),
            max_work: 2_000_000_000,
            max_output_frames: 96_000 * 1800,
        }
    }
}

impl DeliveryLimits {
    pub fn song() -> Self {
        Self {
            max_work: 100_000_000_000,
            ..Self::default()
        }
    }
    fn bounded(self) -> Self {
        let ceiling = Self::song();
        Self {
            max_targets: self.max_targets.min(ceiling.max_targets),
            max_spool_bytes: self.max_spool_bytes.min(ceiling.max_spool_bytes),
            max_disk_bytes: self.max_disk_bytes.min(ceiling.max_disk_bytes),
            max_work: self.max_work.min(ceiling.max_work),
            max_output_frames: self.max_output_frames.min(ceiling.max_output_frames),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeliveryOptions {
    pub delivery_id: String,
    /// Empty selects every target; otherwise IDs must be unique and defined.
    pub targets: Vec<String>,
    pub output_dir: PathBuf,
    pub overwrite: bool,
    pub limits: DeliveryLimits,
}

#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct DeliveryError {
    pub code: String,
    pub message: String,
    /// Present if publication failed after targets had been processed.
    pub manifest: Option<Box<DeliveryManifest>>,
}
impl fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for DeliveryError {}
fn error(code: impl Into<String>, message: impl Into<String>) -> DeliveryError {
    DeliveryError {
        code: code.into(),
        message: message.into(),
        manifest: None,
    }
}
fn io_error(error: std::io::Error) -> DeliveryError {
    self::error("E_IO", error.to_string())
}
fn conversion(error: ConversionError) -> DeliveryError {
    self::error(error.code(), error.to_string())
}
fn resource() -> DeliveryError {
    error(
        "E_RESOURCE_LIMIT",
        "delivery exceeds its finite disk, frame, target, or work allowance",
    )
}

#[derive(Clone, Debug, Serialize)]
pub struct LimitCheck {
    pub metric: String,
    pub unit: String,
    pub comparison: String,
    pub bound: String,
    pub status: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetReport {
    pub target_id: String,
    pub role: data::Role,
    pub output: PortRef,
    pub encoding: String,
    pub dither: Value,
    pub rate: u32,
    pub channels: u8,
    pub channel_order: Vec<String>,
    pub frames: u64,
    pub filename: String,
    pub artifact_status: String,
    pub analysis_status: String,
    pub check_status: String,
    pub file_hash: Option<String>,
    pub pcm_hash: Option<String>,
    pub measurements: Option<Analysis>,
    pub checks: Vec<LimitCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeliveryManifest {
    pub schema: String,
    pub ok: bool,
    pub delivery_id: String,
    pub manifest_file: String,
    pub manifest_status: String,
    pub artifact_status: String,
    pub check_status: String,
    pub engine_rate: u32,
    pub engine_frames: u64,
    pub rate: u32,
    pub frames: u64,
    pub interval: Value,
    pub execution_identity: Value,
    pub render_key: String,
    pub identity_context: Value,
    pub resource_limits: DeliveryLimits,
    pub charged_spool_bytes: u64,
    pub charged_disk_bytes: u64,
    pub charged_work: u64,
    pub targets: BTreeMap<String, TargetReport>,
}

fn encoding(value: data::Encoding) -> convert::Encoding {
    match value {
        data::Encoding::Float32 => convert::Encoding::Float32,
        data::Encoding::Pcm16 => convert::Encoding::Pcm16,
        data::Encoding::Pcm24 => convert::Encoding::Pcm24,
    }
}
fn dither(value: data::Dither, delivery: &str, target: &str) -> convert::Dither {
    match value {
        data::Dither::None => convert::Dither::None,
        data::Dither::Tpdf { seed } => convert::Dither::Tpdf {
            seed,
            delivery_id: delivery.into(),
            target_id: target.into(),
        },
    }
}
fn order(channels: u8) -> Vec<String> {
    if channels == 1 {
        vec!["mono".into()]
    } else {
        vec!["left".into(), "right".into()]
    }
}
fn dither_identity(dither: &convert::Dither) -> Value {
    json!({"identity":dither.identity(), "seed":dither.seed()})
}

// Source IDs are case-sensitive ASCII, while supported host filesystems may be
// case-insensitive and limit each component to 255 bytes. Hash exceptional
// spellings independently of target selection; ordinary lowercase names keep
// their readable spelling. Distinct separator domains prevent cross-kind names.
fn artifact_filename(delivery: &str, target: &str) -> String {
    let plain = format!("{delivery}.{target}.wav");
    if plain.len() <= 255
        && !delivery
            .bytes()
            .chain(target.bytes())
            .any(|c| c.is_ascii_uppercase())
    {
        return plain;
    }
    let mut hash = Sha256::new();
    hash.update(b"maac.production.artifact-filename/1\0");
    hash.update(delivery.as_bytes());
    hash.update([0]);
    hash.update(target.as_bytes());
    format!(
        "{}.{}.{:x}.wav",
        &delivery[..delivery.len().min(80)],
        &target[..target.len().min(80)],
        hash.finalize()
    )
}

fn manifest_filename(delivery: &str) -> String {
    if !delivery.bytes().any(|c| c.is_ascii_uppercase()) {
        return format!("{delivery}.manifest.json");
    }
    let mut hash = Sha256::new();
    hash.update(b"maac.production.manifest-filename/1\0");
    hash.update(delivery.as_bytes());
    format!(
        "{}.{:x}.manifest.json",
        &delivery[..delivery.len().min(80)],
        hash.finalize()
    )
}
fn checked_add(total: &mut u64, value: u64) -> Result<(), DeliveryError> {
    *total = total.checked_add(value).ok_or_else(resource)?;
    Ok(())
}
fn byte_count(frames: u64, channels: u8, width: usize) -> Result<u64, DeliveryError> {
    frames
        .checked_mul(u64::from(channels))
        .and_then(|n| n.checked_mul(width as u64))
        .ok_or_else(resource)
}

/// Conservative meter work units: 64 per original channel sample cover
/// K-weighting, loudness blocks/gates and sample-peak overhead. The single
/// four-times interpolator charges 24 units (12 multiplies and 12 additions)
/// per output/channel, including all 44 flush outputs even for an empty input.
fn analysis_work(frames: u64, channels: u8) -> Result<u64, DeliveryError> {
    let ordinary = frames.checked_mul(64).ok_or_else(resource)?;
    let interpolation = frames
        .checked_add(11)
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_mul(24))
        .ok_or_else(resource)?;
    ordinary
        .checked_add(interpolation)
        .and_then(|n| n.checked_mul(u64::from(channels)))
        .ok_or_else(resource)
}

#[derive(Clone, Copy)]
enum ConcretePlan<'a> {
    Legacy(&'a Plan),
    V3(&'a PlanV3),
    Artifact(&'a PlanArtifact),
}

impl<'a> ConcretePlan<'a> {
    fn view(self) -> PlanView<'a> {
        match self {
            Self::Legacy(plan) => plan.view(),
            Self::V3(plan) => plan.view(),
            Self::Artifact(plan) => plan.view(),
        }
    }

    fn to_json_with_limits(self, limits: &PlanLimits) -> Result<Vec<u8>, DeliveryError> {
        match self {
            Self::Legacy(plan) => plan.to_json_with_limits(limits),
            Self::V3(plan) => plan.to_json_with_limits(limits),
            Self::Artifact(plan) => plan.to_json_with_limits(limits),
        }
        .map_err(|e| error(e.code, e.message))
    }

    fn uses_exact_duration(self) -> bool {
        self.view().version >= 3
    }
}

/// Execute and publish a validated named delivery. Failed checks return a
/// complete manifest with `ok=false`; fatal preflight/render errors return Err.
/// Per-target conversion/analysis/publication failures are retained in the
/// manifest while independently successful targets remain published.
pub fn deliver(
    plan: &Plan,
    options: &DeliveryOptions,
    plan_limits: &PlanLimits,
) -> Result<DeliveryManifest, DeliveryError> {
    deliver_concrete(ConcretePlan::Legacy(plan), options, plan_limits)
}

/// Execute a named delivery from either retained plan representation.
pub fn deliver_versioned(
    plan: &VersionedPlan,
    options: &DeliveryOptions,
    plan_limits: &PlanLimits,
) -> Result<DeliveryManifest, DeliveryError> {
    let plan = match plan {
        VersionedPlan::Legacy(plan) => ConcretePlan::Legacy(plan),
        VersionedPlan::V3(plan) => ConcretePlan::V3(plan),
    };
    deliver_concrete(plan, options, plan_limits)
}

/// Execute a named delivery from a validated standalone artifact, including sample kits.
pub fn deliver_artifact(
    plan: &PlanArtifact,
    options: &DeliveryOptions,
    plan_limits: &PlanLimits,
) -> Result<DeliveryManifest, DeliveryError> {
    deliver_concrete(ConcretePlan::Artifact(plan), options, plan_limits)
}

fn deliver_concrete(
    plan: ConcretePlan<'_>,
    options: &DeliveryOptions,
    plan_limits: &PlanLimits,
) -> Result<DeliveryManifest, DeliveryError> {
    let view = plan.view();
    view.validate_with_limits(plan_limits)
        .map_err(|e| error(e.code, e.message))?;
    let settings = view
        .production
        .as_ref()
        .ok_or_else(|| error("E_CAPABILITY", "plan has no named production deliveries"))?;
    let delivery = settings
        .deliveries
        .get(&options.delivery_id)
        .ok_or_else(|| error("E_REFERENCE", "unknown delivery ID"))?;
    let identity = settings
        .execution_identity
        .as_ref()
        .ok_or_else(|| error("E_HASH", "delivery plan is missing its execution identity"))?;
    let limits = options.limits.bounded();
    let mut selected = BTreeSet::new();
    if options.targets.is_empty() {
        selected.extend(delivery.targets.keys().cloned());
    } else {
        for target in &options.targets {
            if !delivery.targets.contains_key(target) {
                return Err(error("E_REFERENCE", format!("unknown target `{target}`")));
            }
            if !selected.insert(target.clone()) {
                return Err(error(
                    "E_RANGE",
                    format!("duplicate selected target `{target}`"),
                ));
            }
        }
    }
    if selected.is_empty() || selected.len() > limits.max_targets {
        return Err(resource());
    }
    let (legacy_duration, engine_frames, output_frames, interval) = if plan.uses_exact_duration() {
        let timing = TimingContext::new_with_limits(view.tempo, view.output, plan_limits)
            .map_err(|e| error(e.code, e.message))?;
        let engine_frames = timing
            .duration_frames(view.output, plan_limits)
            .map_err(|e| error(e.code, e.message))?;
        let output_frames = timing
            .ceil_time(&timing.duration, u64::from(delivery.rate))
            .map_err(|e| error(e.code, e.message))?
            .to_u64()
            .ok_or_else(|| error("E_TIME_PRECISION", "delivery frame ceiling out of range"))?;
        let interval = json!({
            "score_start_q": view.output.score_start_q.to_string(),
            "score_end_q": view.output.score_end_q.to_string(),
            "tail_seconds": view.output.tail_seconds.to_string(),
            "tempo": view.tempo,
            "engine_frames": engine_frames,
            "delivery_frames": output_frames,
        });
        (None, engine_frames, output_frames, interval)
    } else {
        let duration = view
            .tempo
            .seconds_at(&view.output.score_end_q)
            .map_err(|e| error(e.code, e.message))?
            - view
                .tempo
                .seconds_at(&view.output.score_start_q)
                .map_err(|e| error(e.code, e.message))?
            + &view.output.tail_seconds;
        let output_frames =
            convert::delivery_frames(delivery.rate, &duration).map_err(conversion)?;
        let interval = json!({"score_start_q":view.output.score_start_q.to_string(), "score_end_q":view.output.score_end_q.to_string(),
            "tail_seconds":view.output.tail_seconds.to_string(), "duration_seconds":duration.to_string(), "engine_frames":view.output.total_frames, "delivery_frames":output_frames});
        (
            Some(duration),
            view.output.total_frames,
            output_frames,
            interval,
        )
    };
    if engine_frames != view.output.total_frames {
        return Err(error("E_RANGE", "plan engine duration is inconsistent"));
    }
    let mut converters = Vec::new();
    let mut ports = Vec::new();
    let mut reports = BTreeMap::new();
    let mut spool_bytes = 0;
    let mut disk_bytes = 4 * 1024 * 1024; // bounded manifest headroom
    let mut work = 0;
    let manifest_file = manifest_filename(&options.delivery_id);
    let mut destinations = vec![options.output_dir.join(&manifest_file)];
    for id in &selected {
        let target = &delivery.targets[id];
        let channels = view
            .audio_output_channels(&target.output)
            .map_err(|e| error(e.code, e.message))?;
        let conversion_limits = ConversionLimits {
            max_output_frames: limits.max_output_frames,
            max_work: limits.max_work,
            ..ConversionLimits::default()
        };
        let converter = if let Some(duration) = &legacy_duration {
            Converter::new(delivery.rate, duration, channels, conversion_limits)
        } else {
            Converter::from_frame_counts(
                delivery.rate,
                engine_frames,
                output_frames,
                channels,
                conversion_limits,
            )
        }
        .map_err(conversion)?;
        let format = encoding(target.encoding);
        let dither = dither(target.dither, &options.delivery_id, id);
        dither.validate(format).map_err(conversion)?;
        let pcm_bytes = byte_count(output_frames, channels, format.bytes_per_sample())?;
        if pcm_bytes
            .checked_add(128)
            .is_none_or(|bytes| bytes > u64::from(u32::MAX))
        {
            return Err(error(
                "E_FORMAT",
                "delivery exceeds RIFF/WAVE container size",
            ));
        }
        checked_add(&mut spool_bytes, byte_count(engine_frames, channels, 8)?)?;
        checked_add(&mut disk_bytes, pcm_bytes + 128)?;
        checked_add(&mut work, converter.work())?;
        checked_add(&mut work, analysis_work(output_frames, channels)?)?;
        let filename = artifact_filename(&options.delivery_id, id);
        destinations.push(options.output_dir.join(&filename));
        reports.insert(
            id.clone(),
            TargetReport {
                target_id: id.clone(),
                role: target.role,
                output: target.output.clone(),
                encoding: format.identity().into(),
                dither: dither_identity(&dither),
                rate: delivery.rate,
                channels,
                channel_order: order(channels),
                frames: output_frames,
                filename,
                artifact_status: "pending".into(),
                analysis_status: "pending".into(),
                check_status: "not_requested".into(),
                file_hash: None,
                pcm_hash: None,
                measurements: None,
                checks: Vec::new(),
                error: None,
                recovery_path: None,
            },
        );
        converters.push(converter);
        ports.push(target.output.clone());
    }
    checked_add(&mut disk_bytes, spool_bytes)?;
    checked_add(&mut work, spool_bytes / 8)?;
    if spool_bytes > limits.max_spool_bytes
        || disk_bytes > limits.max_disk_bytes
        || work > limits.max_work
    {
        return Err(resource());
    }
    let environment = analysis::numerical_environment().map_err(|e| error(e.code, e.message))?;
    let selection = reports
        .iter()
        .map(|(id, report)| {
            (id.clone(), json!({"delivery_id":options.delivery_id, "port":report.output, "channels":report.channels,
        "channel_order":report.channel_order, "encoding":report.encoding}))
        })
        .collect::<BTreeMap<_, _>>();
    let converter_context = json!({"identity":convert::CONVERTER_ID, "rate":delivery.rate, "arithmetic_mode":convert::ARITHMETIC_ID,
        "coefficient_generator":convert::COEFFICIENT_GENERATOR_ID, "coefficient_digest":converters[0].coefficient_digest(),
        "certificate":serde_json::from_str::<Value>(convert::COEFFICIENT_CERTIFICATE_JSON).map_err(|e| error("E_HASH", e.to_string()))?});
    let mut dependencies = json!({"production_schema":settings.schema_hash,
        "instrument_dependencies":view.instruments.as_ref().map(|resources| &resources.dependencies),
        "source_files":view.instruments.as_ref().map(|resources| &resources.source_files),
        "wavetable_sources":view.instruments.as_ref().map(|resources| &resources.wavetable_sources)});
    if let Some(assets) = view.audio_assets {
        dependencies["audio_assets"] = json!(assets.iter().map(|asset| (asset.id.clone(),
            json!({"hash":asset.hash,"format":asset.format,"rate_hz":asset.rate_hz,"channels":asset.channels,"frames":asset.frames})))
            .collect::<BTreeMap<_,_>>());
    }
    let context = json!({"selection":selection,
        "dependencies":dependencies,
        "processors":view.nodes.iter().map(|node| (node.id.clone(), node)).collect::<BTreeMap<_,_>>(),
        "engine":{"implementation":"maac-rust","version":env!("CARGO_PKG_VERSION"),"rate":48000,"arithmetic_mode":convert::ARITHMETIC_ID},
        "converter":converter_context, "dither":reports.iter().map(|(id,r)| (id.clone(),r.dither.clone())).collect::<BTreeMap<_,_>>(),
        "analyzer":{"identity":analysis::ANALYZER_ID,"true_peak_profile":analysis::TRUE_PEAK_PROFILE,"numerical_environment":environment}, "interval":interval});
    let plan_bytes = plan.to_json_with_limits(plan_limits)?;
    let render_key = crate::production_identity::render_key(identity, &plan_bytes, &context)
        .map_err(|e| error("E_HASH", e.to_string()))?;
    // Detect existing files, symlinks (including dangling links), and directories
    // before creating spools. persist_noclobber also protects races at commit.
    let mut destination_keys = BTreeSet::new();
    for path in &destinations {
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .expect("generated ASCII filename");
        if !destination_keys.insert(name.to_ascii_lowercase()) {
            return Err(error(
                "E_OUTPUT_COLLISION",
                "delivery filenames collide on a case-insensitive filesystem",
            ));
        }
        preflight_destination(path, options.overwrite)?;
    }
    fs::create_dir_all(&options.output_dir).map_err(io_error)?;
    let mut spools = Vec::new();
    let mut writers = Vec::new();
    for _ in &selected {
        let temporary = NamedTempFile::new_in(&options.output_dir).map_err(io_error)?;
        writers.push(BufWriter::with_capacity(
            64 * 1024,
            temporary.as_file().try_clone().map_err(io_error)?,
        ));
        spools.push(temporary);
    }
    let mut captured = 0_u64;
    DspEngine::new_for_view(view, plan_limits)
        .map_err(|e| error(e.code(), e.to_string()))?
        .render_ports(&ports, |frames| {
            if captured >= engine_frames || frames.len() != writers.len() {
                return Err(RenderError::RenderState(
                    "delivery capture shape changed".into(),
                ));
            }
            for ((frame, writer), converter) in frames.iter().zip(&mut writers).zip(&converters) {
                if frame.len() != usize::from(converter.channels()) {
                    return Err(RenderError::RenderState(
                        "delivery capture channel count changed".into(),
                    ));
                }
                for sample in frame {
                    if !sample.is_finite() {
                        return Err(RenderError::Nonfinite("nonfinite delivery capture".into()));
                    }
                    writer
                        .write_all(&sample.to_le_bytes())
                        .map_err(|e| RenderError::Callback(e.to_string()))?;
                }
            }
            captured += 1;
            Ok(())
        })
        .map_err(|e| error(e.code(), e.to_string()))?;
    if captured != engine_frames {
        return Err(error("E_RENDER_STATE", "delivery capture ended early"));
    }
    for writer in &mut writers {
        writer.flush().map_err(io_error)?;
    }
    drop(writers);
    let mut manifest = DeliveryManifest {
        schema: if plan.uses_exact_duration() {
            "maac.production.delivery-manifest/2"
        } else {
            "maac.production.delivery-manifest/1"
        }
        .into(),
        ok: true,
        delivery_id: options.delivery_id.clone(),
        manifest_file,
        manifest_status: "complete".into(),
        artifact_status: "complete".into(),
        check_status: "not_requested".into(),
        engine_rate: view.output.sample_rate_hz,
        engine_frames: captured,
        rate: delivery.rate,
        frames: output_frames,
        interval,
        execution_identity: json!({"algorithm":identity.algorithm,"execution_hash":identity.execution_hash,"source_input_hash":identity.source_input_hash}),
        render_key,
        identity_context: context,
        resource_limits: limits,
        charged_spool_bytes: spool_bytes,
        charged_disk_bytes: disk_bytes,
        charged_work: work,
        targets: reports,
    };
    for ((id, spool), converter) in selected.iter().zip(spools.iter_mut()).zip(&converters) {
        let target = &delivery.targets[id];
        let report = manifest.targets.get_mut(id).expect("selected target");
        match export_target(spool.as_file_mut(), converter, target, options, id, report) {
            Ok(()) => {}
            Err(e) => {
                if report.artifact_status == "pending" {
                    report.artifact_status = "failed".into();
                }
                if report.analysis_status == "pending" {
                    report.analysis_status = "not_completed".into();
                }
                if report.analysis_status != "complete" {
                    report.checks =
                        unavailable_checks(target.limits.as_ref(), "artifact_unavailable");
                    report.check_status = if report.checks.is_empty() {
                        "not_requested"
                    } else {
                        "fail"
                    }
                    .into();
                }
                report.error = Some(Failure {
                    code: e.code,
                    message: e.message,
                });
            }
        }
    }
    manifest.artifact_status = if manifest
        .targets
        .values()
        .all(|r| r.artifact_status == "complete")
    {
        "complete"
    } else {
        "partial"
    }
    .into();
    manifest.check_status = if manifest.targets.values().any(|r| r.check_status == "fail") {
        "fail"
    } else if manifest.targets.values().any(|r| r.check_status == "pass") {
        "pass"
    } else {
        "not_requested"
    }
    .into();
    manifest.ok = manifest.artifact_status == "complete"
        && manifest.check_status != "fail"
        && manifest.targets.values().all(|r| r.error.is_none());
    if manifest.artifact_status != "complete"
        || manifest
            .targets
            .values()
            .any(|r| r.analysis_status != "complete" || r.error.is_some())
    {
        manifest.manifest_status = "partial".into();
    }
    publish_manifest(manifest, options)
}

fn publish_manifest(
    mut manifest: DeliveryManifest,
    options: &DeliveryOptions,
) -> Result<DeliveryManifest, DeliveryError> {
    let publish_manifest = (|| {
        let bytes =
            serde_json::to_vec_pretty(&manifest).map_err(|e| error("E_IO", e.to_string()))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(error(
                "E_RESOURCE_LIMIT",
                "delivery manifest exceeds its reserved size",
            ));
        }
        let mut temporary = NamedTempFile::new_in(&options.output_dir).map_err(io_error)?;
        temporary.write_all(&bytes).map_err(io_error)?;
        temporary.as_file().sync_all().map_err(io_error)?;
        publish(
            temporary,
            &options.output_dir.join(&manifest.manifest_file),
            options.overwrite,
        )
        .map_err(|(e, _)| e)?;
        Ok::<_, DeliveryError>(())
    })();
    if let Err(mut e) = publish_manifest {
        manifest.ok = false;
        manifest.manifest_status = "failed".into();
        e.manifest = Some(Box::new(manifest));
        return Err(e);
    }
    Ok(manifest)
}

fn preflight_destination(path: &Path, overwrite: bool) -> Result<(), DeliveryError> {
    match fs::symlink_metadata(path) {
        Ok(_) if !overwrite => Err(error(
            "E_OUTPUT_EXISTS",
            format!("destination exists: {}", path.display()),
        )),
        Ok(meta) if meta.is_dir() => Err(error(
            "E_IO",
            format!("destination is a directory: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(e)),
    }
}

fn publish(
    temporary: NamedTempFile,
    path: &Path,
    overwrite: bool,
) -> Result<(), (DeliveryError, NamedTempFile)> {
    let result = if overwrite {
        temporary.persist(path)
    } else {
        temporary.persist_noclobber(path)
    };
    result.map(|_| ()).map_err(|e| {
        let code = if e.error.kind() == std::io::ErrorKind::AlreadyExists {
            "E_OUTPUT_EXISTS"
        } else {
            "E_IO"
        };
        (
            error(
                code,
                format!("cannot publish {}: {}", path.display(), e.error),
            ),
            e.file,
        )
    })
}

struct SpoolReader<'a> {
    file: &'a mut File,
    channels: usize,
    frames: u64,
    start: u64,
    cached: usize,
    bytes: Vec<u8>,
}
impl<'a> SpoolReader<'a> {
    fn new(file: &'a mut File, channels: u8, frames: u64) -> Self {
        Self {
            file,
            channels: usize::from(channels),
            frames,
            start: 0,
            cached: 0,
            bytes: vec![0; 2048 * usize::from(channels) * 8],
        }
    }
    fn read(&mut self, frame: u64, channel: usize) -> Result<f64, ConversionError> {
        if frame >= self.frames || channel >= self.channels {
            return Err(ConversionError::Frame);
        }
        if self.cached == 0 || frame < self.start || frame >= self.start + self.cached as u64 {
            self.start = frame.saturating_sub(1024);
            self.cached = (self.frames - self.start).min(2048) as usize;
            self.file
                .seek(SeekFrom::Start(self.start * self.channels as u64 * 8))
                .map_err(|e| ConversionError::Io(e.to_string()))?;
            self.file
                .read_exact(&mut self.bytes[..self.cached * self.channels * 8])
                .map_err(|e| ConversionError::Io(e.to_string()))?;
        }
        let offset = ((frame - self.start) as usize * self.channels + channel) * 8;
        Ok(f64::from_le_bytes(
            self.bytes[offset..offset + 8]
                .try_into()
                .expect("sample width"),
        ))
    }
}

fn export_target(
    spool: &mut File,
    converter: &Converter,
    target: &Target,
    options: &DeliveryOptions,
    id: &str,
    report: &mut TargetReport,
) -> Result<(), DeliveryError> {
    export_target_with_analyzer(
        spool,
        converter,
        target,
        options,
        id,
        report,
        analysis::analyze_wav_with_limits,
    )
}

fn export_target_with_analyzer(
    spool: &mut File,
    converter: &Converter,
    target: &Target,
    options: &DeliveryOptions,
    id: &str,
    report: &mut TargetReport,
    analyze: impl FnOnce(&Path, AnalyzerLimits) -> Result<Analysis, analysis::AnalysisError>,
) -> Result<(), DeliveryError> {
    let format = encoding(target.encoding);
    let dither = dither(target.dither, &options.delivery_id, id);
    let mut reader = SpoolReader::new(spool, converter.channels(), converter.engine_frames());
    let mut temporary = NamedTempFile::new_in(&options.output_dir).map_err(io_error)?;
    let mut pcm_hash = Sha256::new();
    {
        let mut writer = BufWriter::with_capacity(64 * 1024, temporary.as_file_mut());
        write_wav_header(&mut writer, converter, format)?;
        for frame in 0..converter.output_frames() {
            for channel in 0..usize::from(converter.channels()) {
                let sample = converter
                    .sample(frame, channel, &mut |f, c| reader.read(f, c))
                    .map_err(conversion)?;
                let encoded = convert::encode_sample(sample, format, &dither, frame, channel)
                    .map_err(conversion)?;
                writer
                    .write_all(&encoded.bytes[..encoded.len])
                    .map_err(io_error)?;
                pcm_hash.update(&encoded.bytes[..encoded.len]);
            }
        }
        if byte_count(
            converter.output_frames(),
            converter.channels(),
            format.bytes_per_sample(),
        )? % 2
            != 0
        {
            writer.write_all(&[0]).map_err(io_error)?;
        }
        writer.flush().map_err(io_error)?;
    }
    temporary.as_file().sync_all().map_err(io_error)?;
    report.pcm_hash = Some(format!("sha256:{:x}", pcm_hash.finalize()));
    report.file_hash = Some(hash_file(temporary.as_file_mut())?);
    let analysis_result = analyze(
        temporary.path(),
        AnalyzerLimits {
            max_frames: converter.output_frames(),
            max_loudness_blocks: 1_000_000,
        },
    );
    match analysis_result {
        Ok(measurements) => {
            if measurements.frames != converter.output_frames()
                || measurements.rate != converter.rate()
                || measurements.channels != converter.channels()
            {
                report.analysis_status = "failed".into();
                report.error = Some(Failure {
                    code: "E_RENDER_STATE".into(),
                    message: "encoded artifact shape differs from selected target".into(),
                });
            } else {
                report.checks = evaluate_limits(target.limits.as_ref(), &measurements)?;
                report.check_status = if report.checks.is_empty() {
                    "not_requested"
                } else if report.checks.iter().all(|c| c.status == "pass") {
                    "pass"
                } else {
                    "fail"
                }
                .into();
                report.analysis_status = "complete".into();
                report.measurements = Some(measurements);
            }
        }
        Err(e) => {
            report.analysis_status = "failed".into();
            report.error = Some(Failure {
                code: e.code.into(),
                message: e.message,
            });
        }
    }
    if report.analysis_status != "complete" {
        report.checks = unavailable_checks(target.limits.as_ref(), "analysis_failed");
        report.check_status = if report.checks.is_empty() {
            "not_requested"
        } else {
            "fail"
        }
        .into();
    }
    match publish(
        temporary,
        &options.output_dir.join(&report.filename),
        options.overwrite,
    ) {
        Ok(()) => {
            report.artifact_status = "complete".into();
            Ok(())
        }
        Err((mut error, temporary)) => {
            match temporary.keep() {
                Ok((_, path)) => {
                    report.artifact_status = "retained_for_review".into();
                    report.recovery_path = Some(path.display().to_string());
                }
                Err(e) => {
                    error
                        .message
                        .push_str(&format!("; cannot retain temporary artifact: {e}"));
                }
            }
            Err(error)
        }
    }
}

fn unavailable_checks(limits: Option<&data::Limits>, reason: &str) -> Vec<LimitCheck> {
    let Some(limits) = limits else {
        return Vec::new();
    };
    let mut checks = Vec::new();
    let mut push = |metric: &str, unit: &str, comparison: &str, bound: &Rational| {
        checks.push(LimitCheck {
            metric: metric.into(),
            unit: unit.into(),
            comparison: comparison.into(),
            bound: bound.to_string(),
            status: "fail".into(),
            reason: reason.into(),
        });
    };
    if let Some(limit) = &limits.integrated_loudness {
        if let Some(min) = &limit.min {
            push("integrated_loudness", "LUFS", "min", min);
        }
        if let Some(max) = &limit.max {
            push("integrated_loudness", "LUFS", "max", max);
        }
    }
    if let Some(limit) = &limits.sample_peak {
        push("sample_peak", "dBFS", "max", &limit.max);
    }
    if let Some(limit) = &limits.true_peak {
        push("true_peak", "dBTP", "max", &limit.max);
    }
    checks
}

fn write_wav_header(
    writer: &mut impl Write,
    converter: &Converter,
    format: convert::Encoding,
) -> Result<(), DeliveryError> {
    let float = format == convert::Encoding::Float32;
    let bytes = byte_count(
        converter.output_frames(),
        converter.channels(),
        format.bytes_per_sample(),
    )?;
    let overhead = if float { 50 } else { 36 };
    let riff_size = u32::try_from(bytes + bytes % 2 + overhead).map_err(|_| resource())?;
    let mut header = Vec::with_capacity(58);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&riff_size.to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&(if float { 18_u32 } else { 16_u32 }).to_le_bytes());
    header.extend_from_slice(&(if float { 3_u16 } else { 1_u16 }).to_le_bytes());
    header.extend_from_slice(&u16::from(converter.channels()).to_le_bytes());
    header.extend_from_slice(&converter.rate().to_le_bytes());
    let alignment = u16::from(converter.channels()) * format.bytes_per_sample() as u16;
    header.extend_from_slice(&(converter.rate() * u32::from(alignment)).to_le_bytes());
    header.extend_from_slice(&alignment.to_le_bytes());
    header.extend_from_slice(&(format.bytes_per_sample() as u16 * 8).to_le_bytes());
    if float {
        // IEEE Float32 uses WAVEFORMATEX, including its zero-length extension.
        header.extend_from_slice(&0_u16.to_le_bytes());
        header.extend_from_slice(b"fact");
        header.extend_from_slice(&4_u32.to_le_bytes());
        header.extend_from_slice(&(converter.output_frames() as u32).to_le_bytes());
    }
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(bytes as u32).to_le_bytes());
    writer.write_all(&header).map_err(io_error)
}

fn hash_file(file: &mut File) -> Result<String, DeliveryError> {
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes).map_err(io_error)?;
        if count == 0 {
            break;
        }
        hasher.update(&bytes[..count]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub fn evaluate_limits(
    limits: Option<&data::Limits>,
    measured: &Analysis,
) -> Result<Vec<LimitCheck>, DeliveryError> {
    let Some(limits) = limits else {
        return Ok(Vec::new());
    };
    let mut checks = Vec::new();
    let mut push = |metric: &str,
                    unit: &str,
                    comparison: &str,
                    bound: &Rational,
                    value: Option<f64>,
                    silence: bool|
     -> Result<(), DeliveryError> {
        let bound_f64 = bound
            .to_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| error("E_RANGE", "nonfinite delivery limit"))?;
        let passed = if silence {
            true
        } else {
            value.is_some_and(|v| {
                if comparison == "min" {
                    v >= bound_f64
                } else {
                    v <= bound_f64
                }
            })
        };
        checks.push(LimitCheck {
            metric: metric.into(),
            unit: unit.into(),
            comparison: comparison.into(),
            bound: bound.to_string(),
            status: if passed { "pass" } else { "fail" }.into(),
            reason: if silence {
                "digital_silence"
            } else if value.is_none() {
                "unmeasurable"
            } else if passed {
                "within_limit"
            } else {
                "outside_limit"
            }
            .into(),
        });
        Ok(())
    };
    if let Some(loudness) = &limits.integrated_loudness {
        if let Some(min) = &loudness.min {
            push(
                "integrated_loudness",
                "LUFS",
                "min",
                min,
                measured.integrated_loudness.value,
                false,
            )?;
        }
        if let Some(max) = &loudness.max {
            push(
                "integrated_loudness",
                "LUFS",
                "max",
                max,
                measured.integrated_loudness.value,
                false,
            )?;
        }
    }
    if let Some(peak) = &limits.sample_peak {
        push(
            "sample_peak",
            "dBFS",
            "max",
            &peak.max,
            measured.sample_peak.value,
            measured.sample_peak.status == "digital_silence",
        )?;
    }
    if let Some(peak) = &limits.true_peak {
        push(
            "true_peak",
            "dBTP",
            "max",
            &peak.max,
            measured.true_peak.value,
            measured.true_peak.status == "digital_silence",
        )?;
    }
    Ok(checks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_work_accounts_for_empty_and_tiny_filter_flushes_without_overflow() {
        assert_eq!(analysis_work(0, 1).unwrap(), 1056);
        assert_eq!(analysis_work(1, 2).unwrap(), 2432);
        assert_eq!(
            analysis_work(u64::MAX, 1).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
        // Each intermediate per-channel term fits, but their sum does not.
        assert_eq!(
            analysis_work(u64::MAX / 128, 1).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
        // The per-channel total fits; the stereo aggregate does not.
        assert_eq!(
            analysis_work(u64::MAX / 256, 2).unwrap_err().code,
            "E_RESOURCE_LIMIT"
        );
    }

    fn fixture(
        directory: &Path,
    ) -> (
        NamedTempFile,
        Converter,
        Target,
        DeliveryOptions,
        TargetReport,
    ) {
        let mut spool = NamedTempFile::new_in(directory).unwrap();
        for _ in 0..3 {
            spool.write_all(&0.25_f64.to_le_bytes()).unwrap();
        }
        spool.flush().unwrap();
        let converter = Converter::new(
            48000,
            &crate::exact::parse_rational("1/16000").unwrap(),
            1,
            ConversionLimits::default(),
        )
        .unwrap();
        let target = Target {
            role: data::Role::Master,
            output: PortRef::new("master", "out").unwrap(),
            encoding: data::Encoding::Float32,
            dither: data::Dither::None,
            limits: Some(data::Limits {
                sample_peak: Some(data::PeakLimit {
                    unit: data::LimitUnit::DbFs,
                    max: crate::exact::parse_rational("-1").unwrap(),
                }),
                integrated_loudness: Some(data::LoudnessLimit {
                    unit: data::LimitUnit::LuFs,
                    min: Some(crate::exact::parse_rational("-20").unwrap()),
                    max: None,
                }),
                ..data::Limits::default()
            }),
        };
        let options = DeliveryOptions {
            delivery_id: "release".into(),
            targets: vec!["master".into()],
            output_dir: directory.into(),
            overwrite: false,
            limits: DeliveryLimits::default(),
        };
        let report = TargetReport {
            target_id: "master".into(),
            role: target.role,
            output: target.output.clone(),
            encoding: "wav_f32le".into(),
            dither: json!({"identity":"none","seed":null}),
            rate: 48000,
            channels: 1,
            channel_order: order(1),
            frames: 3,
            filename: "release.master.wav".into(),
            artifact_status: "pending".into(),
            analysis_status: "pending".into(),
            check_status: "not_requested".into(),
            file_hash: None,
            pcm_hash: None,
            measurements: None,
            checks: Vec::new(),
            error: None,
            recovery_path: None,
        };
        (spool, converter, target, options, report)
    }

    #[test]
    fn analyzer_failure_retains_encoded_audio_and_fails_requested_checks() {
        let directory = tempfile::tempdir().unwrap();
        let (mut spool, converter, target, options, mut report) = fixture(directory.path());
        export_target_with_analyzer(
            spool.as_file_mut(),
            &converter,
            &target,
            &options,
            "master",
            &mut report,
            |path, _| {
                assert_eq!(hound::WavReader::open(path).unwrap().duration(), 3);
                Err(analysis::AnalysisError {
                    code: "E_TEST_ANALYSIS",
                    message: "injected analysis failure".into(),
                })
            },
        )
        .unwrap();
        assert_eq!(report.artifact_status, "complete");
        assert_eq!(report.analysis_status, "failed");
        assert_eq!(report.check_status, "fail");
        assert_eq!(report.checks.len(), 2);
        assert!(report
            .checks
            .iter()
            .all(|check| check.reason == "analysis_failed"));
        assert!(report.measurements.is_none());
        assert_eq!(report.error.as_ref().unwrap().code, "E_TEST_ANALYSIS");
        assert_eq!(
            hound::WavReader::open(directory.path().join("release.master.wav"))
                .unwrap()
                .duration(),
            3
        );
    }

    #[test]
    fn analyzer_shape_mismatch_preserves_audio_without_invented_measurements() {
        let directory = tempfile::tempdir().unwrap();
        let (mut spool, converter, target, options, mut report) = fixture(directory.path());
        export_target_with_analyzer(
            spool.as_file_mut(),
            &converter,
            &target,
            &options,
            "master",
            &mut report,
            |_, _| analysis::Analyzer::new(48000, 1).unwrap().finish(),
        )
        .unwrap();
        assert_eq!(report.artifact_status, "complete");
        assert_eq!(report.analysis_status, "failed");
        assert_eq!(report.check_status, "fail");
        assert!(report.measurements.is_none());
        assert!(directory.path().join("release.master.wav").is_file());
    }

    #[test]
    fn publication_race_preserves_destination_and_completed_candidate() {
        let directory = tempfile::tempdir().unwrap();
        let (mut spool, converter, target, options, mut report) = fixture(directory.path());
        let destination = directory.path().join("release.master.wav");
        let failure = export_target_with_analyzer(
            spool.as_file_mut(),
            &converter,
            &target,
            &options,
            "master",
            &mut report,
            |path, limits| {
                let result = analysis::analyze_wav_with_limits(path, limits);
                fs::write(&destination, b"racing writer").unwrap();
                result
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, "E_OUTPUT_EXISTS");
        assert_eq!(fs::read(&destination).unwrap(), b"racing writer");
        assert_eq!(report.artifact_status, "retained_for_review");
        let recovery = Path::new(report.recovery_path.as_ref().unwrap());
        assert_eq!(hound::WavReader::open(recovery).unwrap().duration(), 3);
    }

    #[test]
    fn manifest_publication_and_size_errors_carry_completed_artifact_results() {
        for oversized in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let (mut spool, converter, target, options, mut report) = fixture(directory.path());
            export_target(
                spool.as_file_mut(),
                &converter,
                &target,
                &options,
                "master",
                &mut report,
            )
            .unwrap();
            let manifest = DeliveryManifest {
                schema: "maac.production.delivery-manifest/1".into(),
                ok: false,
                delivery_id: "release".into(),
                manifest_file: "release.manifest.json".into(),
                manifest_status: "complete".into(),
                artifact_status: "complete".into(),
                check_status: "fail".into(),
                engine_rate: 48000,
                engine_frames: 3,
                rate: 48000,
                frames: 3,
                interval: json!({}),
                execution_identity: json!({}),
                render_key: "test".into(),
                identity_context: if oversized {
                    json!({"large":"x".repeat(4*1024*1024)})
                } else {
                    json!({})
                },
                resource_limits: DeliveryLimits::default(),
                charged_spool_bytes: 24,
                charged_disk_bytes: 100,
                charged_work: 100,
                targets: BTreeMap::from([("master".into(), report)]),
            };
            if !oversized {
                fs::create_dir(directory.path().join("release.manifest.json")).unwrap();
            }
            let failure = publish_manifest(manifest, &options).unwrap_err();
            assert_eq!(
                failure.code,
                if oversized {
                    "E_RESOURCE_LIMIT"
                } else {
                    "E_OUTPUT_EXISTS"
                }
            );
            let retained = failure.manifest.unwrap();
            assert_eq!(retained.manifest_status, "failed");
            assert_eq!(retained.targets["master"].artifact_status, "complete");
            assert!(retained.targets["master"].file_hash.is_some());
            assert_eq!(
                hound::WavReader::open(directory.path().join("release.master.wav"))
                    .unwrap()
                    .duration(),
                3
            );
        }
    }
}
