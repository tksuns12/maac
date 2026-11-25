//! Main Elm-architecture `iced` application for MaaC Studio.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::keyboard::{self, key::Named, Key};
use iced::widget::{
    button, canvas, column, container, horizontal_space, row, scrollable, slider, text, toggler,
    Column, Row, Space,
};
use iced::{
    Alignment, Background, Border, Color, Element, Font, Length, Subscription, Task, Theme,
};
use maac::dsp::{BackgroundSession, Playback, PlaybackConfig, RenderSpan, SessionPolicy};
use maac::export::{
    render_wav_to_path_artifact_range_with_limits, render_wav_to_path_artifact_with_limits,
    FrameRange, WavFormat,
};
use maac::PlanArtifact;

use crate::analysis::{run_analysis, AnalysisSummary};
use crate::audio::AudioEngine;
use crate::code_view::{
    changed_new_lines, compute_line_diff, locate_object_lines, DiffLineKind, DiffSummary,
    SourceHighlight,
};
use crate::editor::{
    add_note_at_score_pos, apply_batch_note_edit, apply_note_edit, format_q,
    inspect_note_in_bundle, materialize_note_placement, midi_to_pitch_symbol, BatchNoteEdit,
    NoteEdit, NoteSourceInfo,
};
use crate::mixer::{
    apply_node_param_edit, apply_node_param_reset, MixerNodeInfo, NodeCategory, NodeParamInfo,
};
use crate::project::{format_time, load_and_compile, load_from_bundle, ProjectData};
use crate::timeline::{track_color, TimelineAction, TimelineView};
use crate::watcher::ProjectWatcher;

const WATCH_DEBOUNCE: Duration = Duration::from_millis(120);
const PLAYBACK_EDIT_DEBOUNCE: Duration = Duration::from_millis(80);
const SELF_SAVE_SUPPRESS: Duration = Duration::from_millis(700);
const CHECKPOINT_SECONDS: u64 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodeTab {
    LiveCode,
    UnsavedDiff,
    ExternalDiff,
}

#[derive(Clone, Debug)]
struct ExternalChange {
    prev_source: String,
    diff: DiffSummary,
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenDialog,
    FilePicked(Option<PathBuf>),
    ProjectCompiled {
        path: PathBuf,
        is_reload: bool,
        result: Result<Box<ProjectData>, String>,
    },
    TogglePlay,
    StopToStart,
    Seek(u64),
    SetLoop(Option<RenderSpan>),
    LoopRegion(RenderSpan),
    ToggleLoop,
    ToggleSnap,
    ToggleFollow,
    ToggleWatch(bool),
    ToggleTrackVisible(usize),
    ToggleTrackMute(usize),
    ToggleTrackSolo(usize),
    ClearSoloMute,
    MixPlanReady {
        revision: u64,
        result: Result<Box<PlanArtifact>, String>,
    },
    SelectNote(Option<String>),
    ToggleSelectNote(String),
    SelectNotesBox(Vec<String>),
    ClearSelection,
    ApplyNoteEdit {
        source_path: Vec<String>,
        edit: NoteEdit,
    },
    ApplyBatchNoteEdit(BatchNoteEdit),
    MaterializeSelectedNote,
    MaterializedApplied {
        prev_source: String,
        remapped_addr: Option<String>,
        summary: String,
        result: Result<Box<ProjectData>, String>,
    },
    AddNoteAt {
        frame: u64,
        midi_pitch: i32,
    },
    NoteAdded {
        prev_source: String,
        new_source_path: Vec<String>,
        summary: String,
        result: Result<Box<ProjectData>, String>,
    },
    VelocitySliderChanged(f32),
    CommitVelocity,
    NoteEditApplied {
        prev_source: String,
        summary: String,
        result: Result<Box<ProjectData>, String>,
    },
    Undo,
    Redo,
    HistoryRestored {
        is_undo: bool,
        prev_source: String,
        result: Result<Box<ProjectData>, String>,
    },
    SaveToDisk,
    KeyboardNoteNudge {
        pitch_semis: i32,
        at_q16: i64,
    },
    KeyboardDeleteNote,
    KeyboardDuplicateNote,
    ToggleMixer,
    SelectMixerNode(Option<String>),
    NodeParamDraft {
        node_id: String,
        param_name: String,
        value: f32,
    },
    CommitNodeParam {
        node_id: String,
        param_name: String,
    },
    ResetNodeParam {
        node_id: String,
        param_name: String,
    },
    ToggleCodePanel,
    SelectCodeTab(CodeTab),
    ToggleCodeFocused,
    DiscardUnsavedEdits,
    RevertExternalEdit,
    DismissExternalChange,
    GainChanged(f32),
    ZoomDelta {
        delta: f32,
        anchor: f32,
    },
    ZoomFit,
    PanDelta(f32),
    ScrollChanged(f32),
    RecompileNow,
    ExportWav {
        loop_only: bool,
    },
    ExportPathPicked {
        loop_only: bool,
        path: Option<PathBuf>,
    },
    ExportFinished(Result<String, String>),
    RunAnalysis,
    AnalysisFinished(Result<AnalysisSummary, String>),
    ToggleSidebar,
    Tick,
    Timeline(TimelineAction),
}

pub struct StudioApp {
    audio: AudioEngine,
    project: Option<ProjectData>,
    active_plan: Option<PlanArtifact>,
    session: Option<BackgroundSession>,
    playback: Option<Arc<Playback>>,
    watcher: Option<ProjectWatcher>,

    auto_watch: bool,
    pending_watch_at: Option<Instant>,
    last_self_save: Option<Instant>,
    compiling: bool,
    compile_error: Option<String>,
    last_reload_note: Option<String>,

    // Direct editing & transactional history
    selected_note_address: Option<String>,
    selected_notes: HashSet<String>,
    selected_note_info: Option<NoteSourceInfo>,
    velocity_draft: Option<f32>,
    saved_source: Option<String>,
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
    editing: bool,
    edit_error: Option<String>,

    // Live Mixer & Node/FX Parameter Editor
    show_mixer: bool,
    selected_node_id: Option<String>,
    param_drafts: HashMap<(String, String), f32>,

    // Live .maac Code & Diff Collaboration Panel
    show_code: bool,
    code_tab: CodeTab,
    code_focused_only: bool,
    cached_highlight: Option<SourceHighlight>,
    cached_unsaved_diff: DiffSummary,
    cached_changed_lines: HashSet<usize>,
    external_change: Option<ExternalChange>,

    // Transport & view state
    playhead_frame: u64,
    checkpoint_frame: u64,
    checkpoint_count: usize,
    worker_busy: bool,
    underruns: u64,
    stale: bool,
    approximate: bool,
    playback_error: Option<String>,
    loop_range: Option<RenderSpan>,
    last_loop_range: Option<RenderSpan>,
    snap_to_bar: bool,
    auto_follow: bool,
    visible_tracks: Vec<bool>,
    muted_tracks: HashSet<String>,
    solo_tracks: HashSet<String>,
    mix_revision: u64,
    zoom: f32,
    scroll: f32,
    peaks: (f32, f32),

    // Export
    exporting: bool,

    // Analysis sidebar
    show_sidebar: bool,
    analyzing: bool,
    analysis: Option<AnalysisSummary>,
    analysis_error: Option<String>,
}

impl StudioApp {
    pub fn new(initial_path: Option<PathBuf>) -> (Self, Task<Message>) {
        let app = Self {
            audio: AudioEngine::new(48_000),
            project: None,
            active_plan: None,
            session: None,
            playback: None,
            watcher: None,
            auto_watch: true,
            pending_watch_at: None,
            last_self_save: None,
            compiling: false,
            compile_error: None,
            last_reload_note: None,
            selected_note_address: None,
            selected_notes: HashSet::new(),
            selected_note_info: None,
            velocity_draft: None,
            saved_source: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            editing: false,
            edit_error: None,
            show_mixer: false,
            selected_node_id: None,
            param_drafts: HashMap::new(),
            show_code: false,
            code_tab: CodeTab::LiveCode,
            code_focused_only: true,
            cached_highlight: None,
            cached_unsaved_diff: DiffSummary::default(),
            cached_changed_lines: HashSet::new(),
            external_change: None,
            playhead_frame: 0,
            checkpoint_frame: 0,
            checkpoint_count: 0,
            worker_busy: false,
            underruns: 0,
            stale: false,
            approximate: false,
            playback_error: None,
            loop_range: None,
            last_loop_range: None,
            snap_to_bar: true,
            auto_follow: true,
            visible_tracks: Vec::new(),
            muted_tracks: HashSet::new(),
            solo_tracks: HashSet::new(),
            mix_revision: 0,
            zoom: 1.0,
            scroll: 0.0,
            peaks: (0.0, 0.0),
            exporting: false,
            show_sidebar: true,
            analyzing: false,
            analysis: None,
            analysis_error: None,
        };

        let task = match initial_path {
            Some(path) => spawn_compile(path, false),
            None => Task::none(),
        };
        (app, task)
    }

    fn is_dirty(&self) -> bool {
        match (&self.project, &self.saved_source) {
            (Some(proj), Some(saved)) => proj
                .bundle
                .sources
                .get(&proj.bundle.entry)
                .is_some_and(|cur| cur != saved),
            _ => false,
        }
    }

