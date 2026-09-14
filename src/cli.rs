//! Public command-line orchestration. Filesystem reads and publication live
//! here; compiler and DSP modules receive parsed source/plans only.

use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{error::ErrorKind, Arg, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::bundle::sha256_digest;
use crate::bundle::SourceBundle;
use crate::bundle_fs;
use crate::compiler;
use crate::diagnostic::{Diagnostic, Diagnostics, Span};
use crate::dsp::RenderError;
use crate::export::{self, ExportError, FrameRange, WavFormat, WavStats, MAX_INPUT_BYTES};
use crate::plan::{Plan, PlanError, PlanLimits};
use crate::plan_v3::VersionedPlan;
use crate::syntax::{parse, Document};
use crate::{ModuleArtifact, PlanArtifact, MAX_MODULE_ARTIFACT_JSON_BYTES};

#[derive(Parser, Debug)]
#[command(
    name = "maac",
    version,
    about = "MaaC (Music as a Code) compiler and renderer"
)]
pub struct Cli {
    /// Emit one structured JSON result or error on stdout.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Resolve and validate a composition bundle or reusable library.
    Check {
        /// Source file or project directory (defaults to main.maac in cwd).
        input: Option<PathBuf>,
        /// Root used to resolve project-relative imports and assets.
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Finite execution-work allowance; other resource limits stay unchanged.
        #[arg(long, value_enum, default_value_t = ProfileArg::Default)]
        profile: ProfileArg,
    },
    /// Resolve a composition bundle into a standalone versioned performance plan.
    Compile {
        /// Source file or project directory (defaults to main.maac in cwd).
        input: Option<PathBuf>,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        force: bool,
        /// Root used to resolve project-relative imports and assets.
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Finite execution-work allowance; other resource limits stay unchanged.
        #[arg(long, value_enum, default_value_t = ProfileArg::Default)]
        profile: ProfileArg,
    },
    /// Validate an imported performance plan and stream it to WAV.
    Render {
        input: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = FormatArg::Float32)]
        format: FormatArg,
        #[arg(long)]
        force: bool,
        /// Finite execution-work allowance; other resource limits stay unchanged.
        #[arg(long, value_enum, default_value_t = ProfileArg::Default)]
        profile: ProfileArg,
    },
    /// Resolve and compile a composition bundle directly to WAV.
    Build {
        /// Source file or project directory (defaults to main.maac in cwd).
        input: Option<PathBuf>,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = FormatArg::Float32)]
        format: FormatArg,
        #[arg(long)]
        force: bool,
        /// Root used to resolve project-relative imports and assets.
        #[arg(long)]
        project_root: Option<PathBuf>,
        /// Finite execution-work allowance; other resource limits stay unchanged.
        #[arg(long, value_enum, default_value_t = ProfileArg::Default)]
        profile: ProfileArg,
    },
    /// Render a named master/stem delivery, analyze final WAVs, and publish its manifest.
    Deliver {
        /// Source file/project directory, or a retained performance-plan JSON file.
        input: PathBuf,
        #[arg(long)]
        delivery: String,
        #[arg(long)]
        output_dir: PathBuf,
        /// Select a named target; repeat to select several (default: all).
        #[arg(long = "target")]
        targets: Vec<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        project_root: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = ProfileArg::Default)]
        profile: ProfileArg,
    },
    /// Compute the canonical SHA-256 pin for a bounded local file.
    Hash { input: PathBuf },
    /// List embedded instruments, or describe one export with a runnable example.
    Instruments {
        name: Option<String>,
        /// Select an exact embedded library identity.
        #[arg(long)]
        library: Option<String>,
        /// List all embedded library identities.
        #[arg(long, conflicts_with_all = ["name", "library"])]
        libraries: bool,
    },
    /// Export, validate, or unpack a reusable musical source module.
    Module {
        #[command(subcommand)]
        command: ModuleCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum ModuleCommand {
    /// Package one library and its complete pinned dependency closure.
    Export {
        library: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        project_root: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// Validate a retained musical module artifact.
    Check {
        module: PathBuf,
        #[arg(long)]
        expect_hash: Option<String>,
    },
    /// Restore local sources and assets into a new directory.
    Unpack {
        module: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        expect_hash: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ProfileArg {
    Default,
    Song,
}

impl ProfileArg {
    fn limits(self) -> PlanLimits {
        match self {
            Self::Default => PlanLimits::default(),
            Self::Song => PlanLimits::song(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum FormatArg {
    Float32,
    Pcm16,
}

impl From<FormatArg> for WavFormat {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Float32 => WavFormat::Float32,
            FormatArg::Pcm16 => WavFormat::Pcm16,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandResult {
    pub ok: bool,
    pub command: String,
    pub input: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exports: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<crate::stdlib::Catalog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<crate::stdlib::InstrumentInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libraries: Option<Vec<crate::stdlib::LibraryInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<crate::production_delivery::DeliveryManifest>,
}

/// CLI result for artifact-aware callers. Legacy fields retain their existing wire shape.
#[derive(Clone, Debug, Serialize)]
pub struct ArtifactCommandResult {
    #[serde(flatten)]
    base: CommandResult,
    #[serde(skip_serializing_if = "is_zero")]
    hits: usize,
    #[serde(skip_serializing_if = "is_zero")]
    audio_clips: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_frame: Option<u64>,
}
impl ArtifactCommandResult {
    pub fn base(&self) -> &CommandResult {
        &self.base
    }
    pub fn hits(&self) -> usize {
        self.hits
    }
    pub fn audio_clips(&self) -> usize {
        self.audio_clips
    }
    pub fn start_frame(&self) -> Option<u64> {
        self.start_frame
    }
    pub fn end_frame(&self) -> Option<u64> {
        self.end_frame
    }
}
fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl CommandResult {
    fn check(input: &Path, notes: usize, frames: u64) -> Self {
        Self {
            ok: true,
            command: "check".into(),
            input: input.display().to_string(),
            output: None,
            format: None,
            notes: Some(notes),
            frames: Some(frames),
            digest: None,
            exports: None,
            catalog: None,
            instrument: None,
            library: None,
            libraries: None,
            delivery: None,
        }
    }

    fn compile(input: &Path, output: &Path, notes: usize, frames: u64) -> Self {
        Self {
            ok: true,
            command: "compile".into(),
            input: input.display().to_string(),
            output: Some(output.display().to_string()),
            format: None,
            notes: Some(notes),
            frames: Some(frames),
            digest: None,
            exports: None,
            catalog: None,
            instrument: None,
            library: None,
            libraries: None,
            delivery: None,
        }
    }

    fn render(
        command: &'static str,
        input: &Path,
        output: &Path,
        format: WavFormat,
        stats: WavStats,
    ) -> Self {
        Self {
            ok: true,
            command: command.into(),
            input: input.display().to_string(),
            output: Some(output.display().to_string()),
            format: Some(format.as_str().into()),
            notes: None,
            frames: Some(stats.frames),
            digest: None,
            exports: None,
            catalog: None,
            instrument: None,
            library: None,
            libraries: None,
            delivery: None,
        }
    }

    fn hash(input: &Path, digest: String) -> Self {
        Self {
            ok: true,
            command: "hash".into(),
            input: input.display().to_string(),
            output: None,
            format: None,
            notes: None,
            frames: None,
            digest: Some(digest),
            exports: None,
            catalog: None,
            instrument: None,
            library: None,
            libraries: None,
            delivery: None,
        }
    }

    fn library_check(input: &Path, exports: usize) -> Self {
        Self {
            ok: true,
            command: "check".into(),
            input: input.display().to_string(),
            output: None,
            format: None,
            notes: None,
            frames: None,
            digest: None,
            exports: Some(exports),
            catalog: None,
            instrument: None,
            library: None,
            libraries: None,
            delivery: None,
        }
    }

    fn module(
        command: &'static str,
        input: &Path,
        output: Option<&Path>,
        digest: String,
        exports: usize,
    ) -> Self {
        Self {
            ok: true,
            command: command.into(),
            input: input.display().to_string(),
            output: output.map(|path| path.display().to_string()),
            format: Some(crate::MODULE_ARTIFACT_FORMAT.into()),
            notes: None,
            frames: None,
            digest: Some(digest),
            exports: Some(exports),
            catalog: None,
            instrument: None,
            library: None,
            libraries: None,
            delivery: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CliError {
    pub ok: bool,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Box<crate::production_delivery::DeliveryManifest>>,
}

impl CliError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            code: code.into(),
            message: message.into(),
            path: None,
            span: None,
            delivery: None,
        }
    }

    fn with_span(mut self, span: Option<Span>) -> Self {
        self.span = span;
        self
    }

    pub fn from_diagnostics(diagnostics: &Diagnostics) -> Self {
        diagnostics
            .first()
            .map(Self::from_diagnostic)
            .unwrap_or_else(|| Self::new("E_SYNTAX", "operation failed without a diagnostic"))
    }

    pub fn from_diagnostic(diagnostic: &Diagnostic) -> Self {
        let mut error = Self::new(diagnostic.code.as_str(), diagnostic.message.clone())
            .with_span(diagnostic.span);
        let mut path = diagnostic.object_path.join(".");
        if !diagnostic.field_path.is_empty() {
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(&diagnostic.field_path.join("."));
        }
        if !path.is_empty() {
            error.path = Some(path);
        }
        error
    }

    pub fn from_plan(error: PlanError) -> Self {
        let mut result = Self::new(error.code, error.message).with_span(error.span);
        if !error.path.is_empty() {
            result.path = Some(error.path);
        }
        result
    }

    pub fn from_render(error: RenderError) -> Self {
        if let RenderError::Plan(plan_error) = &error {
            return Self::from_plan(plan_error.clone());
        }
        let code = error.code().to_owned();
        Self::new(code, error.to_string())
    }

    pub fn from_export(error: ExportError) -> Self {
        if let ExportError::Render(render_error) = &error {
            return Self::from_render(render_error.clone());
        }
        let code = error.code().to_owned();
        let path = match &error {
            ExportError::OutputExists { path } => Some(path.display().to_string()),
            _ => None,
        };
        let mut result = Self::new(code, error.to_string());
        result.path = path;
        result
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(path) = &self.path {
            write!(f, " at {path}")?;
        }
        if let Some(span) = self.span {
            write!(f, " ({}..{})", span.start, span.end)?;
        }
        Ok(())
    }
}

impl std::error::Error for CliError {}

/// Execute one parsed CLI request without printing. This is convenient for
/// embedding and gives the binary a single output policy for human/JSON modes.
pub fn execute(command: &Command) -> Result<CommandResult, CliError> {
    execute_impl(
        command,
        PlanCapability::Legacy,
        &mut EventCounts::default(),
        None,
    )
}
/// Execute the same commands with standalone artifact support, including hits and audio clips.
pub fn execute_artifact(command: &Command) -> Result<ArtifactCommandResult, CliError> {
    execute_artifact_with_range(command, None)
}

/// Execute an artifact-aware command with an optional offline render range.
/// The range is additive to the existing command API so callers that do not
/// need excerpts retain the original `Command` and result contracts.
pub fn execute_artifact_with_range(
    command: &Command,
    range: Option<FrameRange>,
) -> Result<ArtifactCommandResult, CliError> {
    if range.is_some() && !matches!(command, Command::Render { .. }) {
        return Err(CliError::new(
            "E_USAGE",
            "a frame range is supported only by the render command",
        ));
    }
    let mut counts = EventCounts::default();
    let mut base = execute_impl(command, PlanCapability::Artifact, &mut counts, range)?;
    if counts.hits > 0 || counts.audio_clips > 0 {
        base.notes = Some(counts.notes);
    }
    Ok(ArtifactCommandResult {
        base,
        hits: counts.hits,
        audio_clips: counts.audio_clips,
        start_frame: range.map(|range| range.start_frame),
        end_frame: range.map(|range| range.end_frame),
    })
}
#[derive(Default)]
struct EventCounts {
    notes: usize,
    hits: usize,
    audio_clips: usize,
}
impl EventCounts {
    fn record(&mut self, plan: &PlanArtifact) {
        self.notes = 0;
        self.hits = 0;
        self.audio_clips = plan.audio_clip_count();
        for event in plan.view().events() {
            match event.kind {
                crate::plan::EventKind::Note { .. } => self.notes += 1,
                crate::plan::EventKind::Hit { .. } => self.hits += 1,
                _ => {}
            }
        }
    }
}
#[derive(Clone, Copy)]
enum PlanCapability {
    Legacy,
    Artifact,
}
impl PlanCapability {
    fn compile(self, bundle: &SourceBundle, limits: &PlanLimits) -> Result<PlanArtifact, CliError> {
        match self {
            Self::Legacy => compiler::compile_bundle_versioned_with_limits(bundle, limits)
                .map(PlanArtifact::from),
            Self::Artifact => compiler::compile_bundle_artifact_with_limits(bundle, limits),
        }
        .map_err(|error| CliError::from_diagnostics(&error))
    }
    fn decode(self, bytes: &[u8], limits: &PlanLimits) -> Result<PlanArtifact, CliError> {
        match self {
            Self::Legacy => {
                VersionedPlan::from_json_with_limits(bytes, limits).map(PlanArtifact::from)
            }
            Self::Artifact => PlanArtifact::from_json_with_limits(bytes, limits),
        }
        .map_err(CliError::from_plan)
    }
    fn check_library(self, bundle: &SourceBundle, limits: &PlanLimits) -> Result<(), CliError> {
        match self {
            Self::Legacy => compiler::check_bundle_versioned_with_limits(bundle, limits),
            Self::Artifact => compiler::check_bundle_artifact_with_limits(bundle, limits),
        }
        .map_err(|error| CliError::from_diagnostics(&error))
    }
}
fn execute_impl(
    command: &Command,
    capability: PlanCapability,
    counts: &mut EventCounts,
    range: Option<FrameRange>,
) -> Result<CommandResult, CliError> {
    match command {
        Command::Check {
            input,
            project_root,
            profile,
        } => {
            let (input, root) = resolve_source_input(input.as_deref(), project_root.as_deref());
            let limits = profile.limits();
            let bundle = load_source_bundle(&input, root.as_deref())?;
            let document = bundle_entry_document(&bundle)?;
            if document
                .objects
                .values()
                .any(|object| object.kind == "library")
            {
                capability.check_library(&bundle, &limits)?;
                let exports = document
                    .objects
                    .values()
                    .filter(|object| {
                        matches!(
                            object.kind.as_str(),
                            "instrument" | "preset" | "wavetable" | "pattern" | "curve" | "tuning"
                        )
                    })
                    .count();
                Ok(CommandResult::library_check(&input, exports))
            } else {
                let plan = capability.compile(&bundle, &limits)?;
                counts.record(&plan);
                Ok(CommandResult::check(
                    &input,
                    artifact_plan_stats(&plan).0,
                    artifact_plan_stats(&plan).1,
                ))
            }
        }
        Command::Compile {
            input,
            output,
            force,
            project_root,
            profile,
        } => {
            let (input, root) = resolve_source_input(input.as_deref(), project_root.as_deref());
            let limits = profile.limits();
            let bundle = load_source_bundle(&input, root.as_deref())?;
            let plan = capability.compile(&bundle, &limits)?;
            counts.record(&plan);
            let bytes = plan
                .to_json_with_limits(&limits)
                .map_err(CliError::from_plan)?;
            export::atomic_write(output, &bytes, *force).map_err(CliError::from_export)?;
            Ok(CommandResult::compile(
                &input,
                output,
                artifact_plan_stats(&plan).0,
                artifact_plan_stats(&plan).1,
            ))
        }
        Command::Render {
            input,
            output,
            format,
            force,
            profile,
        } => {
            let limits = profile.limits();
            let plan = capability.decode(&read_bounded(input)?, &limits)?;
            counts.record(&plan);
            let wav_format = (*format).into();
            let stats = match range {
                Some(range) => export::render_wav_to_path_artifact_range_with_limits(
                    &plan, output, wav_format, *force, range, &limits,
                ),
                None => export::render_wav_to_path_artifact_with_limits(
                    &plan, output, wav_format, *force, &limits,
                ),
            }
            .map_err(CliError::from_export)?;
            Ok(CommandResult::render(
                "render", input, output, wav_format, stats,
            ))
        }
        Command::Build {
            input,
            output,
            format,
            force,
            project_root,
            profile,
        } => {
            let (input, root) = resolve_source_input(input.as_deref(), project_root.as_deref());
            let limits = profile.limits();
            let bundle = load_source_bundle(&input, root.as_deref())?;
            let plan = capability.compile(&bundle, &limits)?;
            counts.record(&plan);
            let wav_format = (*format).into();
            let stats = export::render_wav_to_path_artifact_with_limits(
                &plan, output, wav_format, *force, &limits,
            )
            .map_err(CliError::from_export)?;
            Ok(CommandResult::render(
                "build", &input, output, wav_format, stats,
            ))
        }
        Command::Deliver {
            input,
            delivery,
            output_dir,
            targets,
            force,
            project_root,
            profile,
        } => {
            let limits = profile.limits();
            let (resolved, root) = resolve_source_input(Some(input), project_root.as_deref());
            let bytes = read_bounded(&resolved)?;
            let plan = if bytes
                .iter()
                .copied()
                .find(|byte| !byte.is_ascii_whitespace())
                == Some(b'{')
            {
                capability.decode(&bytes, &limits)?
            } else {
                let bundle = load_source_bundle(&resolved, root.as_deref())?;
                capability.compile(&bundle, &limits)?
            };
            counts.record(&plan);
            let options = crate::production_delivery::DeliveryOptions {
                delivery_id: delivery.clone(),
                targets: targets.clone(),
                output_dir: output_dir.clone(),
                overwrite: *force,
                limits: match profile {
                    ProfileArg::Default => crate::production_delivery::DeliveryLimits::default(),
                    ProfileArg::Song => crate::production_delivery::DeliveryLimits::song(),
                },
            };
            let report = crate::production_delivery::deliver_artifact(&plan, &options, &limits)
                .map_err(|e| {
                    let mut result = CliError::new(e.code, e.message);
                    result.delivery = e.manifest;
                    result
                })?;
            Ok(CommandResult {
                ok: report.ok,
                command: "deliver".into(),
                input: resolved.display().to_string(),
                output: Some(output_dir.join(&report.manifest_file).display().to_string()),
                format: None,
                notes: Some(artifact_plan_stats(&plan).0),
                frames: Some(report.frames),
                digest: None,
                exports: None,
                catalog: None,
                instrument: None,
                library: None,
                libraries: None,
                delivery: Some(report),
            })
        }
        Command::Instruments {
            name,
            library,
            libraries,
        } => {
            if *libraries && (name.is_some() || library.is_some()) {
                return Err(CliError::new(
                    "E_USAGE",
                    "--libraries conflicts with a name or --library",
                ));
            }
            if *libraries {
                return Ok(CommandResult {
                    ok: true,
                    command: "instruments".into(),
                    input: "@builtin".into(),
                    output: None,
                    format: None,
                    notes: None,
                    frames: None,
                    digest: None,
                    exports: None,
                    catalog: None,
                    instrument: None,
                    library: None,
                    libraries: Some(crate::stdlib::libraries()),
                    delivery: None,
                });
            }
            let selected = library.as_deref().unwrap_or(crate::stdlib::BASIC_ID);
            let (catalog, instrument) = match name {
                Some(name) => (
                    None,
                    Some(
                        crate::stdlib::instrument_in(selected, name)
                            .map_err(|error| CliError::from_diagnostics(&error))?,
                    ),
                ),
                None => (
                    Some(
                        crate::stdlib::catalog_for(selected)
                            .map_err(|error| CliError::from_diagnostics(&error))?,
                    ),
                    None,
                ),
            };
            Ok(CommandResult {
                ok: true,
                command: "instruments".into(),
                input: name.as_deref().unwrap_or(selected).into(),
                output: None,
                format: None,
                notes: None,
                frames: None,
                digest: None,
                exports: catalog.as_ref().map(|catalog| catalog.instruments.len()),
                catalog,
                instrument,
                library: library.clone(),
                libraries: None,
                delivery: None,
            })
        }
        Command::Hash { input } => {
            let bytes = read_bounded(input)?;
            Ok(CommandResult::hash(input, sha256_digest(&bytes)))
        }
        Command::Module { command } => match command {
            ModuleCommand::Export {
                library,
                output,
                project_root,
                force,
            } => {
                let (input, root) = resolve_source_input(Some(library), project_root.as_deref());
                let bundle = load_source_bundle(&input, root.as_deref())?;
                let module = ModuleArtifact::from_source_bundle(&bundle)
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                let bytes = module
                    .to_json()
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                let digest = sha256_digest(&bytes);
                export::atomic_write(output, &bytes, *force).map_err(CliError::from_export)?;
                Ok(CommandResult::module(
                    "module export",
                    &input,
                    Some(output),
                    digest,
                    module.exports().len(),
                ))
            }
            ModuleCommand::Check {
                module,
                expect_hash,
            } => {
                let artifact = read_module_artifact(module)?;
                let digest = artifact
                    .digest()
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                verify_expected_module_hash(expect_hash.as_deref(), &digest)?;
                Ok(CommandResult::module(
                    "module check",
                    module,
                    None,
                    digest,
                    artifact.exports().len(),
                ))
            }
            ModuleCommand::Unpack {
                module,
                output_dir,
                expect_hash,
            } => {
                let artifact = read_module_artifact(module)?;
                let digest = artifact
                    .digest()
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                verify_expected_module_hash(expect_hash.as_deref(), &digest)?;
                unpack_module(&artifact, output_dir)?;
                Ok(CommandResult::module(
                    "module unpack",
                    module,
                    Some(output_dir),
                    digest,
                    artifact.exports().len(),
                ))
            }
        },
    }
}

/// Select the entry spelling and containment root without following a main
/// file symlink to choose its root. Explicit files retain their previous root
/// inference in load_source_bundle; explicit project roots always override.
fn resolve_source_input(
    input: Option<&Path>,
    project_root: Option<&Path>,
) -> (PathBuf, Option<PathBuf>) {
    let (entry, implicit_root) = match input {
        None => (PathBuf::from("main.maac"), Some(PathBuf::from("."))),
        Some(directory) if directory.is_dir() => {
            (directory.join("main.maac"), Some(directory.to_owned()))
        }
        Some(file) => (file.to_owned(), None),
    };
    (entry, project_root.map(Path::to_path_buf).or(implicit_root))
}

fn load_source_bundle(input: &Path, project_root: Option<&Path>) -> Result<SourceBundle, CliError> {
    let entry = fs::canonicalize(input).map_err(|error| {
        CliError::new(
            "E_IO",
            format!("cannot resolve {}: {error}", input.display()),
        )
    })?;
    let root = project_root
        .map(Path::to_path_buf)
        .or_else(|| entry.parent().map(Path::to_path_buf))
        .ok_or_else(|| CliError::new("E_REFERENCE", "entry source has no parent directory"))?;
    bundle_fs::load_bundle(&entry, &root).map_err(|error| CliError::from_diagnostics(&error))
}

fn bundle_entry_document(bundle: &SourceBundle) -> Result<Document, CliError> {
    let source = bundle.sources.get(&bundle.entry).ok_or_else(|| {
        CliError::new(
            "E_REFERENCE",
            format!("entry source `{}` is missing from the bundle", bundle.entry),
        )
    })?;
    parse(source).map_err(|error| CliError::from_diagnostics(&error))
}

pub fn read_source(path: &Path) -> Result<Document, CliError> {
    let bytes = read_bounded(path)?;
    let source = String::from_utf8(bytes).map_err(|error| {
        CliError::new("E_SYNTAX", format!("source is not UTF-8: {error}")).with_span(Some(
            Span::new(
                error.utf8_error().valid_up_to(),
                error.utf8_error().valid_up_to() + 1,
            ),
        ))
    })?;
    parse(&source).map_err(|error| CliError::from_diagnostics(&error))
}

pub fn read_plan(path: &Path) -> Result<Plan, CliError> {
    read_plan_with_limits(path, &PlanLimits::default())
}

/// Read the same bounded strict plan wire under a caller-selected allowance.
pub fn read_plan_with_limits(path: &Path, limits: &PlanLimits) -> Result<Plan, CliError> {
    let bytes = read_bounded(path)?;
    Plan::from_json_with_limits(&bytes, limits).map_err(CliError::from_plan)
}

/// Read and independently validate a legacy or version 3 plan artifact.
pub fn read_plan_versioned(path: &Path) -> Result<VersionedPlan, CliError> {
    read_plan_versioned_with_limits(path, &PlanLimits::default())
}

/// Read either supported plan version under an explicit caller allowance.
pub fn read_plan_versioned_with_limits(
    path: &Path,
    limits: &PlanLimits,
) -> Result<VersionedPlan, CliError> {
    VersionedPlan::from_json_with_limits(&read_bounded(path)?, limits).map_err(CliError::from_plan)
}

/// Read and independently validate a standalone artifact, including native kits.
pub fn read_plan_artifact(path: &Path) -> Result<PlanArtifact, CliError> {
    read_plan_artifact_with_limits(path, &PlanLimits::default())
}
/// Read a bounded artifact under explicit caller limits.
pub fn read_plan_artifact_with_limits(
    path: &Path,
    limits: &PlanLimits,
) -> Result<PlanArtifact, CliError> {
    PlanArtifact::from_json_with_limits(&read_bounded(path)?, limits).map_err(CliError::from_plan)
}
fn artifact_plan_stats(plan: &PlanArtifact) -> (usize, u64) {
    (
        plan.view()
            .events()
            .filter(|event| matches!(event.kind, crate::plan::EventKind::Note { .. }))
            .count(),
        plan.output().total_frames,
    )
}

pub fn read_bounded(path: &Path) -> Result<Vec<u8>, CliError> {
    let mut file = File::open(path).map_err(|error| {
        CliError::new("E_IO", format!("cannot read {}: {error}", path.display()))
    })?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CliError::new("E_IO", format!("cannot read {}: {error}", path.display()))
        })?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(CliError::new(
            "E_RESOURCE_LIMIT",
            format!("input exceeds {MAX_INPUT_BYTES} bytes: {}", path.display()),
        ));
    }
    Ok(bytes)
}

fn read_module_artifact(path: &Path) -> Result<ModuleArtifact, CliError> {
    let mut file = File::open(path).map_err(|error| {
        CliError::new("E_IO", format!("cannot read {}: {error}", path.display()))
    })?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_MODULE_ARTIFACT_JSON_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CliError::new("E_IO", format!("cannot read {}: {error}", path.display()))
        })?;
    if bytes.len() > MAX_MODULE_ARTIFACT_JSON_BYTES {
        return Err(CliError::new(
            "E_RESOURCE_LIMIT",
            format!(
                "module input exceeds {MAX_MODULE_ARTIFACT_JSON_BYTES} bytes: {}",
                path.display()
            ),
        ));
    }
    ModuleArtifact::from_json(&bytes).map_err(|error| CliError::from_diagnostics(&error))
}

fn verify_expected_module_hash(expected: Option<&str>, actual: &str) -> Result<(), CliError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    crate::bundle::validate_hash_pin(expected, "expected module hash")
        .map_err(|error| CliError::from_diagnostics(&error))?;
    if expected != actual {
        return Err(CliError::new(
            "E_HASH",
            format!("expected module hash `{expected}`, but artifact has `{actual}`"),
        ));
    }
    Ok(())
}

fn unpack_module(artifact: &ModuleArtifact, output_dir: &Path) -> Result<(), CliError> {
    if export::path_exists(output_dir).map_err(CliError::from_export)? {
        return Err(CliError::from_export(ExportError::OutputExists {
            path: output_dir.to_owned(),
        }));
    }
    let bundle = artifact
        .to_source_bundle()
        .map_err(|error| CliError::from_diagnostics(&error))?;
    let parent = output_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staging = tempfile::tempdir_in(parent).map_err(|error| {
        CliError::new(
            "E_IO",
            format!(
                "cannot stage module unpack in {}: {error}",
                parent.display()
            ),
        )
    })?;
    for (path, source) in &bundle.sources {
        write_module_member(staging.path(), path, source.as_bytes())?;
    }
    for (path, bytes) in &bundle.assets {
        write_module_member(staging.path(), path, bytes)?;
    }
    fs::rename(staging.path(), output_dir).map_err(|error| {
        CliError::new(
            "E_IO",
            format!(
                "cannot publish module directory {}: {error}",
                output_dir.display()
            ),
        )
    })?;
    Ok(())
}

fn write_module_member(root: &Path, logical_path: &str, bytes: &[u8]) -> Result<(), CliError> {
    let path = root.join(logical_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::new(
                "E_IO",
                format!("cannot create {}: {error}", parent.display()),
            )
        })?;
    }
    fs::write(&path, bytes)
        .map_err(|error| CliError::new("E_IO", format!("cannot write {}: {error}", path.display())))
}

pub fn format_human(result: &CommandResult) -> String {
    format_human_with_counts(result, 0, 0)
}
/// Format an artifact-aware result with actual note, hit, and audio clip counts.
pub fn format_human_artifact(result: &ArtifactCommandResult) -> String {
    format_human_with_counts(result.base(), result.hits(), result.audio_clips())
}
fn format_human_with_counts(result: &CommandResult, hits: usize, audio_clips: usize) -> String {
    if let Some(delivery) = &result.delivery {
        let mut message = format!(
            "deliver {} -> {} (artifacts: {}, checks: {})",
            result.input,
            result.output.as_deref().unwrap_or(&delivery.manifest_file),
            delivery.artifact_status,
            delivery.check_status
        );
        if audio_clips > 0 {
            message.push_str(&format!(" ({audio_clips} audio clips)"));
        }
        return message;
    }
    if let Some(libraries) = &result.libraries {
        return libraries
            .iter()
            .map(|library| format!("{}  {}", library.library, library.source_hash))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(instrument) = &result.instrument {
        let detail = format_instrument(instrument);
        return match &result.library {
            Some(library) => format!("{library}\n{detail}"),
            None => detail,
        };
    }
    if let Some(catalog) = &result.catalog {
        let mut groups =
            std::collections::BTreeMap::<&str, Vec<&crate::stdlib::InstrumentInfo>>::new();
        for instrument in &catalog.instruments {
            groups
                .entry(&instrument.family)
                .or_default()
                .push(instrument);
        }
        let mut message = format!(
            "{} ({} instruments)",
            catalog.library,
            catalog.instruments.len()
        );
        for (family, instruments) in groups {
            message.push_str(&format!("\n\n{family}:"));
            for instrument in instruments {
                message.push_str(&format!(
                    "\n  {} — {}",
                    instrument.name, instrument.description
                ));
            }
        }
        match &result.library {
            Some(library) => message.push_str(&format!("\n\nUse maac instruments --library {library} NAME for controls and a runnable composition.")),
            None => message.push_str("\n\nUse maac instruments NAME for controls and a runnable composition."),
        }
        return message;
    }
    if let Some(digest) = &result.digest {
        return digest.clone();
    }
    let mut message = format!("{} {}", result.command, result.input);
    if let Some(output) = &result.output {
        message.push_str(&format!(" -> {output}"));
    }
    if let Some(notes) = result.notes {
        if audio_clips > 0 && hits > 0 {
            message.push_str(&format!(
                " ({notes} notes, {hits} hits, {audio_clips} audio clips)"
            ));
        } else if audio_clips > 0 {
            message.push_str(&format!(" ({notes} notes, {audio_clips} audio clips)"));
        } else if hits > 0 {
            message.push_str(&format!(" ({notes} notes, {hits} hits)"));
        } else {
            message.push_str(&format!(" ({notes} notes)"));
        }
    }
    if let Some(frames) = result.frames {
        message.push_str(&format!(" ({frames} frames)"));
    }
    message
}

fn format_instrument(instrument: &crate::stdlib::InstrumentInfo) -> String {
    let guidance = &instrument.guidance;
    let mut message = format!(
        "{} ({}, {} channels)\n{}\n\nPitch: {}–{} (MIDI {}–{}). {}\nNote duration: {}–{} seconds. {}",
        instrument.name, instrument.family, instrument.channels, instrument.description,
        guidance.pitch_low, guidance.pitch_high, guidance.midi_min, guidance.midi_max,
        guidance.pitch_behavior, guidance.duration_min_seconds, guidance.duration_max_seconds,
        guidance.notes,
    );
    message.push_str(&format!(
        "\nTested pitches: {}\n\nControls:",
        guidance.tested_pitches.join(", ")
    ));
    for (name, control) in &instrument.controls {
        let unit = match control.unit {
            crate::graph::GraphUnit::Dimensionless => "dimensionless",
            crate::graph::GraphUnit::Seconds => "seconds",
            crate::graph::GraphUnit::Hertz => "hertz",
        };
        let rate = match control.rate {
            crate::graph::ParameterRate::Sample => "sample",
            crate::graph::ParameterRate::NoteOn => "note_on",
            crate::graph::ParameterRate::NoteOff => "note_off",
            crate::graph::ParameterRate::Reset => "reset",
        };
        message.push_str(&format!(
            "\n  {name}: default {} {unit}; range {}{}, {}{}; rate {rate}",
            control.default,
            if control.min_open { "(" } else { "[" },
            control.min,
            control.max,
            if control.max_open { ")" } else { "]" },
        ));
    }
    message.push_str(
        "\n\nSave the following as demo.maac, then run maac build demo.maac -o demo.wav:\n\n",
    );
    message.push_str(&instrument.usage);
    message
}

enum ParsedArgsError {
    Clap(clap::Error),
    Usage(CliError),
}

/// Parse the existing public CLI schema while adding range flags only to the
/// process-facing `render` command. Keeping the flags outside `Command`
/// preserves Rust callers that construct the established command enum.
fn parse_cli_with_render_range(
    args: Vec<std::ffi::OsString>,
) -> Result<(Cli, Option<FrameRange>), ParsedArgsError> {
    let mut command = Cli::command();
    let render = command
        .find_subcommand_mut("render")
        .expect("render command is declared");
    *render = render
        .clone()
        .arg(
            Arg::new("start-frame")
                .long("start-frame")
                .value_name("N")
                .value_parser(clap::value_parser!(u64))
                .help("reset-origin engine frame at which an excerpt starts"),
        )
        .arg(
            Arg::new("end-frame")
                .long("end-frame")
                .value_name("M")
                .value_parser(clap::value_parser!(u64))
                .help("exclusive reset-origin engine frame at which an excerpt ends"),
        );
    let matches = command
        .try_get_matches_from(args)
        .map_err(ParsedArgsError::Clap)?;
    let range = match matches.subcommand_matches("render") {
        Some(render) => match (
            render.get_one::<u64>("start-frame"),
            render.get_one::<u64>("end-frame"),
        ) {
            (None, None) => None,
            (Some(start), Some(end)) => Some(FrameRange::new(*start, *end)),
            _ => {
                return Err(ParsedArgsError::Usage(CliError::new(
                    "E_USAGE",
                    "--start-frame and --end-frame must be provided together",
                )))
            }
        },
        None => None,
    };
    let cli = Cli::from_arg_matches(&matches).map_err(ParsedArgsError::Clap)?;
    Ok((cli, range))
}

pub fn run_from_args<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let json_requested = args.iter().any(|arg| arg == "--json");
    let (cli, range) = match parse_cli_with_render_range(args) {
        Ok(parsed) => parsed,
        Err(ParsedArgsError::Clap(error)) => {
            let result = CliError::new("E_USAGE", error.to_string());
            let informational = matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            );
            if informational {
                print!("{error}");
            } else if json_requested {
                println!(
                    "{}",
                    serde_json::to_string(&result).expect("CLI error is serializable")
                );
            } else {
                eprint!("{error}");
            }
            return error.exit_code();
        }
        Err(ParsedArgsError::Usage(error)) => {
            if json_requested {
                println!(
                    "{}",
                    serde_json::to_string(&error).expect("CLI error is serializable")
                );
            } else {
                eprintln!("{error}");
            }
            return 1;
        }
    };
    let json = cli.json;
    match execute_artifact_with_range(&cli.command, range) {
        Ok(result) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&result).expect("CLI result is serializable")
                );
            } else {
                println!("{}", format_human_artifact(&result));
            }
            if result.base().ok {
                0
            } else {
                1
            }
        }
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&error).expect("CLI error is serializable")
                );
            } else {
                eprintln!("{error}");
            }
            1
        }
    }
}
