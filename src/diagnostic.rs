//! Stable diagnostics shared by the parser and later validation layers.

use std::fmt;

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// A half-open byte range into the original UTF-8 source text.
///
/// Spans deliberately use byte offsets rather than character offsets so that
/// they can be used directly with `str::get` and remain stable for source
/// locations containing Unicode strings.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }

    pub fn join(self, other: Self) -> Self {
        Self::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub fn slice(self, source: &str) -> &str {
        source.get(self.start..self.end).unwrap_or_default()
    }
}

impl Serialize for Span {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Span", 2)?;
        state.serialize_field("start", &self.start)?;
        state.serialize_field("end", &self.end)?;
        state.end()
    }
}

/// Stable diagnostic identifiers from section 23 of the MaaC/1
/// specification, plus parser-specific resource/syntax details.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticCode {
    Syntax,
    DuplicateId,
    DuplicateField,
    UnknownField,
    UnknownKind,
    Reference,
    Unit,
    Range,
    Tempo,
    MeterBoundary,
    Interval,
    TimePrecision,
    SubsampleNote,
    PatternCycle,
    InstanceTarget,
    AutomationWriter,
    Capability,
    PortType,
    AlgebraicLoop,
    Asset,
    Hash,
    VoiceLimit,
    RenderState,
    Nonfinite,
    Conflict,
    ResourceLimit,
    Version,
}

impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Syntax => "E_SYNTAX",
            Self::DuplicateId => "E_DUPLICATE_ID",
            Self::DuplicateField => "E_DUPLICATE_FIELD",
            Self::UnknownField => "E_UNKNOWN_FIELD",
            Self::UnknownKind => "E_UNKNOWN_KIND",
            Self::Reference => "E_REFERENCE",
            Self::Unit => "E_UNIT",
            Self::Range => "E_RANGE",
            Self::Tempo => "E_TEMPO",
            Self::MeterBoundary => "E_METER_BOUNDARY",
            Self::Interval => "E_INTERVAL",
            Self::TimePrecision => "E_TIME_PRECISION",
            Self::SubsampleNote => "E_SUBSAMPLE_NOTE",
            Self::PatternCycle => "E_PATTERN_CYCLE",
            Self::InstanceTarget => "E_INSTANCE_TARGET",
            Self::AutomationWriter => "E_AUTOMATION_WRITER",
            Self::Capability => "E_CAPABILITY",
            Self::PortType => "E_PORT_TYPE",
            Self::AlgebraicLoop => "E_ALGEBRAIC_LOOP",
            Self::Asset => "E_ASSET",
            Self::Hash => "E_HASH",
            Self::VoiceLimit => "E_VOICE_LIMIT",
            Self::RenderState => "E_RENDER_STATE",
            Self::Nonfinite => "E_NONFINITE",
            Self::Conflict => "E_CONFLICT",
            Self::ResourceLimit => "E_RESOURCE_LIMIT",
            Self::Version => "E_VERSION",
        }
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for DiagnosticCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// Severity is included in the public shape now so CLI clients can consume
/// one diagnostic representation as validation grows warnings.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One actionable error or warning. Paths are source object/field paths, not
/// filesystem paths; an empty vector means that no semantic path is known.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub message: String,
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub object_path: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub field_path: Vec<String>,
}

impl Diagnostic {
    pub fn error(code: DiagnosticCode, message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span,
            object_path: Vec::new(),
            field_path: Vec::new(),
        }
    }

    pub fn warning(code: DiagnosticCode, message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, message, span)
        }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn object_path<I, S>(mut self, path: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.object_path = path.into_iter().map(Into::into).collect();
        self
    }

    pub fn field_path<I, S>(mut self, path: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.field_path = path.into_iter().map(Into::into).collect();
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    pub fn code_str(&self) -> &'static str {
        self.code.as_str()
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if !self.object_path.is_empty() {
            write!(f, " [{}", self.object_path.join("."))?;
            if !self.field_path.is_empty() {
                write!(f, ".{}", self.field_path.join("."))?;
            }
            f.write_str("]")?;
        } else if !self.field_path.is_empty() {
            write!(f, " [{}]", self.field_path.join("."))?;
        }
        if let Some(span) = self.span {
            write!(f, " at {}..{}", span.start, span.end)?;
        }
        Ok(())
    }
}

/// A collection of diagnostics. Parser errors are returned as a collection so
/// duplicate declarations can be reported together without losing the first
/// useful location.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.items.push(diagnostic);
    }

    pub fn extend<I>(&mut self, diagnostics: I)
    where
        I: IntoIterator<Item = Diagnostic>,
    {
        self.items.extend(diagnostics);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }

    pub fn first(&self) -> Option<&Diagnostic> {
        self.items.first()
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(Diagnostic::is_error)
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a> IntoIterator for &'a Diagnostics {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.items.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            diagnostic.fmt(f)?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostics {}

impl Serialize for Diagnostics {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.items.serialize(serializer)
    }
}
