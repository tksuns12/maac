//! Loading, compiling, and extracting timeline data from a `.maac` project.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use maac::analyze::meter_of;
use maac::bundle::SourceBundle;
use maac::bundle_fs::load_bundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::music::MeterMap;
use maac::plan::{EventKind, PlanLimits, Region, TempoMap};
use maac::{parse_rational, PlanArtifact};
use num_traits::ToPrimitive;

use crate::mixer::{extract_mixer_nodes, MixerNodeInfo};

#[derive(Clone, Debug)]
pub struct RegionSpan {
    pub id: String,
    pub label: Option<String>,
    pub start_frame: u64,
    pub end_frame: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct BarTick {
    pub bar: i64,
    pub frame: u64,
    pub beats: u32,
}

#[derive(Clone, Debug)]
pub struct RollNote {
    pub address: String,
    pub source_path: Vec<String>,
    pub shared_occurrences: usize,
    pub track_idx: usize,
    pub on_frame: u64,
    pub off_frame: u64,
    pub midi_pitch: f32,
    pub velocity: f32,
}

#[derive(Clone, Debug)]
pub struct RollHit {
    pub track_idx: usize,
    pub on_frame: u64,
    pub velocity: f32,
}

#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub name: String,
    pub note_count: usize,
    pub hit_count: usize,
}

#[derive(Clone, Debug)]
pub struct ProjectData {
    pub entry_path: PathBuf,
    pub project_root: PathBuf,
    pub title: String,
    pub bundle: SourceBundle,
    pub plan: PlanArtifact,
    pub plan_json: Option<Arc<serde_json::Value>>,
    pub limits: PlanLimits,
    pub meter: Option<MeterMap>,
    pub total_frames: u64,
    pub sample_rate_hz: u32,
    pub channels: usize,
    pub regions: Vec<RegionSpan>,
    pub bars: Vec<BarTick>,
    pub notes: Vec<RollNote>,
    pub hits: Vec<RollHit>,
    pub tracks: Vec<TrackInfo>,
    pub mixer_nodes: Vec<MixerNodeInfo>,
    pub min_pitch: f32,
    pub max_pitch: f32,
    pub compile_ms: u64,
}

impl ProjectData {
    /// Snap `frame` to the nearest bar boundary (or return `frame` if no bars).
    pub fn snap_frame_to_bar(&self, frame: u64) -> u64 {
        if self.bars.is_empty() {
            return frame;
        }
        let idx = self.bars.partition_point(|b| b.frame < frame);
        let prev = idx.checked_sub(1).and_then(|i| self.bars.get(i));
        let next = self.bars.get(idx);
        match (prev, next) {
            (Some(a), Some(b)) => {
                if frame.abs_diff(a.frame) <= frame.abs_diff(b.frame) {
                    a.frame
                } else {
                    b.frame
                }
            }
            (Some(a), None) => {
                // Also consider the song's end as a snap point after the last bar.
                if frame.abs_diff(a.frame) <= frame.abs_diff(self.total_frames) {
                    a.frame
                } else {
                    self.total_frames
                }
            }
            (None, Some(b)) => b.frame,
            (None, None) => frame,
        }
    }

    /// Return the bar boundary strictly after `frame`, or `total_frames`.
    pub fn next_bar_after(&self, frame: u64) -> u64 {
        let idx = self.bars.partition_point(|b| b.frame <= frame);
        self.bars
            .get(idx)
            .map_or(self.total_frames, |b| b.frame)
            .max(frame + 1)
            .min(self.total_frames)
    }

    /// Format a frame as `Bar N.B` using the precomputed bar grid.
    pub fn format_bar_position(&self, frame: u64) -> String {
        if self.bars.is_empty() {
            return String::new();
        }
        let idx = self.bars.partition_point(|b| b.frame <= frame);
        let cur_idx = idx.saturating_sub(1);
        let cur = self.bars[cur_idx];
        let next_frame = self
            .bars
            .get(cur_idx + 1)
            .map_or(self.total_frames, |b| b.frame);
        let span = next_frame.saturating_sub(cur.frame).max(1);
        let frac = (frame.saturating_sub(cur.frame) as f64 / span as f64).clamp(0.0, 0.9999);
        let beat = (frac * cur.beats.max(1) as f64).floor() as u32 + 1;
        format!("Bar {}.{}", cur.bar, beat)
    }

