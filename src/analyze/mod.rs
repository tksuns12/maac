//! `maac analyze`: listening tools for AI producers.
//!
//! One render taps the project output and every source port that reaches it.
//! The report gives section-aware measurements named by authored IDs, a short
//! findings list, and optional spectrogram and piano-roll images. It never
//! changes the source, and the language and plan formats are unchanged. See
//! `docs/analyze.md`.

mod dynamics;
mod image;
mod lead;
mod meter;
mod notes;

pub use dynamics::Dynamics;
pub use lead::{LeadNote, LeadSummary};
pub use notes::{NoteReport, NoteSummary};

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
use meter::{
    band_energy, block_range_lu, db_power, floor_blocks, floor_energy, floor_loudness, loudness_of,
    measures, presence, Block, SourceMeter,
};

pub const SCHEMA: &str = "maac.analysis-report/1";
pub const PROFILE: &str = "maac.analyze.default/6";
pub const BANDS: &str = "maac.analyze.octave-bands/1";
pub const BAND_CENTERS_HZ: [f64; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

// Findings profile `maac.analyze.default/6`: stated limits, not taste.
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
/// A stem is steady when its block range is at most this many LU and it is
/// above -70 LUFS in at least this fraction of the section's blocks.
const NOISE_MAX_RANGE_LU: f64 = 6.0;
const NOISE_MIN_PRESENCE: f64 = 0.9;
/// In the output's floor blocks, a steady stem is exposed when it supplies at
/// least this share of its mix point's energy in a band within
/// `NOISE_BAND_SPAN_DB` of the mix point's strongest band...
const NOISE_MIN_SHARE: f64 = 0.1;
const NOISE_BAND_SPAN_DB: f64 = 30.0;
/// ...and the stem's estimated loudness at the output in those blocks is
/// above this.
const NOISE_AUDIBLE_LUFS: f64 = -50.0;
/// A note is silent when its target's K-weighted loudness over the note is
/// at most this.
const SILENT_NOTE_LUFS: f64 = -70.0;
/// A note is buried when its target holds at most this share of its mix
/// point in the written pitch's octave band (broadband for a hit), in dB.
const BURIED_NOTE_DB: f64 = -20.0;
/// A lone note sounds off its written pitch beyond this many cents.
const PITCH_LIMIT_CENTS: f64 = 50.0;
/// A lead is under its accompaniment in a section when the median of its
/// notes' margins is at most this many dB. The owner's listening round put it
/// between a lead that still led (-7.2 dB) and one that was buried (-10.1 dB).
const LEAD_UNDER_DB: f64 = -8.5;
/// ...over at least this many of the lead's notes in the section.
const LEAD_MIN_NOTES: usize = 4;

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
    /// List every measured note in the report.
    pub notes: bool,
    /// Parts meant to lead, each compared with the rest of its mix point.
    pub leads: Vec<PortRef>,
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
            notes: false,
            leads: Vec::new(),
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
    /// Every measured note, with `--notes`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteReport>,
    /// The ports named with `--lead`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub leads: Vec<String>,
    /// Every measured lead note, with `--notes`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lead_notes: Vec<LeadNote>,
    /// The compressors and limiters between the last mix point and the
    /// project output, in signal order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub master_chain: Vec<String>,
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
    /// Loudness of the quietest tenth of the blocks above -70 LUFS: the floor
    /// that quiet passages fall to. `None` with under one second of them.
    pub floor_lufs: Option<f64>,
    /// The 95th percentile of block loudness minus the floor percentile. A
    /// steady source has a small range.
    pub block_range_lu: Option<f64>,
    /// Octave-band energy of the floor blocks, relative to their total.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor_bands_db: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stereo: Option<Stereo>,
    /// Score–audio agreement for the notes and hits this source receives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<NoteSummary>,
    /// For a lead, its margin over the rest of its mix point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<LeadSummary>,
    /// For a compressor or limiter, the gain it takes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamics: Option<Dynamics>,
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
    /// The source address of the note a score–audio finding names first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
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
        |ports, taps, span, callback| {
            crate::dsp::render_ports_artifact_tapped_with_limits(
                plan, limits, ports, taps, span, callback,
            )
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
        |ports, taps, span, callback| plan.render_ports_tapped(ports, taps, span, callback),
    )
}

type RenderPorts<'c> = &'c mut dyn FnMut(&[Vec<f64>], &[f64]) -> crate::dsp::Result<()>;