    pub fn title(&self) -> String {
        match &self.project {
            Some(proj) => {
                let star = if self.is_dirty() { " *" } else { "" };
                format!("MaaC Studio — {}{star}", proj.title)
            }
            None => "MaaC Studio".into(),
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let tick = iced::time::every(Duration::from_millis(16)).map(|_| Message::Tick);
        let keys = keyboard::on_key_press(|key, modifiers| {
            if modifiers.alt() {
                return None;
            }
            if modifiers.command() || modifiers.control() {
                return match key {
                    Key::Character(ref c) if c.eq_ignore_ascii_case("z") => {
                        if modifiers.shift() {
                            Some(Message::Redo)
                        } else {
                            Some(Message::Undo)
                        }
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("y") => Some(Message::Redo),
                    Key::Character(ref c) if c.eq_ignore_ascii_case("s") => {
                        Some(Message::SaveToDisk)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("d") => {
                        Some(Message::KeyboardDuplicateNote)
                    }
                    _ => None,
                };
            }
            match key {
                Key::Named(Named::Space) => Some(Message::TogglePlay),
                Key::Named(Named::Home) => Some(Message::StopToStart),
                Key::Named(Named::Escape) => Some(Message::ClearSelection),
                Key::Named(Named::Delete | Named::Backspace) => Some(Message::KeyboardDeleteNote),
                Key::Named(Named::ArrowUp) => Some(Message::KeyboardNoteNudge {
                    pitch_semis: if modifiers.shift() { 12 } else { 1 },
                    at_q16: 0,
                }),
                Key::Named(Named::ArrowDown) => Some(Message::KeyboardNoteNudge {
                    pitch_semis: if modifiers.shift() { -12 } else { -1 },
                    at_q16: 0,
                }),
                Key::Named(Named::ArrowRight) => Some(Message::KeyboardNoteNudge {
                    pitch_semis: 0,
                    at_q16: if modifiers.shift() { 4 } else { 1 },
                }),
                Key::Named(Named::ArrowLeft) => Some(Message::KeyboardNoteNudge {
                    pitch_semis: 0,
                    at_q16: if modifiers.shift() { -4 } else { -1 },
                }),
                Key::Character(ref c) if c.eq_ignore_ascii_case("l") => Some(Message::ToggleLoop),
                Key::Character(ref c) if c.eq_ignore_ascii_case("m") => Some(Message::ToggleMixer),
                Key::Character(ref c) if c.eq_ignore_ascii_case("c") => {
                    Some(Message::ToggleCodePanel)
                }
                Key::Character(ref c) if c.eq_ignore_ascii_case("s") => Some(Message::ToggleSnap),
                Key::Character(ref c) if c.eq_ignore_ascii_case("f") => Some(Message::ToggleFollow),
                Key::Character(ref c) if c.eq_ignore_ascii_case("r") => Some(Message::RecompileNow),
                Key::Character(ref c) if c.eq_ignore_ascii_case("a") => Some(Message::RunAnalysis),
                _ => None,
            }
        });
        Subscription::batch([tick, keys])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenDialog => Task::perform(
                async {
                    rfd::FileDialog::new()
                        .add_filter("MaaC Score", &["maac"])
                        .pick_file()
                },
                Message::FilePicked,
            ),
            Message::FilePicked(Some(path)) => {
                self.compiling = true;
                self.compile_error = None;
                spawn_compile(path, false)
            }
            Message::FilePicked(None) => Task::none(),

            Message::RecompileNow => {
                if let Some(proj) = &self.project {
                    if !self.compiling {
                        self.compiling = true;
                        return spawn_compile(proj.entry_path.clone(), true);
                    }
                }
                Task::none()
            }

            Message::ProjectCompiled {
                path,
                is_reload,
                result,
            } => {
                self.compiling = false;
                match result {
                    Ok(data) => {
                        self.compile_error = None;
                        return self.apply_compiled_project(*data, is_reload);
                    }
                    Err(err) => {
                        self.compile_error =
                            Some(format!("Compile failed ({}):\n{err}", path.display()));
                    }
                }
                Task::none()
            }

            Message::TogglePlay => {
                if self.project.is_none() {
                    return Task::none();
                }
                if self.audio.is_playing() {
                    self.audio.set_playing(false);
                } else {
                    self.ensure_playback_started();
                    self.audio.set_playing(true);
                }
                Task::none()
            }

            Message::StopToStart => {
                self.audio.set_playing(false);
                let target = self.loop_range.map_or(0, |r| r.start);
                self.seek_to(target);
                Task::none()
            }

            Message::Seek(frame) => {
                self.seek_to(frame);
                Task::none()
            }

            Message::SetLoop(range) => {
                self.apply_loop(range);
                Task::none()
            }

            Message::LoopRegion(span) => {
                self.apply_loop(Some(span));
                self.seek_to(span.start);
                if self.project.is_some() && !self.audio.is_playing() {
                    self.ensure_playback_started();
                    self.audio.set_playing(true);
                }
                Task::none()
            }

            Message::ToggleLoop => {
                if self.loop_range.is_some() {
                    self.last_loop_range = self.loop_range;
                    self.apply_loop(None);
                } else if let Some(prev) = self.last_loop_range {
                    self.apply_loop(Some(prev));
                } else if let Some(proj) = &self.project {
                    let span = proj
                        .regions
                        .first()
                        .map(|r| RenderSpan {
                            start: r.start_frame,
                            end: r.end_frame,
                        })
                        .unwrap_or(RenderSpan {
                            start: 0,
                            end: proj.total_frames.max(1),
                        });
                    self.apply_loop(Some(span));
                }
                Task::none()
            }

            Message::ToggleSnap => {
                self.snap_to_bar = !self.snap_to_bar;
                Task::none()
            }

            Message::ToggleFollow => {
                self.auto_follow = !self.auto_follow;
                Task::none()
            }

            Message::ToggleWatch(on) => {
                self.auto_watch = on;
                self.pending_watch_at = None;
                Task::none()
            }

            Message::ToggleTrackVisible(idx) => {
                if let Some(vis) = self.visible_tracks.get_mut(idx) {
                    *vis = !*vis;
                }
                Task::none()
            }

            Message::ToggleTrackMute(idx) => {
                if let Some(name) = self
                    .project
                    .as_ref()
                    .and_then(|p| p.tracks.get(idx))
                    .map(|t| t.name.clone())
                {
                    if !self.muted_tracks.remove(&name) {
                        self.muted_tracks.insert(name);
                    }
                    return self.spawn_mix_update();
                }
                Task::none()
            }

            Message::ToggleTrackSolo(idx) => {
                if let Some(name) = self
                    .project
                    .as_ref()
                    .and_then(|p| p.tracks.get(idx))
                    .map(|t| t.name.clone())
                {
                    if !self.solo_tracks.remove(&name) {
                        self.solo_tracks.insert(name);
                    }
                    return self.spawn_mix_update();
                }
                Task::none()
            }

            Message::ClearSoloMute => {
                if !self.muted_tracks.is_empty() || !self.solo_tracks.is_empty() {
                    self.muted_tracks.clear();
                    self.solo_tracks.clear();
                    return self.spawn_mix_update();
                }
                Task::none()
            }

            Message::MixPlanReady { revision, result } => {
                if revision != self.mix_revision {
                    return Task::none();
                }
                match result {
                    Ok(plan) => {
                        if let Some(session) = &self.session {
                            session.update(&plan);
                        }
                        self.active_plan = Some(*plan);
                        self.playback_error = None;
                    }
                    Err(err) => {
                        self.playback_error = Some(format!("Solo/Mute update failed: {err}"));
                    }
                }
                Task::none()
            }

            Message::SelectNote(addr) => {
                self.selected_notes.clear();
                if let Some(a) = &addr {
                    self.selected_notes.insert(a.clone());
                }
                self.selected_note_address = addr;
                self.edit_error = None;
                self.refresh_selected_note_info();
                Task::none()
            }

            Message::ToggleSelectNote(addr) => {
                if self.selected_notes.contains(&addr) {
                    self.selected_notes.remove(&addr);
                    if self.selected_note_address.as_deref() == Some(addr.as_str()) {
                        self.selected_note_address = self.selected_notes.iter().next().cloned();
                    }
                } else {
                    self.selected_notes.insert(addr.clone());
                    self.selected_note_address = Some(addr);
                }
                self.edit_error = None;
                self.refresh_selected_note_info();
                Task::none()
            }

            Message::SelectNotesBox(addrs) => {
                self.selected_notes = addrs.into_iter().collect();
                self.selected_note_address = self.selected_notes.iter().next().cloned();
                self.edit_error = None;
                self.refresh_selected_note_info();
                Task::none()
            }

            Message::ClearSelection => {
                self.selected_notes.clear();
                self.selected_note_address = None;
                self.selected_note_info = None;
                self.velocity_draft = None;
                self.edit_error = None;
                self.refresh_code_cache();
                Task::none()
            }

            Message::VelocitySliderChanged(v) => {
                self.velocity_draft = Some(v.clamp(0.05, 1.0));
                Task::none()
            }

            Message::CommitVelocity => {
                let Some(vel) = self.velocity_draft else {
                    return Task::none();
                };
                if self.selected_notes.len() > 1 {
                    return self
                        .update(Message::ApplyBatchNoteEdit(BatchNoteEdit::SetVelocity(vel)));
                }
                if let Some(info) = &self.selected_note_info {
                    let source_path = info.path.clone();
                    return self.update(Message::ApplyNoteEdit {
                        source_path,
                        edit: NoteEdit::SetVelocity(vel),
                    });
                }
                Task::none()
            }

            Message::ApplyBatchNoteEdit(edit) => {
                if self.editing || self.selected_notes.is_empty() {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let source_paths: Vec<Vec<String>> = proj
                    .notes
                    .iter()
                    .filter(|n| self.selected_notes.contains(&n.address))
                    .map(|n| n.source_path.clone())
                    .collect();
                if source_paths.is_empty() {
                    return Task::none();
                }
                let bundle = proj.bundle.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let (updated_bundle, summary) =
                                apply_batch_note_edit(&bundle, &source_paths, edit, &limits)?;
                            let recompiled =
                                load_from_bundle(entry_path, project_root, updated_bundle)?;
                            Ok((Box::new(recompiled), summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("batch edit worker panicked".into()))
                    },
                    move |res| match res {
                        Ok((data, summary)) => Message::NoteEditApplied {
                            prev_source: prev_source.clone(),
                            summary,
                            result: Ok(data),
                        },
                        Err(err) => Message::NoteEditApplied {
                            prev_source: prev_source.clone(),
                            summary: String::new(),
                            result: Err(err),
                        },
                    },
                )
            }

            Message::MaterializeSelectedNote => {
                if self.editing {
                    return Task::none();
                }
                let (Some(proj), Some(addr)) = (&self.project, self.selected_note_address.clone())
                else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let bundle = proj.bundle.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let (updated_bundle, remapped, summary) =
                                materialize_note_placement(&bundle, &addr, &limits)?;
                            let recompiled =
                                load_from_bundle(entry_path, project_root, updated_bundle)?;
                            Ok((Box::new(recompiled), remapped, summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("materialize worker panicked".into()))
                    },
                    move |res| match res {
                        Ok((data, remapped_addr, summary)) => Message::MaterializedApplied {
                            prev_source: prev_source.clone(),
                            remapped_addr,
                            summary,
                            result: Ok(data),
                        },
                        Err(err) => Message::MaterializedApplied {
                            prev_source: prev_source.clone(),
                            remapped_addr: None,
                            summary: String::new(),
                            result: Err(err),
                        },
                    },
                )
            }

            Message::MaterializedApplied {
                prev_source,
                remapped_addr,
                summary,
                result,
            } => {
                self.editing = false;
                match result {
                    Ok(data) => {
                        self.undo_stack.push(prev_source);
                        if self.undo_stack.len() > 100 {
                            self.undo_stack.remove(0);
                        }
                        self.redo_stack.clear();
                        self.edit_error = None;
                        if let Some(new_addr) = remapped_addr {
                            self.selected_notes.clear();
                            self.selected_notes.insert(new_addr.clone());
                            self.selected_note_address = Some(new_addr);
                        }
                        let ms = data.compile_ms;
                        return self.apply_in_memory_project(*data, format!("{summary} ({ms} ms)"));
                    }
                    Err(err) => {
                        self.edit_error = Some(err);
                    }
                }
                Task::none()
            }

            Message::AddNoteAt { frame, midi_pitch } => {
                if self.editing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                // Pick target track: selected note's track -> selected mixer track -> first visible audible melodic track.
                let audible = self.audible_track_flags(proj);
                let target_track = self
                    .selected_note_address
                    .as_deref()
                    .and_then(|addr| proj.notes.iter().find(|n| n.address == addr))
                    .and_then(|n| proj.tracks.get(n.track_idx))
                    .map(|t| t.name.clone())
                    .or_else(|| {
                        self.selected_node_id
                            .as_deref()
                            .filter(|id| {
                                proj.tracks
                                    .iter()
                                    .any(|t| t.name == *id && t.note_count > 0)
                            })
                            .map(str::to_owned)
                    })
                    .or_else(|| {
                        proj.tracks
                            .iter()
                            .enumerate()
                            .find(|(idx, t)| {
                                t.note_count > 0
                                    && self.visible_tracks.get(*idx).copied().unwrap_or(true)
                                    && audible.get(*idx).copied().unwrap_or(true)
                            })
                            .map(|(_, t)| t.name.clone())
                    })
                    .or_else(|| {
                        proj.tracks
                            .iter()
                            .find(|t| t.note_count > 0)
                            .map(|t| t.name.clone())
                    });
                let Some(track_name) = target_track else {
                    self.edit_error = Some("No melodic track available to insert a note".into());
                    return Task::none();
                };
                let score_q = proj.frame_to_score_q(frame);
                let bundle = proj.bundle.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let (updated_bundle, new_path, summary) = add_note_at_score_pos(
                                &bundle,
                                &track_name,
                                score_q,
                                midi_pitch,
                                &limits,
                            )?;
                            let recompiled =
                                load_from_bundle(entry_path, project_root, updated_bundle)?;
                            Ok((Box::new(recompiled), new_path, summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("add note worker panicked".into()))
                    },
                    move |res| match res {
                        Ok((data, new_source_path, summary)) => Message::NoteAdded {
                            prev_source: prev_source.clone(),
                            new_source_path,
                            summary,
                            result: Ok(data),
                        },
                        Err(err) => Message::NoteAdded {
                            prev_source: prev_source.clone(),
                            new_source_path: Vec::new(),
                            summary: String::new(),
                            result: Err(err),
                        },
                    },
                )
            }

            Message::NoteAdded {
                prev_source,
                new_source_path,
                summary,
                result,
            } => {
                self.editing = false;
                match result {
                    Ok(data) => {
                        self.undo_stack.push(prev_source);
                        if self.undo_stack.len() > 100 {
                            self.undo_stack.remove(0);
                        }
                        self.redo_stack.clear();
                        self.edit_error = None;
                        if let Some(added) =
                            data.notes.iter().find(|n| n.source_path == new_source_path)
                        {
                            self.selected_notes.clear();
                            self.selected_notes.insert(added.address.clone());
                            self.selected_note_address = Some(added.address.clone());
                        }
                        let ms = data.compile_ms;
                        return self.apply_in_memory_project(*data, format!("{summary} ({ms} ms)"));
                    }
                    Err(err) => {
                        self.edit_error = Some(err);
                    }
                }
                Task::none()
            }

            Message::ApplyNoteEdit { source_path, edit } => {
                if self.editing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let bundle = proj.bundle.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let (updated_bundle, summary) =
                                apply_note_edit(&bundle, &source_path, edit, &limits)?;
                            let recompiled =
                                load_from_bundle(entry_path, project_root, updated_bundle)?;
                            Ok((Box::new(recompiled), summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("note edit worker panicked".into()))
                    },
                    move |res| match res {
                        Ok((data, summary)) => Message::NoteEditApplied {
                            prev_source: prev_source.clone(),
                            summary,
                            result: Ok(data),
                        },
                        Err(err) => Message::NoteEditApplied {
                            prev_source: prev_source.clone(),
                            summary: String::new(),
                            result: Err(err),
                        },
                    },
                )
            }

            Message::NoteEditApplied {
                prev_source,
                summary,
                result,
            } => {
                self.editing = false;
                match result {
                    Ok(data) => {
                        self.undo_stack.push(prev_source);
                        if self.undo_stack.len() > 100 {
                            self.undo_stack.remove(0);
                        }
                        self.redo_stack.clear();
                        self.edit_error = None;
                        let ms = data.compile_ms;
                        return self.apply_in_memory_project(*data, format!("{summary} ({ms} ms)"));
                    }
                    Err(err) => {
                        self.edit_error = Some(err);
                    }
                }
                Task::none()
            }

            Message::Undo => {
                if self.editing {
                    return Task::none();
                }
                let Some(target_src) = self.undo_stack.pop() else {
                    return Task::none();
                };
                self.spawn_history_restore(true, target_src)
            }

            Message::Redo => {
                if self.editing {
                    return Task::none();
                }
                let Some(target_src) = self.redo_stack.pop() else {
                    return Task::none();
                };
                self.spawn_history_restore(false, target_src)
            }

            Message::HistoryRestored {
                is_undo,
                prev_source,
                result,
            } => {
                self.editing = false;
                match result {
                    Ok(data) => {
                        if is_undo {
                            self.redo_stack.push(prev_source);
                        } else {
                            self.undo_stack.push(prev_source);
                        }
                        self.edit_error = None;
                        let label = if is_undo { "Undo" } else { "Redo" };
                        let ms = data.compile_ms;
                        return self.apply_in_memory_project(*data, format!("{label} ({ms} ms)"));
                    }
                    Err(err) => {
                        self.edit_error = Some(format!("History restore failed: {err}"));
                    }
                }
                Task::none()
            }

            Message::SaveToDisk => {
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(src) = proj.bundle.sources.get(&proj.bundle.entry) else {
                    return Task::none();
                };
                let title = proj.title.clone();
                let saved = src.clone();
                match std::fs::write(&proj.entry_path, &saved) {
                    Ok(()) => {
                        self.saved_source = Some(saved);
                        self.last_self_save = Some(Instant::now());
                        self.pending_watch_at = None;
                        self.edit_error = None;
                        self.last_reload_note = Some(format!("Saved {title}"));
                        self.refresh_code_cache();
                    }
                    Err(err) => {
                        self.edit_error = Some(format!("Save failed: {err}"));
                    }
                }
                Task::none()
            }

            Message::KeyboardNoteNudge {
                pitch_semis,
                at_q16,
            } => {
                if self.selected_notes.len() > 1 {
                    if pitch_semis != 0 {
                        return self.update(Message::ApplyBatchNoteEdit(
                            BatchNoteEdit::ShiftPitchSemitones(pitch_semis),
                        ));
                    } else if at_q16 != 0 {
                        return self.update(Message::ApplyBatchNoteEdit(
                            BatchNoteEdit::ShiftAtQuarter16ths(at_q16),
                        ));
                    }
                    return Task::none();
                }
                let Some(info) = self.selected_note_info.clone() else {
                    return Task::none();
                };
                if pitch_semis != 0 {
                    if let Some(midi) = info.pitch_midi {
                        return self.update(Message::ApplyNoteEdit {
                            source_path: info.path,
                            edit: NoteEdit::SetPitchMidi(midi + pitch_semis),
                        });
                    }
                } else if at_q16 != 0 {
                    return self.update(Message::ApplyNoteEdit {
                        source_path: info.path,
                        edit: NoteEdit::ShiftAtQuarter16ths(at_q16),
                    });
                }
                Task::none()
            }

            Message::KeyboardDeleteNote => {
                if self.selected_notes.len() > 1 {
                    return self.update(Message::ApplyBatchNoteEdit(BatchNoteEdit::Delete));
                }
                let Some(info) = self.selected_note_info.clone() else {
                    return Task::none();
                };
                self.update(Message::ApplyNoteEdit {
                    source_path: info.path,
                    edit: NoteEdit::Delete,
                })
            }

            Message::KeyboardDuplicateNote => {
                let Some(info) = self.selected_note_info.clone() else {
                    return Task::none();
                };
                self.update(Message::ApplyNoteEdit {
                    source_path: info.path,
                    edit: NoteEdit::Duplicate,
                })
            }

            Message::ToggleMixer => {
                self.show_mixer = !self.show_mixer;
                if self.show_mixer && self.selected_node_id.is_none() {
                    if let Some(proj) = &self.project {
                        self.selected_node_id = proj
                            .mixer_nodes
                            .iter()
                            .find(|n| n.category == NodeCategory::Effect)
                            .or_else(|| proj.mixer_nodes.first())
                            .map(|n| n.id.clone());
                    }
                }
                self.refresh_code_cache();
                Task::none()
            }

            Message::SelectMixerNode(id) => {
                self.selected_node_id = id;
                self.edit_error = None;
                self.refresh_code_cache();
                Task::none()
            }

            Message::ToggleCodePanel => {
                self.show_code = !self.show_code;
                if self.show_code {
                    self.refresh_code_cache();
                }
                Task::none()
            }

            Message::SelectCodeTab(tab) => {
                self.code_tab = tab;
                self.show_code = true;
                self.refresh_code_cache();
                Task::none()
            }

            Message::ToggleCodeFocused => {
                self.code_focused_only = !self.code_focused_only;
                Task::none()
            }

            Message::DiscardUnsavedEdits => {
                if self.editing {
                    return Task::none();
                }
                let Some(saved) = self.saved_source.clone() else {
                    return Task::none();
                };
                if !self.is_dirty() {
                    return Task::none();
                }
                // Treat discard as a restorable state (pushes discarded state onto redo_stack).
                self.spawn_history_restore(true, saved)
            }

            Message::RevertExternalEdit => {
                if self.editing {
                    return Task::none();
                }
                let Some(ext) = self.external_change.take() else {
                    return Task::none();
                };
                self.code_tab = CodeTab::UnsavedDiff;
                self.spawn_history_restore(true, ext.prev_source)
            }

            Message::DismissExternalChange => {
                self.external_change = None;
                if self.code_tab == CodeTab::ExternalDiff {
                    self.code_tab = CodeTab::LiveCode;
                }
                Task::none()
            }

            Message::NodeParamDraft {
                node_id,
                param_name,
                value,
            } => {
                self.param_drafts.insert((node_id, param_name), value);
                Task::none()
            }

            Message::CommitNodeParam {
                node_id,
                param_name,
            } => {
                if self.editing {
                    return Task::none();
                }
                let Some(value) = self
                    .param_drafts
                    .get(&(node_id.clone(), param_name.clone()))
                    .copied()
                else {
                    return Task::none();
                };
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(param) = proj
                    .mixer_nodes
                    .iter()
                    .find(|n| n.id == node_id)
                    .and_then(|n| n.params.iter().find(|p| p.name == param_name))
                else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let unit = param.unit;
                let bundle = proj.bundle.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let (updated_bundle, summary) = apply_node_param_edit(
                                &bundle,
                                &node_id,
                                &param_name,
                                unit,
                                value,
                                &limits,
                            )?;
                            let recompiled =
                                load_from_bundle(entry_path, project_root, updated_bundle)?;
                            Ok((Box::new(recompiled), summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("node param edit worker panicked".into()))
                    },
                    move |res| match res {
                        Ok((data, summary)) => Message::NoteEditApplied {
                            prev_source: prev_source.clone(),
                            summary,
                            result: Ok(data),
                        },
                        Err(err) => Message::NoteEditApplied {
                            prev_source: prev_source.clone(),
                            summary: String::new(),
                            result: Err(err),
                        },
                    },
                )
            }

