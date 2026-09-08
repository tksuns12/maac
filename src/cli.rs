//! Public command-line orchestration. Filesystem reads and publication live
//! here; compiler and DSP modules receive parsed source/plans only.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{error::ErrorKind, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::compiler;
use crate::diagnostic::{Diagnostic, Diagnostics, Span};
use crate::dsp::RenderError;
use crate::export::{self, ExportError, WavFormat, WavStats, MAX_INPUT_BYTES};
use crate::plan::{Plan, PlanError};
use crate::syntax::{parse, Document};

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
    /// Parse, validate, and resolve a source document without writing output.
    Check { input: PathBuf },
    /// Compile source into a standalone versioned performance plan.
    Compile {
        input: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        force: bool,
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
    },
    /// Compile source and stream the resulting performance directly to WAV.
    Build {
        input: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = FormatArg::Float32)]
        format: FormatArg,
        #[arg(long)]
        force: bool,
    },
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
}

impl CliError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            code: code.into(),
            message: message.into(),
            path: None,
            span: None,
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
    match command {
        Command::Check { input } => {
            let document = read_source(input)?;
            let plan =
                compiler::compile(&document).map_err(|error| CliError::from_diagnostics(&error))?;
            Ok(CommandResult::check(
                input,
                plan.events.len(),
                plan.output.total_frames,
            ))
        }
        Command::Compile {
            input,
            output,
            force,
        } => {
            let document = read_source(input)?;
            let plan =
                compiler::compile(&document).map_err(|error| CliError::from_diagnostics(&error))?;
            let bytes = plan.to_json().map_err(CliError::from_plan)?;
            export::atomic_write(output, &bytes, *force).map_err(CliError::from_export)?;
            Ok(CommandResult::compile(
                input,
                output,
                plan.events.len(),
                plan.output.total_frames,
            ))
        }
        Command::Render {
            input,
            output,
            format,
            force,
        } => {
            let plan = read_plan(input)?;
            let wav_format = (*format).into();
            let stats = export::render_wav_to_path(&plan, output, wav_format, *force)
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
        } => {
            let document = read_source(input)?;
            let plan =
                compiler::compile(&document).map_err(|error| CliError::from_diagnostics(&error))?;
            let wav_format = (*format).into();
            let stats = export::render_wav_to_path(&plan, output, wav_format, *force)
                .map_err(CliError::from_export)?;
            Ok(CommandResult::render(
                "build", input, output, wav_format, stats,
            ))
        }
    }
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
    let bytes = read_bounded(path)?;
    Plan::from_json(&bytes).map_err(CliError::from_plan)
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

pub fn format_human(result: &CommandResult) -> String {
    let mut message = format!("{} {}", result.command, result.input);
    if let Some(output) = &result.output {
        message.push_str(&format!(" -> {output}"));
    }
    if let Some(notes) = result.notes {
        message.push_str(&format!(" ({notes} notes)"));
    }
    if let Some(frames) = result.frames {
        message.push_str(&format!(" ({frames} frames)"));
    }
    message
}

pub fn run_from_args<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let json_requested = args.iter().any(|arg| arg == "--json");
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
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
    };
    let json = cli.json;
    match execute(&cli.command) {
        Ok(result) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&result).expect("CLI result is serializable")
                );
            } else {
                println!("{}", format_human(&result));
            }
            0
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