fn analyze_view(
    view: PlanView<'_>,
    plan_sha256: String,
    meter: Option<&MeterMap>,
    options: &AnalyzeOptions,
    limits: &PlanLimits,
    render: impl FnOnce(
        &[PortRef],
        &[String],
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
    let mut lead_ports: Vec<PortRef> = Vec::new();
    for port in &options.leads {
        if !lead_ports.contains(port) {
            lead_ports.push(port.clone());
        }
    }
    let master_chain = dynamics::master_chain(&view);
    let sources = sources(&view, &options.sources, &lead_ports, &master_chain)?;
    let leads = resolve_leads(&sources, &lead_ports)?;
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
    let mut windows = note_windows(&view, &sources, rate, window_start, window_end);
    let mut next_window = 0;
    let mut active: Vec<usize> = Vec::new();
    let mut lead_windows = lead_windows(&view, &sources, &leads, rate, window_start, window_end);
    let mut next_lead = 0;
    let mut lead_active: Vec<usize> = Vec::new();
    let mut taps: Vec<dynamics::Tap> = sources
        .iter()
        .enumerate()
        .filter(|(_, source)| dynamics::is_dynamics(&view, &source.port.node))
        .map(|(index, _)| dynamics::Tap::new(index, sections.len()))
        .collect();
    let tap_nodes: Vec<String> = taps
        .iter()
        .map(|tap| sources[tap.source].port.node.clone())
        .collect();
    // The sections each frame lies in, for the taps.
    let mut section_order: Vec<usize> = (0..sections.len()).collect();
    section_order.sort_by_key(|&index| sections[index].start_frame);
    let mut next_section = 0;
    let mut in_sections: Vec<usize> = Vec::new();
    let mut values = vec![notes::FrameValues::default(); meters.len()];
    let mut frame_index = render_start;
    render(&ports, &tap_nodes, span, &mut |frame, reductions| {
        if frame_index >= window_start {
            for ((meter, frame), value) in meters.iter_mut().zip(frame).zip(values.iter_mut()) {
                *value = meter.push(frame);
            }
            if !taps.is_empty() {
                while next_section < section_order.len()
                    && sections[section_order[next_section]].start_frame <= frame_index
                {
                    in_sections.push(section_order[next_section]);
                    next_section += 1;
                }
                in_sections.retain(|&index| frame_index < sections[index].end_frame);
                for (tap, &reduction) in taps.iter_mut().zip(reductions) {
                    tap.add(values[tap.source].k, reduction, &in_sections);
                }
            }
            while next_window < windows.len() && windows[next_window].start <= frame_index {
                active.push(next_window);
                next_window += 1;
            }
            active.retain(|&index| {
                let window = &mut windows[index];
                if frame_index >= window.end {
                    return false;
                }
                window.add(frame_index, &values);
                true
            });
            while next_lead < lead_windows.len() && lead_windows[next_lead].start <= frame_index {
                lead_active.push(next_lead);
                next_lead += 1;
            }
            lead_active.retain(|&index| {
                let window = &mut lead_windows[index];
                if frame_index >= window.end {
                    return false;
                }
                window.add(&values);
                true
            });
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
    let mut reports: Vec<SourceReport> = sources
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
    let note_list = note_reports(&windows, &sources, &sections, rate);
    let mut extra = note_findings(&note_list, &positions);
    attach_note_summaries(&mut reports, &note_list);
    let lead_notes = lead_reports(&lead_windows, &leads, &sections);
    extra.extend(lead_findings(&lead_notes, &leads, &positions));
    attach_lead_summaries(&mut reports, &lead_notes, &leads);
    for tap in &taps {
        let report = &mut reports[tap.source];
        report.whole.dynamics = Some(tap.whole.summary());
        for (section, span) in report.sections.iter_mut().zip(&tap.sections) {
            section.measures.dynamics = Some(span.summary());
        }
    }
    let noise = noise_findings(
        &sources, &blocks, &sections, &ranges, hop, available, &positions,
    );
    extra.extend(noise);
    let (findings, findings_omitted) = findings(
        extra,
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
        notes: if options.notes { note_list } else { Vec::new() },
        leads: leads.iter().map(|lead| lead.port.clone()).collect(),
        lead_notes: if options.notes {
            lead_notes
        } else {
            Vec::new()
        },
        master_chain,
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
    /// For a stem, the mix-point nodes it feeds directly.
    mixes: Vec<String>,
    /// The first mix point downstream of this port, or the output.
    mix_point: Option<String>,
    /// Whether a node that receives score events lies at or upstream of this
    /// port, so that notes or hits drive what it plays.
    driven: bool,
    /// For a port named with `--lead`, where it is read and whose notes it
    /// plays.
    lead: Option<LeadRoute>,
}

struct LeadRoute {
    /// The mix point the lead is compared at.
    mix: String,
    /// The nodes at or upstream of the lead that receive notes.
    nodes: BTreeSet<String>,
}

fn port_name(port: &PortRef) -> String {
    format!("{}:{}", port.node, port.port)
}

/// The project output first, then every connected output port upstream of it
/// (or the requested ports, the leads and their mix points, and the master
/// chain), in name order.
fn sources(
    view: &PlanView<'_>,
    requested: &[PortRef],
    leads: &[PortRef],
    chain: &[String],
) -> Result<Vec<Source>, AnalyzeError> {
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
    // Breadth first downstream, in connection order.
    let mix_point = |node: &str| -> Option<String> {
        let mut seen = BTreeSet::from([node]);
        let mut pending = std::collections::VecDeque::from([node]);
        while let Some(current) = pending.pop_front() {
            for connection in view.connections {
                if connection.from.node == current && seen.insert(connection.to.node.as_str()) {
                    if is_mix(&connection.to.node) {
                        return Some(connection.to.node.clone());
                    }
                    pending.push_back(connection.to.node.as_str());
                }
            }
        }
        None
    };
    let targets: BTreeSet<&str> = view
        .events()
        .map(|event| event.target.node.as_str())
        .collect();
    let driven = |node: &str| {
        let mut seen = BTreeSet::from([node]);
        let mut pending = vec![node];
        while let Some(current) = pending.pop() {
            if targets.contains(current) {
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
    let routes = lead_routes(view, leads, &upstream, &is_mix)?;
    // The output port of a lead's mix point; the project output is always
    // analysed.
    let mix_port = |mix: &str| -> Option<PortRef> {
        view.connections
            .iter()
            .find(|connection| {
                connection.from.node == mix && upstream.contains(&connection.to.node)
            })
            .map(|connection| connection.from.clone())
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
            .chain(leads)
            .filter(|port| *port != output)
            .cloned()
            .chain(routes.values().filter_map(|route| mix_port(&route.mix)))
            .chain(
                chain
                    .iter()
                    .filter_map(|node| PortRef::new(node, "out").ok()),
            )
            .filter(|port| port != output)
            .collect()
    };
    let mut routes = routes;
    let mut result = Vec::with_capacity(candidates.len() + 1);
    for (port, role) in
        std::iter::once((output.clone(), "output")).chain(candidates.into_iter().map(|port| {
            let role = role(&port);
            (port, role)
        }))
    {
        match view.audio_output_channels(&port) {
            Ok(channels @ 1..=2) => {
                let mixes = if role == "stem" {
                    view.connections
                        .iter()
                        .filter(|connection| connection.from == port && is_mix(&connection.to.node))
                        .map(|connection| connection.to.node.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect()
                } else {
                    Vec::new()
                };
                let driven = driven(&port.node);
                let mix_point = mix_point(&port.node);
                let lead = routes.remove(&port);
                result.push(Source {
                    port,
                    channels,
                    role,
                    mixes,
                    driven,
                    mix_point,
                    lead,
                })
            }
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

/// Checks each `--lead` port and finds its mix point and its notes. A lead is
/// a mono or stereo port that reaches the project output and is not the
/// output, and notes reach it; no two leads play the same notes. Its mix point
/// is the nearest one downstream and, of several at the same distance, the
/// one nearest the output, so a lead feeding a bus and a reverb send is read
/// at the bus.
fn lead_routes(
    view: &PlanView<'_>,
    leads: &[PortRef],
    upstream: &BTreeSet<String>,
    is_mix: &dyn Fn(&str) -> bool,
) -> Result<BTreeMap<PortRef, LeadRoute>, AnalyzeError> {
    let output = &view.output.output;
    let usage = |message: String| AnalyzeError::new("E_USAGE", message);
    let note_targets: BTreeSet<&str> = view
        .events()
        .filter(|event| matches!(event.kind, EventKind::Note { .. }))
        .map(|event| event.target.node.as_str())
        .collect();
    // Each downstream step from `node`, nearest first.
    let downstream = |node: &str| -> Vec<Vec<String>> {
        let mut seen = BTreeSet::from([node.to_owned()]);
        let mut levels = Vec::new();
        let mut level = vec![node.to_owned()];
        while !level.is_empty() {
            let mut next = Vec::new();
            for current in &level {
                for connection in view.connections {
                    if &connection.from.node == current && seen.insert(connection.to.node.clone()) {
                        next.push(connection.to.node.clone());
                    }
                }
            }
            if !next.is_empty() {
                levels.push(next.clone());
            }
            level = next;
        }
        levels
    };
    let hops_to_output = |node: &str| {
        if node == output.node {
            return 0;
        }
        downstream(node)
            .iter()
            .position(|level| level.contains(&output.node))
            .map_or(usize::MAX, |index| index + 1)
    };
    let mut routes: BTreeMap<PortRef, LeadRoute> = BTreeMap::new();
    for lead in leads {
        let name = port_name(lead);
        if lead == output {
            return Err(usage(format!(
                "`{name}` is the project output; a lead is one part of the mix"
            )));
        }
        match view.audio_output_channels(lead) {
            Ok(1..=2) => {}
            Ok(channels) => {
                return Err(AnalyzeError::new(
                    "E_PORT_TYPE",
                    format!("`{name}` has {channels} channels; analysis supports mono or stereo"),
                ))
            }
            Err(failure) => return Err(AnalyzeError::plan(failure)),
        }
        if !view
            .connections
            .iter()
            .any(|connection| &connection.from == lead && upstream.contains(&connection.to.node))
        {
            return Err(usage(format!("`{name}` does not reach the project output")));
        }
        let mut nodes = BTreeSet::from([lead.node.clone()]);
        let mut pending = vec![lead.node.clone()];
        while let Some(current) = pending.pop() {
            for connection in view.connections {
                if connection.to.node == current && nodes.insert(connection.from.node.clone()) {
                    pending.push(connection.from.node.clone());
                }
            }
        }
        nodes.retain(|node| note_targets.contains(node.as_str()));
        if nodes.is_empty() {
            return Err(usage(format!(
                "no notes reach `{name}`; a lead must play notes"
            )));
        }
        if let Some((other, _)) = routes
            .iter()
            .find(|(other, route)| *other != lead && !route.nodes.is_disjoint(&nodes))
        {
            return Err(usage(format!(
                "`{}` and `{name}` play the same notes; name one of them as the lead",
                port_name(other)
            )));
        }
        // Every port that reaches the output has one: the output at worst.
        let mix = downstream(&lead.node)
            .into_iter()
            .find_map(|level| {
                level
                    .into_iter()
                    .filter(|node| is_mix(node))
                    .min_by_key(|node| hops_to_output(node))
            })
            .expect("a port that reaches the output has a mix point");
        routes.insert(lead.clone(), LeadRoute { mix, nodes });
    }
    Ok(routes)
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
            event: None,
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

/// A window for every note and hit whose onset lies in `[start, end)` and
/// whose target is an analysed source, in onset order.
fn note_windows(
    view: &PlanView<'_>,
    sources: &[Source],
    rate: u32,
    start: u64,
    end: u64,
) -> Vec<notes::Window> {
    let seconds = |s: f64| (s * f64::from(rate)).round() as u64;
    let (note_max, hit_length) = (seconds(notes::NOTE_MAX_S), seconds(notes::HIT_S));
    let events: Vec<_> = view
        .events()
        .filter(|event| !matches!(event.kind, EventKind::Message { .. }))
        .collect();
    // Each node's events, to find notes that no other note overlaps.
    let mut by_node: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        by_node
            .entry(event.target.node.as_str())
            .or_default()
            .push(index);
    }
    let sounding = |index: usize| {
        let event = &events[index];
        let off = event.off_frame.unwrap_or(event.on_frame + hit_length);
        (event.on_frame, off.max(event.on_frame + 1))
    };
    let mut windows = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if event.on_frame < start || event.on_frame >= end {
            continue;
        }
        let Some(target) = sources
            .iter()
            .position(|source| source.port.node == *event.target.node)
        else {
            continue;
        };
        let mix = sources[target]
            .mix_point
            .as_ref()
            .and_then(|node| sources.iter().position(|source| &source.port.node == node));
        let window_end = match event.off_frame {
            Some(off) => off.min(event.on_frame + note_max),
            None => event.on_frame + hit_length,
        }
        .min(end);
        if window_end <= event.on_frame {
            continue;
        }
        let (pitch_hz, expressed) = match event.kind {
            EventKind::Note {
                pitch_hz,
                pitch_expression,
                ..
            } => (Some(*pitch_hz), pitch_expression.is_some()),
            _ => (None, false),
        };
        // Lone while the pitch is measured: from the onset to the end of the
        // samples the estimate needs.
        let pitch_end = pitch_hz.map_or(event.on_frame, |hz| {
            event.on_frame + notes::pitch_skip(rate) + notes::pitch_samples(hz, rate)
        });
        let lone = !expressed
            && by_node[event.target.node.as_str()].iter().all(|&other| {
                let (on, off) = sounding(other);
                other == index || on >= pitch_end || off <= event.on_frame
            });
        windows.push(notes::Window::new(
            event.address.clone(),
            target,
            mix,
            event.on_frame,
            window_end,
            pitch_hz,
            lone,
            rate,
        ));
    }
    windows.sort_by_key(|window| window.start);
    windows
}

fn note_reports(
    windows: &[notes::Window],
    sources: &[Source],
    sections: &[Section],
    rate: u32,
) -> Vec<NoteReport> {
    let channels: Vec<u8> = sources.iter().map(|source| source.channels).collect();
    windows
        .iter()
        .map(|window| {
            let outcome = notes::outcome(window, &channels, rate);
            let periodic = outcome
                .pitch
                .as_ref()
                .filter(|pitch| pitch.aperiodicity <= notes::APERIODIC);
            let status = if outcome
                .level_lufs
                .is_none_or(|level| level <= SILENT_NOTE_LUFS)
            {
                "silent"
            } else if periodic.is_some_and(|pitch| pitch.cents.abs() > PITCH_LIMIT_CENTS) {
                "pitch_mismatch"
            } else if outcome
                .share_db
                .is_some_and(|share| share <= BURIED_NOTE_DB)
            {
                "buried"
            } else if outcome.pitch.is_some() && periodic.is_none() {
                "unpitched"
            } else {
                "ok"
            };
            NoteReport {
                event: window.address.clone(),
                source: port_name(&sources[window.target].port),
                start_frame: window.start,
                section: sections
                    .iter()
                    .find(|s| s.start_frame <= window.start && window.start < s.end_frame)
                    .map(|s| s.id.clone()),
                level_lufs: outcome.level_lufs.filter(|level| *level > SILENT_NOTE_LUFS),
                share_db: outcome.share_db,
                cents: outcome.pitch.as_ref().map(|pitch| pitch.cents),
                aperiodicity: outcome.pitch.as_ref().map(|pitch| pitch.aperiodicity),
                status,
            }
        })
        .collect()
}

fn is_silent(note: &NoteReport) -> bool {
    note.level_lufs.is_none()
}

fn is_buried(note: &NoteReport) -> bool {
    !is_silent(note) && note.share_db.is_some_and(|share| share <= BURIED_NOTE_DB)
}

fn pitched_cents(note: &NoteReport) -> Option<f64> {
    note.cents
        .filter(|_| note.aperiodicity.is_some_and(|a| a <= notes::APERIODIC) && !is_silent(note))
}

fn summary<'n>(notes: impl Iterator<Item = &'n NoteReport>) -> Option<NoteSummary> {
    let notes: Vec<&NoteReport> = notes.collect();
    if notes.is_empty() {
        return None;
    }
    let mut cents: Vec<f64> = notes
        .iter()
        .filter_map(|note| pitched_cents(note))
        .collect();
    cents.sort_by(f64::total_cmp);
    let median = (!cents.is_empty()).then(|| {
        let middle = cents.len() / 2;
        if cents.len() % 2 == 1 {
            cents[middle]
        } else {
            round2((cents[middle - 1] + cents[middle]) / 2.0)
        }
    });
    Some(NoteSummary {
        measured: notes.len(),
        silent: notes.iter().filter(|note| is_silent(note)).count(),
        buried: notes.iter().filter(|note| is_buried(note)).count(),
        pitched: cents.len(),
        unpitched: notes
            .iter()
            .filter(|note| note.aperiodicity.is_some() && pitched_cents(note).is_none())
            .count(),
        pitch_mismatches: cents.iter().filter(|c| c.abs() > PITCH_LIMIT_CENTS).count(),
        median_cents: median,
        max_abs_cents: cents.iter().map(|c| c.abs()).reduce(f64::max),
    })
}

fn attach_note_summaries(reports: &mut [SourceReport], notes: &[NoteReport]) {
    for report in reports.iter_mut() {
        report.whole.notes = summary(notes.iter().filter(|note| note.source == report.port));
        for section in &mut report.sections {
            section.measures.notes = summary(
                notes
                    .iter()
                    .filter(|note| note.source == report.port)
                    .filter(|note| note.section.as_deref() == Some(section.section.as_str())),
            );
        }
    }
}

/// One finding per kind, part and section, naming the worst note.
fn note_findings(notes: &[NoteReport], positions: &PositionMap<'_>) -> Vec<Finding> {
    // (kind, source, section) -> (count, worst note, its value, badness)
    type Group<'n> = (usize, &'n NoteReport, f64, f64);
    let mut groups: BTreeMap<(&'static str, &str, Option<&str>), Group<'_>> = BTreeMap::new();
    for note in notes {
        let mut push = |kind: &'static str, value: f64, badness: f64| {
            let entry = groups
                .entry((kind, note.source.as_str(), note.section.as_deref()))
                .or_insert((0, note, value, badness));
            entry.0 += 1;
            if badness > entry.3 {
                (entry.1, entry.2, entry.3) = (note, value, badness);
            }
        };
        if is_silent(note) {
            push("silent_note", -120.0, 120.0);
            continue;
        }
        if let Some(share) = note.share_db.filter(|share| *share <= BURIED_NOTE_DB) {
            push("buried_note", share, -share);
        }
        if let Some(cents) = pitched_cents(note).filter(|c| c.abs() > PITCH_LIMIT_CENTS) {
            push("pitch_mismatch", cents, cents.abs());
        }
    }
    groups
        .into_iter()
        .map(|((kind, source, section), (count, note, value, _))| {
            let (notes_text, verb) = if count == 1 {
                ("1 note".to_owned(), "s")
            } else {
                (format!("{count} notes"), "")
            };
            let (severity, threshold, message) = match kind {
                "silent_note" => (
                    "warning",
                    SILENT_NOTE_LUFS,
                    format!(
                        "{notes_text} on `{source}` produce{verb} no sound; the first is `{}`",
                        note.event
                    ),
                ),
                "buried_note" => (
                    "info",
                    BURIED_NOTE_DB,
                    format!(
                        "{notes_text} on `{source}` hold{verb} {BURIED_NOTE_DB} dB or less of the band at the mix point; `{}` holds {value} dB",
                        note.event
                    ),
                ),
                _ => (
                    "warning",
                    PITCH_LIMIT_CENTS,
                    format!(
                        "{notes_text} on `{source}` sound{verb} more than {PITCH_LIMIT_CENTS} cents off the written pitch; `{}` is {value:+} cents",
                        note.event
                    ),
                ),
            };
            Finding {
                kind,
                event: Some(note.event.clone()),
                severity,
                source: source.to_owned(),
                section: section.map(str::to_owned),
                at: Some(positions.at(note.start_frame)),
                value,
                threshold,
                message,
            }
        })
        .collect()
}

/// The named leads with their sources and mix points, in the order named.
fn resolve_leads(sources: &[Source], ports: &[PortRef]) -> Result<Vec<lead::Lead>, AnalyzeError> {
    let find = |port: &PortRef| {
        sources
            .iter()
            .position(|source| &source.port == port)
            .and_then(|index| sources[index].lead.as_ref().map(|route| (index, route)))
            .expect("every lead is an analysed source")
    };
    ports
        .iter()
        .map(|port| {
            let (source, route) = find(port);
            let mix = sources
                .iter()
                .position(|source| source.port.node == route.mix)
                .ok_or_else(|| {
                    AnalyzeError::new(
                        "E_USAGE",
                        format!(
                            "the mix point `{}` of lead `{}` is not a mono or stereo source",
                            route.mix,
                            port_name(port)
                        ),
                    )
                })?;
            let others = ports
                .iter()
                .filter(|other| *other != port)
                .map(find)
                .filter(|(_, other)| other.mix == route.mix)
                .map(|(index, _)| index)
                .collect();
            Ok(lead::Lead {
                port: port_name(port),
                source,
                mix_node: route.mix.clone(),
                mix,
                others,
                nodes: route.nodes.clone(),
            })
        })
        .collect()
}

/// A window for every note whose onset lies in `[start, end)` and that
/// reaches a lead, in onset order.
fn lead_windows(
    view: &PlanView<'_>,
    sources: &[Source],
    leads: &[lead::Lead],
    rate: u32,
    start: u64,
    end: u64,
) -> Vec<lead::LeadWindow> {
    let note_max = (notes::NOTE_MAX_S * f64::from(rate)).round() as u64;
    let channels: Vec<u8> = sources.iter().map(|source| source.channels).collect();
    let mut windows: Vec<lead::LeadWindow> = view
        .events()
        .filter(|event| matches!(event.kind, EventKind::Note { .. }))
        .filter(|event| start <= event.on_frame && event.on_frame < end)
        .filter_map(|event| {
            let index = leads
                .iter()
                .position(|lead| lead.nodes.contains(event.target.node.as_str()))?;
            let window_end = event
                .off_frame
                .map_or(event.on_frame + note_max, |off| {
                    off.min(event.on_frame + note_max)
                })
                .min(end);
            (window_end > event.on_frame).then(|| {
                lead::LeadWindow::new(
                    event.address.clone(),
                    index,
                    &leads[index],
                    &channels,
                    event.on_frame,
                    window_end,
                )
            })
        })
        .collect();
    windows.sort_by_key(|window| window.start);
    windows
}

fn lead_reports(
    windows: &[lead::LeadWindow],
    leads: &[lead::Lead],
    sections: &[Section],
) -> Vec<LeadNote> {
    windows
        .iter()
        .map(|window| {
            let (margin_db, status) = window.outcome(LEAD_UNDER_DB);
            LeadNote {
                event: window.address.clone(),
                lead: leads[window.lead].port.clone(),
                start_frame: window.start,
                section: sections
                    .iter()
                    .find(|s| s.start_frame <= window.start && window.start < s.end_frame)
                    .map(|s| s.id.clone()),
                margin_db,
                status,
            }
        })
        .collect()
}

fn attach_lead_summaries(reports: &mut [SourceReport], notes: &[LeadNote], leads: &[lead::Lead]) {
    for lead in leads {
        let report = &mut reports[lead.source];
        let of_lead = || notes.iter().filter(|note| note.lead == lead.port);
        report.whole.lead = lead::summary(&lead.mix_node, of_lead());
        for section in &mut report.sections {
            section.measures.lead = lead::summary(
                &lead.mix_node,
                of_lead().filter(|note| note.section.as_deref() == Some(section.section.as_str())),
            );
        }
    }
}

/// One `lead_under` per lead and section whose median note sits at or below
/// the limit under the rest of its mix point, naming the quietest note.
fn lead_findings(
    notes: &[LeadNote],
    leads: &[lead::Lead],
    positions: &PositionMap<'_>,
) -> Vec<Finding> {
    let mut groups: BTreeMap<(&str, Option<&str>), Vec<&LeadNote>> = BTreeMap::new();
    for note in notes {
        groups
            .entry((note.lead.as_str(), note.section.as_deref()))
            .or_default()
            .push(note);
    }
    groups
        .into_iter()
        .filter_map(|((port, section), group)| {
            let lead = leads.iter().find(|lead| lead.port == port)?;
            let summary = lead::summary(&lead.mix_node, group.iter().copied())?;
            let median = summary
                .median_db
                .filter(|median| summary.notes >= LEAD_MIN_NOTES && *median <= LEAD_UNDER_DB)?;
            let (quietest, lowest) = group
                .iter()
                .filter_map(|note| note.margin_db.map(|margin| (note, margin)))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            Some(Finding {
                kind: "lead_under",
                event: Some(quietest.event.clone()),
                severity: "warning",
                source: port.to_owned(),
                section: section.map(str::to_owned),
                at: Some(positions.at(quietest.start_frame)),
                value: median,
                threshold: LEAD_UNDER_DB,
                message: format!(
                    "lead `{port}` sits {} dB under the rest of `{}` in its strongest band, the median of {} notes; the quietest is `{}` at {lowest} dB",
                    round2(-median),
                    lead.mix_node,
                    summary.notes,
                    quietest.event
                ),
            })
        })
        .collect()
}

/// Steady stems that no note drives and that fill a band in the quietest
/// passages (F13). The share is
/// read at the stem's mix point, a plain sum, where it is close to exact;
/// audibility is read at the output, after any compression has lifted the
/// quiet passages.
fn noise_findings(
    sources: &[Source],
    blocks: &[Vec<Block>],
    sections: &[Section],
    ranges: &[std::ops::Range<usize>],
    hop: u64,
    available: [bool; 10],
    positions: &PositionMap<'_>,
) -> Vec<Finding> {
    let whole = [(None, 0..blocks[0].len())];
    let groups: Vec<(Option<&Section>, std::ops::Range<usize>)> = if sections.is_empty() {
        whole.to_vec()
    } else {
        sections
            .iter()
            .map(Some)
            .zip(ranges.iter().cloned())
            .collect()
    };
    let mut found = Vec::new();
    for (section, range) in groups {
        let output = &blocks[0][range.clone()];
        let Some(floor) = floor_blocks(output, hop) else {
            continue;
        };
        let Some(output_floor) = floor_loudness(output, &floor) else {
            continue;
        };
        let output_energy = floor_energy(output, &floor, sources[0].channels);
        // Notes make music the author wrote, and its steadiness is a choice;
        // a stem that sounds without them is texture or noise.
        for (stem, stem_blocks) in sources
            .iter()
            .zip(blocks)
            .filter(|(s, _)| s.role == "stem" && !s.driven)
        {
            let stem_blocks = &stem_blocks[range.clone()];
            if presence(stem_blocks, hop) < NOISE_MIN_PRESENCE {
                continue;
            }
            let Some(spread) =
                block_range_lu(stem_blocks, hop).filter(|r| *r <= NOISE_MAX_RANGE_LU)
            else {
                continue;
            };
            let stem_bands = band_energy(stem_blocks, &floor, stem.channels);
            // The band where the stem holds the largest share of any mix point.
            let mut worst: Option<(f64, usize, &Source)> = None;
            for mix in sources.iter().filter(|s| stem.mixes.contains(&s.port.node)) {
                let index = sources
                    .iter()
                    .position(|s| s.port == mix.port)
                    .expect("listed");
                let mix_bands = band_energy(&blocks[index][range.clone()], &floor, mix.channels);
                let strongest = (0..10)
                    .filter(|&b| available[b])
                    .map(|b| mix_bands[b])
                    .fold(0.0, f64::max);
                for band in (0..10).filter(|&b| available[b] && mix_bands[b] > 0.0) {
                    if db_power(mix_bands[band] / strongest) < -NOISE_BAND_SPAN_DB {
                        continue;
                    }
                    let share = stem_bands[band] / mix_bands[band];
                    if worst.is_none_or(|(best, _, _)| share > best) {
                        worst = Some((share, band, mix));
                    }
                }
            }
            let Some((share, band, mix)) = worst.filter(|(share, _, _)| *share >= NOISE_MIN_SHARE)
            else {
                continue;
            };
            // The stem's K-weighted share of its mix point, carried to the
            // output: later gain and compression scale both alike.
            let index = sources
                .iter()
                .position(|s| s.port == mix.port)
                .expect("listed");
            let mix_energy = floor_energy(&blocks[index][range.clone()], &floor, mix.channels);
            let stem_energy = floor_energy(stem_blocks, &floor, stem.channels);
            if mix_energy <= 0.0 || stem_energy <= 0.0 {
                continue;
            }
            let Some(heard) = loudness_of(
                stem_energy / mix_energy * output_energy * f64::from(sources[0].channels),
            )
            .filter(|l| *l > NOISE_AUDIBLE_LUFS) else {
                continue;
            };
            let at = positions.at(output[floor[0]].start);
            found.push(Finding {
                kind: "exposed_noise",
                event: None,
                severity: "warning",
                source: port_name(&stem.port),
                section: section.map(|s| s.id.clone()),
                at: Some(at),
                value: round2(db_power(share)),
                threshold: round2(db_power(NOISE_MIN_SHARE)),
                message: format!(
                    "`{}` is steady (range {} LU), no note plays it, and it makes up {}% of the {} Hz band at `{}` in the quietest passages; there it reaches about {} LUFS at the output, whose floor is {} LUFS",
                    port_name(&stem.port),
                    round2(spread),
                    (share * 100.0).round(),
                    BAND_CENTERS_HZ[band],
                    port_name(&mix.port),
                    round2(heard),
                    round2(output_floor)
                ),
            });
        }
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
    extra: Vec<Finding>,
    reports: &[SourceReport],
    sections: &[Section],
    positions: &PositionMap<'_>,
    view: &PlanView<'_>,
    analysed: (u64, u64),
) -> (Vec<Finding>, usize) {
    let mut found = groove_findings(view, sections, positions, analysed);
    found.extend(extra);
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
            event: None,
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
            event: None,
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
                    event: None,
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
                    event: None,
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
                    event: None,
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
                            event: None,
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
                event: None,
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
