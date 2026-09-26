//! Public command-line orchestration. Filesystem reads and publication live
//! here; compiler and DSP modules receive parsed source/plans only.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{
    error::ErrorKind, Arg, ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum,
};
use serde::Serialize;

use crate::archive::ArchiveSnapshot;
use crate::archive_history::{ArchiveHistory, FreezeCheckStatus};
use crate::bundle::sha256_digest;
use crate::bundle::SourceBundle;
use crate::bundle_fs;
use crate::compiler;
use crate::diagnostic::{Diagnostic, Diagnostics, Span};
use crate::disk_media::DiskMediaProject;
use crate::dsp::RenderError;
use crate::export::{self, ExportError, FrameRange, WavFormat, WavStats, MAX_INPUT_BYTES};
use crate::media_import::{self, ImportedWav, WavImportRange};
use crate::plan::{Plan, PlanError, PlanLimits, PortRef};
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
    /// Apply one Protocol 2 transaction to source-preserving MaaC text.
    Patch {
        input: PathBuf,
        patch: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        project_root: Option<PathBuf>,
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
    /// Import a WAV file or a frame crop into a new editable MaaC project.
    ImportWav {
        input: PathBuf,
        /// Retain the exact snapshotted source bytes as original.wav.
        #[arg(long)]
        retain_original: bool,
        /// Inclusive source frame at which the crop begins.
        #[arg(long, requires = "end_frame")]
        start_frame: Option<u64>,
        /// Exclusive source frame at which the crop ends.
        #[arg(long, requires = "start_frame")]
        end_frame: Option<u64>,
        /// New directory to receive main.maac, media.pcm, and import.json.
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Verify a retained WAV import and compile its current project closure.
    VerifyImport { project: PathBuf },
    /// Create, verify, or unpack a native composition archive.
    Archive {
        #[command(subcommand)]
        command: ArchiveCommand,
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

#[derive(Subcommand, Debug)]
pub enum ArchiveCommand {
    /// Capture a verified composition closure in a new portable directory.
    Create {
        input: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        project_root: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = ProfileArg::Default)]
        profile: ProfileArg,
        /// Fully verified archive whose checkpoints become this archive's history.
        #[arg(long)]
        previous: Option<PathBuf>,
        /// Expected manifest hash of --previous.
        #[arg(long, requires = "previous")]
        expect_previous_hash: Option<String>,
        /// Retain a verified full-output float32 WAV beside this checkpoint.
        #[arg(long, conflicts_with = "freeze_node")]
        freeze_output: bool,
        /// Retain exact f64 output from one or more independent native effect nodes.
        #[arg(long)]
        freeze_node: Vec<String>,
        /// Retain an existing WAV import directory whose media.pcm is in the composition.
        #[arg(long = "retain-import")]
        retain_imports: Vec<PathBuf>,
    },
    /// Apply one Protocol 2 transaction to the archived head and retain its inverse.
    Patch {
        archive: PathBuf,
        patch: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        expect_hash: Option<String>,
    },
    /// Edit one directly imported local library and repin its entry import.
    PatchImport {
        archive: PathBuf,
        patch: PathBuf,
        #[arg(long = "import")]
        import_alias: String,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        expect_hash: Option<String>,
    },
    /// Verify a captured archive and its current reopenable composition.
    Verify {
        archive: PathBuf,
        #[arg(long)]
        expect_hash: Option<String>,
    },
    /// Restore verified source and dependency files into a new directory.
    Unpack {
        archive: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        expect_hash: Option<String>,
        /// Select an exact historical checkpoint ID (default: head).
        #[arg(long)]
        revision: Option<String>,
    },
    /// Check whether a stored output freeze still matches a current project.
    FreezeCheck {
        archive: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        project_root: Option<PathBuf>,
        #[arg(long)]
        revision: Option<String>,
        #[arg(long)]
        expect_hash: Option<String>,
        /// Render matching inputs again and compare exact output bytes.
        #[arg(long)]
        replay: bool,
    },
    /// Copy a current verified full-output freeze to a new WAV file.
    FreezeRender {
        archive: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        project_root: Option<PathBuf>,
        #[arg(long)]
        revision: Option<String>,
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
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub freeze: Option<Box<FreezeCheckReport>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FreezeCheckReport {
    pub integrity: &'static str,
    pub eligibility: &'static str,
    pub replay: &'static str,
    pub revision: String,
    pub source_digest: String,
    pub frozen_source_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<PortRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<FreezeNodeReport>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FreezeNodeReport {
    pub boundary: PortRef,
    pub freeze_digest: String,
    pub cache_digest: String,
    pub eligibility: &'static str,
    pub replay: &'static str,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    edit: Option<crate::editing::AppliedTransaction>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    archive_patch: Option<ArchivePatchReport>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    archive_import_patch: Option<ArchiveImportPatchReport>,
}

#[derive(Clone, Debug, Serialize)]
struct ArchivePatchReport {
    checkpoint: String,
    edit_digest: String,
}

#[derive(Clone, Debug, Serialize)]
struct ArchiveImportPatchReport {
    checkpoint: String,
    edit_digest: String,
    import_alias: String,
    import_source: String,
    before_revision: String,
    after_revision: String,
    pin: String,
    importer_before_revision: String,
    importer_after_revision: String,
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
    pub fn edit(&self) -> Option<&crate::editing::AppliedTransaction> {
        self.edit.as_ref()
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
            freeze: None,
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
            freeze: None,
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
            freeze: None,
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
            freeze: None,
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
            freeze: None,
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
            freeze: None,
        }
    }

    fn archive(
        command: &'static str,
        input: &Path,
        output: Option<&Path>,
        digest: String,
        version: u32,
    ) -> Self {
        Self {
            ok: true,
            command: command.into(),
            input: input.display().to_string(),
            output: output.map(|path| path.display().to_string()),
            format: Some(format!("maac.editable-archive/{version}")),
            notes: None,
            frames: None,
            digest: Some(digest),
            exports: None,
            catalog: None,
            instrument: None,
            library: None,
            libraries: None,
            delivery: None,
            freeze: None,
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
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub freeze: Option<Box<FreezeCheckReport>>,
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
            freeze: None,
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

    pub fn from_edit(error: crate::editing::EditError) -> Self {
        let mut result = Self::new(error.code, error.message);
        let mut path = error.object_path.join(".");
        if !error.field_path.is_empty() {
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(&error.field_path.join("."));
        }
        if !path.is_empty() {
            result.path = Some(path);
        }
        result
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
        edit: counts.edit,
        archive_patch: counts.archive_patch,
        archive_import_patch: counts.archive_import_patch,
    })
}

fn execute_disk_media(command: &Command) -> Result<ArtifactCommandResult, CliError> {
    let (input, project_root, profile) = match command {
        Command::Check {
            input,
            project_root,
            profile,
        }
        | Command::Build {
            input,
            project_root,
            profile,
            ..
        } => (input.as_deref(), project_root.as_deref(), *profile),
        _ => {
            return Err(CliError::new(
                "E_USAGE",
                "--disk-media supports check and build only",
            ))
        }
    };
    let (entry, implicit_root) = resolve_source_input(input, project_root);
    let root = implicit_root.unwrap_or_else(|| {
        entry
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let absolute_entry = if entry.is_absolute() {
        entry.clone()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CliError::new("E_IO", format!("cannot get working directory: {error}"))
            })?
            .join(&entry)
    };
    let project = DiskMediaProject::load(&absolute_entry, &root)
        .map_err(|error| CliError::from_diagnostics(&error))?;
    let limits = profile.limits();
    let plan = project
        .build_with_limits(&limits)
        .map_err(|error| CliError::from_diagnostics(&error))?;
    let (notes, hits, audio_clips) = plan.stats();
    let mut base = match command {
        Command::Check { .. } => CommandResult::check(&entry, notes, plan.output().total_frames),
        Command::Build {
            output,
            format,
            force,
            ..
        } => {
            let wav_format = (*format).into();
            let stats = export::render_wav_to_path_disk_media(&plan, output, wav_format, *force)
                .map_err(CliError::from_export)?;
            CommandResult::render("build", &entry, output, wav_format, stats)
        }
        _ => unreachable!("disk-media command was checked above"),
    };
    if hits > 0 || audio_clips > 0 {
        base.notes = Some(notes);
    }
    Ok(ArtifactCommandResult {
        base,
        hits,
        audio_clips,
        start_frame: None,
        end_frame: None,
        edit: None,
        archive_patch: None,
        archive_import_patch: None,
    })
}
#[derive(Default)]
struct EventCounts {
    notes: usize,
    hits: usize,
    audio_clips: usize,
    edit: Option<crate::editing::AppliedTransaction>,
    archive_patch: Option<ArchivePatchReport>,
    archive_import_patch: Option<ArchiveImportPatchReport>,
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
                freeze: None,
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
                    freeze: None,
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
                freeze: None,
            })
        }
        Command::Patch {
            input,
            patch,
            output,
            force,
            project_root,
        } => {
            let (resolved_input, root) = resolve_source_input(Some(input), project_root.as_deref());
            let bundle = load_source_bundle(&resolved_input, root.as_deref())?;
            let source = bundle.sources.get(&bundle.entry).ok_or_else(|| {
                CliError::new(
                    "E_REFERENCE",
                    "resolved patch source is missing from bundle",
                )
            })?;
            let mut document = crate::editing::SourceDocument::parse(source.clone())
                .map_err(CliError::from_edit)?;
            let context =
                crate::editing::BundleEditContext::new(&bundle).map_err(CliError::from_edit)?;
            let transaction = crate::editing::Transaction::from_json(&read_bounded(patch)?)
                .map_err(CliError::from_edit)?;
            let applied = document
                .apply(&transaction, &context)
                .map_err(CliError::from_edit)?;
            export::atomic_write(output, document.source().as_bytes(), *force)
                .map_err(CliError::from_export)?;
            counts.edit = Some(applied.clone());
            Ok(CommandResult {
                ok: true,
                command: "patch".into(),
                input: input.display().to_string(),
                output: Some(output.display().to_string()),
                format: Some("maac.edit/2".into()),
                notes: None,
                frames: None,
                digest: Some(applied.new_revision),
                exports: None,
                catalog: None,
                instrument: None,
                library: None,
                libraries: None,
                delivery: None,
                freeze: None,
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
        Command::ImportWav {
            input,
            retain_original,
            start_frame,
            end_frame,
            output_dir,
        } => {
            if export::path_exists(output_dir).map_err(CliError::from_export)? {
                return Err(CliError::from_export(ExportError::OutputExists {
                    path: output_dir.to_owned(),
                }));
            }
            let range = match (start_frame, end_frame) {
                (Some(start), Some(end)) => Some(WavImportRange::new(*start, *end)),
                (None, None) => None,
                _ => {
                    return Err(CliError::new(
                        "E_USAGE",
                        "WAV crop requires both --start-frame and --end-frame",
                    ))
                }
            };
            let imported =
                media_import::import_wav_file_with_retention(input, range, *retain_original)
                    .map_err(|error| CliError::new(error.code(), error.message().to_owned()))?;
            let bundle = imported.source_bundle();
            let plan = compiler::compile_bundle_artifact(&bundle)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            plan.to_json().map_err(CliError::from_plan)?;
            write_media_import_project(&imported, &bundle, output_dir)?;
            Ok(CommandResult {
                ok: true,
                command: "import-wav".into(),
                input: input.display().to_string(),
                output: Some(output_dir.display().to_string()),
                format: Some(format!(
                    "{}/{}",
                    media_import::MEDIA_IMPORT_FORMAT,
                    imported.manifest_version()
                )),
                notes: None,
                frames: Some(imported.frames()),
                digest: Some(imported.source_hash().into()),
                exports: None,
                catalog: None,
                instrument: None,
                library: None,
                libraries: None,
                delivery: None,
                freeze: None,
            })
        }
        Command::VerifyImport { project } => verify_import_project_with_hook(project, || {}),
        Command::Archive { command } => execute_archive(command, counts),
    }
}

fn execute_archive(
    command: &ArchiveCommand,
    counts: &mut EventCounts,
) -> Result<CommandResult, CliError> {
    match command {
        ArchiveCommand::Create {
            input,
            output_dir,
            project_root,
            profile,
            previous,
            expect_previous_hash,
            freeze_output,
            freeze_node,
            retain_imports,
        } => {
            let (entry, root, absolute_entry) =
                resolve_archive_source(input, project_root.as_deref())?;
            let profile = match profile {
                ProfileArg::Default => "default",
                ProfileArg::Song => "song",
            };
            let node_boundaries: Vec<PortRef> = freeze_node
                .iter()
                .map(|node| PortRef {
                    node: node.clone(),
                    port: "out".into(),
                })
                .collect();
            let node_boundary = node_boundaries.first();
            let mut history = if let Some(previous) = previous {
                ensure_output_outside_archive(output_dir, previous)?;
                let prior = ArchiveHistory::verify(previous)
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                verify_expected_archive_hash(expect_previous_hash.as_deref(), &prior.digest())?;
                prior
            } else {
                if node_boundaries.len() > 1 {
                    let snapshot = if retain_imports.is_empty() {
                        ArchiveSnapshot::capture(&absolute_entry, &root, profile)
                    } else {
                        ArchiveSnapshot::capture_with_imports(
                            &absolute_entry,
                            &root,
                            profile,
                            retain_imports,
                        )
                    }
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                    ArchiveHistory::from_snapshot_node_group(snapshot, &node_boundaries)
                } else if let Some(boundary) = node_boundary {
                    let snapshot = if retain_imports.is_empty() {
                        ArchiveSnapshot::capture(&absolute_entry, &root, profile)
                    } else {
                        ArchiveSnapshot::capture_with_imports(
                            &absolute_entry,
                            &root,
                            profile,
                            retain_imports,
                        )
                    }
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                    ArchiveHistory::from_snapshot_node_frozen(snapshot, boundary)
                } else if !retain_imports.is_empty() {
                    let snapshot = ArchiveSnapshot::capture_with_imports(
                        &absolute_entry,
                        &root,
                        profile,
                        retain_imports,
                    )
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                    ArchiveHistory::from_snapshot(snapshot, *freeze_output)
                } else if *freeze_output {
                    ArchiveHistory::capture_frozen(&absolute_entry, &root, profile)
                } else {
                    ArchiveHistory::capture(&absolute_entry, &root, profile)
                }
                .map_err(|error| CliError::from_diagnostics(&error))?
            };
            if previous.is_some() {
                let snapshot = if retain_imports.is_empty() {
                    ArchiveSnapshot::capture(&absolute_entry, &root, profile)
                } else {
                    ArchiveSnapshot::capture_with_imports(
                        &absolute_entry,
                        &root,
                        profile,
                        retain_imports,
                    )
                }
                .map_err(|error| CliError::from_diagnostics(&error))?;
                if node_boundaries.len() > 1 {
                    history.append_node_group(snapshot, &node_boundaries)
                } else if let Some(boundary) = node_boundary {
                    history.append_node_frozen(snapshot, boundary)
                } else if *freeze_output {
                    history.append_frozen(snapshot)
                } else {
                    history.append(snapshot)
                }
                .map_err(|error| CliError::from_diagnostics(&error))?;
            }
            stage_archive_directory(output_dir, |staging| history.stage(staging))?;
            Ok(CommandResult::archive(
                "archive create",
                &entry,
                Some(output_dir),
                history.digest(),
                history.version(),
            ))
        }
        ArchiveCommand::Patch {
            archive,
            patch,
            output_dir,
            expect_hash,
        } => {
            ensure_output_outside_archive(output_dir, archive)?;
            let mut history = ArchiveHistory::verify(archive)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            verify_expected_archive_hash(expect_hash.as_deref(), &history.digest())?;
            let applied = history
                .patch_head(&read_bounded(patch)?)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            stage_archive_directory(output_dir, |staging| history.stage(staging))?;
            counts.archive_patch = Some(ArchivePatchReport {
                checkpoint: applied.checkpoint,
                edit_digest: applied.edit_digest,
            });
            counts.edit = Some(applied.applied);
            Ok(CommandResult::archive(
                "archive patch",
                archive,
                Some(output_dir),
                history.digest(),
                history.version(),
            ))
        }
        ArchiveCommand::PatchImport {
            archive,
            patch,
            import_alias,
            output_dir,
            expect_hash,
        } => {
            ensure_output_outside_archive(output_dir, archive)?;
            let mut history = ArchiveHistory::verify(archive)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            verify_expected_archive_hash(expect_hash.as_deref(), &history.digest())?;
            let applied = history
                .patch_import_head(import_alias, &read_bounded(patch)?)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            stage_archive_directory(output_dir, |staging| history.stage(staging))?;
            let changed_source = applied.before_revision != applied.after_revision
                || applied.importer_before_revision != applied.importer_after_revision;
            let mut edit = applied.applied;
            if changed_source {
                edit.impact.render_invalidation_scope =
                    crate::editing::RenderInvalidationScope::Full;
                edit.impact.full_render_invalidated = true;
                edit.impact.all_expanded_events_may_be_affected = true;
            }
            counts.archive_import_patch = Some(ArchiveImportPatchReport {
                checkpoint: applied.checkpoint,
                edit_digest: applied.edit_digest,
                import_alias: import_alias.clone(),
                import_source: applied.import_source,
                before_revision: applied.before_revision,
                after_revision: applied.after_revision,
                pin: applied.pin,
                importer_before_revision: applied.importer_before_revision,
                importer_after_revision: applied.importer_after_revision,
            });
            counts.edit = Some(edit);
            Ok(CommandResult::archive(
                "archive patch-import",
                archive,
                Some(output_dir),
                history.digest(),
                history.version(),
            ))
        }
        ArchiveCommand::Verify {
            archive,
            expect_hash,
        } => {
            let history = ArchiveHistory::verify(archive)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            verify_expected_archive_hash(expect_hash.as_deref(), &history.digest())?;
            Ok(CommandResult::archive(
                "archive verify",
                archive,
                None,
                history.digest(),
                history.version(),
            ))
        }
        ArchiveCommand::Unpack {
            archive,
            output_dir,
            expect_hash,
            revision,
        } => {
            ensure_output_outside_archive(output_dir, archive)?;
            let history = ArchiveHistory::verify(archive)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            verify_expected_archive_hash(expect_hash.as_deref(), &history.digest())?;
            let snapshot = history
                .select(revision.as_deref())
                .map_err(|error| CliError::from_diagnostics(&error))?;
            stage_archive_directory(output_dir, |staging| snapshot.stage_members(staging))?;
            Ok(CommandResult::archive(
                "archive unpack",
                archive,
                Some(output_dir),
                history.digest(),
                history.version(),
            ))
        }
        ArchiveCommand::FreezeCheck {
            archive,
            source,
            project_root,
            revision,
            expect_hash,
            replay,
        } => {
            let history = ArchiveHistory::verify(archive)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            verify_expected_archive_hash(expect_hash.as_deref(), &history.digest())?;
            let (_, root, absolute_entry) =
                resolve_archive_source(source, project_root.as_deref())?;
            if history.node_group_info(revision.as_deref()).is_ok() {
                let check = history
                    .node_group_check(&absolute_entry, &root, revision.as_deref(), *replay)
                    .map_err(|error| CliError::from_diagnostics(&error))?;
                let eligibility = match check.status {
                    FreezeCheckStatus::Current => "current",
                    FreezeCheckStatus::Stale => "stale",
                };
                let replay = if check
                    .leaves
                    .iter()
                    .all(|leaf| leaf.replay_matches == Some(true))
                {
                    "matched"
                } else if check
                    .leaves
                    .iter()
                    .any(|leaf| leaf.replay_matches == Some(false))
                {
                    "mismatch"
                } else {
                    "not_requested"
                };
                let report = FreezeCheckReport {
                    integrity: "verified",
                    eligibility,
                    replay,
                    revision: check.revision,
                    source_digest: check.source_digest,
                    frozen_source_digest: check.frozen_source_digest,
                    output_digest: None,
                    reused: None,
                    boundary: None,
                    cache_digest: None,
                    nodes: Some(
                        check
                            .leaves
                            .into_iter()
                            .map(|leaf| FreezeNodeReport {
                                boundary: leaf.boundary,
                                freeze_digest: leaf.freeze_digest,
                                cache_digest: leaf.output_digest,
                                eligibility: if leaf.eligible { "current" } else { "stale" },
                                replay: match leaf.replay_matches {
                                    None => "not_requested",
                                    Some(true) => "matched",
                                    Some(false) => "mismatch",
                                },
                            })
                            .collect(),
                    ),
                };
                if eligibility == "stale" || replay == "mismatch" {
                    let mut error = if eligibility == "stale" {
                        CliError::new(
                            "E_FREEZE_STALE",
                            "stored freeze group is stale for this source",
                        )
                    } else {
                        CliError::new(
                            "E_FREEZE_REPLAY",
                            "replayed output differs from a stored node freeze",
                        )
                    };
                    error.freeze = Some(Box::new(report));
                    return Err(error);
                }
                let mut result = CommandResult::archive(
                    "archive freeze-check",
                    archive,
                    None,
                    history.digest(),
                    history.version(),
                );
                result.freeze = Some(Box::new(report));
                return Ok(result);
            }
            let node_freeze = history.node_freeze_info(revision.as_deref()).ok();
            let check = if node_freeze.is_some() {
                history.node_freeze_check(&absolute_entry, &root, revision.as_deref(), *replay)
            } else {
                history.freeze_check(&absolute_entry, &root, revision.as_deref(), *replay)
            }
            .map_err(|error| CliError::from_diagnostics(&error))?;
            let eligibility = match check.status {
                FreezeCheckStatus::Current => "current",
                FreezeCheckStatus::Stale => "stale",
            };
            let replay = match check.replay_matches {
                None => "not_requested",
                Some(true) => "matched",
                Some(false) => "mismatch",
            };
            let cache_digest = node_freeze.as_ref().map(|_| check.output_digest.clone());
            let report = FreezeCheckReport {
                integrity: "verified",
                eligibility,
                replay,
                revision: check.revision,
                source_digest: check.source_digest,
                frozen_source_digest: check.frozen_source_digest,
                output_digest: Some(check.output_digest),
                reused: None,
                boundary: node_freeze.map(|info| info.boundary),
                cache_digest,
                nodes: None,
            };
            if eligibility == "stale" {
                let mut error =
                    CliError::new("E_FREEZE_STALE", "stored freeze is stale for this source");
                error.freeze = Some(Box::new(report));
                return Err(error);
            }
            if replay == "mismatch" {
                let mut error = CliError::new(
                    "E_FREEZE_REPLAY",
                    "replayed output differs from the stored freeze",
                );
                error.freeze = Some(Box::new(report));
                return Err(error);
            }
            let mut result = CommandResult::archive(
                "archive freeze-check",
                archive,
                None,
                history.digest(),
                history.version(),
            );
            result.freeze = Some(Box::new(report));
            Ok(result)
        }
        ArchiveCommand::FreezeRender {
            archive,
            source,
            output,
            project_root,
            revision,
            expect_hash,
        } => {
            ensure_output_outside_archive(output, archive)?;
            if export::path_exists(output).map_err(CliError::from_export)? {
                return Err(CliError::from_export(ExportError::OutputExists {
                    path: output.clone(),
                }));
            }
            let history = ArchiveHistory::verify(archive)
                .map_err(|error| CliError::from_diagnostics(&error))?;
            verify_expected_archive_hash(expect_hash.as_deref(), &history.digest())?;
            let (_, root, absolute_entry) =
                resolve_archive_source(source, project_root.as_deref())?;
            if let Ok(info) = history.node_group_info(revision.as_deref()) {
                let mut activation_error = None;
                let mut activation_result = None;
                let (stats, final_digest) = export::render_wav_callback_to_path(
                    info.sample_rate_hz,
                    info.channels,
                    info.frames,
                    output,
                    |callback| match history.render_from_node_group(
                        &absolute_entry,
                        &root,
                        revision.as_deref(),
                        callback,
                    ) {
                        Ok(result) => {
                            activation_result = Some(result);
                            Ok(())
                        }
                        Err(error) => {
                            activation_error = Some(error);
                            Err(RenderError::Callback(
                                "node freeze group activation failed".into(),
                            ))
                        }
                    },
                )
                .map_err(|error| {
                    let core_failure = matches!(
                        &error,
                        ExportError::Render(RenderError::Callback(message))
                            if message == "node freeze group activation failed"
                    );
                    if core_failure {
                        let diagnostics = activation_error
                            .take()
                            .expect("node freeze group failure retains diagnostics");
                        let mut error = CliError::from_diagnostics(&diagnostics);
                        if error.code == "E_RENDER_STATE"
                            && error
                                .message
                                .starts_with("stored node freeze group is stale")
                        {
                            error.code = "E_FREEZE_STALE".into();
                        }
                        error
                    } else {
                        CliError::from_export(error)
                    }
                })?;
                let rendered =
                    activation_result.expect("successful node group render returns metadata");
                debug_assert_eq!(stats.frames, rendered.frames);
                debug_assert_eq!(info.leaves.len(), rendered.leaves.len());
                let mut result = CommandResult::archive(
                    "archive freeze-render",
                    archive,
                    Some(output),
                    history.digest(),
                    history.version(),
                );
                result.frames = Some(rendered.frames);
                result.freeze = Some(Box::new(FreezeCheckReport {
                    integrity: "verified",
                    eligibility: "current",
                    replay: "not_requested",
                    revision: rendered.revision,
                    source_digest: rendered.source_digest,
                    frozen_source_digest: rendered.frozen_source_digest,
                    output_digest: Some(final_digest),
                    reused: Some(true),
                    boundary: None,
                    cache_digest: None,
                    nodes: Some(
                        rendered
                            .leaves
                            .into_iter()
                            .map(|leaf| FreezeNodeReport {
                                boundary: leaf.boundary,
                                freeze_digest: leaf.freeze_digest,
                                cache_digest: leaf.output_digest,
                                eligibility: "current",
                                replay: "not_requested",
                            })
                            .collect(),
                    ),
                }));
                return Ok(result);
            }
            let node_freeze = history.node_freeze_info(revision.as_deref()).ok();
            let (rendered, output_digest, cache_digest, boundary) = if let Some(info) = node_freeze
            {
                let mut activation_error = None;
                let mut activation_result = None;
                let (stats, final_digest) = export::render_wav_callback_to_path(
                    info.sample_rate_hz,
                    info.channels,
                    info.frames,
                    output,
                    |callback| match history.render_from_node_freeze(
                        &absolute_entry,
                        &root,
                        revision.as_deref(),
                        callback,
                    ) {
                        Ok(result) => {
                            activation_result = Some(result);
                            Ok(())
                        }
                        Err(error) => {
                            activation_error = Some(error);
                            Err(RenderError::Callback(
                                "node freeze activation failed".into(),
                            ))
                        }
                    },
                )
                .map_err(|error| {
                    let core_failure = matches!(
                        &error,
                        ExportError::Render(RenderError::Callback(message))
                            if message == "node freeze activation failed"
                    );
                    if core_failure {
                        let diagnostics = activation_error
                            .take()
                            .expect("node freeze failure retains diagnostics");
                        let mut error = CliError::from_diagnostics(&diagnostics);
                        if error.code == "E_RENDER_STATE"
                            && error.message == "stored node freeze is stale for this source"
                        {
                            error.code = "E_FREEZE_STALE".into();
                        }
                        error
                    } else {
                        CliError::from_export(error)
                    }
                })?;
                let rendered = activation_result.expect("successful node render returns metadata");
                debug_assert_eq!(stats.frames, rendered.frames);
                let cache_digest = rendered.output_digest.clone();
                (
                    rendered,
                    final_digest,
                    Some(cache_digest),
                    Some(info.boundary),
                )
            } else {
                let rendered = history
                    .render_from_freeze(&absolute_entry, &root, revision.as_deref(), output)
                    .map_err(|error| {
                        let mut error = CliError::from_diagnostics(&error);
                        if error.code == "E_RENDER_STATE"
                            && error.message == "stored freeze is stale for this source"
                        {
                            error.code = "E_FREEZE_STALE".into();
                        } else if error.code == "E_CONFLICT" {
                            error.code = "E_OUTPUT_EXISTS".into();
                        }
                        error
                    })?;
                let digest = rendered.output_digest.clone();
                (rendered, digest, None, None)
            };
            let mut result = CommandResult::archive(
                "archive freeze-render",
                archive,
                Some(output),
                history.digest(),
                history.version(),
            );
            result.frames = Some(rendered.frames);
            result.freeze = Some(Box::new(FreezeCheckReport {
                integrity: "verified",
                eligibility: "current",
                replay: "not_requested",
                revision: rendered.revision,
                frozen_source_digest: rendered.frozen_source_digest,
                source_digest: rendered.source_digest,
                output_digest: Some(output_digest),
                reused: Some(true),
                boundary,
                cache_digest,
                nodes: None,
            }));
            Ok(result)
        }
    }
}

fn resolve_archive_source(
    input: &Path,
    project_root: Option<&Path>,
) -> Result<(PathBuf, PathBuf, PathBuf), CliError> {
    let (entry, implicit_root) = resolve_source_input(Some(input), project_root);
    let root = implicit_root.unwrap_or_else(|| {
        entry
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let absolute_entry = if entry.is_absolute() {
        entry.clone()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CliError::new("E_IO", format!("cannot get working directory: {error}"))
            })?
            .join(&entry)
    };
    Ok((entry, root, absolute_entry))
}

fn ensure_output_outside_archive(output: &Path, archive: &Path) -> Result<(), CliError> {
    let archive = fs::canonicalize(archive).map_err(|error| {
        CliError::new(
            "E_REFERENCE",
            format!("cannot resolve archive {}: {error}", archive.display()),
        )
    })?;
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|error| {
        CliError::new(
            "E_REFERENCE",
            format!("cannot resolve output parent {}: {error}", parent.display()),
        )
    })?;
    if parent.starts_with(&archive) {
        return Err(CliError::new(
            "E_REFERENCE",
            format!(
                "output {} is inside input archive {}",
                output.display(),
                archive.display()
            ),
        ));
    }
    Ok(())
}

fn stage_archive_directory(
    output_dir: &Path,
    stage: impl FnOnce(&Path) -> Result<(), Diagnostics>,
) -> Result<(), CliError> {
    if export::path_exists(output_dir).map_err(CliError::from_export)? {
        return Err(CliError::from_export(ExportError::OutputExists {
            path: output_dir.to_owned(),
        }));
    }
    let parent = output_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staging = tempfile::tempdir_in(parent).map_err(|error| {
        CliError::new(
            "E_IO",
            format!("cannot stage archive in {}: {error}", parent.display()),
        )
    })?;
    stage(staging.path()).map_err(|error| CliError::from_diagnostics(&error))?;
    publish_staged_directory_noclobber(staging.path(), output_dir, "archive")
}

fn verify_expected_archive_hash(expected: Option<&str>, actual: &str) -> Result<(), CliError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    crate::bundle::validate_hash_pin(expected, "expected archive hash")
        .map_err(|error| CliError::from_diagnostics(&error))?;
    if expected != actual {
        return Err(CliError::new(
            "E_HASH",
            format!("expected archive hash `{expected}`, but archive has `{actual}`"),
        ));
    }
    Ok(())
}

fn verify_import_project_with_hook(
    project: &Path,
    after_media: impl FnOnce(),
) -> Result<CommandResult, CliError> {
    let root = bundle_fs::ProjectRoot::open_pinned(project, cap_std::ambient_authority())
        .map_err(|error| CliError::from_diagnostics(&error))?;
    let imported = media_import::verify_retained_import_in_root(&root)
        .map_err(|error| CliError::new(error.code(), error.message().to_owned()))?;
    after_media();
    let bundle = bundle_fs::load_bundle_from_resolved_entry(Path::new("main.maac"), &root)
        .map_err(|error| CliError::from_diagnostics(&error))?;
    if bundle.assets.get("media.pcm").map(Vec::as_slice) != Some(imported.pcm_bytes()) {
        return Err(CliError::new(
            "E_IMPORT_MISMATCH",
            "current MaaC project does not depend on the verified media.pcm asset",
        ));
    }
    let plan = compiler::compile_bundle_artifact(&bundle)
        .map_err(|error| CliError::from_diagnostics(&error))?;
    plan.to_json().map_err(CliError::from_plan)?;
    Ok(CommandResult {
        ok: true,
        command: "verify-import".into(),
        input: project.display().to_string(),
        output: None,
        format: Some(format!(
            "{}/{}",
            media_import::MEDIA_IMPORT_FORMAT,
            media_import::MEDIA_IMPORT_RETAINED_VERSION
        )),
        notes: None,
        frames: Some(imported.frames()),
        digest: Some(imported.source_hash().into()),
        exports: None,
        catalog: None,
        instrument: None,
        library: None,
        libraries: None,
        delivery: None,
        freeze: None,
    })
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
    stage_module_members(staging.path(), &bundle)?;
    publish_staged_module_noclobber(staging.path(), output_dir)?;
    Ok(())
}

fn write_media_import_project(
    imported: &ImportedWav,
    bundle: &SourceBundle,
    output_dir: &Path,
) -> Result<(), CliError> {
    if export::path_exists(output_dir).map_err(CliError::from_export)? {
        return Err(CliError::from_export(ExportError::OutputExists {
            path: output_dir.to_owned(),
        }));
    }
    let source = bundle
        .sources
        .get("main.maac")
        .ok_or_else(|| CliError::new("E_INTERNAL", "generated media project has no main.maac"))?;
    let manifest = imported
        .manifest_json()
        .map_err(|error| CliError::new(error.code(), error.message().to_owned()))?;
    let parent = output_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staging = tempfile::tempdir_in(parent).map_err(|error| {
        CliError::new(
            "E_IO",
            format!(
                "cannot stage media project in {}: {error}",
                parent.display()
            ),
        )
    })?;
    fs::write(staging.path().join("main.maac"), source.as_bytes())
        .map_err(|error| CliError::new("E_IO", format!("cannot stage main.maac: {error}")))?;
    fs::write(staging.path().join("media.pcm"), imported.pcm_bytes())
        .map_err(|error| CliError::new("E_IO", format!("cannot stage media.pcm: {error}")))?;
    fs::write(staging.path().join("import.json"), manifest)
        .map_err(|error| CliError::new("E_IO", format!("cannot stage import.json: {error}")))?;
    if imported.manifest_version() == media_import::MEDIA_IMPORT_RETAINED_VERSION {
        imported
            .copy_original_to(&staging.path().join("original.wav"))
            .map_err(|error| CliError::new(error.code(), error.message().to_owned()))?;
    }
    publish_staged_directory_noclobber(staging.path(), output_dir, "media project")
}

fn stage_module_members(root: &Path, bundle: &SourceBundle) -> Result<(), CliError> {
    // Probe the real destination filesystem semantics in the staging directory
    // before copying content. This catches case-folding, Unicode-normalization,
    // and file/directory aliases that are distinct logical bundle paths.
    let mut directories = BTreeSet::new();
    for logical_path in bundle.sources.keys().chain(bundle.assets.keys()) {
        preflight_module_member(root, logical_path, &mut directories)?;
    }
    for (path, source) in &bundle.sources {
        write_staged_module_member(root, path, source.as_bytes())?;
    }
    for (path, bytes) in &bundle.assets {
        write_staged_module_member(root, path, bytes)?;
    }
    Ok(())
}

fn preflight_module_member(
    root: &Path,
    logical_path: &str,
    directories: &mut BTreeSet<String>,
) -> Result<(), CliError> {
    let components = logical_path.split('/').collect::<Vec<_>>();
    let mut physical = root.to_owned();
    let mut logical_directory = String::new();
    for component in &components[..components.len() - 1] {
        physical.push(component);
        if !logical_directory.is_empty() {
            logical_directory.push('/');
        }
        logical_directory.push_str(component);
        if directories.insert(logical_directory.clone()) {
            match fs::create_dir(&physical) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(module_path_collision(logical_path, &physical));
                }
                Err(error) => {
                    return Err(CliError::new(
                        "E_IO",
                        format!("cannot create {}: {error}", physical.display()),
                    ));
                }
            }
        }
    }

    let path = root.join(logical_path);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(module_path_collision(logical_path, &path))
        }
        Err(error) => Err(CliError::new(
            "E_IO",
            format!("cannot stage {}: {error}", path.display()),
        )),
    }
}

