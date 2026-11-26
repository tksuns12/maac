//! AI + Human live collaboration engine for MaaC Studio.
//!
//! Provides:
//! - Timeline review pins (`ReviewPin`) persisted in `<song>.maac-collab.json`
//! - Shared-focus context builder (playhead, loop, selection, mixer, plugins, pins)
//! - Fast focused-window audio telemetry (RMS, Peak, Crest, Stereo Width, Low/Mid/High balance)
//! - Automatic diffing of AI edits (`AiEditHighlight`) for timeline overlays & jump-to-loop
//! - Pre-compiled Before/After A·B comparison (`AbComparison`) for instant zero-stop auditioning
//! - Local CLI agent runner auto-detecting `claude`, `agy`, `codex`, and `gemini`

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use maac::dsp::{DspEngine, RenderSpan};
use serde::{Deserialize, Serialize};

use crate::analysis::AnalysisSummary;
use crate::code_view::compute_line_diff;
use crate::editor::midi_to_pitch_symbol;
use crate::plugins::{PluginSlotSummary, SlotTarget};
use crate::project::{format_time, load_and_compile, ProjectData};

/// Sidecar collaboration file stored next to `<song>.maac` (keeping `.maac` pure).
pub fn collab_path_for(entry_path: &Path) -> PathBuf {
    let mut name = entry_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project.maac".into());
    name.push_str("-collab.json");
    entry_path.with_file_name(name)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PinAuthor {
    Human,
    Agent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewPin {
    pub id: u64,
    pub frame: u64,
    #[serde(default)]
    pub end_frame: Option<u64>,
    pub bar_label: String,
    #[serde(default)]
    pub track: Option<String>,
    pub text: String,
    #[serde(default)]
    pub resolved: bool,
    pub created_by: PinAuthor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ChatRole {
    User,
    Agent,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CollabFile {
    pub version: u32,
    #[serde(default)]
    pub next_pin_id: u64,
    #[serde(default)]
    pub pins: Vec<ReviewPin>,
    #[serde(default)]
    pub chat: Vec<ChatMessage>,
}

pub fn load_collab_file(entry_path: &Path) -> Result<Option<CollabFile>, String> {
    let path = collab_path_for(entry_path);
    if !path.exists() {
        return Ok(None);
    }
    let bytes =
        std::fs::read(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let parsed: CollabFile = serde_json::from_slice(&bytes)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    Ok(Some(parsed))
}

pub fn save_collab_file(entry_path: &Path, file: &CollabFile) -> Result<(), String> {
    let path = collab_path_for(entry_path);
    if file.pins.is_empty() && file.chat.is_empty() {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        return Ok(());
    }
    let json = serde_json::to_string_pretty(file)
        .map_err(|e| format!("failed to serialize collab file: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

/// Visual summary of what an AI edit changed so the timeline & A/B banner can
/// highlight modified notes, drum hits, and time spans.
#[derive(Clone, Debug, Default)]
pub struct AiEditHighlight {
    pub changed_note_addresses: HashSet<String>,
    pub changed_hit_addresses: HashSet<String>,
    pub changed_spans: Vec<RenderSpan>,
    pub bounding_span: Option<RenderSpan>,
    pub changed_tracks: Vec<String>,
    pub added_lines: usize,
    pub removed_lines: usize,
}

impl AiEditHighlight {
    pub fn summary_line(&self, project: &ProjectData) -> String {
        let mut parts = Vec::new();
        parts.push(format!(
            "+{} / -{} lines",
            self.added_lines, self.removed_lines
        ));
        let ev_count = self.changed_note_addresses.len() + self.changed_hit_addresses.len();
        if ev_count > 0 {
            parts.push(format!("{ev_count} events"));
        }
        if !self.changed_tracks.is_empty() {
            parts.push(self.changed_tracks.join(", "));
        }
        if let Some(span) = self.bounding_span {
            let b0 = project.format_bar_position(span.start);
            let b1 = project.format_bar_position(span.end);
            parts.push(format!("{b0}–{b1}"));
        }
        parts.join(" · ")
    }
}

/// Compare `before` and `after` compiled projects to pinpoint every note, drum
/// hit, track, and time span modified by an AI edit.
pub fn diff_projects_for_highlight(
    before: &ProjectData,
    after: &ProjectData,
    before_src: &str,
    after_src: &str,
) -> AiEditHighlight {
    let diff = compute_line_diff(before_src, after_src, 2);

    let before_notes: HashMap<&str, (u64, u64, i32, i32)> = before
        .notes
        .iter()
        .map(|n| {
            (
                n.address.as_str(),
                (
                    n.on_frame,
                    n.off_frame,
                    n.midi_pitch.round() as i32,
                    (n.velocity * 100.0).round() as i32,
                ),
            )
        })
        .collect();

    let after_notes: HashMap<&str, (u64, u64, i32, i32)> = after
        .notes
        .iter()
        .map(|n| {
            (
                n.address.as_str(),
                (
                    n.on_frame,
                    n.off_frame,
                    n.midi_pitch.round() as i32,
                    (n.velocity * 100.0).round() as i32,
                ),
            )
        })
        .collect();

    let mut changed_note_addresses = HashSet::new();
    let mut raw_spans: Vec<RenderSpan> = Vec::new();
    let mut track_set: Vec<String> = Vec::new();
    let mut push_track = |name: &str| {
        if !track_set.iter().any(|t| t == name) {
            track_set.push(name.to_owned());
        }
    };

    for n in &after.notes {
        let key = (
            n.on_frame,
            n.off_frame,
            n.midi_pitch.round() as i32,
            (n.velocity * 100.0).round() as i32,
        );
        if before_notes.get(n.address.as_str()) != Some(&key) {
            changed_note_addresses.insert(n.address.clone());
            raw_spans.push(RenderSpan {
                start: n.on_frame,
                end: n.off_frame.max(n.on_frame + 1),
            });
            if let Some(tr) = after.tracks.get(n.track_idx) {
                push_track(&tr.name);
            }
        }
    }
    // Also record time spans where notes were deleted.
    for n in &before.notes {
        if !after_notes.contains_key(n.address.as_str()) {
            raw_spans.push(RenderSpan {
                start: n.on_frame,
                end: n.off_frame.max(n.on_frame + 1),
            });
            if let Some(tr) = before.tracks.get(n.track_idx) {
                push_track(&tr.name);
            }
        }
    }

    let before_hits: HashMap<&str, (u64, &str, i32)> = before
        .hits
        .iter()
        .map(|h| {
            (
                h.address.as_str(),
                (
                    h.on_frame,
                    h.key.as_str(),
                    (h.velocity * 100.0).round() as i32,
                ),
            )
        })
        .collect();
    let after_hits: HashMap<&str, (u64, &str, i32)> = after
        .hits
        .iter()
        .map(|h| {
            (
                h.address.as_str(),
                (
                    h.on_frame,
                    h.key.as_str(),
                    (h.velocity * 100.0).round() as i32,
                ),
            )
        })
        .collect();

    let mut changed_hit_addresses = HashSet::new();
    let hit_pad = u64::from(after.sample_rate_hz / 4);
    for h in &after.hits {
        let key = (
            h.on_frame,
            h.key.as_str(),
            (h.velocity * 100.0).round() as i32,
        );
        if before_hits.get(h.address.as_str()) != Some(&key) {
            changed_hit_addresses.insert(h.address.clone());
            raw_spans.push(RenderSpan {
                start: h.on_frame,
                end: h.on_frame.saturating_add(hit_pad),
            });
            if let Some(tr) = after.tracks.get(h.track_idx) {
                push_track(&tr.name);
            }
        }
    }
    for h in &before.hits {
        if !after_hits.contains_key(h.address.as_str()) {
            raw_spans.push(RenderSpan {
                start: h.on_frame,
                end: h.on_frame.saturating_add(hit_pad),
            });
            if let Some(tr) = before.tracks.get(h.track_idx) {
                push_track(&tr.name);
            }
        }
    }

    // Merge adjacent spans within ~1.5 seconds into clean timeline highlight regions.
    raw_spans.sort_by_key(|s| s.start);
    let merge_gap = u64::from(after.sample_rate_hz) * 3 / 2;
    let mut merged: Vec<RenderSpan> = Vec::new();
    for s in raw_spans {
        if let Some(last) = merged.last_mut() {
            if s.start <= last.end.saturating_add(merge_gap) {
                last.end = last.end.max(s.end);
                continue;
            }
        }
        merged.push(s);
    }

    let bounding_span = if merged.is_empty() {
        None
    } else {
        let raw_start = merged.first().map(|s| s.start).unwrap_or(0);
        let raw_end = merged.last().map(|s| s.end).unwrap_or(after.total_frames);
        let snapped_start = after.snap_frame_to_bar(raw_start);
        let snapped_end = after
            .next_bar_after(raw_end.saturating_sub(1))
            .min(after.total_frames)
            .max(snapped_start.saturating_add(u64::from(after.sample_rate_hz)));
        Some(RenderSpan {
            start: snapped_start,
            end: snapped_end.min(after.total_frames),
        })
    };

    AiEditHighlight {
        changed_note_addresses,
        changed_hit_addresses,
        changed_spans: merged,
        bounding_span,
        changed_tracks: track_set,
        added_lines: diff.added,
        removed_lines: diff.removed,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbSide {
    Before,
    After,
}

/// Pre-compiled Before/After snapshot pair for zero-stop A/B auditioning.
#[derive(Clone)]
pub struct AbComparison {
    pub label: String,
    pub before_source: String,
    pub before_project: ProjectData,
    pub after_source: String,
    pub after_project: ProjectData,
    pub active_side: AbSide,
    pub highlight: AiEditHighlight,
}

/// Measured audio telemetry for the currently focused loop or playhead window.
#[derive(Clone, Debug)]
pub struct AudioRegionSnapshot {
    pub span: RenderSpan,
    pub bar_span_label: String,
    pub rms_dbfs: f64,
    pub peak_dbfs: f64,
    pub crest_db: f64,
    pub stereo_width_pct: f64,
    pub low_pct: f64,
    pub mid_pct: f64,
    pub high_pct: f64,
    pub track_activity: Vec<(String, usize)>,
    pub elapsed_ms: u64,
}

impl AudioRegionSnapshot {
    pub fn compact_label(&self) -> String {
        format!(
            "RMS {:.1} dBFS · Peak {:.1} dB · L{:.0}/M{:.0}/H{:.0}%",
            self.rms_dbfs, self.peak_dbfs, self.low_pct, self.mid_pct, self.high_pct
        )
    }
}

/// Render up to 10 seconds of the focused span (`loop_range` or 4 bars around
/// `playhead_frame`) using pure `DspEngine` and compute objective level, stereo,
/// and 3-band spectral telemetry so the AI agent can "hear" the section numerically.
pub fn measure_region_audio(
    project: &ProjectData,
    loop_range: Option<RenderSpan>,
    playhead_frame: u64,
) -> Result<AudioRegionSnapshot, String> {
    let started = Instant::now();
    let rate = f64::from(project.sample_rate_hz.max(1));
    let max_frames = (project.sample_rate_hz as u64) * 10; // cap at 10s for instant response
    let raw_span = if let Some(lr) = loop_range {
        RenderSpan {
            start: lr.start.min(project.total_frames.saturating_sub(1)),
            end: lr.end.min(project.total_frames),
        }
    } else {
        let start = project.snap_frame_to_bar(playhead_frame);
        let default_len = (project.sample_rate_hz as u64) * 6;
        RenderSpan {
            start,
            end: start.saturating_add(default_len).min(project.total_frames),
        }
    };
    let span = RenderSpan {
        start: raw_span.start,
        end: raw_span
            .end
            .min(raw_span.start.saturating_add(max_frames))
            .min(project.total_frames),
    };
    if span.end <= span.start {
        return Err("empty focus span for audio snapshot".into());
    }

    let mut engine = DspEngine::new_artifact_with_limits(&project.plan, &project.limits)
        .map_err(|e| format!("audio snapshot init failed: {e}"))?;
    let ch = project.channels.max(1);
    let mut samples: Vec<f64> = Vec::with_capacity((span.end - span.start) as usize * ch);
    let mut pos = span.start;
    while pos < span.end {
        let next = (pos + 1024).min(span.end);
        let chunk = RenderSpan {
            start: pos,
            end: next,
        };
        let res = if span.start > 0 {
            engine.render_preview_chunk(chunk, |s| {
                samples.extend_from_slice(s);
                Ok(())
            })
        } else {
            engine.render_chunk(chunk, |s| {
                samples.extend_from_slice(s);
                Ok(())
            })
        };
        res.map_err(|e| format!("audio snapshot render failed: {e}"))?;
        pos = next;
    }

    // Simple 1-pole crossovers at 250 Hz and 4000 Hz on the mono sum for fast
    // 3-band spectral balance (pure Rust, zero FFT dependency).
    let alpha_lo = 1.0 - (-2.0 * std::f64::consts::PI * 250.0 / rate).exp();
    let alpha_hi = 1.0 - (-2.0 * std::f64::consts::PI * 4000.0 / rate).exp();
    let mut lp_lo = 0.0_f64;
    let mut lp_hi = 0.0_f64;

    let mut sum_sq = 0.0_f64;
    let mut peak = 0.0_f64;
    let mut mid_sq = 0.0_f64;
    let mut side_sq = 0.0_f64;
    let mut low_sq = 0.0_f64;
    let mut mid_band_sq = 0.0_f64;
    let mut high_sq = 0.0_f64;
    let frame_count = samples.len() / ch;

    for f in 0..frame_count {
        let l = samples[f * ch];
        let r = if ch >= 2 { samples[f * ch + 1] } else { l };
        sum_sq += 0.5 * (l * l + r * r);
        peak = peak.max(l.abs()).max(r.abs());
        let mid = 0.5 * (l + r);
        let side = 0.5 * (l - r);
        mid_sq += mid * mid;
        side_sq += side * side;

        lp_lo += alpha_lo * (mid - lp_lo);
        lp_hi += alpha_hi * (mid - lp_hi);
        let low = lp_lo;
        let mid_b = lp_hi - lp_lo;
        let high = mid - lp_hi;
        low_sq += low * low;
        mid_band_sq += mid_b * mid_b;
        high_sq += high * high;
    }

    let rms = (sum_sq / frame_count.max(1) as f64).sqrt();
    let to_db = |v: f64| {
        if v <= 1e-9 {
            -120.0
        } else {
            20.0 * v.log10()
        }
    };
    let rms_dbfs = to_db(rms);
    let peak_dbfs = to_db(peak);
    let crest_db = (peak_dbfs - rms_dbfs).max(0.0);
    let stereo_width_pct = if mid_sq + side_sq > 1e-12 {
        (side_sq.sqrt() / (mid_sq.sqrt() + side_sq.sqrt())) * 200.0
    } else {
        0.0
    }
    .clamp(0.0, 100.0);

    let total_band = (low_sq + mid_band_sq + high_sq).max(1e-12);
    let low_pct = (low_sq / total_band) * 100.0;
    let mid_pct = (mid_band_sq / total_band) * 100.0;
    let high_pct = (high_sq / total_band) * 100.0;

    // Count active notes + hits per track inside `span`.
    let mut per_track: BTreeMap<String, usize> = BTreeMap::new();
    for n in &project.notes {
        if n.off_frame > span.start && n.on_frame < span.end {
            if let Some(tr) = project.tracks.get(n.track_idx) {
                *per_track.entry(tr.name.clone()).or_insert(0) += 1;
            }
        }
    }
    for h in &project.hits {
        if h.on_frame >= span.start && h.on_frame < span.end {
            if let Some(tr) = project.tracks.get(h.track_idx) {
                *per_track.entry(tr.name.clone()).or_insert(0) += 1;
            }
        }
    }

    let bar_span_label = format!(
        "{}–{} ({}–{})",
        project.format_bar_position(span.start),
        project.format_bar_position(span.end),
        format_time(span.start, project.sample_rate_hz),
        format_time(span.end, project.sample_rate_hz),
    );

    Ok(AudioRegionSnapshot {
        span,
        bar_span_label,
        rms_dbfs,
        peak_dbfs,
        crest_db,
        stereo_width_pct,
        low_pct,
        mid_pct,
        high_pct,
        track_activity: per_track.into_iter().collect(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Supported local CLI agent backends auto-detected on the user's machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentKind {
    Claude,
    Agy,
    Codex,
    Gemini,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Agy => "AGY",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
        }
    }

    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Agy => "agy",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DetectedAgent {
    pub kind: AgentKind,
    pub path: PathBuf,
}

/// Search `$PATH` plus standard macOS/Linux user bin directories (`~/.local/bin`,
/// `/opt/homebrew/bin`, `/usr/local/bin`) for installed agent CLIs.
pub fn detect_local_agents() -> Vec<DetectedAgent> {
    let mut search_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(path_env) = std::env::var("PATH") {
        search_dirs.extend(std::env::split_paths(&path_env));
    }
    if let Ok(home) = std::env::var("HOME") {
        let home_p = PathBuf::from(home);
        search_dirs.push(home_p.join(".local/bin"));
        search_dirs.push(home_p.join(".cargo/bin"));
        search_dirs.push(home_p.join("bin"));
    }
    search_dirs.push(PathBuf::from("/opt/homebrew/bin"));
    search_dirs.push(PathBuf::from("/usr/local/bin"));

    let mut found = Vec::new();
    for kind in [
        AgentKind::Claude,
        AgentKind::Agy,
        AgentKind::Codex,
        AgentKind::Gemini,
    ] {
        let bin = kind.binary_name();
        for dir in &search_dirs {
            let cand = dir.join(bin);
            if cand.is_file() {
                found.push(DetectedAgent { kind, path: cand });
                break;
            }
        }
    }
    found
}

/// Snapshot of everything the user currently sees & selected in Studio, fed into
/// the agent prompt so human and AI literally look at the exact same musical state.
#[derive(Clone, Debug)]
pub struct SharedFocusContext {
    pub entry_path: PathBuf,
    pub project_root: PathBuf,
    pub title: String,
    pub sample_rate_hz: u32,
    pub total_frames: u64,
    pub playhead_bar: String,
    pub playhead_time: String,
    pub loop_label: Option<String>,
    pub selected_track: Option<String>,
    pub selected_notes_desc: Vec<String>,
    pub selected_node: Option<String>,
    pub muted_tracks: Vec<String>,
    pub solo_tracks: Vec<String>,
    pub mounted_plugins: Vec<String>,
    pub open_pins: Vec<ReviewPin>,
    pub audio_snapshot: Option<AudioRegionSnapshot>,
    pub analysis: Option<AnalysisSummary>,
    pub recent_chat: Vec<ChatMessage>,
}

impl SharedFocusContext {
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        project: &ProjectData,
        playhead_frame: u64,
        loop_range: Option<RenderSpan>,
        selected_notes: &HashSet<String>,
        selected_node_id: Option<&str>,
        muted_tracks: &HashSet<String>,
        solo_tracks: &HashSet<String>,
        plugin_slots: &[PluginSlotSummary],
        pins: &[ReviewPin],
        audio_snapshot: Option<&AudioRegionSnapshot>,
        analysis: Option<&AnalysisSummary>,
        recent_chat: &[ChatMessage],
    ) -> Self {
        let playhead_bar = project.format_bar_position(playhead_frame);
        let playhead_time = format_time(playhead_frame, project.sample_rate_hz);
        let loop_label = loop_range.map(|lr| {
            format!(
                "{}–{} ({}–{})",
                project.format_bar_position(lr.start),
                project.format_bar_position(lr.end),
                format_time(lr.start, project.sample_rate_hz),
                format_time(lr.end, project.sample_rate_hz),
            )
        });

        let mut selected_notes_desc = Vec::new();
        let mut selected_track = None;
        for n in &project.notes {
            if selected_notes.contains(&n.address) {
                let tr = project
                    .tracks
                    .get(n.track_idx)
                    .map(|t| t.name.as_str())
                    .unwrap_or("?");
                if selected_track.is_none() {
                    selected_track = Some(tr.to_owned());
                }
                if selected_notes_desc.len() < 16 {
                    selected_notes_desc.push(format!(
                        "{} @ {} {} (vel {:.2}, path={})",
                        tr,
                        project.format_bar_position(n.on_frame),
                        midi_to_pitch_symbol(n.midi_pitch.round() as i32),
                        n.velocity,
                        n.source_path.join(".")
                    ));
                }
            }
        }
        if selected_notes.len() > 16 {
            selected_notes_desc.push(format!(
                "... +{} more selected notes",
                selected_notes.len() - 16
            ));
        }

        let mounted_plugins = plugin_slots
            .iter()
            .map(|s| {
                let tgt = match &s.target {
                    SlotTarget::TrackInstrument(n) => format!("{n} Inst"),
                    SlotTarget::NodeInsert(n) => format!("{n} FX"),
                    SlotTarget::MasterInsert => "Master FX".into(),
                };
                format!(
                    "[{}] {} on {tgt}{}",
                    s.descriptor.format.label(),
                    s.descriptor.name,
                    if s.bypassed { " (bypassed)" } else { "" }
                )
            })
            .collect();

        let open_pins = pins.iter().filter(|p| !p.resolved).cloned().collect();

        let mut muted: Vec<String> = muted_tracks.iter().cloned().collect();
        muted.sort();
        let mut solo: Vec<String> = solo_tracks.iter().cloned().collect();
        solo.sort();

        Self {
            entry_path: project.entry_path.clone(),
            project_root: project.project_root.clone(),
            title: project.title.clone(),
            sample_rate_hz: project.sample_rate_hz,
            total_frames: project.total_frames,
            playhead_bar,
            playhead_time,
            loop_label,
            selected_track,
            selected_notes_desc,
            selected_node: selected_node_id.map(str::to_owned),
            muted_tracks: muted,
            solo_tracks: solo,
            mounted_plugins,
            open_pins,
            audio_snapshot: audio_snapshot.cloned(),
            analysis: analysis.cloned(),
            recent_chat: recent_chat.iter().rev().take(6).rev().cloned().collect(),
        }
    }
}

/// Build the structured collaboration prompt passed to the local CLI agent (or
/// copied to clipboard via `Copy Context`).
pub fn build_agent_prompt(ctx: &SharedFocusContext, user_instruction: &str) -> String {
    let mut out = String::new();
    out.push_str("You are co-producing a MaaC (Music as a Code) project live alongside the user inside MaaC Studio.\n");
    out.push_str(&format!(
        "Project file: {}\nProject dir: {}\nTitle: {} ({} Hz, {:.1}s)\n",
        ctx.entry_path.display(),
        ctx.project_root.display(),
        ctx.title,
        ctx.sample_rate_hz,
        ctx.total_frames as f64 / f64::from(ctx.sample_rate_hz.max(1)),
    ));
    out.push_str("\n## Shared Studio View (What the user is looking at right now)\n");
    out.push_str(&format!(
        "- Playhead: {} ({})\n",
        ctx.playhead_bar, ctx.playhead_time
    ));
    if let Some(loop_lbl) = &ctx.loop_label {
        out.push_str(&format!("- Active Loop: {loop_lbl}\n"));
    }
    if let Some(tr) = &ctx.selected_track {
        out.push_str(&format!("- Focused Track: {tr}\n"));
    }
    if let Some(node) = &ctx.selected_node {
        out.push_str(&format!("- Selected Mixer/FX Node: {node}\n"));
    }
    if !ctx.muted_tracks.is_empty() {
        out.push_str(&format!(
            "- Muted Tracks: {}\n",
            ctx.muted_tracks.join(", ")
        ));
    }
    if !ctx.solo_tracks.is_empty() {
        out.push_str(&format!("- Solo Tracks: {}\n", ctx.solo_tracks.join(", ")));
    }
    if !ctx.mounted_plugins.is_empty() {
        out.push_str(&format!(
            "- External AU/CLAP/VST3 Plugins: {}\n",
            ctx.mounted_plugins.join("; ")
        ));
    }
    if !ctx.selected_notes_desc.is_empty() {
        out.push_str("- Selected Notes:\n");
        for line in &ctx.selected_notes_desc {
            out.push_str(&format!("  - {line}\n"));
        }
    }

    if let Some(snap) = &ctx.audio_snapshot {
        out.push_str("\n## Focused Audio Telemetry (Measured from current MaaC render)\n");
        out.push_str(&format!(
            "- Span: {} (frames {}..{})\n- RMS: {:.1} dBFS | Peak: {:.1} dBFS | Crest: {:.1} dB | Stereo Width: {:.0}%\n- Spectral Energy: Low(<250Hz) {:.0}% | Mid(250Hz-4kHz) {:.0}% | High(>4kHz) {:.0}%\n",
            snap.bar_span_label,
            snap.span.start,
            snap.span.end,
            snap.rms_dbfs,
            snap.peak_dbfs,
            snap.crest_db,
            snap.stereo_width_pct,
            snap.low_pct,
            snap.mid_pct,
            snap.high_pct,
        ));
        if !snap.track_activity.is_empty() {
            let act: Vec<String> = snap
                .track_activity
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            out.push_str(&format!("- Active events in span: {}\n", act.join(", ")));
        }
    }

    if let Some(an) = &ctx.analysis {
        out.push_str("\n## Latest `maac analyze` Summary\n");
        if let Some(lufs) = an.integrated_lufs {
            out.push_str(&format!("- Integrated Loudness: {lufs:.1} LUFS"));
            if let Some(tp) = an.true_peak_dbtp {
                out.push_str(&format!(" | True Peak: {tp:.1} dBTP"));
            }
            out.push('\n');
        }
        if !an.findings.is_empty() {
            out.push_str("- Findings:\n");
            for f in an.findings.iter().take(6) {
                out.push_str(&format!(
                    "  - [{}] {}: {}\n",
                    f.severity, f.source, f.message
                ));
            }
        }
    }

    if !ctx.open_pins.is_empty() {
        out.push_str("\n## Open Timeline Review Pins\n");
        for pin in &ctx.open_pins {
            let tr = pin.track.as_deref().unwrap_or("all");
            out.push_str(&format!(
                "- [Pin #{}] {} ({tr}): {}\n",
                pin.id, pin.bar_label, pin.text
            ));
        }
    }

    if !ctx.recent_chat.is_empty() {
        out.push_str("\n## Recent Collaboration History\n");
        for msg in &ctx.recent_chat {
            let who = match msg.role {
                ChatRole::User => "User",
                ChatRole::Agent => "Agent",
                ChatRole::System => "System",
            };
            let snippet: String = msg.text.chars().take(240).collect();
            out.push_str(&format!("- {who}: {snippet}\n"));
        }
    }

    out.push_str("\n## Editing Guidelines\n");
    out.push_str(&format!(
        "- Edit `{}` directly on disk using valid MaaC v1 syntax, keeping changes minimal and musical.\n",
        ctx.entry_path.display()
    ));
    out.push_str("- Preserve existing comments and unrelated sections.\n");
    out.push_str("- If you resolve any Open Timeline Review Pins, include `[RESOLVED #<id>]` in your reply.\n");
    out.push_str("- Keep your final reply concise (2-5 bullet points) explaining what you changed and which bars to listen to.\n");

    out.push_str("\n## User Request\n");
    out.push_str(user_instruction.trim());
    out.push('\n');
    out
}

/// Extract `[RESOLVED #N]` pin IDs from an agent reply.
pub fn extract_resolved_pin_ids(reply: &str) -> Vec<u64> {
    let mut ids = Vec::new();
    let upper = reply.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let needle = b"RESOLVED #";
    let mut i = 0;
    while i + needle.len() < bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let mut j = i + needle.len();
            let mut num: u64 = 0;
            let mut digits = 0;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                num = num
                    .saturating_mul(10)
                    .saturating_add(u64::from(bytes[j] - b'0'));
                digits += 1;
                j += 1;
            }
            if digits > 0 && !ids.contains(&num) {
                ids.push(num);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    ids
}

#[derive(Clone)]
pub struct AgentTurnOutcome {
    pub reply_text: String,
    pub elapsed_ms: u64,
    pub resolved_pins: Vec<u64>,
    pub updated_project: Option<(String, ProjectData, AiEditHighlight)>,
    pub compile_error: Option<String>,
}

impl std::fmt::Debug for AgentTurnOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTurnOutcome")
            .field("elapsed_ms", &self.elapsed_ms)
            .field("resolved_pins", &self.resolved_pins)
            .field("has_edit", &self.updated_project.is_some())
            .field("compile_error", &self.compile_error)
            .finish()
    }
}

/// Run one non-interactive turn against a local agent CLI (`claude`, `agy`,
/// `codex`, `gemini`) on a background thread, then check & compile any `.maac`
/// edit it wrote to disk.
pub fn run_agent_turn(
    agent: &DetectedAgent,
    before_project: &ProjectData,
    before_source: &str,
    prompt: &str,
) -> Result<AgentTurnOutcome, String> {
    let started = Instant::now();
    // Ensure the current in-memory source is saved to disk so the CLI sees the exact state.
    std::fs::write(&before_project.entry_path, before_source).map_err(|e| {
        format!(
            "failed to sync {} before agent run: {e}",
            before_project.entry_path.display()
        )
    })?;

    let mut cmd = Command::new(&agent.path);
    cmd.current_dir(&before_project.project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match agent.kind {
        AgentKind::Claude => {
            cmd.args(["-p", "--permission-mode", "acceptEdits"]);
        }
        AgentKind::Agy => {
            cmd.args(["-p", "--mode", "accept-edits"]);
        }
        AgentKind::Codex => {
            cmd.args(["exec", "-"]);
        }
        AgentKind::Gemini => {
            cmd.args(["-p"]);
        }
    }

    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "failed to launch {} ({}): {e}",
            agent.kind.label(),
            agent.path.display()
        )
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("{} execution failed: {e}", agent.kind.label()))?;

    let elapsed_ms = started.elapsed().as_millis() as u64;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();

    if !output.status.success() && stdout.is_empty() {
        return Err(format!(
            "{} exited with {}: {}",
            agent.kind.label(),
            output.status,
            if stderr.is_empty() {
                "no error output"
            } else {
                &stderr
            }
        ));
    }

    let reply_text = if !stdout.is_empty() {
        stdout
    } else if !stderr.is_empty() {
        stderr
    } else {
        format!("{} completed.", agent.kind.label())
    };
    let resolved_pins = extract_resolved_pin_ids(&reply_text);

    // Check if the agent modified the `.maac` file on disk.
    let after_source = std::fs::read_to_string(&before_project.entry_path)
        .unwrap_or_else(|_| before_source.to_owned());
    let (updated_project, compile_error) = if after_source != before_source {
        match load_and_compile(&before_project.entry_path) {
            Ok(after_proj) => {
                let highlight = diff_projects_for_highlight(
                    before_project,
                    &after_proj,
                    before_source,
                    &after_source,
                );
                (Some((after_source, after_proj, highlight)), None)
            }
            Err(err) => (None, Some(err)),
        }
    } else {
        (None, None)
    };

    Ok(AgentTurnOutcome {
        reply_text,
        elapsed_ms,
        resolved_pins,
        updated_project,
        compile_error,
    })
}

/// Copy text to the macOS clipboard via `pbcopy` (best-effort fallback on other OSes).
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("pbcopy unavailable: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("pbcopy write failed: {e}"))?;
        }
        let st = child
            .wait()
            .map_err(|e| format!("pbcopy wait failed: {e}"))?;
        if !st.success() {
            return Err("pbcopy failed".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Err("clipboard copy helper is implemented for macOS".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::load_and_compile;

    #[test]
    fn collab_sidecar_roundtrips_pins_and_chat() {
        let dir = std::env::temp_dir().join("maac_studio_collab_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("song.maac");

        let file = CollabFile {
            version: 1,
            next_pin_id: 2,
            pins: vec![ReviewPin {
                id: 1,
                frame: 96_000,
                end_frame: Some(192_000),
                bar_label: "Bar 3.1".into(),
                track: Some("drums".into()),
                text: "Add snare fill before chorus".into(),
                resolved: false,
                created_by: PinAuthor::Human,
            }],
            chat: vec![ChatMessage {
                role: ChatRole::User,
                text: "Make the chorus punchier".into(),
                detail: None,
            }],
        };
        save_collab_file(&entry, &file).unwrap();
        let loaded = load_collab_file(&entry).unwrap().expect("collab file");
        assert_eq!(loaded.pins.len(), 1);
        assert_eq!(loaded.pins[0].text, "Add snare fill before chorus");
        assert_eq!(loaded.chat.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn measures_audio_telemetry_and_diffs_edits_on_hello() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let hello = manifest.join("../examples/hello.maac");
        let proj = load_and_compile(&hello).expect("load hello");

        let snap = measure_region_audio(
            &proj,
            Some(RenderSpan {
                start: 0,
                end: 48_000 * 2,
            }),
            0,
        )
        .expect("measure audio snapshot");
        assert!(
            snap.rms_dbfs > -60.0 && snap.rms_dbfs < 0.0,
            "rms={}",
            snap.rms_dbfs
        );
        assert!(snap.peak_dbfs > -40.0);
        let band_sum = snap.low_pct + snap.mid_pct + snap.high_pct;
        assert!((band_sum - 100.0).abs() < 1.0, "band_sum={band_sum}");
        assert!(!snap.track_activity.is_empty());

        // Simulate an AI edit changing pitch C3 -> D3 in hello.maac
        let before_src = proj.bundle.sources.get(&proj.bundle.entry).unwrap().clone();
        let after_src = before_src.replacen("C3", "D3", 1);
        let mut edited_bundle = proj.bundle.clone();
        edited_bundle
            .sources
            .insert(edited_bundle.entry.clone(), after_src.clone());
        let after_proj = crate::project::load_from_bundle(
            proj.entry_path.clone(),
            proj.project_root.clone(),
            edited_bundle,
        )
        .expect("compile edited hello");

        let hl = diff_projects_for_highlight(&proj, &after_proj, &before_src, &after_src);
        assert!(
            !hl.changed_note_addresses.is_empty(),
            "expected changed notes"
        );
        assert!(hl.bounding_span.is_some());
        assert_eq!(hl.added_lines, 1);
        assert_eq!(hl.removed_lines, 1);

        let ctx = SharedFocusContext::capture(
            &proj,
            0,
            Some(snap.span),
            &HashSet::new(),
            None,
            &HashSet::new(),
            &HashSet::new(),
            &[],
            &[ReviewPin {
                id: 7,
                frame: 0,
                end_frame: None,
                bar_label: "Bar 1.1".into(),
                track: Some("piano".into()),
                text: "Brighten chord voicing".into(),
                resolved: false,
                created_by: PinAuthor::Human,
            }],
            Some(&snap),
            None,
            &[],
        );
        let prompt = build_agent_prompt(&ctx, "Resolve pin #7");
        assert!(prompt.contains("[Pin #7]"));
        assert!(prompt.contains("Focused Audio Telemetry"));
        assert_eq!(
            extract_resolved_pin_ids("Done! [RESOLVED #7] and [resolved #12]."),
            vec![7, 12]
        );
    }
}