    /// Build a validated `PlanArtifact` with events for `muted_nodes` removed so
    /// solo/mute plays through the exact synth, effect, and master chain.
    pub fn plan_with_muted(&self, muted_nodes: &HashSet<String>) -> Result<PlanArtifact, String> {
        if muted_nodes.is_empty() {
            return Ok(self.plan.clone());
        }
        let Some(base) = self.plan_json.as_ref() else {
            return Err("plan JSON cache is unavailable".into());
        };
        let mut cloned = (**base).clone();
        if let Some(events) = cloned.get_mut("events").and_then(|v| v.as_array_mut()) {
            events.retain(|ev| {
                let target_node = ev
                    .pointer("/target/node")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                !muted_nodes.contains(target_node)
            });
        }
        let bytes = serde_json::to_vec(&cloned)
            .map_err(|e| format!("failed to encode solo/mute plan: {e}"))?;
        PlanArtifact::from_json_with_limits(&bytes, &self.limits)
            .map_err(|e| format!("{}: {}", e.code, e.message))
    }
}

pub fn load_and_compile(entry_path: &Path) -> Result<ProjectData, String> {
    let canonical_entry = std::fs::canonicalize(entry_path)
        .map_err(|err| format!("cannot open {}: {err}", entry_path.display()))?;
    let project_root = canonical_entry
        .parent()
        .ok_or_else(|| format!("no parent directory for {}", canonical_entry.display()))?
        .to_path_buf();

    let bundle = load_bundle(&canonical_entry, &project_root).map_err(|diags| {
        diags
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    load_from_bundle(canonical_entry, project_root, bundle)
}

pub fn load_from_bundle(
    canonical_entry: PathBuf,
    project_root: PathBuf,
    bundle: SourceBundle,
) -> Result<ProjectData, String> {
    let started = Instant::now();

    let meter = bundle
        .sources
        .get(&bundle.entry)
        .and_then(|src| maac::parse(src).ok())
        .and_then(|doc| meter_of(&doc));

    let limits = PlanLimits::song();
    let plan = compile_bundle_artifact_with_limits(&bundle, &limits).map_err(|diags| {
        diags
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    let output = plan.output();
    let total_frames = output.total_frames;
    let sample_rate_hz = output.sample_rate_hz;
    let channels = usize::from(output.channels);

    let events = plan
        .query_events_score_window_with_limits(&output.score_start_q, &output.score_end_q, &limits)
        .map_err(|err| format!("{}: {}", err.code, err.message))?;

    let mut track_map: BTreeMap<String, usize> = BTreeMap::new();
    let mut tracks: Vec<TrackInfo> = Vec::new();
    let mut notes: Vec<RollNote> = Vec::new();
    let mut hits: Vec<RollHit> = Vec::new();
    let mut source_counts: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    let mut min_pitch = f32::INFINITY;
    let mut max_pitch = f32::NEG_INFINITY;

    for ev in events {
        let name = ev.target.node.clone();
        let track_idx = match track_map.get(&name) {
            Some(&idx) => idx,
            None => {
                let idx = tracks.len();
                track_map.insert(name.clone(), idx);
                tracks.push(TrackInfo {
                    name,
                    note_count: 0,
                    hit_count: 0,
                });
                idx
            }
        };
        match ev.kind {
            EventKind::Note {
                pitch_hz, velocity, ..
            } => {
                let midi = if pitch_hz > 0.0 {
                    (69.0 + 12.0 * (pitch_hz / 440.0).log2()) as f32
                } else {
                    60.0
                };
                min_pitch = min_pitch.min(midi);
                max_pitch = max_pitch.max(midi);
                let off_frame = ev
                    .off_frame
                    .unwrap_or_else(|| ev.on_frame.saturating_add(sample_rate_hz as u64 / 8))
                    .min(total_frames);
                let vel = velocity.to_f32().unwrap_or(0.7).clamp(0.05, 1.0);
                tracks[track_idx].note_count += 1;
                if !ev.source.path.is_empty() {
                    *source_counts.entry(ev.source.path.clone()).or_insert(0) += 1;
                }
                notes.push(RollNote {
                    address: ev.address,
                    source_path: ev.source.path,
                    shared_occurrences: 1,
                    track_idx,
                    on_frame: ev.on_frame,
                    off_frame: off_frame.max(ev.on_frame + 1),
                    midi_pitch: midi,
                    velocity: vel,
                });
            }
            EventKind::Hit { velocity, .. } => {
                let vel = velocity.to_f32().unwrap_or(0.7).clamp(0.05, 1.0);
                tracks[track_idx].hit_count += 1;
                hits.push(RollHit {
                    track_idx,
                    on_frame: ev.on_frame,
                    velocity: vel,
                });
            }
            EventKind::Message { .. } => {}
        }
    }

    for note in &mut notes {
        if let Some(&count) = source_counts.get(&note.source_path) {
            note.shared_occurrences = count;
        }
    }

    if !min_pitch.is_finite() || !max_pitch.is_finite() {
        min_pitch = 48.0;
        max_pitch = 72.0;
    } else if (max_pitch - min_pitch) < 12.0 {
        let mid = 0.5 * (min_pitch + max_pitch);
        min_pitch = mid - 6.0;
        max_pitch = mid + 6.0;
    }

    let plan_json = plan
        .to_json_with_limits(&limits)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .map(Arc::new);

    let (regions, bars, mixer_nodes) = match plan_json.as_deref() {
        Some(json) => {
            let (regions, bars) =
                extract_timeline_grid(json, &plan, meter.as_ref(), sample_rate_hz, total_frames);
            let mixer_nodes = extract_mixer_nodes(&bundle, json, &tracks, sample_rate_hz);
            (regions, bars, mixer_nodes)
        }
        None => (Vec::new(), Vec::new(), Vec::new()),
    };

    let title = canonical_entry
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into());

    Ok(ProjectData {
        entry_path: canonical_entry,
        project_root,
        title,
        bundle,
        plan,
        plan_json,
        limits,
        meter,
        total_frames,
        sample_rate_hz,
        channels,
        regions,
        bars,
        notes,
        hits,
        tracks,
        mixer_nodes,
        min_pitch: (min_pitch - 2.0).max(0.0),
        max_pitch: (max_pitch + 2.0).min(127.0),
        compile_ms: started.elapsed().as_millis() as u64,
    })
}

fn extract_timeline_grid(
    json: &serde_json::Value,
    plan: &PlanArtifact,
    meter: Option<&MeterMap>,
    sample_rate_hz: u32,
    total_frames: u64,
) -> (Vec<RegionSpan>, Vec<BarTick>) {
    let Some(raw_tempo) = json.get("tempo").cloned() else {
        return (Vec::new(), Vec::new());
    };
    let Ok(tempo_map) = serde_json::from_value::<TempoMap>(raw_tempo) else {
        return (Vec::new(), Vec::new());
    };
    let Ok(checked_tempo) = tempo_map.checked() else {
        return (Vec::new(), Vec::new());
    };
    let score_start_q = &plan.output().score_start_q;
    let score_end_q = &plan.output().score_end_q;
    let origin_s = checked_tempo
        .seconds_at(score_start_q)
        .ok()
        .and_then(|t| t.to_f64())
        .unwrap_or(0.0);

    let q_to_frame = |q: &maac::Rational| -> Option<u64> {
        let secs = checked_tempo.seconds_at(q).ok()?.to_f64()?;
        let rel = (secs - origin_s).max(0.0);
        Some(((rel * sample_rate_hz as f64).round() as u64).min(total_frames))
    };

    // 1. Regions
    let mut regions_out = Vec::new();
    if let Some(raw_regions) = json.get("regions").cloned() {
        if let Ok(regions) = serde_json::from_value::<Vec<Region>>(raw_regions) {
            for region in regions {
                let (Some(start_frame), Some(end_frame)) =
                    (q_to_frame(&region.start_q), q_to_frame(&region.end_q))
                else {
                    continue;
                };
                regions_out.push(RegionSpan {
                    id: region.id,
                    label: region.label,
                    start_frame,
                    end_frame: end_frame.max(start_frame),
                });
            }
            regions_out.sort_by_key(|r| (r.start_frame, r.end_frame));
        }
    }

    // 2. Bar ticks (from MeterMap, or 4/4 fallback).
    let mut bars = Vec::new();
    let one = parse_rational("1").unwrap();
    if let Some(meter) = meter {
        if let (Ok(first), Ok(last)) = (meter.q_to_bar(score_start_q), meter.q_to_bar(score_end_q))
        {
            let max_bars = (last.bar - first.bar + 2).clamp(0, 4_096);
            for offset in 0..max_bars {
                let bar_num = first.bar + offset;
                let Ok(bar_q) = meter.bar_to_q(bar_num, &one) else {
                    break;
                };
                if &bar_q > score_end_q {
                    break;
                }
                if &bar_q < score_start_q {
                    continue;
                }
                let Some(frame) = q_to_frame(&bar_q) else {
                    break;
                };
                // Infer beat count from bar length in quarter notes.
                let beats = meter
                    .bar_to_q(bar_num + 1, &one)
                    .ok()
                    .and_then(|next_q| (next_q - bar_q).to_f64())
                    .map(|dq| dq.round().clamp(1.0, 16.0) as u32)
                    .unwrap_or(4);
                bars.push(BarTick {
                    bar: bar_num,
                    frame,
                    beats,
                });
            }
        }
    } else {
        // Fallback: 4/4 bars starting from score_start_q.
        let four = parse_rational("4").unwrap();
        let mut q = score_start_q.clone();
        let mut bar_num = 1i64;
        while &q <= score_end_q && bars.len() < 4_096 {
            let Some(frame) = q_to_frame(&q) else {
                break;
            };
            bars.push(BarTick {
                bar: bar_num,
                frame,
                beats: 4,
            });
            q += &four;
            bar_num += 1;
        }
    }

    (regions_out, bars)
}

pub fn format_time(frames: u64, sample_rate_hz: u32) -> String {
    let rate = sample_rate_hz.max(1) as f64;
    let total_secs = frames as f64 / rate;
    let mins = (total_secs / 60.0).floor() as u64;
    let secs = total_secs - (mins as f64) * 60.0;
    format!("{mins:02}:{secs:04.1}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_example_hello() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/hello.maac");
        let data = load_and_compile(&path).expect("compile hello.maac");
        assert!(data.total_frames > 0);
        assert_eq!(data.sample_rate_hz, 48_000);
        assert!(!data.notes.is_empty());
        assert!(!data.hits.is_empty());
        assert!(data.tracks.len() >= 2);
        assert!(!data.bars.is_empty());
    }

    #[test]
    fn extracts_regions_and_bars_from_real_song() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry.maac");
        assert!(
            data.regions.len() >= 4,
            "expected declared regions in ferry"
        );
        assert!(data.regions.iter().all(|r| r.end_frame > r.start_frame));
        assert!(data.bars.len() >= 30, "expected bar ticks in ferry");
        assert!(data.bars.windows(2).all(|w| w[0].frame < w[1].frame));
        let snapped = data.snap_frame_to_bar(data.bars[4].frame + 500);
        assert_eq!(snapped, data.bars[4].frame);
        assert!(data
            .format_bar_position(data.bars[4].frame)
            .starts_with("Bar 5."));
    }

    #[test]
    fn mutes_and_solos_tracks_cleanly() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry.maac");
        let muted: HashSet<String> = data
            .tracks
            .iter()
            .filter(|t| t.name != "lead")
            .map(|t| t.name.clone())
            .collect();
        let solo_plan = data.plan_with_muted(&muted).expect("solo lead plan");
        assert!(solo_plan.event_count() < data.plan.event_count());
        assert!(solo_plan.event_count() > 0);
        // Verify the filtered plan renders cleanly.
        let mut engine =
            maac::dsp::DspEngine::new_artifact_with_limits(&solo_plan, &data.limits).unwrap();
        engine
            .render_chunk(
                maac::dsp::RenderSpan {
                    start: 0,
                    end: 1_024,
                },
                |_| Ok(()),
            )
            .expect("render solo chunk");
    }

    #[test]
    fn runs_analysis_on_hello() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/hello.maac");
        let data = load_and_compile(&path).expect("compile hello.maac");
        let summary = crate::analysis::run_analysis(data.plan, data.meter, data.limits)
            .expect("analyze hello.maac");
        assert!(summary.integrated_lufs.is_some());
        assert!(!summary.sources.is_empty());
    }
}
