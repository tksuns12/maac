//! `maac analyze`: listening tools for AI producers.
//!
//! One render taps the project output and every source port that reaches it.
//! The report gives section-aware measurements named by authored IDs, a short
//! findings list, and optional spectrogram and piano-roll images. It never
//! changes the source, and the language and plan formats are unchanged. See
//! `docs/analyze-proposal.md`.

mod image;
mod meter;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use num_traits::{ToPrimitive, Zero};
use serde::Serialize;

use crate::exact::Rational;
use crate::music::MeterMap;
use crate::plan::{EventKind, PlanError, PlanLimits, PlanView, PortRef};
use crate::plan_artifact::PlanArtifact;
use crate::plan_v3::TimingContext;

use image::{Mark, Spectrogram};
use meter::{measures, Block, SourceMeter};

pub const SCHEMA: &str = "maac.analysis-report/1";
pub const PROFILE: &str = "maac.analyze.default/2";
pub const BANDS: &str = "maac.analyze.octave-bands/1";
pub const BAND_CENTERS_HZ: [f64; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

// Findings profile `maac.analyze.default/2`: stated limits, not taste.
const TRUE_PEAK_LIMIT_DBTP: f64 = -1.0;
const LOUDNESS_JUMP_LU: f64 = 6.0;
const BALANCE_LIMIT_DB: f64 = 3.0;
const MONO_LOSS_LIMIT_DB: f64 = -6.0;
/// Stereo findings ignore passages quieter than this.
const AUDIBLE_RMS_DBFS: f64 = -50.0;
const DOMINANCE_DB: f64 = 9.0;
/// Dominance is checked from 125 Hz through 4 kHz, in bands within this many
/// dB of the output's strongest band, against sources holding at least 2% of
/// the band.
const DOMINANCE_BANDS: std::ops::Range<usize> = 2..8;
const DOMINANCE_BAND_FLOOR_DB: f64 = -12.0;
const DOMINANCE_MIN_SHARE: f64 = 0.02;
const MAX_FINDINGS: usize = 50;
/// A section mixes swing feels when one part plays at least this many
/// straight off-beat eighths (x + 1/2q) and another this many swung ones
/// (x + 2/3q).
const GROOVE_MIN_ONSETS: usize = 4;

/// How the piece is divided into sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionMode {
    /// Regions when the source has any, otherwise 4-bar blocks.
    Auto,
    Regions,
    Bars(u32),
    /// No sections; only whole-piece measurements.
    Whole,
}