fn module_path_collision(logical_path: &str, physical_path: &Path) -> CliError {
    CliError::new(
        "E_CONFLICT",
        format!(
            "module member `{logical_path}` collides on the destination filesystem at {}",
            physical_path.display()
        ),
    )
}

fn write_staged_module_member(
    root: &Path,
    logical_path: &str,
    bytes: &[u8],
) -> Result<(), CliError> {
    let path = root.join(logical_path);
    fs::write(&path, bytes)
        .map_err(|error| CliError::new("E_IO", format!("cannot write {}: {error}", path.display())))
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn native_path(path: &Path) -> Result<std::ffi::CString, CliError> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        CliError::new(
            "E_REFERENCE",
            format!("path contains a null byte: {}", path.display()),
        )
    })
}

fn publish_staged_module_noclobber(staging: &Path, destination: &Path) -> Result<(), CliError> {
    publish_staged_directory_noclobber(staging, destination, "module")
}

#[cfg(target_vendor = "apple")]
fn publish_staged_directory_noclobber(
    staging: &Path,
    destination: &Path,
    artifact: &str,
) -> Result<(), CliError> {
    let staging_native = native_path(staging)?;
    let destination_native = native_path(destination)?;
    // SAFETY: both arguments are owned, NUL-terminated C strings that remain
    // alive for the call. RENAME_EXCL makes publication atomically no-replace.
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            staging_native.as_ptr(),
            libc::AT_FDCWD,
            destination_native.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    finish_staged_directory_publish(result, destination, artifact)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn publish_staged_directory_noclobber(
    staging: &Path,
    destination: &Path,
    artifact: &str,
) -> Result<(), CliError> {
    let staging_native = native_path(staging)?;
    let destination_native = native_path(destination)?;
    // SAFETY: both arguments are owned, NUL-terminated C strings that remain
    // alive for the call. RENAME_NOREPLACE makes publication atomically no-replace.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            staging_native.as_ptr(),
            libc::AT_FDCWD,
            destination_native.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    finish_staged_directory_publish(result, destination, artifact)
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn finish_staged_directory_publish(
    result: libc::c_int,
    destination: &Path,
    artifact: &str,
) -> Result<(), CliError> {
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(libc::EEXIST | libc::ENOTEMPTY)) {
        return Err(CliError::from_export(ExportError::OutputExists {
            path: destination.to_owned(),
        }));
    }
    Err(CliError::new(
        "E_IO",
        format!(
            "cannot publish {artifact} directory {}: {error}",
            destination.display()
        ),
    ))
}

#[cfg(target_os = "windows")]
fn publish_staged_directory_noclobber(
    staging: &Path,
    destination: &Path,
    artifact: &str,
) -> Result<(), CliError> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let staging_native = staging
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_native = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both pointers reference NUL-terminated buffers alive for the
    // call. Flags zero deliberately omits MOVEFILE_REPLACE_EXISTING.
    let result = unsafe { MoveFileExW(staging_native.as_ptr(), destination_native.as_ptr(), 0) };
    if result != 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(80 | 183)) {
        return Err(CliError::from_export(ExportError::OutputExists {
            path: destination.to_owned(),
        }));
    }
    Err(CliError::new(
        "E_IO",
        format!(
            "cannot publish {artifact} directory {}: {error}",
            destination.display()
        ),
    ))
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "windows"
)))]
fn publish_staged_directory_noclobber(
    _staging: &Path,
    destination: &Path,
    _artifact: &str,
) -> Result<(), CliError> {
    Err(CliError::new(
        "E_CAPABILITY",
        format!(
            "atomic no-replace directory publication is unavailable for {}",
            destination.display()
        ),
    ))
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

#[derive(Default)]
struct ProcessOptions {
    range: Option<FrameRange>,
    disk_media: bool,
}

/// Add process-only flags without changing the public Command enum used by
/// library callers.
fn parse_cli_with_process_options(
    args: Vec<std::ffi::OsString>,
) -> Result<(Cli, ProcessOptions), ParsedArgsError> {
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
    for name in ["check", "build"] {
        let subcommand = command
            .find_subcommand_mut(name)
            .expect("disk-media command is declared");
        *subcommand = subcommand.clone().arg(
            Arg::new("disk-media")
                .long("disk-media")
                .action(ArgAction::SetTrue)
                .help("verify and sample native PCM from private disk snapshots"),
        );
    }
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
    let disk_media = ["check", "build"].iter().any(|name| {
        matches
            .subcommand_matches(name)
            .is_some_and(|subcommand| subcommand.get_flag("disk-media"))
    });
    let cli = Cli::from_arg_matches(&matches).map_err(ParsedArgsError::Clap)?;
    Ok((cli, ProcessOptions { range, disk_media }))
}

pub fn run_from_args<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let json_requested = args.iter().any(|arg| arg == "--json");
    let (cli, options) = match parse_cli_with_process_options(args) {
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
    let result = if options.disk_media {
        execute_disk_media(&cli.command)
    } else {
        execute_artifact_with_range(&cli.command, options.range)
    };
    match result {
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

#[cfg(test)]
mod module_unpack_tests {
    use super::*;

    #[test]
    fn staged_directory_publication_never_replaces_a_concurrent_destination() {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir_in(root.path()).unwrap();
        fs::write(staging.path().join("module.maac"), b"module").unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("owner"), b"concurrent").unwrap();

        let error = publish_staged_module_noclobber(staging.path(), &destination).unwrap_err();
        assert_eq!(error.code, "E_OUTPUT_EXISTS");
        assert_eq!(fs::read(destination.join("owner")).unwrap(), b"concurrent");
        assert!(!destination.join("module.maac").exists());
    }
}

#[cfg(test)]
mod import_verification_tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};

    #[test]
    fn root_replacement_cannot_combine_media_and_source_from_different_directories() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.wav");
        let mut writer = WavWriter::create(
            &input,
            WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        writer.write_sample(100i16).unwrap();
        writer.finalize().unwrap();
        let project = directory.path().join("project");
        execute(&Command::ImportWav {
            input,
            retain_original: true,
            start_frame: None,
            end_frame: None,
            output_dir: project.clone(),
        })
        .unwrap();

        let replacement = directory.path().join("replacement");
        let archived = directory.path().join("archived");
        fs::create_dir(&replacement).unwrap();
        let failure = verify_import_project_with_hook(&project, || {
            fs::rename(project.join("main.maac"), replacement.join("main.maac")).unwrap();
            fs::rename(project.join("media.pcm"), replacement.join("media.pcm")).unwrap();
            fs::rename(&project, &archived).unwrap();
            fs::rename(&replacement, &project).unwrap();
        })
        .unwrap_err();
        assert_eq!(failure.code, "E_REFERENCE");
        assert!(!archived.join("main.maac").exists());
        assert!(!project.join("import.json").exists());
    }
}
