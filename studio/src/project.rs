//! Loading, compiling, and extracting timeline data from a `.maac` project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use maac::analyze::meter_of;
use maac::bundle_fs::load_bundle;
use maac::compiler::compile_bundle_artifact_with_limits;
use maac::music::MeterMap;
use maac::plan::{EventKind, PlanLimits, Region, TempoMap};
use maac::PlanArtifact;
use num_traits::ToPrimitive;

#[derive(Clone, Debug)]
pub struct RegionSpan {
    pub id: String,
    pub label: Option<String>,
    pub start_frame: u64,
    pub end_frame: u64,
}

#[derive(Clone, Debug)]
pub struct RollNote {
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
    pub plan: PlanArtifact,
    pub limits: PlanLimits,
    pub meter: Option<MeterMap>,
    pub total_frames: u64,
    pub sample_rate_hz: u32,
    pub channels: usize,
    pub regions: Vec<RegionSpan>,
    pub notes: Vec<RollNote>,
    pub hits: Vec<RollHit>,
    pub tracks: Vec<TrackInfo>,
    pub min_pitch: f32,
    pub max_pitch: f32,
    pub compile_ms: u64,
}

pub fn load_and_compile(entry_path: &Path) -> Result<ProjectData, String> {
    let started = Instant::now();
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
                notes.push(RollNote {
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

    if !min_pitch.is_finite() || !max_pitch.is_finite() {
        min_pitch = 48.0;
        max_pitch = 72.0;
    } else if (max_pitch - min_pitch) < 12.0 {
        let mid = 0.5 * (min_pitch + max_pitch);
        min_pitch = mid - 6.0;
        max_pitch = mid + 6.0;
    }

    let regions = extract_regions(&plan, &limits, sample_rate_hz, total_frames);
    let title = canonical_entry
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into());

    Ok(ProjectData {
        entry_path: canonical_entry,
        project_root,
        title,
        plan,
        limits,
        meter,
        total_frames,
        sample_rate_hz,
        channels,
        regions,
        notes,
        hits,
        tracks,
        min_pitch: (min_pitch - 2.0).max(0.0),
        max_pitch: (max_pitch + 2.0).min(127.0),
        compile_ms: started.elapsed().as_millis() as u64,
    })
}

fn extract_regions(
    plan: &PlanArtifact,
    limits: &PlanLimits,
    sample_rate_hz: u32,
    total_frames: u64,
) -> Vec<RegionSpan> {
    let Ok(bytes) = plan.to_json_with_limits(limits) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(raw_regions) = json.get("regions").cloned() else {
        return Vec::new();
    };
    let Ok(regions) = serde_json::from_value::<Vec<Region>>(raw_regions) else {
        return Vec::new();
    };
    let Some(raw_tempo) = json.get("tempo").cloned() else {
        return Vec::new();
    };
    let Ok(tempo_map) = serde_json::from_value::<TempoMap>(raw_tempo) else {
        return Vec::new();
    };
    let Ok(checked_tempo) = tempo_map.checked() else {
        return Vec::new();
    };
    let origin_s = checked_tempo
        .seconds_at(&plan.output().score_start_q)
        .ok()
        .and_then(|t| t.to_f64())
        .unwrap_or(0.0);

    let mut out = Vec::with_capacity(regions.len());
    for region in regions {
        let Some(s0) = checked_tempo
            .seconds_at(&region.start_q)
            .ok()
            .and_then(|t| t.to_f64())
        else {
            continue;
        };
        let Some(s1) = checked_tempo
            .seconds_at(&region.end_q)
            .ok()
            .and_then(|t| t.to_f64())
        else {
            continue;
        };
        let t0 = (s0 - origin_s).max(0.0);
        let t1 = (s1 - origin_s).max(t0);
        let start_frame = ((t0 * sample_rate_hz as f64).round() as u64).min(total_frames);
        let end_frame = ((t1 * sample_rate_hz as f64).round() as u64)
            .max(start_frame)
            .min(total_frames);
        out.push(RegionSpan {
            id: region.id,
            label: region.label,
            start_frame,
            end_frame,
        });
    }
    out.sort_by_key(|r| (r.start_frame, r.end_frame));
    out
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
    }

    #[test]
    fn extracts_regions_from_real_song() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry.maac");
        assert!(
            data.regions.len() >= 4,
            "expected declared regions in ferry"
        );
        assert!(data.regions.iter().all(|r| r.end_frame > r.start_frame));
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