impl SectionMode {
    pub fn parse(text: &str) -> Result<Self, AnalyzeError> {
        match text {
            "auto" => Ok(Self::Auto),
            "regions" => Ok(Self::Regions),
            "whole" => Ok(Self::Whole),
            _ => text
                .strip_prefix("bars:")
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n > 0)
                .map(Self::Bars)
                .ok_or_else(|| {
                    AnalyzeError::new(
                        "E_USAGE",
                        "--section must be auto, regions, whole, or bars:N with N >= 1",
                    )
                }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnalyzeOptions {
    pub sections: SectionMode,
    /// Analyse only this score window instead of the whole piece.
    pub window: Option<Window>,
    /// With a window: start rendering this many seconds before it, from
    /// reset state, instead of at the start of the piece. Faster, but only
    /// approximate; see [`crate::dsp::RenderSpan`].
    pub preroll_seconds: Option<f64>,
    /// Ports to analyse besides the project output; empty selects every
    /// output port that reaches it.
    pub sources: Vec<PortRef>,
    /// Write spectrogram and piano-roll PNGs here.
    pub images: Option<PathBuf>,
    /// Replace existing image files.
    pub force: bool,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            sections: SectionMode::Auto,
            window: None,
            preroll_seconds: None,
            sources: Vec::new(),
            images: None,
            force: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AnalyzeError {
    pub code: String,
    pub message: String,
}

impl AnalyzeError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    fn plan(failure: PlanError) -> Self {
        Self::new(
            &failure.code,
            format!("{}: {}", failure.path, failure.message),
        )
    }
}

impl fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AnalyzeError {}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisReport {
    pub schema: &'static str,
    pub profile: &'static str,
    pub loudness_analyzer: &'static str,
    pub true_peak_profile: &'static str,
    pub bands: &'static str,
    pub band_centers_hz: Vec<f64>,
    pub plan_sha256: String,
    pub sample_rate_hz: u32,
    pub frames: u64,
    pub seconds: f64,
    /// Measurements aggregate blocks of this many frames (100 ms).
    pub block_frames: u64,
    pub findings: Vec<Finding>,
    #[serde(skip_serializing_if = "is_zero")]
    pub findings_omitted: usize,
    pub sections: Vec<Section>,
    pub sources: Vec<SourceReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageFile>,
    /// The analysed window, when not the whole piece.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowReport>,
}

/// A score window to analyse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Window {
    /// A declared region.
    Region(String),
    /// Bars `first` through `last`, inclusive.
    Bars(i64, i64),
}

impl Window {
    /// `region:ID` or `bars:FIRST-LAST`.
    pub fn parse(text: &str) -> Result<Self, AnalyzeError> {
        let usage = || {
            AnalyzeError::new(
                "E_USAGE",
                format!("window `{text}` must be region:ID or bars:FIRST-LAST"),
            )
        };
        if let Some(id) = text.strip_prefix("region:") {
            return if id.is_empty() {
                Err(usage())
            } else {
                Ok(Self::Region(id.to_owned()))
            };
        }
        let (first, last) = text
            .strip_prefix("bars:")
            .and_then(|range| range.split_once('-'))
            .ok_or_else(usage)?;
        let first: i64 = first.parse().map_err(|_| usage())?;
        let last: i64 = last.parse().map_err(|_| usage())?;
        if first < 1 || last < first {
            return Err(usage());
        }
        Ok(Self::Bars(first, last))
    }
}

/// The analysed window in the report.
#[derive(Clone, Debug, Serialize)]
pub struct WindowReport {
    pub start_q: String,
    pub end_q: String,
    pub start_frame: u64,
    pub end_frame: u64,
    /// The frame rendering started at: 0 for an exact window.
    pub render_start_frame: u64,
    /// True when rendering started after frame 0, so the measurements only
    /// approximate a full render's.
    pub approximate: bool,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Clone, Debug, Serialize)]
pub struct Section {
    pub id: String,
    /// `region`, `bars`, or `quarters` (fixed blocks when no meter is known).
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub start_q: String,
    pub end_q: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_bar: Option<i64>,
    pub start_frame: u64,
    pub end_frame: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceReport {
    /// `node:port`.
    pub port: String,
    /// `output` (the project output); `bus` (a mix point with two or more
    /// inputs, or a signal fed from one); `stem` (an independent source
    /// feeding a mix point or the output); or `chain` (an earlier stage of a
    /// source, such as a generator before its gain).
    pub role: &'static str,
    pub channels: u8,
    pub whole: Measures,
    pub sections: Vec<SectionMeasures>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SectionMeasures {
    pub section: String,
    #[serde(flatten)]
    pub measures: Measures,
}

#[derive(Clone, Debug, Serialize)]
pub struct Measures {
    pub integrated_lufs: Option<f64>,
    pub max_momentary_lufs: Option<f64>,
    pub max_short_term_lufs: Option<f64>,
    pub rms_dbfs: Option<f64>,
    pub crest_db: Option<f64>,
    pub sample_peak_dbfs: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    pub clipped_samples: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_clip: Option<Position>,
    /// Octave-band energy relative to this source's total, in dB.
    pub bands_db: Vec<Option<f64>>,
    /// This source's band energy as a share of the project output's. Shares
    /// are approximate, because later processing changes the sum.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master_share: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stereo: Option<Stereo>,
    pub active_fraction: f64,
    pub leading_silence_s: f64,
    pub trailing_silence_s: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stereo {
    pub balance_db: Option<f64>,
    pub correlation: Option<f64>,
    pub side_to_mid_db: Option<f64>,
    pub mono_loss_db: Option<f64>,
}

/// A rendered frame, with the bar and the quarter-note grid point at or
/// before it.
#[derive(Clone, Debug, Serialize)]
pub struct Position {
    pub frame: u64,
    pub seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bar: Option<i64>,
    pub q: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub kind: &'static str,
    /// `error`, `warning`, or `info`.
    pub severity: &'static str,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
    pub value: f64,
    pub threshold: f64,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImageFile {
    pub file: String,
    /// `spectrogram` or `piano_roll`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub width: usize,
    pub height: usize,
    pub frames_per_pixel: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_key: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_key: Option<i32>,
    pub sections: Vec<ImageSection>,
    /// Piano-roll colours by event target node, as `#rrggbb`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub legend: Vec<LegendEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LegendEntry {
    pub target: String,
    pub color: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImageSection {
    pub id: String,
    pub x0: usize,
    pub x1: usize,
}

pub(crate) fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub(crate) fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Converts frames to bar and quarter-note positions on a 1q grid.
pub(crate) struct PositionMap<'a> {
    rate: u32,
    grid: Vec<(Rational, u64)>,
    meter: Option<&'a MeterMap>,
}

impl PositionMap<'_> {
    pub(crate) fn at(&self, frame: u64) -> Position {
        let index = self.grid.partition_point(|(_, start)| *start <= frame);
        let q = self
            .grid
            .get(index.saturating_sub(1))
            .map(|(q, _)| q.clone())
            .unwrap_or_else(Rational::zero);
        Position {
            frame,
            seconds: Position::seconds_of(frame, self.rate),
            bar: self
                .meter
                .and_then(|meter| meter.q_to_bar(&q).ok())
                .map(|coordinate| coordinate.bar),
            q: q.to_string(),
        }
    }
}

/// The meter map of a composition document, for bar positions and bar
/// sections; `None` when it has no explicit meter.
pub fn meter_of(document: &crate::syntax::Document) -> Option<MeterMap> {
    crate::production_identity::meter_map(document).ok()
}

/// Analyse a compiled plan. `meter` comes from the source document; without
/// it, bar sections fall back to 16q blocks and positions carry no bar.
pub fn analyze_artifact(
    plan: &PlanArtifact,
    meter: Option<&MeterMap>,
    options: &AnalyzeOptions,
    limits: &PlanLimits,
) -> Result<AnalysisReport, AnalyzeError> {
    let bytes = plan
        .to_json_with_limits(limits)
        .map_err(AnalyzeError::plan)?;
    analyze_view(
        plan.view(),
        crate::bundle::sha256_digest(&bytes),
        meter,
        options,
        limits,
        |ports, span, callback| match span {
            Some(span) => crate::dsp::render_ports_artifact_span_with_limits(
                plan, limits, ports, span, callback,
            ),
            None => crate::dsp::render_ports_artifact_with_limits(plan, limits, ports, callback),
        },
    )
}

/// Analyse a disk-backed plan.
pub(crate) fn analyze_disk_media(
    plan: &crate::disk_media::DiskMediaPlan,
    meter: Option<&MeterMap>,
    options: &AnalyzeOptions,
    limits: &PlanLimits,
) -> Result<AnalysisReport, AnalyzeError> {
    let bytes = plan
        .artifact()
        .to_json_with_limits(limits)
        .map_err(AnalyzeError::plan)?;
    analyze_view(
        plan.view(),
        crate::bundle::sha256_digest(&bytes),
        meter,
        options,
        limits,
        |ports, span, callback| match span {
            Some(span) => plan.render_ports_span(ports, span, callback),
            None => plan.render_ports(ports, callback),
        },
    )
}

type RenderPorts<'c> = &'c mut dyn FnMut(&[Vec<f64>]) -> crate::dsp::Result<()>;

fn analyze_view(
    view: PlanView<'_>,
    plan_sha256: String,
    meter: Option<&MeterMap>,
    options: &AnalyzeOptions,
    limits: &PlanLimits,
    render: impl FnOnce(
        &[PortRef],
        Option<crate::dsp::RenderSpan>,
        RenderPorts<'_>,
    ) -> crate::dsp::Result<()>,
) -> Result<AnalysisReport, AnalyzeError> {
    let rate = view.output.sample_rate_hz;
    let frames = view.output.total_frames;
    if rate < 10 || frames == 0 {
        return Err(AnalyzeError::new(
            "E_RANGE",
            "analysis needs a sample rate of at least 10 Hz and at least one frame",
        ));
    }
    let hop = u64::from(rate / 10);
    let timing = TimingContext::new_with_limits(view.tempo, view.output, limits)
        .map_err(AnalyzeError::plan)?;
    let frame_at = |q: &Rational| -> Result<u64, AnalyzeError> {
        let frame = timing
            .frame_at_score(q, &Rational::zero(), u64::from(rate))
            .map_err(AnalyzeError::plan)?;
        Ok(frame.to_u64().unwrap_or(0).min(frames))
    };
    let positions = position_map(&view, meter, rate, &frame_at)?;
    let window = match &options.window {
        Some(window) => Some(resolve_window(&view, meter, window, &frame_at)?),
        None => None,
    };
    if options.preroll_seconds.is_some() && window.is_none() {
        return Err(AnalyzeError::new("E_USAGE", "--preroll needs --window"));
    }
    let (window_start, window_end) = window
        .as_ref()
        .map_or((0, frames), |(_, _, start, end)| (*start, *end));
    if window_start >= window_end {
        return Err(AnalyzeError::new("E_RANGE", "the analysis window is empty"));
    }
    let render_start = match options.preroll_seconds {
        Some(seconds) if seconds.is_finite() && seconds >= 0.0 => {
            window_start.saturating_sub((seconds * f64::from(rate)).round() as u64)
        }
        Some(_) => {
            return Err(AnalyzeError::new(
                "E_USAGE",
                "--preroll must be a nonnegative number of seconds",
            ))
        }
        None => 0,
    };
    let span = window.as_ref().map(|_| crate::dsp::RenderSpan {
        start: render_start,
        end: window_end,
    });
    let sections = clip_sections(
        sections(&view, meter, &options.sections, &frame_at)?,
        window.as_ref(),
    );
    let sources = sources(&view, &options.sources)?;
    let analysed_frames = window_end - window_start;

    let columns_hop = image::frames_per_column(analysed_frames);
    let ports: Vec<PortRef> = sources.iter().map(|source| source.port.clone()).collect();
    let mut meters: Vec<SourceMeter> = sources
        .iter()
        .map(|source| {
            let spectrogram = options
                .images
                .as_ref()
                .map(|_| Spectrogram::new(rate, columns_hop));
            SourceMeter::new(rate, source.channels, hop, spectrogram).starting_at(window_start)
        })
        .collect();
    let mut frame_index = render_start;
    render(&ports, span, &mut |frame| {
        if frame_index >= window_start {
            for (meter, values) in meters.iter_mut().zip(frame) {
                meter.push(values);
            }
        }
        frame_index += 1;
        Ok(())
    })
    .map_err(|failure| AnalyzeError::new("E_RENDER", failure.to_string()))?;
    let mut blocks = Vec::with_capacity(meters.len());
    let mut spectrograms = Vec::with_capacity(meters.len());
    for meter in meters {
        let (source_blocks, spectrogram) = meter
            .finish()
            .map_err(|message| AnalyzeError::new("E_NONFINITE", message))?;
        blocks.push(source_blocks);
        spectrograms.push(spectrogram);
    }

    let available = meter::available_bands(rate);
    let ranges: Vec<std::ops::Range<usize>> = sections
        .iter()
        .map(|section| block_range(&blocks[0], section.start_frame, section.end_frame))
        .collect();
    let reports: Vec<SourceReport> = sources
        .iter()
        .zip(&blocks)
        .enumerate()
        .map(|(index, (source, source_blocks))| {
            let output = (index != 0).then_some(blocks[0].as_slice());
            SourceReport {
                port: port_name(&source.port),
                role: source.role,
                channels: source.channels,
                whole: measures(
                    source_blocks,
                    source.channels,
                    rate,
                    hop,
                    available,
                    output,
                    &positions,
                ),
                sections: sections
                    .iter()
                    .zip(&ranges)
                    .map(|(section, range)| SectionMeasures {
                        section: section.id.clone(),
                        measures: measures(
                            &source_blocks[range.clone()],
                            source.channels,
                            rate,
                            hop,
                            available,
                            output.map(|output| &output[range.clone()]),
                            &positions,
                        ),
                    })
                    .collect(),
            }
        })
        .collect();
    let (findings, findings_omitted) = findings(
        &reports,
        &sections,
        &positions,
        &view,
        (window_start, window_end),
    );

    let images = match &options.images {
        Some(directory) => write_images(
            directory,
            options.force,
            &view,
            &reports,
            &spectrograms,
            &sections,
            window_start,
            analysed_frames,
            columns_hop,
        )?,
        None => Vec::new(),
    };

    Ok(AnalysisReport {
        schema: SCHEMA,
        profile: PROFILE,
        loudness_analyzer: crate::production_analysis::ANALYZER_ID,
        true_peak_profile: crate::production_analysis::TRUE_PEAK_PROFILE,
        bands: BANDS,
        band_centers_hz: BAND_CENTERS_HZ.to_vec(),
        plan_sha256,
        sample_rate_hz: rate,
        frames: analysed_frames,
        seconds: round3(analysed_frames as f64 / f64::from(rate)),
        block_frames: hop,
        findings,
        findings_omitted,
        sections,
        sources: reports,
        images,
        window: window.map(|(start_q, end_q, start_frame, end_frame)| WindowReport {
            start_q: start_q.to_string(),
            end_q: end_q.to_string(),
            start_frame,
            end_frame,
            render_start_frame: render_start,
            approximate: render_start > 0,
        }),
    })
}

/// The window's score bounds and frames, within the score.
fn resolve_window(
    view: &PlanView<'_>,
    meter: Option<&MeterMap>,
    window: &Window,
    frame_at: &dyn Fn(&Rational) -> Result<u64, AnalyzeError>,
) -> Result<(Rational, Rational, u64, u64), AnalyzeError> {
    let (start, end) = match window {
        Window::Region(id) => {
            let region = view
                .regions
                .iter()
                .find(|region| &region.id == id)
                .ok_or_else(|| {
                    AnalyzeError::new("E_REFERENCE", format!("no region `{id}` to analyse"))
                })?;
            (region.start_q.clone(), region.end_q.clone())
        }
        Window::Bars(first, last) => {
            let meter = meter.ok_or_else(|| {
                AnalyzeError::new("E_RANGE", "a bars window needs the source's meter")
            })?;
            let one = Rational::from_integer(1.into());
            let at = |bar: i64| {
                meter
                    .bar_to_q(bar, &one)
                    .map_err(|e| AnalyzeError::new("E_RANGE", e.to_string()))
            };
            (at(*first)?, at(*last + 1)?)
        }
    };
    let start = start.max(view.output.score_start_q.clone());
    let end = end.min(view.output.score_end_q.clone());
    if start >= end {
        return Err(AnalyzeError::new(
            "E_RANGE",
            "the analysis window does not overlap the score",
        ));
    }
    let (start_frame, end_frame) = (frame_at(&start)?, frame_at(&end)?);
    Ok((start, end, start_frame, end_frame))
}

/// Keep the sections that overlap the window, cut to it.
fn clip_sections(
    sections: Vec<Section>,
    window: Option<&(Rational, Rational, u64, u64)>,
) -> Vec<Section> {
    let Some((start_q, end_q, start, end)) = window else {
        return sections;
    };
    sections
        .into_iter()
        .filter(|section| section.end_frame > *start && section.start_frame < *end)
        .map(|mut section| {
            if section.start_frame < *start {
                section.start_frame = *start;
                section.start_q = start_q.to_string();
            }
            if section.end_frame > *end {
                section.end_frame = *end;
                section.end_q = end_q.to_string();
            }
            section
        })
        .collect()
}

fn position_map<'m>(
    view: &PlanView<'_>,
    meter: Option<&'m MeterMap>,
    rate: u32,
    frame_at: &dyn Fn(&Rational) -> Result<u64, AnalyzeError>,
) -> Result<PositionMap<'m>, AnalyzeError> {
    let one = Rational::from_integer(1.into());
    let mut grid = Vec::new();
    let mut q = view.output.score_start_q.clone();
    while q < view.output.score_end_q {
        grid.push((q.clone(), frame_at(&q)?));
        q += &one;
    }
    Ok(PositionMap { rate, grid, meter })
}

fn sections(
    view: &PlanView<'_>,
    meter: Option<&MeterMap>,
    mode: &SectionMode,
    frame_at: &dyn Fn(&Rational) -> Result<u64, AnalyzeError>,
) -> Result<Vec<Section>, AnalyzeError> {
    let start = &view.output.score_start_q;
    let end = &view.output.score_end_q;
    let bar_of = |q: &Rational| {
        meter
            .and_then(|meter| meter.q_to_bar(q).ok())
            .map(|c| c.bar)
    };
    let make = |id: String, kind, label, from: &Rational, to: &Rational| {
        Ok(Section {
            id,
            kind,
            label,
            start_q: from.to_string(),
            end_q: to.to_string(),
            start_bar: bar_of(from),
            start_frame: frame_at(from)?,
            end_frame: frame_at(to)?,
        })
    };
    let regions = || -> Result<Vec<Section>, AnalyzeError> {
        let mut regions: Vec<_> = view.regions.iter().collect();
        regions.sort_by(|a, b| (&a.start_q, &a.id).cmp(&(&b.start_q, &b.id)));
        regions
            .into_iter()
            .map(|region| {
                make(
                    region.id.clone(),
                    "region",
                    region.label.clone(),
                    &region.start_q,
                    &region.end_q,
                )
            })
            .collect()
    };
    let blocks = |bars: u32| -> Result<Vec<Section>, AnalyzeError> {
        let mut result = Vec::new();
        match meter {
            Some(meter) => {
                let one = Rational::from_integer(1.into());
                let mut bar = meter
                    .q_to_bar(start)
                    .map_err(|e| AnalyzeError::new("E_RANGE", e.to_string()))?
                    .bar;
                let mut from = start.clone();
                while &from < end {
                    let last = bar + i64::from(bars) - 1;
                    let to = meter
                        .bar_to_q(last + 1, &one)
                        .map_err(|e| AnalyzeError::new("E_RANGE", e.to_string()))?;
                    let to = if &to > end { end.clone() } else { to };
                    result.push(make(
                        format!("bars-{bar}-{last}"),
                        "bars",
                        None,
                        &from,
                        &to,
                    )?);
                    bar = last + 1;
                    from = to;
                }
            }
            None => {
                let width = Rational::from_integer((4 * i64::from(bars)).into());
                let mut from = start.clone();
                while &from < end {
                    let next = &from + &width;
                    let to = if &next > end { end.clone() } else { next };
                    result.push(make(format!("q{from}-{to}"), "quarters", None, &from, &to)?);
                    from = to;
                }
            }
        }
        Ok(result)
    };
    match mode {
        SectionMode::Whole => Ok(Vec::new()),
        SectionMode::Regions => regions(),
        SectionMode::Bars(n) => blocks(*n),
        SectionMode::Auto if !view.regions.is_empty() => regions(),
        SectionMode::Auto => blocks(4),
    }
}

/// The blocks whose first frame lies in `[start, end)`.
fn block_range(blocks: &[Block], start: u64, end: u64) -> std::ops::Range<usize> {
    let first = blocks.partition_point(|block| block.start < start);
    let last = blocks.partition_point(|block| block.start < end);
    first..last.max(first)
}

struct Source {
    port: PortRef,
    channels: u8,
    role: &'static str,
}

fn port_name(port: &PortRef) -> String {
    format!("{}:{}", port.node, port.port)
}

/// The project output first, then every connected output port upstream of it
/// (or the requested ports), in name order.
fn sources(view: &PlanView<'_>, requested: &[PortRef]) -> Result<Vec<Source>, AnalyzeError> {
    let output = &view.output.output;
    let mut upstream = BTreeSet::from([output.node.clone()]);
    loop {
        let before = upstream.len();
        for connection in view.connections {
            if upstream.contains(&connection.to.node) {
                upstream.insert(connection.from.node.clone());
            }
        }
        if upstream.len() == before {
            break;
        }
    }
    let mut inputs: BTreeMap<&str, usize> = BTreeMap::new();
    for connection in view.connections {
        *inputs.entry(connection.to.node.as_str()).or_default() += 1;
    }
    let is_mix =
        |node: &str| node == output.node || inputs.get(node).is_some_and(|count| *count >= 2);
    // Whether a mix point lies at or upstream of `node`.
    let fed_from_mix = |node: &str| {
        let mut seen = BTreeSet::from([node]);
        let mut pending = vec![node];
        while let Some(current) = pending.pop() {
            if current != output.node && is_mix(current) {
                return true;
            }
            for connection in view.connections {
                if connection.to.node == current && seen.insert(connection.from.node.as_str()) {
                    pending.push(connection.from.node.as_str());
                }
            }
        }
        false
    };
    let role = |port: &PortRef| -> &'static str {
        if fed_from_mix(&port.node) {
            "bus"
        } else if view
            .connections
            .iter()
            .any(|connection| &connection.from == port && is_mix(&connection.to.node))
        {
            "stem"
        } else {
            "chain"
        }
    };
    let candidates: BTreeSet<PortRef> = if requested.is_empty() {
        view.connections
            .iter()
            .filter(|connection| upstream.contains(&connection.to.node))
            .map(|connection| connection.from.clone())
            .filter(|port| port != output && !port.node.starts_with("__"))
            .collect()
    } else {
        requested
            .iter()
            .filter(|port| *port != output)
            .cloned()
            .collect()
    };
    let mut result = Vec::with_capacity(candidates.len() + 1);
    for (port, role) in
        std::iter::once((output.clone(), "output")).chain(candidates.into_iter().map(|port| {
            let role = role(&port);
            (port, role)
        }))
    {
        match view.audio_output_channels(&port) {
            Ok(channels @ 1..=2) => result.push(Source {
                port,
                channels,
                role,
            }),
            Ok(_) if requested.is_empty() && role != "output" => {}
            Err(_) if requested.is_empty() && role != "output" => {}
            Ok(channels) => {
                return Err(AnalyzeError::new(
                    "E_PORT_TYPE",
                    format!(
                        "`{}` has {channels} channels; analysis supports mono or stereo",
                        port_name(&port)
                    ),
                ))
            }
            Err(failure) => return Err(AnalyzeError::plan(failure)),
        }
    }
    Ok(result)
}

/// Parts that disagree about swing in one section: some play off-beat eighths
/// straight (x + 1/2q) while others swing them (x + 2/3q). At 90 bpm the two
/// positions are 111 ms apart.
fn groove_findings(
    view: &PlanView<'_>,
    sections: &[Section],
    positions: &PositionMap<'_>,
    (start, end): (u64, u64),
) -> Vec<Finding> {
    let half = Rational::new(1.into(), 2.into());
    let two_thirds = Rational::new(2.into(), 3.into());
    let whole = [(None, start, end)];
    let groups: Vec<(Option<&Section>, u64, u64)> = if sections.is_empty() {
        whole.to_vec()
    } else {
        sections
            .iter()
            .map(|s| (Some(s), s.start_frame, s.end_frame))
            .collect()
    };
    let mut found = Vec::new();
    for (section, start, end) in groups {
        // Per target node: (straight, swung) off-beat eighths.
        let mut parts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for event in view.events() {
            if matches!(event.kind, EventKind::Message { .. })
                || event.on_frame < start
                || event.on_frame >= end
            {
                continue;
            }
            let q = event.score_on_q;
            let fraction = q - Rational::from_integer(q.floor().to_integer());
            let counts = parts.entry(event.target.node.clone()).or_default();
            if fraction == half {
                counts.0 += 1;
            } else if fraction == two_thirds {
                counts.1 += 1;
            }
        }
        let straight: Vec<(&String, usize)> = parts
            .iter()
            .filter(|(_, c)| c.0 >= GROOVE_MIN_ONSETS)
            .map(|(p, c)| (p, c.0))
            .collect();
        let swung: Vec<(&String, usize)> = parts
            .iter()
            .filter(|(_, c)| c.1 >= GROOVE_MIN_ONSETS)
            .map(|(p, c)| (p, c.1))
            .collect();
        if straight.is_empty() || swung.is_empty() {
            continue;
        }
        let list = |parts: &[(&String, usize)]| {
            parts
                .iter()
                .map(|(part, count)| format!("`{part}` ({count})"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        // Name the part on the grid with fewer onsets.
        let total = |parts: &[(&String, usize)]| parts.iter().map(|(_, n)| n).sum::<usize>();
        let minority = if total(&straight) < total(&swung) {
            &straight
        } else {
            &swung
        };
        let (part, count) = minority
            .iter()
            .max_by_key(|(_, count)| *count)
            .expect("nonempty");
        found.push(Finding {
            kind: "groove_mismatch",
            severity: "warning",
            source: format!("{part}:events"),
            section: section.map(|s| s.id.clone()),
            at: section.map(|s| positions.at(s.start_frame)),
            value: *count as f64,
            threshold: GROOVE_MIN_ONSETS as f64,
            message: format!(
                "off-beat eighths are swung in {} but straight in {}",
                list(&swung),
                list(&straight)
            ),
        });
    }
    found
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    }
}

fn findings(
    reports: &[SourceReport],
    sections: &[Section],
    positions: &PositionMap<'_>,
    view: &PlanView<'_>,
    analysed: (u64, u64),
) -> (Vec<Finding>, usize) {
    let mut found = groove_findings(view, sections, positions, analysed);
    let output = &reports[0];
    let section_start = |id: &str| {
        sections
            .iter()
            .find(|section| section.id == id)
            .map(|section| positions.at(section.start_frame))
    };
    if output.whole.clipped_samples > 0 {
        found.push(Finding {
            kind: "clipping",
            severity: "error",
            source: output.port.clone(),
            section: output.whole.first_clip.as_ref().and_then(|clip| {
                sections
                    .iter()
                    .find(|s| s.start_frame <= clip.frame && clip.frame < s.end_frame)
                    .map(|s| s.id.clone())
            }),
            at: output.whole.first_clip.clone(),
            value: output.whole.clipped_samples as f64,
            threshold: 0.0,
            message: format!(
                "{} samples reach full scale; the first is at {}s",
                output.whole.clipped_samples,
                output.whole.first_clip.as_ref().map_or(0.0, |p| p.seconds)
            ),
        });
    }
    if let Some(peak) = output
        .whole
        .true_peak_dbtp
        .filter(|p| *p > TRUE_PEAK_LIMIT_DBTP)
    {
        found.push(Finding {
            kind: "true_peak_over",
            severity: "warning",
            source: output.port.clone(),
            section: None,
            at: None,
            value: peak,
            threshold: TRUE_PEAK_LIMIT_DBTP,
            message: format!("true peak {peak} dBTP exceeds {TRUE_PEAK_LIMIT_DBTP} dBTP"),
        });
    }
    // Loudness jumps between consecutive non-overlapping sections.
    let mut ordered: Vec<&Section> = sections.iter().collect();
    ordered.sort_by_key(|section| (section.start_frame, section.end_frame));
    for pair in ordered.windows(2) {
        if pair[1].start_frame < pair[0].end_frame {
            continue;
        }
        let level = |id: &str| {
            output
                .sections
                .iter()
                .find(|s| s.section == id)
                .and_then(|s| {
                    s.measures
                        .max_short_term_lufs
                        .or(s.measures.max_momentary_lufs)
                })
        };
        if let (Some(a), Some(b)) = (level(&pair[0].id), level(&pair[1].id)) {
            let jump = round2(b - a);
            if jump.abs() > LOUDNESS_JUMP_LU {
                found.push(Finding {
                    kind: "loudness_jump",
                    severity: "info",
                    source: output.port.clone(),
                    section: Some(pair[1].id.clone()),
                    at: section_start(&pair[1].id),
                    value: jump,
                    threshold: LOUDNESS_JUMP_LU,
                    message: format!(
                        "loudness changes by {jump} LU from section `{}` to `{}`",
                        pair[0].id, pair[1].id
                    ),
                });
            }
        }
    }
    // Stereo problems: the worst audible section of each stereo source.
    for report in reports.iter().filter(|r| r.channels == 2) {
        let audible = report.sections.iter().filter(|s| {
            s.measures
                .rms_dbfs
                .is_some_and(|rms| rms > AUDIBLE_RMS_DBFS)
        });
        let worst = |key: &dyn Fn(&Stereo) -> Option<f64>| {
            audible
                .clone()
                .filter_map(|s| s.measures.stereo.as_ref().and_then(key).map(|v| (s, v)))
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        };
        // Panning a single stem is a choice; imbalance matters in a mix.
        if let Some((section, balance)) =
            worst(&|s: &Stereo| s.balance_db).filter(|_| matches!(report.role, "output" | "bus"))
        {
            if balance.abs() > BALANCE_LIMIT_DB {
                found.push(Finding {
                    kind: "stereo_imbalance",
                    severity: "warning",
                    source: report.port.clone(),
                    section: Some(section.section.clone()),
                    at: section_start(&section.section),
                    value: balance,
                    threshold: BALANCE_LIMIT_DB,
                    message: if balance > 0.0 {
                        format!("left is {balance} dB above right")
                    } else {
                        format!("right is {} dB above left", -balance)
                    },
                });
            }
        }
        if let Some((section, loss)) = audible
            .clone()
            .filter_map(|s| {
                s.measures
                    .stereo
                    .as_ref()
                    .and_then(|stereo| stereo.mono_loss_db)
                    .map(|v| (s, v))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
        {
            if loss < MONO_LOSS_LIMIT_DB {
                found.push(Finding {
                    kind: "mono_cancellation",
                    severity: "warning",
                    source: report.port.clone(),
                    section: Some(section.section.clone()),
                    at: section_start(&section.section),
                    value: loss,
                    threshold: MONO_LOSS_LIMIT_DB,
                    message: format!("summing to mono loses {} dB", -loss),
                });
            }
        }
    }
    // A stem that dominates another stem in a prominent mid band.
    let leaves: Vec<&SourceReport> = reports.iter().filter(|r| r.role == "stem").collect();
    for (index, section) in sections.iter().enumerate() {
        let Some(output_bands) = output.sections.get(index).map(|s| &s.measures.bands_db) else {
            continue;
        };
        let strongest = output_bands
            .iter()
            .flatten()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        for band in DOMINANCE_BANDS {
            if output_bands[band].is_none_or(|db| db < strongest + DOMINANCE_BAND_FLOOR_DB) {
                continue;
            }
            let mut shares: Vec<(&SourceReport, f64)> = leaves
                .iter()
                .filter_map(|leaf| {
                    leaf.sections[index]
                        .measures
                        .master_share
                        .as_ref()
                        .and_then(|shares| shares[band])
                        .map(|share| (*leaf, share))
                })
                .collect();
            shares.sort_by(|a, b| b.1.total_cmp(&a.1));
            if let [(top, first), (second, next), ..] = shares[..] {
                if next >= DOMINANCE_MIN_SHARE {
                    let difference = round2(10.0 * (first / next).log10());
                    if difference >= DOMINANCE_DB {
                        found.push(Finding {
                            kind: "band_dominance",
                            severity: "info",
                            source: top.port.clone(),
                            section: Some(section.id.clone()),
                            at: Some(positions.at(section.start_frame)),
                            value: difference,
                            threshold: DOMINANCE_DB,
                            message: format!(
                                "`{}` is {difference} dB above `{}` in the {} Hz band",
                                top.port, second.port, BAND_CENTERS_HZ[band]
                            ),
                        });
                    }
                }
            }
        }
    }
    for report in reports.iter().filter(|r| r.role == "stem") {
        if !report.whole.is_active() {
            found.push(Finding {
                kind: "silent_source",
                severity: "warning",
                source: report.port.clone(),
                section: None,
                at: None,
                value: report.whole.rms_dbfs.unwrap_or(-120.0),
                threshold: -60.0,
                message: "this source never rises above -60 dBFS".into(),
            });
        }
    }
    found.sort_by(|a, b| {
        (
            severity_rank(a.severity),
            a.at.as_ref().map(|p| p.frame),
            &a.source,
        )
            .cmp(&(
                severity_rank(b.severity),
                b.at.as_ref().map(|p| p.frame),
                &b.source,
            ))
    });
    let omitted = found.len().saturating_sub(MAX_FINDINGS);
    found.truncate(MAX_FINDINGS);
    (found, omitted)
}

#[allow(clippy::too_many_arguments)]
fn write_images(
    directory: &Path,
    force: bool,
    view: &PlanView<'_>,
    reports: &[SourceReport],
    spectrograms: &[Option<Spectrogram>],
    sections: &[Section],
    origin: u64,
    frames: u64,
    hop: u64,
) -> Result<Vec<ImageFile>, AnalyzeError> {
    let io = |message: String| AnalyzeError::new("E_IO", message);
    std::fs::create_dir_all(directory)
        .map_err(|e| io(format!("cannot create `{}`: {e}", directory.display())))?;
    let width = image::columns(frames, hop) as usize;
    let image_sections: Vec<ImageSection> = sections
        .iter()
        .map(|section| ImageSection {
            id: section.id.clone(),
            x0: ((section.start_frame - origin) / hop) as usize,
            x1: ((section.end_frame - origin) / hop) as usize,
        })
        .collect();
    let boundaries: Vec<usize> = image_sections.iter().map(|s| s.x0).collect();
    let write = |name: &str, bytes: Vec<u8>| -> Result<(), AnalyzeError> {
        let path = directory.join(name);
        if path.exists() && !force {
            return Err(io(format!(
                "`{}` exists; pass --force to replace it",
                path.display()
            )));
        }
        std::fs::write(&path, bytes)
            .map_err(|e| io(format!("cannot write `{}`: {e}", path.display())))
    };
    let mut files = Vec::new();
    for (report, spectrogram) in reports.iter().zip(spectrograms) {
        let Some(spectrogram) = spectrogram else {
            continue;
        };
        let name = format!("spectrogram-{}.png", file_stem(&report.port));
        write(
            &name,
            image::png(width, image::ROWS, &spectrogram.image(width, &boundaries)),
        )?;
        files.push(ImageFile {
            file: name,
            kind: "spectrogram",
            source: Some(report.port.clone()),
            width,
            height: image::ROWS,
            frames_per_pixel: hop,
            min_hz: Some(image::MIN_HZ),
            max_hz: Some(round2(spectrogram.max_hz)),
            min_key: None,
            max_key: None,
            sections: image_sections.clone(),
            legend: Vec::new(),
        });
    }

    // Notes by pitch and hits in lanes, coloured by target node.
    let targets: BTreeMap<&str, usize> = view
        .events()
        .map(|event| event.target.node.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, node)| (node, index))
        .collect();
    let key_of = |pitch_hz: f64| (69.0 + 12.0 * (pitch_hz / 440.0).log2()).round() as i32;
    // A part that strikes one pitch at least four times (a synthesized drum)
    // gets its own lane instead of sharing that pitch's row.
    let mut pitches: BTreeMap<&str, (BTreeSet<i32>, usize)> = BTreeMap::new();
    for event in view.events() {
        if let &EventKind::Note { pitch_hz, .. } = event.kind {
            let entry = pitches.entry(event.target.node.as_str()).or_default();
            entry.0.insert(key_of(pitch_hz));
            entry.1 += 1;
        }
    }
    let drum = |node: &str| {
        pitches
            .get(node)
            .is_some_and(|(keys, n)| keys.len() == 1 && *n >= 4)
    };
    let mut lanes: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut marks = Vec::new();
    // Marks are relative to the window; events outside it are left out.
    let window_end = origin + frames;
    for event in view.events() {
        let color = targets[event.target.node.as_str()];
        let off = event.off_frame.unwrap_or(event.on_frame + hop);
        if off <= origin || event.on_frame >= window_end {
            continue;
        }
        let shift = |frame: u64| frame.clamp(origin, window_end) - origin;
        let (event_on, off) = (shift(event.on_frame), shift(off));
        let event_hit_off = shift(event.on_frame + hop);
        match event.kind {
            &EventKind::Note { .. } if drum(&event.target.node) => {
                let next = lanes.len();
                let lane = *lanes
                    .entry((event.target.node.clone(), String::new()))
                    .or_insert(next);
                marks.push(Mark {
                    on_frame: event_on,
                    off_frame: event_hit_off,
                    key: None,
                    lane,
                    color,
                });
            }
            &EventKind::Note { pitch_hz, .. } if pitch_hz > 0.0 => marks.push(Mark {
                on_frame: event_on,
                off_frame: off,
                key: Some(key_of(pitch_hz)),
                lane: 0,
                color,
            }),
            EventKind::Hit { key, .. } => {
                let next = lanes.len();
                let lane = *lanes
                    .entry((event.target.node.clone(), key.clone()))
                    .or_insert(next);
                marks.push(Mark {
                    on_frame: event_on,
                    off_frame: event_hit_off,
                    key: None,
                    lane,
                    color,
                });
            }
            _ => {}
        }
    }
    let (rgb, height, keys) = image::piano_roll(&marks, lanes.len(), width, hop, &boundaries);
    write("piano-roll.png", image::png(width, height, &rgb))?;
    files.push(ImageFile {
        file: "piano-roll.png".into(),
        kind: "piano_roll",
        source: None,
        width,
        height,
        frames_per_pixel: hop,
        min_hz: None,
        max_hz: None,
        min_key: keys.map(|(low, _)| low),
        max_key: keys.map(|(_, high)| high),
        sections: image_sections,
        legend: targets
            .iter()
            .map(|(target, &index)| LegendEntry {
                target: (*target).to_owned(),
                color: image::color_hex(index),
            })
            .collect(),
    });
    Ok(files)
}

fn file_stem(port: &str) -> String {
    port.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}