            Message::ResetNodeParam {
                node_id,
                param_name,
            } => {
                if self.editing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                self.param_drafts
                    .remove(&(node_id.clone(), param_name.clone()));
                let bundle = proj.bundle.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let (updated_bundle, summary) =
                                apply_node_param_reset(&bundle, &node_id, &param_name, &limits)?;
                            let recompiled =
                                load_from_bundle(entry_path, project_root, updated_bundle)?;
                            Ok((Box::new(recompiled), summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("node param reset worker panicked".into()))
                    },
                    move |res| match res {
                        Ok((data, summary)) => Message::NoteEditApplied {
                            prev_source: prev_source.clone(),
                            summary,
                            result: Ok(data),
                        },
                        Err(err) => Message::NoteEditApplied {
                            prev_source: prev_source.clone(),
                            summary: String::new(),
                            result: Err(err),
                        },
                    },
                )
            }

            Message::GainChanged(g) => {
                self.audio.set_gain(g);
                Task::none()
            }

            Message::ZoomDelta { delta, anchor } => {
                let old_zoom = self.zoom;
                let factor = if delta > 0.0 {
                    1.18_f32
                } else {
                    1.0 / 1.18_f32
                };
                let new_zoom = (old_zoom * factor).clamp(1.0, 40.0);
                let old_vis = 1.0 / old_zoom;
                let new_vis = 1.0 / new_zoom;
                let cursor_frac = self.scroll + anchor * old_vis;
                self.zoom = new_zoom;
                self.scroll = (cursor_frac - anchor * new_vis).clamp(0.0, (1.0 - new_vis).max(0.0));
                Task::none()
            }

            Message::ZoomFit => {
                self.zoom = 1.0;
                self.scroll = 0.0;
                Task::none()
            }

            Message::PanDelta(delta) => {
                let vis = 1.0 / self.zoom.max(1.0);
                self.scroll = (self.scroll + delta * vis).clamp(0.0, (1.0 - vis).max(0.0));
                Task::none()
            }

            Message::ScrollChanged(val) => {
                let vis = 1.0 / self.zoom.max(1.0);
                self.scroll = val.clamp(0.0, (1.0 - vis).max(0.0));
                Task::none()
            }

            Message::ExportWav { loop_only } => {
                if self.exporting {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let stem = proj
                    .entry_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "export".into());
                let default_name = if loop_only {
                    format!("{stem}-loop.wav")
                } else {
                    format!("{stem}.wav")
                };
                let dir = proj.project_root.clone();
                Task::perform(
                    async move {
                        rfd::FileDialog::new()
                            .set_directory(dir)
                            .set_file_name(default_name)
                            .add_filter("WAV Audio", &["wav"])
                            .save_file()
                    },
                    move |path| Message::ExportPathPicked { loop_only, path },
                )
            }

            Message::ExportPathPicked {
                loop_only,
                path: Some(path),
            } => {
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let plan = self
                    .active_plan
                    .clone()
                    .unwrap_or_else(|| proj.plan.clone());
                let limits = proj.limits;
                let loop_span = if loop_only { self.loop_range } else { None };
                let rate = proj.sample_rate_hz;
                self.exporting = true;
                self.last_reload_note = Some(format!(
                    "Exporting {}...",
                    path.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                ));
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let stats = match loop_span {
                                Some(span) => {
                                    let end = span.end.min(plan.output().total_frames);
                                    let range = FrameRange::new(span.start, end);
                                    render_wav_to_path_artifact_range_with_limits(
                                        &plan,
                                        &path,
                                        WavFormat::Float32,
                                        true,
                                        range,
                                        &limits,
                                    )
                                }
                                None => render_wav_to_path_artifact_with_limits(
                                    &plan,
                                    &path,
                                    WavFormat::Float32,
                                    true,
                                    &limits,
                                ),
                            }
                            .map_err(|e| format!("Export failed: {e}"))?;
                            let secs = started.elapsed().as_secs_f64();
                            let name = path
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string());
                            Ok(format!(
                                "Exported {name} ({}) in {secs:.1}s",
                                format_time(stats.frames, rate)
                            ))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("export worker panicked".into()))
                    },
                    Message::ExportFinished,
                )
            }
            Message::ExportPathPicked { path: None, .. } => Task::none(),

            Message::ExportFinished(res) => {
                self.exporting = false;
                match res {
                    Ok(msg) => self.last_reload_note = Some(msg),
                    Err(err) => self.playback_error = Some(err),
                }
                Task::none()
            }

            Message::RunAnalysis => {
                if self.analyzing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                self.analyzing = true;
                self.analysis_error = None;
                let plan = self
                    .active_plan
                    .clone()
                    .unwrap_or_else(|| proj.plan.clone());
                let meter = proj.meter.clone();
                let limits = proj.limits;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || run_analysis(plan, meter, limits));
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("analysis worker panicked".into()))
                    },
                    Message::AnalysisFinished,
                )
            }

            Message::AnalysisFinished(res) => {
                self.analyzing = false;
                match res {
                    Ok(summary) => {
                        self.analysis = Some(summary);
                        self.analysis_error = None;
                    }
                    Err(err) => {
                        self.analysis_error = Some(err);
                    }
                }
                Task::none()
            }

            Message::ToggleSidebar => {
                self.show_sidebar = !self.show_sidebar;
                Task::none()
            }

            Message::Timeline(action) => match action {
                TimelineAction::Seek(frame) => self.update(Message::Seek(frame)),
                TimelineAction::SetLoop(range) => self.update(Message::SetLoop(range)),
                TimelineAction::LoopRegion(span) => self.update(Message::LoopRegion(span)),
                TimelineAction::SelectNote(addr) => self.update(Message::SelectNote(addr)),
                TimelineAction::ToggleSelectNote(addr) => {
                    self.update(Message::ToggleSelectNote(addr))
                }
                TimelineAction::SelectNotesBox(addrs) => {
                    self.update(Message::SelectNotesBox(addrs))
                }
                TimelineAction::AddNoteAt { frame, midi_pitch } => {
                    self.update(Message::AddNoteAt { frame, midi_pitch })
                }
                TimelineAction::DragNotePitch {
                    source_path,
                    delta_semitones,
                } => {
                    if self.selected_notes.len() > 1 {
                        return self.update(Message::ApplyBatchNoteEdit(
                            BatchNoteEdit::ShiftPitchSemitones(delta_semitones),
                        ));
                    }
                    let cur_midi = self
                        .project
                        .as_ref()
                        .and_then(|proj| inspect_note_in_bundle(&proj.bundle, &source_path))
                        .and_then(|info| info.pitch_midi);
                    if let Some(midi) = cur_midi {
                        self.update(Message::ApplyNoteEdit {
                            source_path,
                            edit: NoteEdit::SetPitchMidi(midi + delta_semitones),
                        })
                    } else {
                        Task::none()
                    }
                }
                TimelineAction::DragNoteTiming {
                    source_path,
                    delta_quarter_16ths,
                } => {
                    if self.selected_notes.len() > 1 {
                        return self.update(Message::ApplyBatchNoteEdit(
                            BatchNoteEdit::ShiftAtQuarter16ths(delta_quarter_16ths),
                        ));
                    }
                    self.update(Message::ApplyNoteEdit {
                        source_path,
                        edit: NoteEdit::ShiftAtQuarter16ths(delta_quarter_16ths),
                    })
                }
                TimelineAction::DragNoteDuration {
                    source_path,
                    delta_quarter_16ths,
                } => {
                    if self.selected_notes.len() > 1 {
                        return self.update(Message::ApplyBatchNoteEdit(
                            BatchNoteEdit::ShiftDurQuarter16ths(delta_quarter_16ths),
                        ));
                    }
                    self.update(Message::ApplyNoteEdit {
                        source_path,
                        edit: NoteEdit::ShiftDurQuarter16ths(delta_quarter_16ths),
                    })
                }
                TimelineAction::Zoom { delta, anchor } => {
                    self.update(Message::ZoomDelta { delta, anchor })
                }
                TimelineAction::Pan(delta) => self.update(Message::PanDelta(delta)),
            },

            Message::Tick => {
                // 1. Poll session & playback status.
                if let Some(session) = &self.session {
                    let s = session.status();
                    self.checkpoint_frame = s.last_checkpoint;
                    self.checkpoint_count = s.checkpoints;
                    self.worker_busy = s.busy;
                }
                if let Some(playback) = &self.playback {
                    let st = playback.status();
                    self.playhead_frame = st.position;
                    self.underruns = st.underruns;
                    self.stale = st.stale;
                    self.approximate = st.approximate;
                    self.playback_error = st.last_error;
                    if st.ended && self.audio.is_playing() {
                        self.audio.set_playing(false);
                    }
                }
                self.peaks = self.audio.peaks();

                // 2. Auto-follow playhead when zoomed in and playing.
                if self.auto_follow && self.audio.is_playing() && self.zoom > 1.01 {
                    if let Some(proj) = &self.project {
                        let total = proj.total_frames.max(1) as f32;
                        let pos_frac = self.playhead_frame as f32 / total;
                        let vis = 1.0 / self.zoom;
                        let max_scroll = (1.0 - vis).max(0.0);
                        if pos_frac < self.scroll || pos_frac > self.scroll + vis * 0.88 {
                            self.scroll = (pos_frac - vis * 0.12).clamp(0.0, max_scroll);
                        }
                    }
                }

                // 3. Poll project file watcher (debounced, suppressing our own saves).
                if self.auto_watch {
                    let suppress = self
                        .last_self_save
                        .is_some_and(|t| t.elapsed() < SELF_SAVE_SUPPRESS);
                    if let Some(watcher) = &self.watcher {
                        if let Some(at) = watcher.poll_latest() {
                            if !suppress {
                                self.pending_watch_at = Some(at);
                            }
                        }
                    }
                    if suppress {
                        self.pending_watch_at = None;
                    } else if let Some(at) = self.pending_watch_at {
                        if at.elapsed() >= WATCH_DEBOUNCE && !self.compiling && !self.editing {
                            self.pending_watch_at = None;
                            if let Some(proj) = &self.project {
                                self.compiling = true;
                                return spawn_compile(proj.entry_path.clone(), true);
                            }
                        }
                    }
                }
                Task::none()
            }
        }
    }

    fn refresh_selected_note_info(&mut self) {
        let Some(proj) = &self.project else {
            self.selected_notes.clear();
            self.selected_note_address = None;
            self.selected_note_info = None;
            self.velocity_draft = None;
            return;
        };
        let valid: HashSet<&str> = proj.notes.iter().map(|n| n.address.as_str()).collect();
        self.selected_notes.retain(|a| valid.contains(a.as_str()));
        if self
            .selected_note_address
            .as_deref()
            .is_some_and(|a| !valid.contains(a))
        {
            self.selected_note_address = self.selected_notes.iter().next().cloned();
        }
        let Some(addr) = self.selected_note_address.as_deref() else {
            self.selected_note_info = None;
            self.velocity_draft = None;
            return;
        };
        if let Some(note) = proj.notes.iter().find(|n| n.address == addr) {
            let info = inspect_note_in_bundle(&proj.bundle, &note.source_path);
            self.velocity_draft = info
                .as_ref()
                .and_then(|i| i.velocity)
                .or(Some(note.velocity));
            self.selected_note_info = info;
        } else {
            self.selected_note_address = None;
            self.selected_note_info = None;
            self.velocity_draft = None;
        }
        self.refresh_code_cache();
    }

    fn refresh_code_cache(&mut self) {
        let Some(proj) = &self.project else {
            self.cached_highlight = None;
            self.cached_unsaved_diff = DiffSummary::default();
            self.cached_changed_lines.clear();
            return;
        };
        let Some(current) = proj.bundle.sources.get(&proj.bundle.entry) else {
            self.cached_highlight = None;
            self.cached_unsaved_diff = DiffSummary::default();
            self.cached_changed_lines.clear();
            return;
        };

        // Highlight priority: selected note (if in Roll mode or note selected) -> selected mixer/FX node.
        let target_path: Option<Vec<String>> = if !self.show_mixer {
            self.selected_note_info
                .as_ref()
                .map(|i| i.path.clone())
                .or_else(|| self.selected_node_id.clone().map(|id| vec![id]))
        } else {
            self.selected_node_id
                .clone()
                .map(|id| vec![id])
                .or_else(|| self.selected_note_info.as_ref().map(|i| i.path.clone()))
        };
        self.cached_highlight = target_path
            .as_deref()
            .and_then(|path| locate_object_lines(current, path));

        if let Some(saved) = self.saved_source.as_deref() {
            self.cached_unsaved_diff = compute_line_diff(saved, current, 2);
            self.cached_changed_lines = changed_new_lines(saved, current);
        } else {
            self.cached_unsaved_diff = DiffSummary::default();
            self.cached_changed_lines.clear();
        }
    }

    fn spawn_history_restore(&mut self, is_undo: bool, target_source: String) -> Task<Message> {
        let Some(proj) = &self.project else {
            return Task::none();
        };
        let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
            return Task::none();
        };
        let mut bundle = proj.bundle.clone();
        bundle.sources.insert(bundle.entry.clone(), target_source);
        let entry_path = proj.entry_path.clone();
        let project_root = proj.project_root.clone();
        self.editing = true;
        self.edit_error = None;
        Task::perform(
            async move {
                let handle = std::thread::spawn(move || {
                    load_from_bundle(entry_path, project_root, bundle).map(Box::new)
                });
                handle
                    .join()
                    .unwrap_or_else(|_| Err("history restore worker panicked".into()))
            },
            move |result| Message::HistoryRestored {
                is_undo,
                prev_source: prev_source.clone(),
                result,
            },
        )
    }

    fn apply_in_memory_project(&mut self, data: ProjectData, status_msg: String) -> Task<Message> {
        self.param_drafts.clear();
        let track_count = data.tracks.len();
        if self.visible_tracks.len() != track_count {
            self.visible_tracks = vec![true; track_count];
        }
        let has_mix_filter = !self.solo_tracks.is_empty() || !self.muted_tracks.is_empty();
        self.last_reload_note = Some(status_msg);
        if !has_mix_filter {
            if let Some(session) = &self.session {
                session.update(&data.plan);
            }
            self.active_plan = Some(data.plan.clone());
            self.project = Some(data);
            self.refresh_selected_note_info();
            return Task::none();
        }
        self.project = Some(data);
        self.refresh_selected_note_info();
        self.spawn_mix_update()
    }

    fn effective_muted_nodes(&self, proj: &ProjectData) -> HashSet<String> {
        if !self.solo_tracks.is_empty() {
            proj.tracks
                .iter()
                .filter(|t| !self.solo_tracks.contains(&t.name))
                .map(|t| t.name.clone())
                .collect()
        } else {
            self.muted_tracks.clone()
        }
    }

    fn audible_track_flags(&self, proj: &ProjectData) -> Vec<bool> {
        let muted = self.effective_muted_nodes(proj);
        proj.tracks
            .iter()
            .map(|t| !muted.contains(&t.name))
            .collect()
    }

    fn spawn_mix_update(&mut self) -> Task<Message> {
        let Some(proj) = self.project.clone() else {
            return Task::none();
        };
        self.mix_revision += 1;
        let revision = self.mix_revision;
        let muted = self.effective_muted_nodes(&proj);
        if muted.is_empty() {
            let plan = proj.plan.clone();
            if let Some(session) = &self.session {
                session.update(&plan);
            }
            self.active_plan = Some(plan);
            return Task::none();
        }
        Task::perform(
            async move {
                let handle = std::thread::spawn(move || proj.plan_with_muted(&muted).map(Box::new));
                handle
                    .join()
                    .unwrap_or_else(|_| Err("solo/mute worker panicked".into()))
            },
            move |result| Message::MixPlanReady { revision, result },
        )
    }

    fn apply_compiled_project(&mut self, data: ProjectData, is_reload: bool) -> Task<Message> {
        let same_format = self.project.as_ref().is_some_and(|prev| {
            prev.sample_rate_hz == data.sample_rate_hz && prev.channels == data.channels
        });

        let disk_source = data.bundle.sources.get(&data.bundle.entry).cloned();
        let compile_ms = data.compile_ms;
        let track_count = data.tracks.len();
        if self.visible_tracks.len() != track_count {
            self.visible_tracks = vec![true; track_count];
        }

        if is_reload && same_format && self.session.is_some() {
            self.param_drafts.clear();
            let prev_saved = self.saved_source.clone();
            let source_changed = prev_saved != disk_source;
            if let (Some(before), Some(after)) = (prev_saved, disk_source.as_deref()) {
                if before != after {
                    let diff = compute_line_diff(&before, after, 2);
                    if !diff.is_empty() {
                        self.external_change = Some(ExternalChange {
                            prev_source: before,
                            diff,
                        });
                    }
                }
            }
            self.saved_source = disk_source;
            if source_changed {
                self.undo_stack.clear();
                self.redo_stack.clear();
            }
            self.edit_error = None;
            let has_mix_filter = !self.solo_tracks.is_empty() || !self.muted_tracks.is_empty();
            self.last_reload_note = Some(format!("Reloaded in {compile_ms} ms — live update sent"));
            if !has_mix_filter {
                if let Some(session) = &self.session {
                    session.update(&data.plan);
                }
                self.active_plan = Some(data.plan.clone());
                self.project = Some(data);
                self.refresh_selected_note_info();
                return Task::none();
            }
            self.project = Some(data);
            self.refresh_selected_note_info();
            return self.spawn_mix_update();
        }

        // Fresh project open (or audio format changed): rebuild session, watcher, playback.
        let was_playing = self.audio.is_playing();
        self.audio.set_playing(false);
        self.audio
            .set_playback(None, data.sample_rate_hz, data.channels);
        self.playback = None;
        self.muted_tracks.clear();
        self.solo_tracks.clear();
        self.saved_source = disk_source;
        self.external_change = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.selected_notes.clear();
        self.selected_note_address = None;
        self.selected_note_info = None;
        self.velocity_draft = None;
        self.selected_node_id = None;
        self.param_drafts.clear();
        self.edit_error = None;

        let spacing = (data.sample_rate_hz as u64 * CHECKPOINT_SECONDS).max(4_096);
        let policy = SessionPolicy::new(spacing)
            .with_render_threads(4)
            .with_prerender_threads(1);

        match BackgroundSession::new(&data.plan, &data.limits, policy) {
            Ok(session) => {
                self.session = Some(session);
                self.playback_error = None;
            }
            Err(err) => {
                self.session = None;
                self.playback_error = Some(format!("{err:?}"));
            }
        }

        self.watcher = ProjectWatcher::watch(&data.project_root).ok();
        self.playhead_frame = 0;
        self.checkpoint_frame = 0;
        self.checkpoint_count = 0;
        self.loop_range = None;
        self.last_loop_range = None;
        self.zoom = 1.0;
        self.scroll = 0.0;
        self.analysis = None;
        self.analysis_error = None;
        self.active_plan = Some(data.plan.clone());
        self.last_reload_note = Some(format!("Loaded in {compile_ms} ms"));
        self.project = Some(data);
        self.refresh_code_cache();

        self.ensure_playback_started();
        if was_playing {
            self.audio.set_playing(true);
        }
        Task::none()
    }

    fn ensure_playback_started(&mut self) {
        if self.playback.is_some() {
            return;
        }
        let (Some(session), Some(proj)) = (&self.session, &self.project) else {
            return;
        };
        let start = self.playhead_frame.min(proj.total_frames.saturating_sub(1));
        let config = PlaybackConfig {
            lead_frames: 65_536,
            chunk_frames: 2_048,
            pause_prerender: false,
            preview_preroll_frames: proj.sample_rate_hz as u64 / 2,
            approximate_start: true,
            render_threads: Some(4),
            loop_range: self.loop_range,
            edit_debounce: PLAYBACK_EDIT_DEBOUNCE,
        };
        match session.play(start, config) {
            Ok(pb) => {
                let pb = Arc::new(pb);
                self.audio
                    .set_playback(Some(Arc::clone(&pb)), proj.sample_rate_hz, proj.channels);
                self.playback = Some(pb);
                self.playback_error = None;
            }
            Err(err) => {
                self.playback_error = Some(format!("{err:?}"));
            }
        }
    }

    fn seek_to(&mut self, frame: u64) {
        let Some(proj) = &self.project else {
            return;
        };
        let clamped = frame.min(proj.total_frames.saturating_sub(1));
        self.playhead_frame = clamped;
        if let Some(pb) = &self.playback {
            pb.seek(clamped);
        } else {
            self.ensure_playback_started();
        }
    }

    fn apply_loop(&mut self, range: Option<RenderSpan>) {
        if let Some(span) = range {
            self.last_loop_range = Some(span);
        }
        self.loop_range = range;
        if let Some(pb) = &self.playback {
            if let Err(err) = pb.set_loop(range) {
                self.playback_error = Some(format!("{err:?}"));
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let top_bar = self.view_transport_bar();
        let body: Element<'_, Message> = match &self.project {
            Some(proj) => self.view_workspace(proj),
            None => self.view_empty_state(),
        };

        let root = column![top_bar, body]
            .width(Length::Fill)
            .height(Length::Fill);

        container(root)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.070, 0.078, 0.098))),
                ..Default::default()
            })
            .into()
    }

    fn view_transport_bar(&self) -> Element<'_, Message> {
        let open_btn = button(text("Open .maac").size(12.5))
            .padding([5, 10])
            .style(studio_button_style)
            .on_press(Message::OpenDialog);
        let playing = self.audio.is_playing();
        let play_btn = button(text(if playing { "Pause" } else { "Play" }).size(12.5))
            .padding([5, 12])
            .style(move |theme, status| {
                if playing {
                    accent_button_style(Color::from_rgb(0.16, 0.68, 0.46), theme, status)
                } else {
                    accent_button_style(Color::from_rgb(0.24, 0.48, 0.86), theme, status)
                }
            })
            .on_press_maybe(self.project.is_some().then_some(Message::TogglePlay));
        let stop_btn = button(text("Start").size(12.5))
            .padding([5, 10])
            .style(studio_button_style)
            .on_press_maybe(self.project.is_some().then_some(Message::StopToStart));

        let loop_on = self.loop_range.is_some();
        let loop_label = if loop_on { "Loop: ON" } else { "Loop: OFF" };
        let loop_btn = button(text(loop_label).size(12.5))
            .padding([5, 10])
            .style(move |theme, status| {
                if loop_on {
                    accent_button_style(Color::from_rgb(0.84, 0.60, 0.14), theme, status)
                } else {
                    studio_button_style(theme, status)
                }
            })
            .on_press_maybe(self.project.is_some().then_some(Message::ToggleLoop));

        let time_display = match &self.project {
            Some(proj) => {
                let bar_str = proj.format_bar_position(self.playhead_frame);
                if bar_str.is_empty() {
                    format!(
                        "{} / {}",
                        format_time(self.playhead_frame, proj.sample_rate_hz),
                        format_time(proj.total_frames, proj.sample_rate_hz)
                    )
                } else {
                    format!(
                        "{bar_str}  ·  {} / {}",
                        format_time(self.playhead_frame, proj.sample_rate_hz),
                        format_time(proj.total_frames, proj.sample_rate_hz)
                    )
                }
            }
            None => "--:--.- / --:--.-".into(),
        };

        let (badge_text, badge_color) =
            if self.compile_error.is_some() || self.playback_error.is_some() {
                ("ERROR", Color::from_rgb(0.92, 0.30, 0.32))
            } else if self.compiling || self.editing {
                ("COMPILING", Color::from_rgb(0.55, 0.62, 0.96))
            } else if self.approximate {
                ("APPROX", Color::from_rgb(0.95, 0.72, 0.22))
            } else if self.stale {
                ("CATCHING UP", Color::from_rgb(0.32, 0.76, 0.96))
            } else if self.project.is_some() {
                ("EXACT", Color::from_rgb(0.24, 0.80, 0.56))
            } else {
                ("IDLE", Color::from_rgb(0.48, 0.52, 0.60))
            };

        let status_badge = container(text(badge_text).size(11).color(Color::BLACK))
            .padding([3, 8])
            .style(move |_| container::Style {
                background: Some(Background::Color(badge_color)),
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        let ckpt_text = match &self.project {
            Some(proj) => {
                let pct = if proj.total_frames > 0 {
                    ((self.checkpoint_frame as f64 / proj.total_frames as f64) * 100.0).min(100.0)
                } else {
                    0.0
                };
                let suffix = if self.worker_busy { "..." } else { "" };
                format!(
                    "Ckpt: {} ({pct:.0}%{suffix})  UR: {}",
                    self.checkpoint_count, self.underruns
                )
            }
            None => String::new(),
        };

        let dirty = self.is_dirty();
        let can_undo = !self.undo_stack.is_empty() && !self.editing;
        let can_redo = !self.redo_stack.is_empty() && !self.editing;
        let undo_btn = button(text("Undo").size(12))
            .padding([5, 8])
            .style(studio_button_style)
            .on_press_maybe(can_undo.then_some(Message::Undo));
        let redo_btn = button(text("Redo").size(12))
            .padding([5, 8])
            .style(studio_button_style)
            .on_press_maybe(can_redo.then_some(Message::Redo));
        let save_btn = button(text(if dirty { "Save*" } else { "Save" }).size(12))
            .padding([5, 9])
            .style(move |theme, status| {
                if dirty {
                    accent_button_style(Color::from_rgb(0.84, 0.54, 0.16), theme, status)
                } else {
                    studio_button_style(theme, status)
                }
            })
            .on_press_maybe(dirty.then_some(Message::SaveToDisk));

        let meter_text = format!(
            "L {:>3.0}% R {:>3.0}%",
            (self.peaks.0 * 100.0).min(100.0),
            (self.peaks.1 * 100.0).min(100.0)
        );

        let vol_slider = slider(0.0..=1.2_f32, self.audio.gain(), Message::GainChanged)
            .step(0.01_f32)
            .width(Length::Fixed(72.0));

        let watch_toggle = toggler(self.auto_watch)
            .label("Watch")
            .on_toggle(Message::ToggleWatch)
            .size(16.0);

        let export_btn = button(
            text(if self.exporting {
                "Exporting..."
            } else {
                "Export WAV"
            })
            .size(12),
        )
        .padding([5, 8])
        .style(studio_button_style)
        .on_press_maybe(
            (self.project.is_some() && !self.exporting)
                .then_some(Message::ExportWav { loop_only: false }),
        );

        let jit_tag = if cfg!(feature = "jit") { "JIT" } else { "INT" };
        let sidebar_btn = button(
            text(if self.show_sidebar {
                "Hide Panel"
            } else {
                "Panel"
            })
            .size(12),
        )
        .padding([5, 8])
        .style(studio_button_style)
        .on_press(Message::ToggleSidebar);

        let bar = row![
            open_btn,
            play_btn,
            stop_btn,
            loop_btn,
            Space::with_width(4),
            text(time_display).size(13),
            Space::with_width(4),
            status_badge,
            Space::with_width(4),
            text(ckpt_text)
                .size(11)
                .color(Color::from_rgb(0.66, 0.72, 0.80)),
            horizontal_space(),
            undo_btn,
            redo_btn,
            save_btn,
            Space::with_width(4),
            text(meter_text)
                .size(11)
                .color(Color::from_rgb(0.55, 0.85, 0.72)),
            vol_slider,
            watch_toggle,
            export_btn,
            text(jit_tag)
                .size(11)
                .color(Color::from_rgb(0.56, 0.66, 0.82)),
            sidebar_btn,
        ]
        .spacing(5)
        .align_y(Alignment::Center)
        .padding([8, 12]);

        container(bar)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.105, 0.118, 0.148))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_empty_state(&self) -> Element<'_, Message> {
        let mut col = Column::new()
            .spacing(12)
            .align_x(Alignment::Center)
            .push(text("MaaC Studio").size(28))
            .push(
                text("Open a .maac project to play, loop, visualize, and live-reload edits from an AI or editor.")
                    .size(14)
                    .color(Color::from_rgb(0.68, 0.74, 0.84)),
            )
            .push(
                button(text("Open .maac Project").size(15))
                    .padding([8, 16])
                    .style(|t, s| accent_button_style(Color::from_rgb(0.24, 0.48, 0.86), t, s))
                    .on_press(Message::OpenDialog),
            )
            .push(
                text(self.audio.device_summary())
                    .size(12)
                    .color(Color::from_rgb(0.54, 0.60, 0.70)),
            );

        if let Some(err) = &self.compile_error {
            col = col.push(
                container(text(err).size(12).color(Color::from_rgb(0.98, 0.56, 0.56)))
                    .padding(12)
                    .max_width(760),
            );
        }

        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn view_workspace<'a>(&'a self, proj: &'a ProjectData) -> Element<'a, Message> {
        let audible_tracks = self.audible_track_flags(proj);
        let any_mix_filter = !self.muted_tracks.is_empty() || !self.solo_tracks.is_empty();

        // Track chips with integrated M (Mute) / S (Solo) audio controls
        let mut track_row = Row::new().spacing(6).align_y(Alignment::Center);
        for (idx, tr) in proj.tracks.iter().enumerate() {
            let visible = self.visible_tracks.get(idx).copied().unwrap_or(true);
            let is_muted = self.muted_tracks.contains(&tr.name);
            let is_solo = self.solo_tracks.contains(&tr.name);
            let audible = audible_tracks.get(idx).copied().unwrap_or(true);
            let c = track_color(idx);
            let label = format!(
                "{} ({})",
                tr.name,
                if tr.note_count > 0 {
                    tr.note_count
                } else {
                    tr.hit_count
                }
            );
            let chip_color = if visible && audible {
                c
            } else {
                Color::from_rgb(0.42, 0.46, 0.54)
            };

            let name_btn = button(text(label).size(11).color(chip_color))
                .padding([3, 7])
                .style(move |theme, status| track_chip_style(c, visible && audible, theme, status))
                .on_press(Message::ToggleTrackVisible(idx));

            let m_btn = button(text("M").size(10))
                .padding([3, 5])
                .style(move |theme, status| {
                    if is_muted {
                        accent_button_style(Color::from_rgb(0.80, 0.26, 0.30), theme, status)
                    } else {
                        studio_button_style(theme, status)
                    }
                })
                .on_press(Message::ToggleTrackMute(idx));

            let s_btn = button(text("S").size(10))
                .padding([3, 5])
                .style(move |theme, status| {
                    if is_solo {
                        accent_button_style(Color::from_rgb(0.84, 0.62, 0.14), theme, status)
                    } else {
                        studio_button_style(theme, status)
                    }
                })
                .on_press(Message::ToggleTrackSolo(idx));

            track_row = track_row.push(
                row![name_btn, m_btn, s_btn]
                    .spacing(2)
                    .align_y(Alignment::Center),
            );
        }
        if any_mix_filter {
            track_row = track_row.push(
                button(text("All").size(10.5))
                    .padding([3, 7])
                    .style(studio_button_style)
                    .on_press(Message::ClearSoloMute),
            );
        }

        let snap_on = self.snap_to_bar;
        let follow_on = self.auto_follow;
        let mixer_on = self.show_mixer;
        let code_on = self.show_code;
        let has_ext = self.external_change.is_some();
        let diff_changes = self.cached_unsaved_diff.added + self.cached_unsaved_diff.removed;
        let code_label = if has_ext && !code_on {
            "Code · AI!".to_owned()
        } else if diff_changes > 0 {
            format!("Code (±{diff_changes})")
        } else {
            "Code (C)".to_owned()
        };
        let mut zoom_controls = row![
            button(text(if mixer_on { "Piano Roll" } else { "Mixer (M)" }).size(11.5))
                .padding([3, 8])
                .style(move |theme, status| {
                    if mixer_on {
                        accent_button_style(Color::from_rgb(0.42, 0.34, 0.78), theme, status)
                    } else {
                        studio_button_style(theme, status)
                    }
                })
                .on_press(Message::ToggleMixer),
            button(text(code_label).size(11.5))
                .padding([3, 8])
                .style(move |theme, status| {
                    if has_ext && !code_on {
                        accent_button_style(Color::from_rgb(0.14, 0.56, 0.58), theme, status)
                    } else if code_on {
                        accent_button_style(Color::from_rgb(0.24, 0.44, 0.74), theme, status)
                    } else {
                        studio_button_style(theme, status)
                    }
                })
                .on_press(Message::ToggleCodePanel),
            button(text(if snap_on { "Snap: Bar" } else { "Snap: Free" }).size(11.5))
                .padding([3, 7])
                .style(move |theme, status| {
                    if snap_on {
                        accent_button_style(Color::from_rgb(0.22, 0.50, 0.66), theme, status)
                    } else {
                        studio_button_style(theme, status)
                    }
                })
                .on_press(Message::ToggleSnap),
            button(text("Follow").size(11.5))
                .padding([3, 7])
                .style(move |theme, status| {
                    if follow_on {
                        accent_button_style(Color::from_rgb(0.20, 0.56, 0.48), theme, status)
                    } else {
                        studio_button_style(theme, status)
                    }
                })
                .on_press(Message::ToggleFollow),
            Space::with_width(4),
            button(text("-").size(12))
                .padding([3, 7])
                .style(studio_button_style)
                .on_press(Message::ZoomDelta {
                    delta: -1.0,
                    anchor: 0.5
                }),
            button(text("Fit").size(11.5))
                .padding([3, 7])
                .style(studio_button_style)
                .on_press(Message::ZoomFit),
            button(text("+").size(12))
                .padding([3, 7])
                .style(studio_button_style)
                .on_press(Message::ZoomDelta {
                    delta: 1.0,
                    anchor: 0.5
                }),
            text(format!("{:.1}x", self.zoom))
                .size(11)
                .color(Color::from_rgb(0.66, 0.72, 0.80)),
            Space::with_width(4),
            button(text("Recompile").size(11.5))
                .padding([3, 8])
                .style(studio_button_style)
                .on_press(Message::RecompileNow),
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        if self.loop_range.is_some() {
            zoom_controls = zoom_controls.push(
                button(text("Export Loop").size(11.5))
                    .padding([3, 8])
                    .style(studio_button_style)
                    .on_press_maybe(
                        (!self.exporting).then_some(Message::ExportWav { loop_only: true }),
                    ),
            );
        }

        let sub_bar = container(
            row![
                scrollable(track_row).width(Length::Fill).direction(
                    scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(4).scroller_width(4),
                    )
                ),
                zoom_controls,
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .padding([6, 12]),
        )
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.090, 0.100, 0.125))),
            ..Default::default()
        });

        let main_pane: Element<'a, Message> = if self.show_mixer {
            self.view_mixer_console(proj)
        } else {
            let selected_source_path = self.selected_note_address.as_deref().and_then(|addr| {
                proj.notes
                    .iter()
                    .find(|n| n.address == addr)
                    .map(|n| n.source_path.as_slice())
            });

            Element::from(
                canvas(TimelineView {
                    project: proj,
                    visible_tracks: &self.visible_tracks,
                    audible_tracks,
                    selected_notes: &self.selected_notes,
                    selected_source_path,
                    playhead_frame: self.playhead_frame,
                    checkpoint_frame: self.checkpoint_frame,
                    loop_range: self.loop_range,
                    snap_to_bar: self.snap_to_bar,
                    approximate: self.approximate,
                    stale: self.stale,
                    zoom: self.zoom,
                    scroll: self.scroll,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .map(Message::Timeline)
        };

        let mut left_col = column![sub_bar]
            .width(Length::FillPortion(3))
            .height(Length::Fill);

        if self.show_code {
            let top_wrap = container(main_pane)
                .width(Length::Fill)
                .height(Length::FillPortion(5));
            let code_wrap = container(self.view_code_panel(proj))
                .width(Length::Fill)
                .height(Length::FillPortion(3));
            left_col = left_col.push(top_wrap);
            if !self.show_mixer && self.zoom > 1.01 {
                let max_scroll = (1.0 - 1.0 / self.zoom).max(0.001);
                let pan_bar = container(
                    slider(0.0_f32..=max_scroll, self.scroll, Message::ScrollChanged)
                        .step(0.002_f32)
                        .width(Length::Fill),
                )
                .padding([2, 12]);
                left_col = left_col.push(pan_bar);
            }
            left_col = left_col.push(code_wrap);
        } else {
            left_col = left_col.push(main_pane);
            if !self.show_mixer && self.zoom > 1.01 {
                let max_scroll = (1.0 - 1.0 / self.zoom).max(0.001);
                let pan_bar = container(
                    slider(0.0_f32..=max_scroll, self.scroll, Message::ScrollChanged)
                        .step(0.002_f32)
                        .width(Length::Fill),
                )
                .padding([2, 12]);
                left_col = left_col.push(pan_bar);
            }
        }

        // Bottom status strip
        let loop_desc = match self.loop_range {
            Some(span) => format!(
                "Loop {}–{} ({}–{}) · Right-click to clear",
                proj.format_bar_position(span.start),
                proj.format_bar_position(span.end.saturating_sub(1)),
                format_time(span.start, proj.sample_rate_hz),
                format_time(span.end, proj.sample_rate_hz)
            ),
            None => {
                "Click/⌘-click/⌘-drag notes · Double-click empty roll to add note · Drag ruler / Shift+drag to loop"
                    .into()
            }
        };
        let reload_note = self.last_reload_note.as_deref().unwrap_or("");
        let footer = container(
            row![
                text(loop_desc)
                    .size(11)
                    .color(Color::from_rgb(0.64, 0.70, 0.80)),
                horizontal_space(),
                text(reload_note)
                    .size(11)
                    .color(Color::from_rgb(0.48, 0.80, 0.68)),
                Space::with_width(12),
                text(self.audio.device_summary())
                    .size(11)
                    .color(Color::from_rgb(0.55, 0.60, 0.70)),
            ]
            .align_y(Alignment::Center)
            .padding([5, 12]),
        )
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.095, 0.105, 0.132))),
            ..Default::default()
        });

        let main_split: Element<'a, Message> = if self.show_sidebar {
            row![left_col, self.view_sidebar(proj)]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            left_col.into()
        };

        column![main_split, footer]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_sidebar<'a>(&'a self, proj: &'a ProjectData) -> Element<'a, Message> {
        let mut col = Column::new().spacing(10).padding(12);

        if let Some(err) = &self.compile_error {
            col = col.push(
                container(
                    text(err)
                        .size(11.5)
                        .color(Color::from_rgb(0.98, 0.56, 0.56)),
                )
                .padding(8)
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color::from_rgba(0.65, 0.12, 0.14, 0.35))),
                    border: Border {
                        color: Color::from_rgb(0.85, 0.30, 0.32),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }),
            );
        }

        if let Some(err) = &self.playback_error {
            col = col.push(
                container(
                    text(format!("Playback: {err}"))
                        .size(11.5)
                        .color(Color::from_rgb(0.98, 0.68, 0.42)),
                )
                .padding(8)
                .width(Length::Fill),
            );
        }

        if let Some(err) = &self.edit_error {
            col = col.push(
                container(
                    text(format!("Edit: {err}"))
                        .size(11.5)
                        .color(Color::from_rgb(0.98, 0.56, 0.56)),
                )
                .padding(8)
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color::from_rgba(0.65, 0.12, 0.14, 0.30))),
                    border: Border {
                        color: Color::from_rgb(0.85, 0.30, 0.32),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }),
            );
        }

        // Direct note editor inspector (multi-note batch mode or single-note mode)
        if self.selected_notes.len() > 1 {
            let unique_sources: HashSet<&[String]> = proj
                .notes
                .iter()
                .filter(|n| self.selected_notes.contains(&n.address))
                .map(|n| n.source_path.as_slice())
                .filter(|sp| !sp.is_empty())
                .collect();
            let busy = self.editing;
            let header = row![
                text(format!("SELECTED NOTES ({})", self.selected_notes.len()))
                    .size(10.5)
                    .color(Color::from_rgb(0.45, 0.82, 0.98)),
                horizontal_space(),
                button(text("×").size(11))
                    .padding([1, 6])
                    .style(studio_button_style)
                    .on_press(Message::ClearSelection),
            ]
            .align_y(Alignment::Center);

            let bp_btn = |lbl: &'static str, delta: i32| {
                button(text(lbl).size(10.5))
                    .padding([2, 7])
                    .style(studio_button_style)
                    .on_press_maybe((!busy).then_some(Message::ApplyBatchNoteEdit(
                        BatchNoteEdit::ShiftPitchSemitones(delta),
                    )))
            };
            let bat_btn = |lbl: &'static str, delta: i64| {
                button(text(lbl).size(10.5))
                    .padding([2, 6])
                    .style(studio_button_style)
                    .on_press_maybe((!busy).then_some(Message::ApplyBatchNoteEdit(
                        BatchNoteEdit::ShiftAtQuarter16ths(delta),
                    )))
            };
            let bdur_btn = |lbl: &'static str, delta: i64| {
                button(text(lbl).size(10.5))
                    .padding([2, 6])
                    .style(studio_button_style)
                    .on_press_maybe((!busy).then_some(Message::ApplyBatchNoteEdit(
                        BatchNoteEdit::ShiftDurQuarter16ths(delta),
                    )))
            };

            let vel_val = self.velocity_draft.unwrap_or(0.7);
            let vel_slider = slider(0.05..=1.0_f32, vel_val, Message::VelocitySliderChanged)
                .step(0.01_f32)
                .on_release(Message::CommitVelocity)
                .width(Length::Fill);

            let del_btn = button(text("Delete All (⌫)").size(11))
                .padding([3, 9])
                .style(|theme, status| {
                    accent_button_style(Color::from_rgb(0.68, 0.22, 0.26), theme, status)
                })
                .on_press_maybe(
                    (!busy).then_some(Message::ApplyBatchNoteEdit(BatchNoteEdit::Delete)),
                );

            let batch_card = column![
                header,
                text(format!(
                    "{} unique authored note(s) · edits apply in one Undo step",
                    unique_sources.len()
                ))
                .size(10.5)
                .color(Color::from_rgb(0.66, 0.72, 0.82)),
                row![
                    text("Pitch")
                        .size(11)
                        .width(Length::Fixed(34.0))
                        .color(Color::from_rgb(0.62, 0.68, 0.78)),
                    bp_btn("-12", -12),
                    bp_btn("-1", -1),
                    bp_btn("+1", 1),
                    bp_btn("+12", 12),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
                row![
                    text("At")
                        .size(11)
                        .width(Length::Fixed(34.0))
                        .color(Color::from_rgb(0.62, 0.68, 0.78)),
                    bat_btn("-½", -2),
                    bat_btn("-¼", -1),
                    bat_btn("+¼", 1),
                    bat_btn("+½", 2),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
                row![
                    text("Dur")
                        .size(11)
                        .width(Length::Fixed(34.0))
                        .color(Color::from_rgb(0.62, 0.68, 0.78)),
                    bdur_btn("-¼", -1),
                    bdur_btn("+¼", 1),
                    bdur_btn("+½", 2),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
                row![
                    text("Vel")
                        .size(11)
                        .width(Length::Fixed(34.0))
                        .color(Color::from_rgb(0.62, 0.68, 0.78)),
                    vel_slider,
                    text(format!("{vel_val:.2}"))
                        .size(11)
                        .color(Color::from_rgb(0.76, 0.82, 0.90)),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                row![horizontal_space(), del_btn],
            ]
            .spacing(6);

            col = col.push(
                container(batch_card)
                    .padding(9)
                    .width(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(Background::Color(Color::from_rgb(0.115, 0.132, 0.170))),
                        border: Border {
                            color: Color::from_rgba(0.40, 0.84, 0.98, 0.45),
                            width: 1.2,
                            radius: 5.0.into(),
                        },
                        ..Default::default()
                    }),
            );
        } else if let Some(note) = self
            .selected_note_address
            .as_deref()
            .and_then(|addr| proj.notes.iter().find(|n| n.address == addr))
        {
            let track_name = proj
                .tracks
                .get(note.track_idx)
                .map(|t| t.name.as_str())
                .unwrap_or("track");
            let track_col = track_color(note.track_idx);
            let mut header = row![
                text("SELECTED NOTE")
                    .size(10.5)
                    .color(Color::from_rgb(0.58, 0.66, 0.78)),
                Space::with_width(6),
                text(track_name).size(11).color(track_col),
                horizontal_space(),
            ]
            .align_y(Alignment::Center);
            if note.shared_occurrences > 1 {
                header = header.push(
                    container(
                        text(format!("×{}", note.shared_occurrences))
                            .size(10)
                            .color(Color::BLACK),
                    )
                    .padding([1, 5])
                    .style(|_| container::Style {
                        background: Some(Background::Color(Color::from_rgb(0.92, 0.72, 0.24))),
                        border: Border {
                            radius: 3.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                );
                header = header.push(Space::with_width(4));
            }
            header = header.push(
                button(text("×").size(11))
                    .padding([1, 6])
                    .style(studio_button_style)
                    .on_press(Message::ClearSelection),
            );

            let path_label = if note.source_path.is_empty() {
                note.address.clone()
            } else {
                note.source_path.join(" / ")
            };
            let pos_label = format!(
                "{} · {}",
                proj.format_bar_position(note.on_frame),
                format_time(note.on_frame, proj.sample_rate_hz)
            );
            let mut card = Column::new().spacing(6).push(header).push(
                text(format!("{path_label}   ({pos_label})"))
                    .size(11)
                    .color(Color::from_rgb(0.76, 0.82, 0.90)),
            );

            if let Some(info) = &self.selected_note_info {
                let busy = self.editing;
                let sp = info.path.clone();

                if info.kind == "note" {
                    if let Some(midi) = info.pitch_midi {
                        let pitch_str = info
                            .pitch_symbol
                            .clone()
                            .unwrap_or_else(|| midi_to_pitch_symbol(midi));
                        let p_btn = |lbl: &'static str, delta: i32| {
                            let path = sp.clone();
                            button(text(lbl).size(10.5))
                                .padding([2, 6])
                                .style(studio_button_style)
                                .on_press_maybe((!busy).then_some(Message::ApplyNoteEdit {
                                    source_path: path,
                                    edit: NoteEdit::SetPitchMidi(midi + delta),
                                }))
                        };
                        card = card.push(
                            row![
                                text("Pitch")
                                    .size(11)
                                    .width(Length::Fixed(34.0))
                                    .color(Color::from_rgb(0.62, 0.68, 0.78)),
                                p_btn("-12", -12),
                                p_btn("-1", -1),
                                container(text(pitch_str).size(12).color(Color::WHITE))
                                    .width(Length::Fixed(36.0))
                                    .center_x(Length::Fixed(36.0)),
                                p_btn("+1", 1),
                                p_btn("+12", 12),
                            ]
                            .spacing(4)
                            .align_y(Alignment::Center),
                        );
                    }
                } else {
                    card = card.push(
                        text(format!(
                            "Kind: {} (timing/dur/velocity editable)",
                            info.kind
                        ))
                        .size(10.5)
                        .color(Color::from_rgb(0.64, 0.70, 0.80)),
                    );
                }

                let at_btn = |lbl: &'static str, delta: i64| {
                    let path = sp.clone();
                    button(text(lbl).size(10.5))
                        .padding([2, 5])
                        .style(studio_button_style)
                        .on_press_maybe((!busy).then_some(Message::ApplyNoteEdit {
                            source_path: path,
                            edit: NoteEdit::ShiftAtQuarter16ths(delta),
                        }))
                };
                card = card.push(
                    row![
                        text("At")
                            .size(11)
                            .width(Length::Fixed(34.0))
                            .color(Color::from_rgb(0.62, 0.68, 0.78)),
                        at_btn("-½", -2),
                        at_btn("-¼", -1),
                        container(text(format_q(info.at_q.0, info.at_q.1)).size(11.5))
                            .width(Length::Fixed(46.0))
                            .center_x(Length::Fixed(46.0)),
                        at_btn("+¼", 1),
                        at_btn("+½", 2),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                );

                let dur_btn = |lbl: &'static str, delta: i64| {
                    let path = sp.clone();
                    button(text(lbl).size(10.5))
                        .padding([2, 5])
                        .style(studio_button_style)
                        .on_press_maybe((!busy).then_some(Message::ApplyNoteEdit {
                            source_path: path,
                            edit: NoteEdit::ShiftDurQuarter16ths(delta),
                        }))
                };
                card = card.push(
                    row![
                        text("Dur")
                            .size(11)
                            .width(Length::Fixed(34.0))
                            .color(Color::from_rgb(0.62, 0.68, 0.78)),
                        dur_btn("-¼", -1),
                        container(text(format_q(info.dur_q.0, info.dur_q.1)).size(11.5))
                            .width(Length::Fixed(46.0))
                            .center_x(Length::Fixed(46.0)),
                        dur_btn("+¼", 1),
                        dur_btn("+½", 2),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                );

                let vel_val = self
                    .velocity_draft
                    .or(info.velocity)
                    .unwrap_or(note.velocity);
                let vel_slider = slider(0.05..=1.0_f32, vel_val, Message::VelocitySliderChanged)
                    .step(0.01_f32)
                    .on_release(Message::CommitVelocity)
                    .width(Length::Fill);
                card = card.push(
                    row![
                        text("Vel")
                            .size(11)
                            .width(Length::Fixed(34.0))
                            .color(Color::from_rgb(0.62, 0.68, 0.78)),
                        vel_slider,
                        text(format!("{vel_val:.2}"))
                            .size(11)
                            .color(Color::from_rgb(0.76, 0.82, 0.90)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                );

                let dup_btn = button(text("Duplicate (⌘D)").size(11))
                    .padding([3, 8])
                    .style(studio_button_style)
                    .on_press_maybe((!busy).then_some(Message::ApplyNoteEdit {
                        source_path: sp.clone(),
                        edit: NoteEdit::Duplicate,
                    }));
                let del_btn = button(text("Delete (⌫)").size(11))
                    .padding([3, 8])
                    .style(|theme, status| {
                        accent_button_style(Color::from_rgb(0.68, 0.22, 0.26), theme, status)
                    })
                    .on_press_maybe((!busy).then_some(Message::ApplyNoteEdit {
                        source_path: sp,
                        edit: NoteEdit::Delete,
                    }));
                card = card.push(row![dup_btn, horizontal_space(), del_btn].spacing(6));

                if note.shared_occurrences > 1 {
                    card = card.push(
                        row![
                            text(format!("Shared ×{} occurrences", note.shared_occurrences))
                                .size(10)
                                .color(Color::from_rgb(0.84, 0.70, 0.36)),
                            horizontal_space(),
                            button(text("Make Unique (Split)").size(10.5))
                                .padding([3, 8])
                                .style(|theme, status| {
                                    accent_button_style(
                                        Color::from_rgb(0.74, 0.50, 0.14),
                                        theme,
                                        status,
                                    )
                                })
                                .on_press_maybe(
                                    (!busy).then_some(Message::MaterializeSelectedNote)
                                ),
                        ]
                        .align_y(Alignment::Center),
                    );
                }
            } else {
                card = card.push(
                    text(format!(
                        "{} · vel {:.2} — defined outside entry file (read-only)",
                        midi_to_pitch_symbol(note.midi_pitch.round() as i32),
                        note.velocity
                    ))
                    .size(10.5)
                    .color(Color::from_rgb(0.60, 0.66, 0.76)),
                );
            }

            col = col.push(container(card).padding(9).width(Length::Fill).style(|_| {
                container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.115, 0.132, 0.170))),
                    border: Border {
                        color: Color::from_rgba(0.45, 0.72, 0.98, 0.35),
                        width: 1.0,
                        radius: 5.0.into(),
                    },
                    ..Default::default()
                }
            }));
        } else {
            col = col.push(
                text("Click or ⌘-drag notes to edit · Double-click empty piano roll to insert a new note.")
                    .size(10.5)
                    .color(Color::from_rgb(0.50, 0.56, 0.66)),
            );
        }

        // Declared regions quick jump & loop
        // Quick FX / Bus selector chips & Node/FX Parameter Inspector
        if let Some(node) = self
            .selected_node_id
            .as_deref()
            .and_then(|id| proj.mixer_nodes.iter().find(|n| n.id == id))
        {
            col = col.push(self.view_node_inspector_card(node));
        } else if proj
            .mixer_nodes
            .iter()
            .any(|n| n.category != NodeCategory::Instrument)
        {
            let mut fx_chips = Row::new().spacing(4).align_y(Alignment::Center);
            for node in proj
                .mixer_nodes
                .iter()
                .filter(|n| n.category != NodeCategory::Instrument)
            {
                let id = node.id.clone();
                fx_chips = fx_chips.push(
                    button(text(&node.id).size(10.5))
                        .padding([2, 7])
                        .style(studio_button_style)
                        .on_press(Message::SelectMixerNode(Some(id))),
                );
            }
            col = col.push(
                column![
                    row![
                        text("BUS / FX NODES")
                            .size(10.5)
                            .color(Color::from_rgb(0.58, 0.66, 0.78)),
                        horizontal_space(),
                        button(text("Open Mixer (M)").size(10))
                            .padding([2, 6])
                            .style(studio_button_style)
                            .on_press(Message::ToggleMixer),
                    ]
                    .align_y(Alignment::Center),
                    scrollable(fx_chips).direction(scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(3).scroller_width(3),
                    )),
                ]
                .spacing(5),
            );
        }

        col = col.push(Space::with_height(4));
        col = col.push(
            text("REGIONS")
                .size(11)
                .color(Color::from_rgb(0.58, 0.66, 0.78)),
        );
        if proj.regions.is_empty() {
            col = col.push(
                text("No declared regions in this score.")
                    .size(11.5)
                    .color(Color::from_rgb(0.50, 0.55, 0.64)),
            );
        } else {
            for r in &proj.regions {
                let caption = r.label.as_deref().unwrap_or(&r.id);
                let span = RenderSpan {
                    start: r.start_frame,
                    end: r.end_frame,
                };
                let time_str = format!(
                    "{}–{}",
                    format_time(r.start_frame, proj.sample_rate_hz),
                    format_time(r.end_frame, proj.sample_rate_hz)
                );
                col = col.push(
                    row![
                        button(text(caption).size(12))
                            .padding([3, 8])
                            .style(studio_button_style)
                            .on_press(Message::Seek(r.start_frame)),
                        horizontal_space(),
                        text(time_str)
                            .size(11)
                            .color(Color::from_rgb(0.60, 0.66, 0.75)),
                        button(text("Loop").size(11))
                            .padding([2, 7])
                            .style(studio_button_style)
                            .on_press(Message::LoopRegion(span)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                );
            }
        }

        // Analysis section
        col = col.push(Space::with_height(6));
        col = col.push(
            row![
                text("MAAC ANALYZE")
                    .size(11)
                    .color(Color::from_rgb(0.58, 0.66, 0.78)),
                horizontal_space(),
                button(
                    text(if self.analyzing {
                        "Running..."
                    } else {
                        "Run (A)"
                    })
                    .size(11)
                )
                .padding([3, 9])
                .style(studio_button_style)
                .on_press_maybe((!self.analyzing).then_some(Message::RunAnalysis)),
            ]
            .align_y(Alignment::Center),
        );

        if let Some(err) = &self.analysis_error {
            col = col.push(
                text(err)
                    .size(11.5)
                    .color(Color::from_rgb(0.95, 0.48, 0.48)),
            );
        } else if let Some(summary) = &self.analysis {
            let lufs = summary
                .integrated_lufs
                .map_or_else(|| "—".into(), |v| format!("{v:.1} LUFS"));
            let peak = summary
                .true_peak_dbtp
                .map_or_else(|| "—".into(), |v| format!("{v:.2} dBTP"));
            let crest = summary
                .crest_db
                .map_or_else(|| "—".into(), |v| format!("{v:.1} dB"));
            col = col.push(
                container(
                    column![
                        text(format!("Loudness: {lufs}")).size(12.5),
                        text(format!("True Peak: {peak}   Crest: {crest}")).size(11.5),
                        text(format!(
                            "Clips: {}   ({} ms)",
                            summary.clipped_samples, summary.elapsed_ms
                        ))
                        .size(11)
                        .color(Color::from_rgb(0.62, 0.68, 0.78)),
                    ]
                    .spacing(4),
                )
                .padding(8)
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.115, 0.130, 0.165))),
                    border: Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );

            if !summary.sections.is_empty() {
                col = col.push(
                    text("SECTIONS")
                        .size(10.5)
                        .color(Color::from_rgb(0.56, 0.64, 0.76)),
                );
                for sec in &summary.sections {
                    let caption = sec.label.as_deref().unwrap_or(&sec.id);
                    let lufs_s = sec
                        .lufs
                        .map_or_else(|| "—".into(), |x| format!("{x:.1} LUFS"));
                    let peak_s = sec
                        .true_peak
                        .map_or_else(String::new, |x| format!(" / {x:.1}"));
                    let span = RenderSpan {
                        start: sec.start_frame,
                        end: sec.end_frame.max(sec.start_frame + 1),
                    };
                    col = col.push(
                        row![
                            button(text(caption).size(11))
                                .padding([2, 7])
                                .style(studio_button_style)
                                .on_press(Message::Seek(sec.start_frame)),
                            horizontal_space(),
                            text(format!("{lufs_s}{peak_s}"))
                                .size(10.5)
                                .color(Color::from_rgb(0.76, 0.82, 0.90)),
                            button(text("Loop").size(10.5))
                                .padding([2, 6])
                                .style(studio_button_style)
                                .on_press(Message::LoopRegion(span)),
                        ]
                        .spacing(4)
                        .align_y(Alignment::Center),
                    );
                }
            }

            if !summary.sources.is_empty() {
                col = col.push(
                    text("SOURCES")
                        .size(10.5)
                        .color(Color::from_rgb(0.56, 0.64, 0.76)),
                );
                for src in &summary.sources {
                    let v = src
                        .lufs
                        .map_or_else(|| "—".into(), |x| format!("{x:.1} LUFS"));
                    col = col.push(
                        row![
                            text(format!("{} [{}]", src.port, src.role)).size(11.5),
                            horizontal_space(),
                            text(v).size(11.5).color(Color::from_rgb(0.76, 0.82, 0.90)),
                        ]
                        .align_y(Alignment::Center),
                    );
                }
            }

            col = col.push(
                text(format!("FINDINGS ({})", summary.findings.len()))
                    .size(10.5)
                    .color(Color::from_rgb(0.56, 0.64, 0.76)),
            );
            if summary.findings.is_empty() {
                col = col.push(
                    text("No findings — mix passes checks.")
                        .size(11.5)
                        .color(Color::from_rgb(0.45, 0.84, 0.62)),
                );
            } else {
                for f in summary.findings.iter().take(25) {
                    let sev_color = match f.severity.as_str() {
                        "error" => Color::from_rgb(0.96, 0.42, 0.44),
                        "warning" => Color::from_rgb(0.96, 0.76, 0.34),
                        _ => Color::from_rgb(0.52, 0.76, 0.96),
                    };
                    let header = match &f.section {
                        Some(sec) => format!("[{}] {} @ {}", f.kind, f.source, sec),
                        None => format!("[{}] {}", f.kind, f.source),
                    };
                    col = col.push(
                        column![
                            text(header).size(11).color(sev_color),
                            text(&f.message)
                                .size(11)
                                .color(Color::from_rgb(0.76, 0.80, 0.86)),
                        ]
                        .spacing(2),
                    );
                }
            }
        } else {
            col = col.push(
                text("Press Run (or key A) to compute loudness, peaks, stems, and findings.")
                    .size(11.5)
                    .color(Color::from_rgb(0.54, 0.60, 0.70)),
            );
        }

        container(scrollable(col).height(Length::Fill))
            .width(Length::FillPortion(1))
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.088, 0.098, 0.122))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_mixer_console<'a>(&'a self, proj: &'a ProjectData) -> Element<'a, Message> {
        let instruments: Vec<&MixerNodeInfo> = proj
            .mixer_nodes
            .iter()
            .filter(|n| n.category == NodeCategory::Instrument)
            .collect();
        let bus_fx: Vec<&MixerNodeInfo> = proj
            .mixer_nodes
            .iter()
            .filter(|n| n.category != NodeCategory::Instrument)
            .collect();

        let header = row![
            text("LIVE MIXER & FX CONSOLE")
                .size(12)
                .color(Color::from_rgb(0.76, 0.84, 0.96)),
            Space::with_width(10),
            text(format!(
                "{} Tracks · {} Bus / FX",
                instruments.len(),
                bus_fx.len()
            ))
            .size(11)
            .color(Color::from_rgb(0.56, 0.64, 0.76)),
            horizontal_space(),
            text("Drag sliders & release to apply live · Click strip header to inspect all params")
                .size(11)
                .color(Color::from_rgb(0.52, 0.58, 0.68)),
        ]
        .align_y(Alignment::Center)
        .padding([10, 14]);

        if proj.mixer_nodes.is_empty() {
            return container(
                text("No parameterized instrument or FX nodes found in this score.")
                    .size(13)
                    .color(Color::from_rgb(0.60, 0.66, 0.76)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        }

        let mut body = Column::new().spacing(10).padding([2, 14]);

        if !instruments.is_empty() {
            let mut inst_row = Row::new()
                .spacing(10)
                .padding([2, 0])
                .align_y(Alignment::Start);
            for node in &instruments {
                inst_row = inst_row.push(self.view_mixer_strip(node));
            }
            body = body.push(
                text(format!("INSTRUMENTS ({})", instruments.len()))
                    .size(10.5)
                    .color(Color::from_rgb(0.56, 0.66, 0.78)),
            );
            body = body.push(scrollable(inst_row).width(Length::Fill).direction(
                scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(6).scroller_width(6),
                ),
            ));
        }

        if !bus_fx.is_empty() {
            let mut fx_row = Row::new()
                .spacing(10)
                .padding([2, 0])
                .align_y(Alignment::Start);
            for node in &bus_fx {
                fx_row = fx_row.push(self.view_mixer_strip(node));
            }
            body = body.push(
                text(format!("BUS / FX / MASTER ({})", bus_fx.len()))
                    .size(10.5)
                    .color(Color::from_rgb(0.90, 0.70, 0.34)),
            );
            body = body.push(scrollable(fx_row).width(Length::Fill).direction(
                scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(6).scroller_width(6),
                ),
            ));
        }

        let scroll_body = scrollable(body).width(Length::Fill).height(Length::Fill);

        container(column![header, scroll_body].height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.075, 0.084, 0.105))),
                ..Default::default()
            })
            .into()
    }

    fn view_mixer_strip<'a>(&'a self, node: &'a MixerNodeInfo) -> Element<'a, Message> {
        let accent = match node.track_idx {
            Some(idx) => track_color(idx),
            None => match node.category {
                NodeCategory::Effect => Color::from_rgb(0.94, 0.66, 0.24),
                NodeCategory::MixBus => Color::from_rgb(0.32, 0.80, 0.76),
                NodeCategory::Instrument => Color::from_rgb(0.42, 0.72, 0.98),
            },
        };
        let is_selected = self.selected_node_id.as_deref() == Some(node.id.as_str());

        let top_stripe = container(Space::with_height(4))
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(accent)),
                border: Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        let title_btn = button(
            column![
                text(&node.id).size(13).color(Color::WHITE),
                text(&node.processor_label)
                    .size(10.5)
                    .color(Color::from_rgb(0.62, 0.70, 0.80)),
            ]
            .spacing(1),
        )
        .width(Length::Fill)
        .padding([4, 6])
        .style(move |theme, status| {
            if is_selected {
                accent_button_style(Color::from_rgb(0.22, 0.34, 0.54), theme, status)
            } else {
                studio_button_style(theme, status)
            }
        })
        .on_press(Message::SelectMixerNode(Some(node.id.clone())));

        let mut strip = Column::new().spacing(8).push(top_stripe).push(title_btn);

        // Live M/S controls directly on instrument track strips
        if let Some(idx) = node.track_idx {
            let is_muted = self.muted_tracks.contains(&node.id);
            let is_solo = self.solo_tracks.contains(&node.id);
            let m_btn = button(text("Mute").size(10.5))
                .padding([3, 8])
                .width(Length::FillPortion(1))
                .style(move |theme, status| {
                    if is_muted {
                        accent_button_style(Color::from_rgb(0.80, 0.26, 0.30), theme, status)
                    } else {
                        studio_button_style(theme, status)
                    }
                })
                .on_press(Message::ToggleTrackMute(idx));
            let s_btn = button(text("Solo").size(10.5))
                .padding([3, 8])
                .width(Length::FillPortion(1))
                .style(move |theme, status| {
                    if is_solo {
                        accent_button_style(Color::from_rgb(0.84, 0.62, 0.14), theme, status)
                    } else {
                        studio_button_style(theme, status)
                    }
                })
                .on_press(Message::ToggleTrackSolo(idx));
            strip = strip.push(row![m_btn, s_btn].spacing(5));
        }

        // Show top 5 priority controls right on the channel strip
        let shown = 5usize;
        for param in node.params.iter().take(shown) {
            strip = strip.push(self.view_param_slider_row(node, param, true));
        }

        if node.params.len() > shown {
            let extra = node.params.len() - shown;
            strip = strip.push(
                button(text(format!("+{extra} more in Inspector →")).size(10.5))
                    .width(Length::Fill)
                    .padding([4, 6])
                    .style(studio_button_style)
                    .on_press(Message::SelectMixerNode(Some(node.id.clone()))),
            );
        }

        let border_color = if is_selected {
            Color::from_rgb(0.45, 0.74, 0.98)
        } else {
            Color::from_rgba(1.0, 1.0, 1.0, 0.09)
        };
        let border_w = if is_selected { 1.6 } else { 1.0 };

        container(strip)
            .width(Length::Fixed(186.0))
            .height(Length::Shrink)
            .padding(9)
            .style(move |_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.105, 0.118, 0.150))),
                border: Border {
                    color: border_color,
                    width: border_w,
                    radius: 6.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_node_inspector_card<'a>(&'a self, node: &'a MixerNodeInfo) -> Element<'a, Message> {
        let accent = match node.track_idx {
            Some(idx) => track_color(idx),
            None => Color::from_rgb(0.94, 0.68, 0.28),
        };
        let mut header = row![
            text("NODE / FX")
                .size(10.5)
                .color(Color::from_rgb(0.58, 0.66, 0.78)),
            Space::with_width(6),
            text(&node.id).size(12).color(accent),
            horizontal_space(),
        ]
        .align_y(Alignment::Center);

        if let Some(idx) = node.track_idx {
            let is_muted = self.muted_tracks.contains(&node.id);
            let is_solo = self.solo_tracks.contains(&node.id);
            header = header
                .push(
                    button(text("M").size(9.5))
                        .padding([2, 5])
                        .style(move |t, s| {
                            if is_muted {
                                accent_button_style(Color::from_rgb(0.80, 0.26, 0.30), t, s)
                            } else {
                                studio_button_style(t, s)
                            }
                        })
                        .on_press(Message::ToggleTrackMute(idx)),
                )
                .push(Space::with_width(2))
                .push(
                    button(text("S").size(9.5))
                        .padding([2, 5])
                        .style(move |t, s| {
                            if is_solo {
                                accent_button_style(Color::from_rgb(0.84, 0.62, 0.14), t, s)
                            } else {
                                studio_button_style(t, s)
                            }
                        })
                        .on_press(Message::ToggleTrackSolo(idx)),
                )
                .push(Space::with_width(4));
        }

        header = header.push(
            button(text("×").size(11))
                .padding([1, 6])
                .style(studio_button_style)
                .on_press(Message::SelectMixerNode(None)),
        );

        let mut card = Column::new().spacing(7).push(header).push(
            text(format!(
                "{}  ·  {} params",
                node.processor_label,
                node.params.len()
            ))
            .size(10.5)
            .color(Color::from_rgb(0.60, 0.66, 0.76)),
        );

        for param in &node.params {
            card = card.push(self.view_param_slider_row(node, param, false));
        }

        if !node.editable_in_entry {
            card = card.push(
                text("Node defined outside entry file (read-only)")
                    .size(10)
                    .color(Color::from_rgb(0.76, 0.62, 0.42)),
            );
        }

        container(card)
            .padding(9)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.112, 0.128, 0.164))),
                border: Border {
                    color: Color::from_rgba(0.92, 0.68, 0.26, 0.38),
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_param_slider_row<'a>(
        &'a self,
        node: &'a MixerNodeInfo,
        param: &'a NodeParamInfo,
        compact: bool,
    ) -> Element<'a, Message> {
        let current = self
            .param_drafts
            .get(&(node.id.clone(), param.name.clone()))
            .copied()
            .unwrap_or(param.value);
        let pos = param.to_slider_pos(current);
        let val_text = param.format_value(current);

        let mut top = row![text(&param.name)
            .size(if compact { 10.5 } else { 11.0 })
            .color(if param.is_authored {
                Color::from_rgb(0.90, 0.94, 0.99)
            } else {
                Color::from_rgb(0.68, 0.74, 0.82)
            })]
        .spacing(4)
        .align_y(Alignment::Center);

        if param.is_automated {
            top = top.push(
                container(text("AUTO").size(8.5).color(Color::BLACK))
                    .padding([1, 4])
                    .style(|_| container::Style {
                        background: Some(Background::Color(Color::from_rgb(0.92, 0.72, 0.24))),
                        border: Border {
                            radius: 3.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            );
        }

        top = top.push(horizontal_space()).push(
            text(val_text)
                .size(if compact { 10.5 } else { 11.0 })
                .color(Color::from_rgb(0.55, 0.86, 0.78)),
        );

        if !compact && param.is_authored && node.editable_in_entry {
            let nid = node.id.clone();
            let pname = param.name.clone();
            top = top.push(
                button(text("↺").size(10))
                    .padding([1, 5])
                    .style(studio_button_style)
                    .on_press_maybe((!self.editing).then_some(Message::ResetNodeParam {
                        node_id: nid,
                        param_name: pname,
                    })),
            );
        }

        let nid_change = node.id.clone();
        let pname_change = param.name.clone();
        let nid_commit = node.id.clone();
        let pname_commit = param.name.clone();
        let param_clone = param.clone();

        let ctrl: Element<'a, Message> = if node.editable_in_entry {
            slider(0.0_f32..=1.0_f32, pos, move |new_pos| {
                Message::NodeParamDraft {
                    node_id: nid_change.clone(),
                    param_name: pname_change.clone(),
                    value: param_clone.value_at_slider_pos(new_pos),
                }
            })
            .step(0.005_f32)
            .on_release(Message::CommitNodeParam {
                node_id: nid_commit,
                param_name: pname_commit,
            })
            .width(Length::Fill)
            .into()
        } else {
            slider(0.0_f32..=1.0_f32, pos, |_| Message::Tick)
                .width(Length::Fill)
                .into()
        };

        column![top, ctrl].spacing(2).into()
    }

    fn view_code_panel<'a>(&'a self, proj: &'a ProjectData) -> Element<'a, Message> {
        let dirty = self.is_dirty();
        let tab = self.code_tab;
        let diff_badge = if !self.cached_unsaved_diff.is_empty() {
            format!(
                "Unsaved Diff (+{}/-{})",
                self.cached_unsaved_diff.added, self.cached_unsaved_diff.removed
            )
        } else {
            "Unsaved Diff".into()
        };
        let ext_badge = match &self.external_change {
            Some(ext) => format!("AI / External (+{}/-{})", ext.diff.added, ext.diff.removed),
            None => "AI / External".into(),
        };
        let has_ext = self.external_change.is_some();

        let tab_code = button(text("Live Code").size(11.5))
            .padding([3, 9])
            .style(move |t, s| {
                if tab == CodeTab::LiveCode {
                    accent_button_style(Color::from_rgb(0.24, 0.44, 0.74), t, s)
                } else {
                    studio_button_style(t, s)
                }
            })
            .on_press(Message::SelectCodeTab(CodeTab::LiveCode));

        let tab_diff = button(text(diff_badge).size(11.5))
            .padding([3, 9])
            .style(move |t, s| {
                if tab == CodeTab::UnsavedDiff {
                    accent_button_style(Color::from_rgb(0.76, 0.50, 0.14), t, s)
                } else if dirty {
                    accent_button_style(Color::from_rgb(0.45, 0.34, 0.14), t, s)
                } else {
                    studio_button_style(t, s)
                }
            })
            .on_press(Message::SelectCodeTab(CodeTab::UnsavedDiff));

        let tab_ext = button(text(ext_badge).size(11.5))
            .padding([3, 9])
            .style(move |t, s| {
                if tab == CodeTab::ExternalDiff {
                    accent_button_style(Color::from_rgb(0.14, 0.56, 0.58), t, s)
                } else if has_ext {
                    accent_button_style(Color::from_rgb(0.14, 0.38, 0.42), t, s)
                } else {
                    studio_button_style(t, s)
                }
            })
            .on_press(Message::SelectCodeTab(CodeTab::ExternalDiff));

        let mut header = row![tab_code, tab_diff, tab_ext]
            .spacing(6)
            .align_y(Alignment::Center);

        if tab == CodeTab::LiveCode {
            if let Some(hl) = &self.cached_highlight {
                let loc = if hl.start_line == hl.end_line {
                    format!("Line {} · {}", hl.start_line, hl.label)
                } else {
                    format!("Lines {}–{} · {}", hl.start_line, hl.end_line, hl.label)
                };
                header = header
                    .push(Space::with_width(8))
                    .push(text(loc).size(11).color(Color::from_rgb(0.54, 0.80, 0.98)));
                let focused = self.code_focused_only;
                header = header.push(Space::with_width(6)).push(
                    button(text(if focused { "Focused" } else { "Full File" }).size(10.5))
                        .padding([2, 7])
                        .style(studio_button_style)
                        .on_press(Message::ToggleCodeFocused),
                );
            }
        }

        header = header.push(horizontal_space());

        if tab == CodeTab::ExternalDiff {
            if self.external_change.is_some() {
                header = header
                    .push(
                        button(text("Revert AI Edit").size(11))
                            .padding([3, 8])
                            .style(|t, s| {
                                accent_button_style(Color::from_rgb(0.72, 0.24, 0.28), t, s)
                            })
                            .on_press_maybe((!self.editing).then_some(Message::RevertExternalEdit)),
                    )
                    .push(
                        button(text("Keep").size(11))
                            .padding([3, 8])
                            .style(studio_button_style)
                            .on_press(Message::DismissExternalChange),
                    );
            }
        } else if dirty {
            header = header
                .push(
                    button(text("Discard").size(11))
                        .padding([3, 8])
                        .style(studio_button_style)
                        .on_press_maybe((!self.editing).then_some(Message::DiscardUnsavedEdits)),
                )
                .push(
                    button(text("Save* (⌘S)").size(11))
                        .padding([3, 8])
                        .style(|t, s| accent_button_style(Color::from_rgb(0.84, 0.54, 0.16), t, s))
                        .on_press(Message::SaveToDisk),
                );
        }

        header = header.push(
            button(text("×").size(11))
                .padding([2, 7])
                .style(studio_button_style)
                .on_press(Message::ToggleCodePanel),
        );

        let header_bar = container(header.padding([5, 10]))
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.105, 0.118, 0.150))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            });

        let body: Element<'a, Message> = match tab {
            CodeTab::LiveCode => {
                let src = proj
                    .bundle
                    .sources
                    .get(&proj.bundle.entry)
                    .map(String::as_str)
                    .unwrap_or("");
                let all_lines: Vec<&str> = src.lines().collect();
                let (start_idx, end_idx) = match (&self.cached_highlight, self.code_focused_only) {
                    (Some(hl), true) => {
                        let s = hl.start_line.saturating_sub(13).max(1);
                        let e = (hl.end_line + 14).min(all_lines.len().max(1));
                        (s - 1, e)
                    }
                    _ => (0, all_lines.len()),
                };

                let mut lines_col = Column::new().spacing(1).padding([6, 10]);
                if start_idx > 0 {
                    lines_col = lines_col.push(
                        text(format!(
                            "  … {} earlier lines hidden (click Focused to view Full File)",
                            start_idx
                        ))
                        .size(10.5)
                        .font(Font::MONOSPACE)
                        .color(Color::from_rgb(0.46, 0.52, 0.62)),
                    );
                }

                for (idx, line) in all_lines[start_idx..end_idx].iter().enumerate() {
                    let line_no = start_idx + idx + 1;
                    let is_hl = self
                        .cached_highlight
                        .as_ref()
                        .is_some_and(|hl| line_no >= hl.start_line && line_no <= hl.end_line);
                    let is_changed = self.cached_changed_lines.contains(&line_no);
                    let marker = if is_hl {
                        "▶"
                    } else if is_changed {
                        "+"
                    } else {
                        " "
                    };
                    let marker_color = if is_hl {
                        Color::from_rgb(0.45, 0.84, 1.0)
                    } else if is_changed {
                        Color::from_rgb(0.42, 0.88, 0.58)
                    } else {
                        Color::from_rgb(0.40, 0.46, 0.56)
                    };
                    let text_color = if line.trim_start().starts_with("//") {
                        Color::from_rgb(0.48, 0.56, 0.66)
                    } else if is_hl {
                        Color::WHITE
                    } else {
                        Color::from_rgb(0.84, 0.88, 0.94)
                    };

                    let line_row = row![
                        text(format!("{line_no:>4} {marker} "))
                            .size(11.5)
                            .font(Font::MONOSPACE)
                            .color(marker_color),
                        text(*line)
                            .size(11.5)
                            .font(Font::MONOSPACE)
                            .color(text_color),
                    ];

                    let styled: Element<'a, Message> = if is_hl {
                        container(line_row)
                            .width(Length::Fill)
                            .style(|_| container::Style {
                                background: Some(Background::Color(Color::from_rgba(
                                    0.22, 0.50, 0.86, 0.28,
                                ))),
                                ..Default::default()
                            })
                            .into()
                    } else if is_changed {
                        container(line_row)
                            .width(Length::Fill)
                            .style(|_| container::Style {
                                background: Some(Background::Color(Color::from_rgba(
                                    0.14, 0.48, 0.26, 0.20,
                                ))),
                                ..Default::default()
                            })
                            .into()
                    } else {
                        line_row.into()
                    };
                    lines_col = lines_col.push(styled);
                }

                if end_idx < all_lines.len() {
                    lines_col = lines_col.push(
                        text(format!(
                            "  … {} following lines hidden",
                            all_lines.len() - end_idx
                        ))
                        .size(10.5)
                        .font(Font::MONOSPACE)
                        .color(Color::from_rgb(0.46, 0.52, 0.62)),
                    );
                }

                scrollable(lines_col)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
            CodeTab::UnsavedDiff => self.view_diff_summary(
                &self.cached_unsaved_diff,
                "Working buffer matches disk — edit notes or mixer sliders to preview exact .maac changes here.",
            ),
            CodeTab::ExternalDiff => match &self.external_change {
                Some(ext) => self.view_diff_summary(
                    &ext.diff,
                    "External reload matched previous content.",
                ),
                None => container(
                    text("No external / AI file edits detected yet. With Watch enabled, edits saved by an AI agent or external editor appear here.")
                        .size(11.5)
                        .color(Color::from_rgb(0.54, 0.60, 0.70)),
                )
                .padding(14)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            },
        };

        container(column![header_bar, body].height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.068, 0.076, 0.095))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_diff_summary<'a>(
        &'a self,
        diff: &'a DiffSummary,
        empty_msg: &'static str,
    ) -> Element<'a, Message> {
        if diff.is_empty() {
            return container(
                text(empty_msg)
                    .size(11.5)
                    .color(Color::from_rgb(0.54, 0.60, 0.70)),
            )
            .padding(14)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        }

        let mut col = Column::new().spacing(2).padding([6, 10]);
        for hunk in &diff.hunks {
            col = col.push(
                text(&hunk.header)
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(Color::from_rgb(0.52, 0.76, 0.96)),
            );
            for line in &hunk.lines {
                let old_s = line
                    .old_line
                    .map_or_else(|| "    ".into(), |n| format!("{n:>4}"));
                let new_s = line
                    .new_line
                    .map_or_else(|| "    ".into(), |n| format!("{n:>4}"));
                let (prefix, fg, bg) = match line.kind {
                    DiffLineKind::Context => {
                        (" ", Color::from_rgb(0.72, 0.78, 0.86), Color::TRANSPARENT)
                    }
                    DiffLineKind::Added => (
                        "+",
                        Color::from_rgb(0.62, 0.96, 0.72),
                        Color::from_rgba(0.14, 0.48, 0.24, 0.26),
                    ),
                    DiffLineKind::Removed => (
                        "-",
                        Color::from_rgb(0.98, 0.62, 0.64),
                        Color::from_rgba(0.58, 0.16, 0.18, 0.28),
                    ),
                };
                let r = row![
                    text(format!("{old_s} {new_s} {prefix} "))
                        .size(11.5)
                        .font(Font::MONOSPACE)
                        .color(fg),
                    text(&line.text).size(11.5).font(Font::MONOSPACE).color(fg),
                ];
                col = col.push(
                    container(r)
                        .width(Length::Fill)
                        .style(move |_| container::Style {
                            background: Some(Background::Color(bg)),
                            ..Default::default()
                        }),
                );
            }
            col = col.push(Space::with_height(6));
        }

        scrollable(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

fn spawn_compile(path: PathBuf, is_reload: bool) -> Task<Message> {
    let target = path.clone();
    Task::perform(
        async move {
            let handle = std::thread::spawn(move || load_and_compile(&target).map(Box::new));
            handle
                .join()
                .unwrap_or_else(|_| Err("compiler thread panicked".into()))
        },
        move |result| Message::ProjectCompiled {
            path: path.clone(),
            is_reload,
            result,
        },
    )
}

fn studio_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    let (bg, border, text_color) = match status {
        button::Status::Active => (
            Color::from_rgb(0.145, 0.165, 0.210),
            Color::from_rgb(0.235, 0.270, 0.340),
            Color::from_rgb(0.86, 0.90, 0.95),
        ),
        button::Status::Hovered => (
            Color::from_rgb(0.200, 0.228, 0.290),
            Color::from_rgb(0.340, 0.390, 0.490),
            Color::WHITE,
        ),
        button::Status::Pressed => (
            Color::from_rgb(0.120, 0.138, 0.178),
            Color::from_rgb(0.280, 0.320, 0.400),
            Color::from_rgb(0.82, 0.86, 0.92),
        ),
        button::Status::Disabled => (
            Color::from_rgb(0.105, 0.118, 0.148),
            Color::from_rgb(0.165, 0.185, 0.230),
            Color::from_rgb(0.45, 0.49, 0.56),
        ),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color,
        border: Border {
            color: border,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn accent_button_style(base: Color, _theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Active => base,
        button::Status::Hovered => Color::from_rgb(
            (base.r + 0.08).min(1.0),
            (base.g + 0.08).min(1.0),
            (base.b + 0.08).min(1.0),
        ),
        button::Status::Pressed => Color::from_rgb(
            (base.r * 0.85).max(0.0),
            (base.g * 0.85).max(0.0),
            (base.b * 0.85).max(0.0),
        ),
        button::Status::Disabled => Color::from_rgb(0.15, 0.17, 0.21),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.16),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn track_chip_style(
    color: Color,
    visible: bool,
    _theme: &Theme,
    status: button::Status,
) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    let bg = if visible {
        Color::from_rgba(color.r, color.g, color.b, if hovered { 0.22 } else { 0.13 })
    } else if hovered {
        Color::from_rgb(0.15, 0.17, 0.21)
    } else {
        Color::from_rgb(0.10, 0.115, 0.145)
    };
    let border = if visible {
        Color::from_rgba(color.r, color.g, color.b, 0.65)
    } else {
        Color::from_rgb(0.22, 0.25, 0.30)
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: if visible {
            color
        } else {
            Color::from_rgb(0.45, 0.49, 0.56)
        },
        border: Border {
            color: border,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}
