//! Main Elm-architecture `iced` application for MaaC Studio.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::keyboard::{self, key::Named, Key};
use iced::widget::{
    button, canvas, column, container, horizontal_space, row, scrollable, slider, text, text_input,
    toggler, Column, Row, Space,
};
use iced::{
    Alignment, Background, Border, Color, Element, Font, Length, Subscription, Task, Theme,
};
use maac::dsp::{BackgroundSession, Playback, PlaybackConfig, RenderSpan, SessionPolicy};
use maac::export::{
    render_wav_to_path_artifact_range_with_limits, render_wav_to_path_artifact_with_limits,
    FrameRange, WavFormat,
};

use crate::analysis::{run_analysis, AnalysisSummary};
use crate::audio::AudioEngine;
use crate::automation::{apply_curve_point_add, apply_curve_point_delete, apply_curve_point_edit};
use crate::code_view::{
    changed_new_lines, compute_line_diff, locate_object_lines, DiffLineKind, DiffSummary,
    SourceHighlight,
};
use crate::collab::{
    build_agent_prompt, copy_to_clipboard, detect_local_agents, diff_projects_for_highlight,
    load_collab_file, measure_region_audio, run_agent_turn, save_collab_file, AbComparison, AbSide,
    AgentKind, AgentTurnOutcome, AudioRegionSnapshot, ChatMessage, ChatRole, CollabFile,
    DetectedAgent, PinAuthor, ReviewPin, SharedFocusContext,
};
use crate::editor::{
    add_hit_at_score_pos, add_note_at_score_pos, apply_batch_note_edit, apply_note_edit, format_q,
    inspect_note_in_bundle, materialize_note_placement, midi_to_pitch_symbol, BatchNoteEdit,
    NoteEdit, NoteSourceInfo,
};
use crate::mixer::{
    apply_node_param_edit, apply_node_param_reset, format_param_value, MixerNodeInfo, NodeCategory,
    NodeParamInfo,
};
use crate::plugins::freeze::{freeze_target_to_maac, FreezeOutcome};
use crate::plugins::rack_file::{load_rack_file, save_rack_file};
use crate::plugins::{
    scan_installed_plugins, HostRack, PluginDescriptor, PluginKind, PluginSlotSummary, SlotTarget,
};
use crate::project::{
    format_time, load_and_compile, load_from_bundle, load_from_edited, ProjectData,
};
use crate::sampler_import::emit_maac::{emit_maac_library, wire_imported_library_into_bundle};
use crate::sampler_import::sf2::{list_sf2_presets, parse_sf2_preset, Sf2PresetInfo};
use crate::sampler_import::sfz::parse_sfz;
use crate::timeline::{track_color, TimelineAction, TimelineView};
use crate::watcher::ProjectWatcher;

const WATCH_DEBOUNCE: Duration = Duration::from_millis(120);
const PLAYBACK_EDIT_DEBOUNCE: Duration = Duration::from_millis(20);
const PLAYBACK_LEAD_FRAMES: u64 = 3_072; // ~64 ms @ 48 kHz for DAW-grade live responsiveness
const PLAYBACK_CHUNK_FRAMES: u64 = 512; // ~10.7 ms @ 48 kHz
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
    SelectHit(Option<String>),
    AddHitAt {
        frame: u64,
        velocity: f32,
    },
    HitAdded {
        prev_source: String,
        new_source_path: Vec<String>,
        summary: String,
        result: Result<Box<ProjectData>, String>,
    },
    ToggleAutomation,
    SelectAutoLane(usize),
    SelectAutoPoint {
        lane_idx: usize,
        point_idx: usize,
    },
    AutoValueSliderChanged(f32),
    CommitAutoPointValue,
    ApplyAutoPointEdit {
        lane_idx: usize,
        point_idx: usize,
        value: Option<f32>,
        shape: Option<String>,
    },
    AddAutoPoint {
        lane_idx: usize,
        frame: u64,
        value: f32,
    },
    DeleteAutoPoint {
        lane_idx: usize,
        point_idx: usize,
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
    PluginScanReady(Vec<PluginDescriptor>),
    OpenPluginPicker {
        target: SlotTarget,
        kind: PluginKind,
    },
    ClosePluginPicker,
    PluginSearchChanged(String),
    AddPluginSlot {
        target: SlotTarget,
        descriptor: PluginDescriptor,
    },
    SelectPluginSlot(Option<u64>),
    TogglePluginNativeUi(u64),
    TogglePluginBypass(u64),
    RemovePluginSlot(u64),
    PluginParamChanged {
        slot_id: u64,
        param_id: u32,
        value: f32,
    },
    FreezePluginTarget(SlotTarget),
    FreezeFinished {
        prev_source: String,
        target: SlotTarget,
        result: Result<Box<FreezeOutcome>, String>,
    },
    OpenSamplerImportDialog,
    SamplerFilePicked(Option<PathBuf>),
    SelectSf2TargetTrack(Option<String>),
    ImportSf2Preset {
        bank: u16,
        program: u16,
    },
    CloseSf2Picker,
    SamplerImported {
        prev_source: String,
        summary: String,
        result: Result<Box<ProjectData>, String>,
    },
    ToggleCodePanel,
    SelectCodeTab(CodeTab),
    ToggleCodeFocused,
    DiscardUnsavedEdits,
    RevertExternalEdit,
    DismissExternalChange,
    // AI + Human Live Collaboration
    ToggleCollabPanel,
    SetSidebarCollab(bool),
    SelectAgentBackend(AgentKind),
    ToggleAutoLoopAiEdit,
    CollabPinDraftChanged(String),
    AddReviewPin,
    TogglePinResolved(u64),
    DeleteReviewPin(u64),
    JumpToReviewPin(u64),
    RefreshAudioSnapshot,
    AudioSnapshotReady(Result<AudioRegionSnapshot, String>),
    CopyCollabContext,
    CollabPromptChanged(String),
    SendCollabPrompt(Option<String>),
    AgentTurnFinished {
        before_source: String,
        before_project: Box<ProjectData>,
        prompt_label: String,
        result: Result<Box<AgentTurnOutcome>, String>,
    },
    SwitchAbSide(AbSide),
    JumpAndLoopAiEdit,
    AcceptAiEdit,
    RejectAiEdit,
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
    selected_hit_address: Option<String>,
    selected_note_info: Option<NoteSourceInfo>,
    velocity_draft: Option<f32>,

    // Automation lane state
    show_automation: bool,
    active_auto_lane: usize,
    selected_auto_point: Option<(usize, usize)>,
    auto_value_draft: Option<f32>,
    saved_source: Option<String>,
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
    editing: bool,
    edit_error: Option<String>,

    // Live Mixer & Node/FX Parameter Editor
    show_mixer: bool,
    selected_node_id: Option<String>,
    param_drafts: HashMap<(String, String), f32>,

    // External DAW Plugins (AU / CLAP) + Freeze + SF2/SFZ Importer
    host_rack: Arc<Mutex<HostRack>>,
    plugin_catalog: Vec<PluginDescriptor>,
    plugin_slots: Vec<PluginSlotSummary>,
    selected_plugin_slot: Option<u64>,
    plugin_ui_sync_tick: u8,
    plugin_picker: Option<(SlotTarget, PluginKind)>,
    plugin_search: String,
    freezing: bool,
    sf2_picker: Option<(PathBuf, Vec<Sf2PresetInfo>, Option<String>)>,

    // Live .maac Code & Diff Collaboration Panel
    show_code: bool,
    code_tab: CodeTab,
    code_focused_only: bool,
    cached_highlight: Option<SourceHighlight>,
    cached_unsaved_diff: DiffSummary,
    cached_changed_lines: HashSet<usize>,
    external_change: Option<ExternalChange>,

    // AI + Human Live Collaboration (Chat, Review Pins, Shared Focus, Audio Telemetry, A/B)
    show_collab: bool,
    detected_agents: Vec<DetectedAgent>,
    selected_agent: Option<AgentKind>,
    auto_loop_ai_edit: bool,
    collab_pins: Vec<ReviewPin>,
    next_pin_id: u64,
    collab_pin_draft: String,
    collab_chat: Vec<ChatMessage>,
    collab_prompt_input: String,
    agent_running: bool,
    audio_snapshot: Option<AudioRegionSnapshot>,
    audio_snapshot_busy: bool,
    ab_comparison: Option<AbComparison>,

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
        let detected_agents = detect_local_agents();
        let selected_agent = detected_agents.first().map(|a| a.kind);
        let app = Self {
            audio: AudioEngine::new(48_000),
            project: None,
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
            selected_hit_address: None,
            selected_note_info: None,
            velocity_draft: None,
            show_automation: true,
            active_auto_lane: 0,
            selected_auto_point: None,
            auto_value_draft: None,
            saved_source: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            editing: false,
            edit_error: None,
            show_mixer: false,
            selected_node_id: None,
            param_drafts: HashMap::new(),
            host_rack: Arc::new(Mutex::new(HostRack::new(
                48_000.0,
                PLAYBACK_CHUNK_FRAMES as usize,
            ))),
            plugin_catalog: Vec::new(),
            plugin_slots: Vec::new(),
            selected_plugin_slot: None,
            plugin_ui_sync_tick: 0,
            plugin_picker: None,
            plugin_search: String::new(),
            freezing: false,
            sf2_picker: None,
            show_code: false,
            code_tab: CodeTab::LiveCode,
            code_focused_only: true,
            cached_highlight: None,
            cached_unsaved_diff: DiffSummary::default(),
            cached_changed_lines: HashSet::new(),
            external_change: None,
            show_collab: false,
            detected_agents,
            selected_agent,
            auto_loop_ai_edit: true,
            collab_pins: Vec::new(),
            next_pin_id: 1,
            collab_pin_draft: String::new(),
            collab_chat: Vec::new(),
            collab_prompt_input: String::new(),
            agent_running: false,
            audio_snapshot: None,
            audio_snapshot_busy: false,
            ab_comparison: None,
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
            zoom: 1.0,
            scroll: 0.0,
            peaks: (0.0, 0.0),
            exporting: false,
            show_sidebar: true,
            analyzing: false,
            analysis: None,
            analysis_error: None,
        };

        let scan_task = Task::perform(
            async {
                let handle = std::thread::spawn(scan_installed_plugins);
                handle.join().unwrap_or_default()
            },
            Message::PluginScanReady,
        );
        let task = match initial_path {
            Some(path) => Task::batch([scan_task, spawn_compile(path, false)]),
            None => scan_task,
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
        let text_input_open = self.show_collab || self.plugin_picker.is_some();
        let keys = if text_input_open {
            keyboard::on_key_press(|key, modifiers| {
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
                        Key::Character(ref c) if c.eq_ignore_ascii_case("g") => {
                            Some(Message::ToggleCollabPanel)
                        }
                        _ => None,
                    };
                }
                match key {
                    Key::Named(Named::Escape) => Some(Message::ClearSelection),
                    _ => None,
                }
            })
        } else {
            keyboard::on_key_press(|key, modifiers| {
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
                        Key::Character(ref c) if c.eq_ignore_ascii_case("g") => {
                            Some(Message::ToggleCollabPanel)
                        }
                        _ => None,
                    };
                }
                match key {
                    Key::Named(Named::Space) => Some(Message::TogglePlay),
                    Key::Named(Named::Home) => Some(Message::StopToStart),
                    Key::Named(Named::Escape) => Some(Message::ClearSelection),
                    Key::Named(Named::Delete | Named::Backspace) => {
                        Some(Message::KeyboardDeleteNote)
                    }
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
                    Key::Character(ref c) if c.eq_ignore_ascii_case("l") => {
                        Some(Message::ToggleLoop)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("m") => {
                        Some(Message::ToggleMixer)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("c") => {
                        Some(Message::ToggleCodePanel)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("g") => {
                        Some(Message::ToggleCollabPanel)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("s") => {
                        Some(Message::ToggleSnap)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("f") => {
                        Some(Message::ToggleFollow)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                        Some(Message::RecompileNow)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("a") => {
                        Some(Message::RunAnalysis)
                    }
                    _ => None,
                }
            })
        };
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
                    self.apply_live_mutes();
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
                    self.apply_live_mutes();
                }
                Task::none()
            }

            Message::ClearSoloMute => {
                if !self.muted_tracks.is_empty() || !self.solo_tracks.is_empty() {
                    self.muted_tracks.clear();
                    self.solo_tracks.clear();
                    self.apply_live_mutes();
                }
                Task::none()
            }

            Message::SelectNote(addr) => {
                self.selected_notes.clear();
                self.selected_hit_address = None;
                self.selected_auto_point = None;
                self.auto_value_draft = None;
                if let Some(a) = &addr {
                    self.selected_notes.insert(a.clone());
                }
                self.selected_note_address = addr;
                self.edit_error = None;
                self.refresh_selected_note_info();
                Task::none()
            }

            Message::SelectHit(addr) => {
                self.selected_notes.clear();
                self.selected_note_address = None;
                self.selected_auto_point = None;
                self.auto_value_draft = None;
                self.selected_hit_address = addr;
                self.edit_error = None;
                self.refresh_selected_note_info();
                Task::none()
            }

            Message::AddHitAt { frame, velocity } => {
                if self.editing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let selected_hit = self
                    .selected_hit_address
                    .as_deref()
                    .and_then(|addr| proj.hits.iter().find(|h| h.address == addr));
                let target_track_idx = selected_hit
                    .map(|h| h.track_idx)
                    .or_else(|| proj.tracks.iter().position(|t| t.hit_count > 0));
                let Some(track_idx) = target_track_idx else {
                    self.edit_error = Some("No drum hit track available".into());
                    return Task::none();
                };
                let track_name = proj.tracks[track_idx].name.clone();
                let key = selected_hit
                    .map(|h| h.key.clone())
                    .or_else(|| {
                        proj.track_hit_keys
                            .get(&track_idx)
                            .and_then(|k| k.first().cloned())
                    })
                    .unwrap_or_else(|| "kick".into());
                let score_q = proj.frame_to_score_q(frame);
                let bundle = proj.bundle.clone();
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, new_path, summary) = add_hit_at_score_pos(
                                &bundle,
                                &track_name,
                                score_q,
                                &key,
                                velocity,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
                            Ok((Box::new(recompiled), new_path, summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("add hit worker panicked".into()))
                    },
                    move |res| match res {
                        Ok((data, new_source_path, summary)) => Message::HitAdded {
                            prev_source: prev_source.clone(),
                            new_source_path,
                            summary,
                            result: Ok(data),
                        },
                        Err(err) => Message::HitAdded {
                            prev_source: prev_source.clone(),
                            new_source_path: Vec::new(),
                            summary: String::new(),
                            result: Err(err),
                        },
                    },
                )
            }

            Message::HitAdded {
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
                            data.hits.iter().find(|h| h.source_path == new_source_path)
                        {
                            self.selected_notes.clear();
                            self.selected_note_address = None;
                            self.selected_hit_address = Some(added.address.clone());
                        }
                        let ms = data.compile_ms;
                        return self.apply_in_memory_project(*data, format!("{summary} ({ms} ms)"));
                    }
                    Err(err) => {
                        self.param_drafts.clear();
                        if let Some(pb) = &self.playback {
                            pb.clear_live_params();
                        }
                        self.edit_error = Some(err);
                    }
                }
                Task::none()
            }

            Message::ToggleAutomation => {
                self.show_automation = !self.show_automation;
                Task::none()
            }

            Message::SelectAutoLane(idx) => {
                self.active_auto_lane = idx;
                self.selected_auto_point = Some((idx, 0));
                if let Some(val) = self
                    .project
                    .as_ref()
                    .and_then(|p| p.automations.get(idx))
                    .and_then(|l| l.points.first())
                    .map(|pt| pt.value)
                {
                    self.auto_value_draft = Some(val);
                }
                self.refresh_code_cache();
                Task::none()
            }

            Message::SelectAutoPoint {
                lane_idx,
                point_idx,
            } => {
                self.selected_notes.clear();
                self.selected_note_address = None;
                self.selected_hit_address = None;
                self.selected_note_info = None;
                self.active_auto_lane = lane_idx;
                self.selected_auto_point = Some((lane_idx, point_idx));
                self.auto_value_draft = self
                    .project
                    .as_ref()
                    .and_then(|p| p.automations.get(lane_idx))
                    .and_then(|l| l.points.iter().find(|pt| pt.index == point_idx))
                    .map(|pt| pt.value);
                self.edit_error = None;
                self.refresh_code_cache();
                Task::none()
            }

            Message::AutoValueSliderChanged(v) => {
                self.auto_value_draft = Some(v);
                Task::none()
            }

            Message::CommitAutoPointValue => {
                if let (Some((lane_idx, point_idx)), Some(val)) =
                    (self.selected_auto_point, self.auto_value_draft)
                {
                    return self.update(Message::ApplyAutoPointEdit {
                        lane_idx,
                        point_idx,
                        value: Some(val),
                        shape: None,
                    });
                }
                Task::none()
            }

            Message::ApplyAutoPointEdit {
                lane_idx,
                point_idx,
                value,
                shape,
            } => {
                if self.editing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(curve_id) = proj
                    .automations
                    .get(lane_idx)
                    .and_then(|l| l.curve_id.clone())
                else {
                    self.edit_error = Some("Automation curve is not editable in entry file".into());
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let bundle = proj.bundle.clone();
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, summary) = apply_curve_point_edit(
                                &bundle,
                                &curve_id,
                                point_idx,
                                value,
                                shape.as_deref(),
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
                            Ok((Box::new(recompiled), summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("automation edit worker panicked".into()))
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

            Message::AddAutoPoint {
                lane_idx,
                frame,
                value,
            } => {
                if self.editing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(lane) = proj.automations.get(lane_idx) else {
                    return Task::none();
                };
                let Some(curve_id) = lane.curve_id.clone() else {
                    self.edit_error = Some("Automation curve is not editable in entry file".into());
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let clock_is_seconds = lane.clock_is_seconds;
                let pos_raw = if clock_is_seconds {
                    (frame as f64 / proj.sample_rate_hz.max(1) as f64 - lane.anchor_raw).max(0.0)
                } else {
                    (proj.frame_to_score_q(frame) - lane.anchor_raw).max(0.0)
                };
                let bundle = proj.bundle.clone();
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, _idx, summary) = apply_curve_point_add(
                                &bundle,
                                &curve_id,
                                pos_raw,
                                value,
                                clock_is_seconds,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
                            Ok((Box::new(recompiled), summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("automation add worker panicked".into()))
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

            Message::DeleteAutoPoint {
                lane_idx,
                point_idx,
            } => {
                if self.editing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(curve_id) = proj
                    .automations
                    .get(lane_idx)
                    .and_then(|l| l.curve_id.clone())
                else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let bundle = proj.bundle.clone();
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.selected_auto_point = None;
                self.auto_value_draft = None;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, summary) = apply_curve_point_delete(
                                &bundle,
                                &curve_id,
                                point_idx,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
                            Ok((Box::new(recompiled), summary))
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("automation delete worker panicked".into()))
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
                self.selected_hit_address = None;
                self.selected_auto_point = None;
                self.auto_value_draft = None;
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
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, summary) = apply_batch_note_edit(
                                &bundle,
                                &source_paths,
                                edit,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
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
                let target_addr = self
                    .selected_note_address
                    .clone()
                    .or_else(|| self.selected_hit_address.clone());
                let (Some(proj), Some(addr)) = (&self.project, target_addr) else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let bundle = proj.bundle.clone();
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, remapped, summary) = materialize_note_placement(
                                &bundle,
                                &addr,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
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
                            if self.selected_hit_address.is_some() {
                                self.selected_hit_address = Some(new_addr);
                            } else {
                                self.selected_notes.clear();
                                self.selected_notes.insert(new_addr.clone());
                                self.selected_note_address = Some(new_addr);
                            }
                        }
                        let ms = data.compile_ms;
                        return self.apply_in_memory_project(*data, format!("{summary} ({ms} ms)"));
                    }
                    Err(err) => {
                        self.param_drafts.clear();
                        if let Some(pb) = &self.playback {
                            pb.clear_live_params();
                        }
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
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, new_path, summary) = add_note_at_score_pos(
                                &bundle,
                                &track_name,
                                score_q,
                                midi_pitch,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
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
                        self.param_drafts.clear();
                        if let Some(pb) = &self.playback {
                            pb.clear_live_params();
                        }
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
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, summary) = apply_note_edit(
                                &bundle,
                                &source_path,
                                edit,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
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
                        self.param_drafts.clear();
                        if let Some(pb) = &self.playback {
                            pb.clear_live_params();
                        }
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
                        let rack_file = self
                            .host_rack
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .to_rack_file();
                        let rack_err = save_rack_file(&proj.entry_path, &rack_file).err();
                        self.saved_source = Some(saved);
                        self.last_self_save = Some(Instant::now());
                        self.pending_watch_at = None;
                        self.edit_error = rack_err;
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

            Message::ToggleCollabPanel => {
                if self.show_sidebar && self.show_collab {
                    self.show_collab = false;
                } else {
                    self.show_sidebar = true;
                    self.show_collab = true;
                    if self.audio_snapshot.is_none() && !self.audio_snapshot_busy {
                        return self.update(Message::RefreshAudioSnapshot);
                    }
                }
                Task::none()
            }

            Message::SetSidebarCollab(enable) => {
                self.show_sidebar = true;
                self.show_collab = enable;
                if enable && self.audio_snapshot.is_none() && !self.audio_snapshot_busy {
                    return self.update(Message::RefreshAudioSnapshot);
                }
                Task::none()
            }

            Message::SelectAgentBackend(kind) => {
                self.selected_agent = Some(kind);
                Task::none()
            }

            Message::ToggleAutoLoopAiEdit => {
                self.auto_loop_ai_edit = !self.auto_loop_ai_edit;
                Task::none()
            }

            Message::CollabPinDraftChanged(val) => {
                self.collab_pin_draft = val;
                Task::none()
            }

            Message::AddReviewPin => {
                let text = self.collab_pin_draft.trim().to_owned();
                if text.is_empty() {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let (frame, end_frame) = match self.loop_range {
                    Some(lr) => (lr.start, Some(lr.end)),
                    None => (self.playhead_frame, None),
                };
                let bar_label = proj.format_bar_position(frame);
                let track = self
                    .selected_note_address
                    .as_deref()
                    .and_then(|addr| proj.notes.iter().find(|n| n.address == addr))
                    .and_then(|n| proj.tracks.get(n.track_idx).map(|t| t.name.clone()))
                    .or_else(|| self.selected_node_id.clone());
                let id = self.next_pin_id;
                self.next_pin_id += 1;
                self.collab_pins.push(ReviewPin {
                    id,
                    frame,
                    end_frame,
                    bar_label: bar_label.clone(),
                    track,
                    text,
                    resolved: false,
                    created_by: PinAuthor::Human,
                });
                self.collab_pin_draft.clear();
                self.persist_collab_file();
                self.last_reload_note = Some(format!("Pinned review note #{id} at {bar_label}"));
                Task::none()
            }

            Message::TogglePinResolved(id) => {
                if let Some(pin) = self.collab_pins.iter_mut().find(|p| p.id == id) {
                    pin.resolved = !pin.resolved;
                    self.persist_collab_file();
                }
                Task::none()
            }

            Message::DeleteReviewPin(id) => {
                self.collab_pins.retain(|p| p.id != id);
                self.persist_collab_file();
                Task::none()
            }

            Message::JumpToReviewPin(id) => {
                let Some(pin) = self.collab_pins.iter().find(|p| p.id == id).cloned() else {
                    return Task::none();
                };
                self.show_sidebar = true;
                self.show_collab = true;
                self.playhead_frame = pin.frame;
                if let Some(pb) = &self.playback {
                    pb.seek(pin.frame);
                }
                if let Some(end) = pin.end_frame {
                    if end > pin.frame + 256 {
                        self.apply_loop(Some(RenderSpan {
                            start: pin.frame,
                            end,
                        }));
                    }
                }
                self.last_reload_note =
                    Some(format!("Jumped to Pin #{} ({})", pin.id, pin.bar_label));
                Task::none()
            }

            Message::RefreshAudioSnapshot => {
                let Some(proj) = self.project.clone() else {
                    return Task::none();
                };
                if self.audio_snapshot_busy {
                    return Task::none();
                }
                self.audio_snapshot_busy = true;
                let lr = self.loop_range;
                let ph = self.playhead_frame;
                Task::perform(
                    async move {
                        let handle =
                            std::thread::spawn(move || measure_region_audio(&proj, lr, ph));
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("audio snapshot worker panicked".into()))
                    },
                    Message::AudioSnapshotReady,
                )
            }

            Message::AudioSnapshotReady(result) => {
                self.audio_snapshot_busy = false;
                match result {
                    Ok(snap) => {
                        self.last_reload_note = Some(format!(
                            "Audio snapshot ({}, {} ms)",
                            snap.bar_span_label, snap.elapsed_ms
                        ));
                        self.audio_snapshot = Some(snap);
                    }
                    Err(err) => {
                        self.edit_error = Some(err);
                    }
                }
                Task::none()
            }

            Message::CopyCollabContext => {
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let ctx = SharedFocusContext::capture(
                    proj,
                    self.playhead_frame,
                    self.loop_range,
                    &self.selected_notes,
                    self.selected_node_id.as_deref(),
                    &self.muted_tracks,
                    &self.solo_tracks,
                    &self.plugin_slots,
                    &self.collab_pins,
                    self.audio_snapshot.as_ref(),
                    self.analysis.as_ref(),
                    &self.collab_chat,
                );
                let draft = if self.collab_prompt_input.trim().is_empty() {
                    "(Describe what you want to change here)"
                } else {
                    self.collab_prompt_input.trim()
                };
                let prompt = build_agent_prompt(&ctx, draft);
                match copy_to_clipboard(&prompt) {
                    Ok(()) => {
                        self.last_reload_note = Some(
                            "Copied shared Studio context + audio telemetry to clipboard".into(),
                        );
                    }
                    Err(err) => {
                        self.edit_error = Some(err);
                    }
                }
                Task::none()
            }

            Message::CollabPromptChanged(val) => {
                self.collab_prompt_input = val;
                Task::none()
            }

            Message::SendCollabPrompt(preset) => {
                if self.agent_running || self.editing {
                    return Task::none();
                }
                let instruction = match preset {
                    Some(text) => text,
                    None => {
                        let t = self.collab_prompt_input.trim().to_owned();
                        if t.is_empty() {
                            return Task::none();
                        }
                        self.collab_prompt_input.clear();
                        t
                    }
                };
                let Some(proj) = self.project.clone() else {
                    return Task::none();
                };
                let Some(agent) = self
                    .selected_agent
                    .and_then(|k| self.detected_agents.iter().find(|a| a.kind == k).cloned())
                    .or_else(|| self.detected_agents.first().cloned())
                else {
                    self.edit_error = Some(
                        "No local agent CLI found (claude / agy / codex / gemini). Use 'Copy Context' to collaborate with an external agent.".into(),
                    );
                    return Task::none();
                };
                let Some(before_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned()
                else {
                    return Task::none();
                };

                let ctx = SharedFocusContext::capture(
                    &proj,
                    self.playhead_frame,
                    self.loop_range,
                    &self.selected_notes,
                    self.selected_node_id.as_deref(),
                    &self.muted_tracks,
                    &self.solo_tracks,
                    &self.plugin_slots,
                    &self.collab_pins,
                    self.audio_snapshot.as_ref(),
                    self.analysis.as_ref(),
                    &self.collab_chat,
                );
                let prompt = build_agent_prompt(&ctx, &instruction);
                let prompt_label: String = instruction.chars().take(48).collect();

                self.collab_chat.push(ChatMessage {
                    role: ChatRole::User,
                    text: instruction,
                    detail: Some(format!(
                        "{} · {}",
                        ctx.playhead_bar,
                        ctx.loop_label.as_deref().unwrap_or("No loop")
                    )),
                });
                self.persist_collab_file();
                self.agent_running = true;
                self.edit_error = None;
                self.last_reload_note = Some(format!(
                    "Running {} co-producer agent...",
                    agent.kind.label()
                ));

                // Suppress file watcher while our in-app agent is editing so we handle A/B cleanly.
                self.last_self_save = Some(Instant::now());
                let before_src_clone = before_source.clone();
                let before_proj_box = Box::new(proj.clone());
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            run_agent_turn(&agent, &proj, &before_src_clone, &prompt).map(Box::new)
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("agent runner thread panicked".into()))
                    },
                    move |result| Message::AgentTurnFinished {
                        before_source: before_source.clone(),
                        before_project: before_proj_box.clone(),
                        prompt_label: prompt_label.clone(),
                        result,
                    },
                )
            }

            Message::AgentTurnFinished {
                before_source,
                before_project,
                prompt_label,
                result,
            } => {
                self.agent_running = false;
                self.last_self_save = Some(Instant::now());
                self.pending_watch_at = None;
                match result {
                    Ok(outcome) => {
                        for pid in &outcome.resolved_pins {
                            if let Some(pin) = self.collab_pins.iter_mut().find(|p| p.id == *pid) {
                                pin.resolved = true;
                            }
                        }
                        if let Some(err) = outcome.compile_error {
                            self.edit_error = Some(format!("Agent edit compile error: {err}"));
                            self.collab_chat.push(ChatMessage {
                                role: ChatRole::Agent,
                                text: outcome.reply_text,
                                detail: Some(format!(
                                    "Compile error ({:.1}s) — file kept for review",
                                    outcome.elapsed_ms as f64 / 1000.0
                                )),
                            });
                            self.persist_collab_file();
                            return Task::none();
                        }

                        if let Some((after_source, after_project, highlight)) =
                            outcome.updated_project
                        {
                            let summary = highlight.summary_line(&after_project);
                            let jump_span = highlight.bounding_span;
                            self.collab_chat.push(ChatMessage {
                                role: ChatRole::Agent,
                                text: outcome.reply_text,
                                detail: Some(format!(
                                    "{summary} · {:.1}s",
                                    outcome.elapsed_ms as f64 / 1000.0
                                )),
                            });

                            let diff = compute_line_diff(&before_source, &after_source, 2);
                            if !diff.is_empty() {
                                self.external_change = Some(ExternalChange {
                                    prev_source: before_source.clone(),
                                    diff,
                                });
                            }

                            let ab = AbComparison {
                                label: prompt_label,
                                before_source,
                                before_project: *before_project,
                                after_source: after_source.clone(),
                                after_project: after_project.clone(),
                                active_side: AbSide::After,
                                highlight,
                            };
                            self.ab_comparison = Some(ab);
                            self.saved_source = Some(after_source);
                            self.apply_ab_project(after_project);
                            if self.auto_loop_ai_edit {
                                if let Some(span) = jump_span {
                                    self.apply_loop(Some(span));
                                    self.playhead_frame = span.start;
                                    if let Some(pb) = &self.playback {
                                        pb.seek(span.start);
                                    }
                                }
                            }
                            self.last_reload_note =
                                Some(format!("AI Edit ready (B: After) · {summary}"));
                            self.persist_collab_file();
                            return self.update(Message::RefreshAudioSnapshot);
                        } else {
                            self.collab_chat.push(ChatMessage {
                                role: ChatRole::Agent,
                                text: outcome.reply_text,
                                detail: Some(format!(
                                    "No file edits · {:.1}s",
                                    outcome.elapsed_ms as f64 / 1000.0
                                )),
                            });
                            self.last_reload_note = Some("Agent replied (no file changes)".into());
                            self.persist_collab_file();
                        }
                    }
                    Err(err) => {
                        self.edit_error = Some(err.clone());
                        self.collab_chat.push(ChatMessage {
                            role: ChatRole::System,
                            text: format!("Agent error: {err}"),
                            detail: None,
                        });
                        self.persist_collab_file();
                    }
                }
                Task::none()
            }

            Message::SwitchAbSide(side) => {
                let target_proj = if let Some(ab) = &mut self.ab_comparison {
                    if ab.active_side == side {
                        return Task::none();
                    }
                    ab.active_side = side;
                    match side {
                        AbSide::Before => Some(ab.before_project.clone()),
                        AbSide::After => Some(ab.after_project.clone()),
                    }
                } else {
                    None
                };
                if let Some(proj) = target_proj {
                    self.apply_ab_project(proj);
                    self.last_reload_note = Some(match side {
                        AbSide::Before => "Auditioning A: Before AI Edit (live)".into(),
                        AbSide::After => "Auditioning B: After AI Edit (live)".into(),
                    });
                }
                Task::none()
            }

            Message::JumpAndLoopAiEdit => {
                if let Some(span) = self
                    .ab_comparison
                    .as_ref()
                    .and_then(|ab| ab.highlight.bounding_span)
                {
                    self.apply_loop(Some(span));
                    self.playhead_frame = span.start;
                    if let Some(pb) = &self.playback {
                        pb.seek(span.start);
                    }
                    self.last_reload_note = Some("Looped AI-edited section".into());
                }
                Task::none()
            }

            Message::AcceptAiEdit => {
                let Some(ab) = self.ab_comparison.take() else {
                    return Task::none();
                };
                self.undo_stack.push(ab.before_source);
                self.redo_stack.clear();
                self.saved_source = Some(ab.after_source.clone());
                self.external_change = None;
                self.last_self_save = Some(Instant::now());
                let _ = std::fs::write(&ab.after_project.entry_path, &ab.after_source);
                if ab.active_side != AbSide::After {
                    self.apply_ab_project(ab.after_project);
                }
                self.last_reload_note = Some("Accepted AI edit (Cmd+Z still undoes)".into());
                Task::none()
            }

            Message::RejectAiEdit => {
                let Some(ab) = self.ab_comparison.take() else {
                    return Task::none();
                };
                self.last_self_save = Some(Instant::now());
                self.pending_watch_at = None;
                let _ = std::fs::write(&ab.before_project.entry_path, &ab.before_source);
                self.saved_source = Some(ab.before_source);
                self.external_change = None;
                self.apply_ab_project(ab.before_project);
                self.last_reload_note = Some("Reverted to A: Before AI Edit".into());
                Task::none()
            }

            Message::NodeParamDraft {
                node_id,
                param_name,
                value,
            } => {
                if let Some(pb) = &self.playback {
                    pb.set_live_param(node_id.clone(), param_name.clone(), f64::from(value));
                }
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
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, summary) = apply_node_param_edit(
                                &bundle,
                                &node_id,
                                &param_name,
                                unit,
                                value,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
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
                let base_plan = proj.plan.clone();
                let entry_path = proj.entry_path.clone();
                let project_root = proj.project_root.clone();
                let limits = proj.limits;
                self.editing = true;
                self.edit_error = None;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let started = Instant::now();
                            let (edited, summary) = apply_node_param_reset(
                                &bundle,
                                &node_id,
                                &param_name,
                                &limits,
                                Some(&base_plan),
                            )?;
                            let mut recompiled =
                                load_from_edited(entry_path, project_root, edited)?;
                            recompiled.compile_ms = started.elapsed().as_millis() as u64;
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

            Message::PluginScanReady(catalog) => {
                self.plugin_catalog = catalog;
                if let Some(proj) = &self.project {
                    let is_empty = self
                        .host_rack
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_empty();
                    if is_empty {
                        if let Ok(Some(rf)) = load_rack_file(&proj.entry_path) {
                            let warnings = self
                                .host_rack
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .load_rack_file(&rf, &self.plugin_catalog);
                            self.refresh_plugin_slots();
                            if !warnings.is_empty() {
                                self.edit_error = Some(warnings.join("; "));
                            }
                        }
                    }
                }
                Task::none()
            }

            Message::OpenPluginPicker { target, kind } => {
                self.plugin_picker = Some((target, kind));
                self.plugin_search.clear();
                self.show_sidebar = true;
                Task::none()
            }

            Message::ClosePluginPicker => {
                self.plugin_picker = None;
                self.plugin_search.clear();
                Task::none()
            }

            Message::PluginSearchChanged(q) => {
                self.plugin_search = q;
                Task::none()
            }

            Message::AddPluginSlot { target, descriptor } => {
                let res = {
                    let mut rack = self
                        .host_rack
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    rack.add_slot(target, &descriptor)
                };
                match res {
                    Ok(slot_id) => {
                        self.plugin_picker = None;
                        self.plugin_search.clear();
                        self.selected_plugin_slot = Some(slot_id);
                        self.edit_error = None;
                        self.refresh_plugin_slots();
                        self.last_reload_note = Some(format!(
                            "Loaded [{}] {} ({})",
                            descriptor.format.label(),
                            descriptor.name,
                            descriptor.kind.label()
                        ));
                    }
                    Err(err) => {
                        self.edit_error = Some(format!("Plugin load failed: {err}"));
                    }
                }
                Task::none()
            }

            Message::SelectPluginSlot(slot_id) => {
                self.selected_plugin_slot = slot_id;
                if slot_id.is_some() {
                    self.show_sidebar = true;
                }
                Task::none()
            }

            Message::TogglePluginNativeUi(slot_id) => {
                let res = self
                    .host_rack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .toggle_native_ui(slot_id);
                match res {
                    Ok(opened) => {
                        self.edit_error = None;
                        self.selected_plugin_slot = Some(slot_id);
                        self.last_reload_note = Some(if opened {
                            "Opened native plugin UI window".into()
                        } else {
                            "Closed native plugin UI window".into()
                        });
                    }
                    Err(err) => {
                        self.edit_error = Some(err);
                    }
                }
                self.refresh_plugin_slots();
                Task::none()
            }

            Message::TogglePluginBypass(slot_id) => {
                self.host_rack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .toggle_bypass(slot_id);
                self.refresh_plugin_slots();
                Task::none()
            }

            Message::RemovePluginSlot(slot_id) => {
                self.host_rack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove_slot(slot_id);
                if self.selected_plugin_slot == Some(slot_id) {
                    self.selected_plugin_slot = None;
                }
                self.refresh_plugin_slots();
                Task::none()
            }

            Message::PluginParamChanged {
                slot_id,
                param_id,
                value,
            } => {
                self.host_rack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_param(slot_id, param_id, value);
                if let Some(slot) = self.plugin_slots.iter_mut().find(|s| s.slot_id == slot_id) {
                    if let Some(entry) =
                        slot.param_values.iter_mut().find(|(id, _)| *id == param_id)
                    {
                        entry.1 = value;
                    }
                }
                Task::none()
            }

            Message::FreezePluginTarget(target) => {
                if self.freezing || self.editing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
                    return Task::none();
                };
                let rack_file = self
                    .host_rack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .to_rack_file();
                let bundle = proj.bundle.clone();
                let plan = proj.plan.clone();
                let limits = proj.limits;
                let project_root = proj.project_root.clone();
                let catalog = self.plugin_catalog.clone();
                let span = self.loop_range;
                let target_clone = target.clone();
                self.freezing = true;
                self.edit_error = None;
                self.last_reload_note = Some("Freezing plugin to pure .maac PCM...".into());
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            freeze_target_to_maac(
                                &bundle,
                                &plan,
                                &limits,
                                &project_root,
                                &rack_file,
                                &catalog,
                                &target_clone,
                                span,
                            )
                            .map(Box::new)
                        });
                        handle
                            .join()
                            .unwrap_or_else(|_| Err("freeze worker panicked".into()))
                    },
                    move |result| Message::FreezeFinished {
                        prev_source: prev_source.clone(),
                        target: target.clone(),
                        result,
                    },
                )
            }

            Message::FreezeFinished {
                prev_source,
                target,
                result,
            } => {
                self.freezing = false;
                match result {
                    Ok(outcome) => {
                        let Some(proj) = &self.project else {
                            return Task::none();
                        };
                        let entry_path = proj.entry_path.clone();
                        let project_root = proj.project_root.clone();
                        let summary = format!(
                            "{} ({}, {} KiB)",
                            outcome.summary,
                            outcome.frozen_node_id,
                            outcome.bytes / 1024
                        );
                        match load_from_edited(entry_path, project_root, outcome.edited) {
                            Ok(recompiled) => {
                                {
                                    let mut rack = self
                                        .host_rack
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    for slot in rack.summaries() {
                                        let matches_target = match &target {
                                            SlotTarget::MasterInsert => true,
                                            SlotTarget::TrackInstrument(n)
                                            | SlotTarget::NodeInsert(n) => matches!(
                                                &slot.target,
                                                SlotTarget::TrackInstrument(id)
                                                    | SlotTarget::NodeInsert(id)
                                                    if id == n
                                            ),
                                        };
                                        if matches_target {
                                            rack.set_bypassed(slot.slot_id, true);
                                        }
                                    }
                                }
                                self.refresh_plugin_slots();
                                self.undo_stack.push(prev_source);
                                if self.undo_stack.len() > 100 {
                                    self.undo_stack.remove(0);
                                }
                                self.redo_stack.clear();
                                self.edit_error = None;
                                return self.apply_in_memory_project(recompiled, summary);
                            }
                            Err(err) => {
                                self.edit_error = Some(format!("Freeze load failed: {err}"));
                            }
                        }
                    }
                    Err(err) => {
                        self.edit_error = Some(format!("Freeze failed: {err}"));
                    }
                }
                Task::none()
            }

            Message::OpenSamplerImportDialog => {
                if self.editing || self.project.is_none() {
                    return Task::none();
                }
                let dir = self
                    .project
                    .as_ref()
                    .map(|p| p.project_root.clone())
                    .unwrap_or_else(|| PathBuf::from("."));
                Task::perform(
                    async move {
                        rfd::FileDialog::new()
                            .set_directory(dir)
                            .add_filter(
                                "Sample Instruments (SF2 / SFZ)",
                                &["sf2", "SF2", "sfz", "SFZ"],
                            )
                            .pick_file()
                    },
                    Message::SamplerFilePicked,
                )
            }

            Message::SamplerFilePicked(None) => Task::none(),

            Message::SamplerFilePicked(Some(path)) => {
                let default_track = self.selected_node_id.clone().or_else(|| {
                    self.project.as_ref().and_then(|p| {
                        p.tracks
                            .iter()
                            .find(|t| t.note_count > 0)
                            .map(|t| t.name.clone())
                    })
                });
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default();
                if ext == "sf2" {
                    match list_sf2_presets(&path) {
                        Ok(presets) if presets.len() > 1 => {
                            self.sf2_picker = Some((path, presets, default_track));
                            self.show_sidebar = true;
                            Task::none()
                        }
                        Ok(presets) if presets.len() == 1 => {
                            let p = &presets[0];
                            self.spawn_sampler_import(
                                path,
                                Some((p.bank, p.program)),
                                default_track,
                            )
                        }
                        Ok(_) => {
                            self.edit_error = Some("SF2 file contains no presets".into());
                            Task::none()
                        }
                        Err(err) => {
                            self.edit_error = Some(err);
                            Task::none()
                        }
                    }
                } else {
                    self.spawn_sampler_import(path, None, default_track)
                }
            }

            Message::SelectSf2TargetTrack(track) => {
                if let Some((_, _, target)) = self.sf2_picker.as_mut() {
                    *target = track;
                }
                Task::none()
            }

            Message::ImportSf2Preset { bank, program } => {
                let Some((path, _, target)) = self.sf2_picker.take() else {
                    return Task::none();
                };
                self.spawn_sampler_import(path, Some((bank, program)), target)
            }

            Message::CloseSf2Picker => {
                self.sf2_picker = None;
                Task::none()
            }

            Message::SamplerImported {
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
                        return self.apply_in_memory_project(*data, summary);
                    }
                    Err(err) => {
                        self.edit_error = Some(format!("Sampler import failed: {err}"));
                    }
                }
                Task::none()
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
                let Some(proj) = self.project.clone() else {
                    return Task::none();
                };
                let muted = self.effective_muted_nodes(&proj);
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
                            let plan = if muted.is_empty() {
                                proj.plan.clone()
                            } else {
                                proj.plan_with_muted(&muted)?
                            };
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
                let Some(proj) = self.project.clone() else {
                    return Task::none();
                };
                self.analyzing = true;
                self.analysis_error = None;
                let muted = self.effective_muted_nodes(&proj);
                let meter = proj.meter.clone();
                let limits = proj.limits;
                Task::perform(
                    async move {
                        let handle = std::thread::spawn(move || {
                            let plan = if muted.is_empty() {
                                proj.plan.clone()
                            } else {
                                proj.plan_with_muted(&muted)?
                            };
                            run_analysis(plan, meter, limits)
                        });
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
                TimelineAction::SelectHit(addr) => self.update(Message::SelectHit(Some(addr))),
                TimelineAction::DragHitVelocity {
                    source_path,
                    velocity,
                } => self.update(Message::ApplyNoteEdit {
                    source_path,
                    edit: NoteEdit::SetVelocity(velocity),
                }),
                TimelineAction::AddHitAt { frame, velocity } => {
                    self.update(Message::AddHitAt { frame, velocity })
                }
                TimelineAction::SelectAutoPoint {
                    lane_idx,
                    point_idx,
                } => self.update(Message::SelectAutoPoint {
                    lane_idx,
                    point_idx,
                }),
                TimelineAction::DragAutoPointValue {
                    lane_idx,
                    point_idx,
                    value,
                } => self.update(Message::ApplyAutoPointEdit {
                    lane_idx,
                    point_idx,
                    value: Some(value),
                    shape: None,
                }),
                TimelineAction::AddAutoPoint {
                    lane_idx,
                    frame,
                    value,
                } => self.update(Message::AddAutoPoint {
                    lane_idx,
                    frame,
                    value,
                }),
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
                TimelineAction::JumpToPin(pin_id) => self.update(Message::JumpToReviewPin(pin_id)),
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

                // 1b. Pump hosted plugin main-thread callbacks & floating Cocoa UI windows.
                let poll_res = self
                    .host_rack
                    .try_lock()
                    .ok()
                    .map(|mut rack| rack.poll_main_thread());
                if let Some((any_ui_open, changed)) = poll_res {
                    if changed {
                        self.refresh_plugin_slots();
                    } else if any_ui_open {
                        self.plugin_ui_sync_tick = self.plugin_ui_sync_tick.wrapping_add(1);
                        if self.plugin_ui_sync_tick.is_multiple_of(6) {
                            self.refresh_plugin_slots();
                        }
                    }
                }

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
            self.selected_hit_address = None;
            self.selected_auto_point = None;
            self.auto_value_draft = None;
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
        if let Some((li, pi)) = self.selected_auto_point {
            if let Some(pt) = proj
                .automations
                .get(li)
                .and_then(|l| l.points.iter().find(|p| p.index == pi))
            {
                self.auto_value_draft = Some(pt.value);
            } else {
                self.selected_auto_point = None;
                self.auto_value_draft = None;
            }
        }
        if let Some(hit_addr) = self.selected_hit_address.as_deref() {
            if let Some(hit) = proj.hits.iter().find(|h| h.address == hit_addr) {
                let info = inspect_note_in_bundle(&proj.bundle, &hit.source_path);
                self.velocity_draft = info
                    .as_ref()
                    .and_then(|i| i.velocity)
                    .or(Some(hit.velocity));
                self.selected_note_info = info;
                self.refresh_code_cache();
                return;
            } else {
                self.selected_hit_address = None;
            }
        }
        let Some(addr) = self.selected_note_address.as_deref() else {
            self.selected_note_info = None;
            self.velocity_draft = None;
            self.refresh_code_cache();
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

        let auto_curve_path = self.selected_auto_point.and_then(|(li, _)| {
            proj.automations
                .get(li)
                .and_then(|l| l.curve_id.clone())
                .map(|cid| vec![cid])
        });

        // Highlight priority: selected note/hit -> selected automation curve -> selected mixer/FX node.
        let target_path: Option<Vec<String>> = if !self.show_mixer {
            self.selected_note_info
                .as_ref()
                .map(|i| i.path.clone())
                .or(auto_curve_path)
                .or_else(|| self.selected_node_id.clone().map(|id| vec![id]))
        } else {
            self.selected_node_id
                .clone()
                .map(|id| vec![id])
                .or_else(|| self.selected_note_info.as_ref().map(|i| i.path.clone()))
                .or(auto_curve_path)
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
        self.last_reload_note = Some(status_msg);
        if let Some(session) = &self.session {
            session.update(&data.plan);
            let target_gen = session.status().generation;
            if let Some(pb) = &self.playback {
                pb.clear_live_params_from_generation(target_gen);
            }
        }
        self.project = Some(data);
        self.apply_live_mutes();
        self.refresh_selected_note_info();
        Task::none()
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

    fn apply_live_mutes(&self) {
        let (Some(proj), Some(pb)) = (&self.project, &self.playback) else {
            return;
        };
        pb.set_muted_nodes(self.effective_muted_nodes(proj));
    }

    fn refresh_plugin_slots(&mut self) {
        self.plugin_slots = self
            .host_rack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .summaries();
    }

    fn spawn_sampler_import(
        &mut self,
        path: PathBuf,
        sf2_preset: Option<(u16, u16)>,
        target_track: Option<String>,
    ) -> Task<Message> {
        let Some(proj) = &self.project else {
            return Task::none();
        };
        let Some(prev_source) = proj.bundle.sources.get(&proj.bundle.entry).cloned() else {
            return Task::none();
        };
        let bundle = proj.bundle.clone();
        let base_plan = proj.plan.clone();
        let entry_path = proj.entry_path.clone();
        let project_root = proj.project_root.clone();
        let limits = proj.limits;
        self.editing = true;
        self.edit_error = None;
        self.last_reload_note = Some("Importing SF2/SFZ instrument into MaaC library...".into());
        Task::perform(
            async move {
                let handle = std::thread::spawn(move || {
                    let started = Instant::now();
                    let inst = match sf2_preset {
                        Some((bank, program)) => parse_sf2_preset(&path, bank, program)?,
                        None => parse_sfz(&path)?,
                    };
                    let (summary, lib_src, lib_assets) =
                        emit_maac_library(inst, &project_root, &limits)?;
                    let (edited, action_desc) = wire_imported_library_into_bundle(
                        &bundle,
                        &summary,
                        lib_src,
                        lib_assets,
                        target_track.as_deref(),
                        &limits,
                        Some(&base_plan),
                    )?;
                    let mut recompiled = load_from_edited(entry_path, project_root, edited)?;
                    recompiled.compile_ms = started.elapsed().as_millis() as u64;
                    let warn_suffix = if summary.warnings.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", summary.warnings.join("; "))
                    };
                    let note = format!(
                        "{action_desc} ({} smp / {}/{} zones, {} vals, {} ms){warn_suffix}",
                        summary.emitted_samples,
                        summary.emitted_zones,
                        summary.original_zones,
                        summary.total_values,
                        recompiled.compile_ms
                    );
                    Ok((Box::new(recompiled), note))
                });
                handle
                    .join()
                    .unwrap_or_else(|_| Err("sampler import worker panicked".into()))
            },
            move |res| match res {
                Ok((data, summary)) => Message::SamplerImported {
                    prev_source: prev_source.clone(),
                    summary,
                    result: Ok(data),
                },
                Err(err) => Message::SamplerImported {
                    prev_source: prev_source.clone(),
                    summary: String::new(),
                    result: Err(err),
                },
            },
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
                        if let Some(prev_proj) = self.project.clone() {
                            let highlight =
                                diff_projects_for_highlight(&prev_proj, &data, &before, after);
                            self.ab_comparison = Some(AbComparison {
                                label: "External / AI Edit".into(),
                                before_source: before.clone(),
                                before_project: prev_proj,
                                after_source: after.to_owned(),
                                after_project: data.clone(),
                                active_side: AbSide::After,
                                highlight,
                            });
                        }
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
            self.last_reload_note = Some(format!("Reloaded in {compile_ms} ms — live update sent"));
            if let Some(session) = &self.session {
                session.update(&data.plan);
                let target_gen = session.status().generation;
                if let Some(pb) = &self.playback {
                    pb.clear_live_params_from_generation(target_gen);
                }
            }
            self.project = Some(data);
            self.apply_live_mutes();
            self.refresh_selected_note_info();
            return Task::none();
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
        self.selected_hit_address = None;
        self.selected_auto_point = None;
        self.auto_value_draft = None;
        self.active_auto_lane = 0;
        self.selected_note_info = None;
        self.velocity_draft = None;
        self.selected_node_id = None;
        self.param_drafts.clear();
        self.selected_plugin_slot = None;
        self.plugin_picker = None;
        self.sf2_picker = None;
        self.edit_error = None;
        self.ab_comparison = None;
        self.audio_snapshot = None;
        self.audio_snapshot_busy = false;
        if let Ok(Some(cf)) = load_collab_file(&data.entry_path) {
            self.collab_pins = cf.pins;
            self.next_pin_id = cf
                .next_pin_id
                .max(self.collab_pins.iter().map(|p| p.id + 1).max().unwrap_or(1));
            self.collab_chat = cf.chat;
        } else {
            self.collab_pins.clear();
            self.next_pin_id = 1;
            self.collab_chat.clear();
        }
        {
            let mut rack = self
                .host_rack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rack.clear();
            rack.set_format(
                f64::from(data.sample_rate_hz),
                PLAYBACK_CHUNK_FRAMES as usize,
            );
            if !self.plugin_catalog.is_empty() {
                if let Ok(Some(rf)) = load_rack_file(&data.entry_path) {
                    let warnings = rack.load_rack_file(&rf, &self.plugin_catalog);
                    if !warnings.is_empty() {
                        self.edit_error = Some(warnings.join("; "));
                    }
                }
            }
        }
        self.refresh_plugin_slots();

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
        self.last_reload_note = Some(format!("Loaded in {compile_ms} ms"));
        self.project = Some(data);
        self.refresh_code_cache();

        self.ensure_playback_started();
        if was_playing {
            self.audio.set_playing(true);
        }
        Task::none()
    }

    fn persist_collab_file(&self) {
        let Some(proj) = &self.project else {
            return;
        };
        let file = CollabFile {
            version: 1,
            next_pin_id: self.next_pin_id,
            pins: self.collab_pins.clone(),
            chat: self.collab_chat.clone(),
        };
        let _ = save_collab_file(&proj.entry_path, &file);
    }

    fn apply_ab_project(&mut self, data: ProjectData) {
        if self.visible_tracks.len() != data.tracks.len() {
            self.visible_tracks = vec![true; data.tracks.len()];
        }
        if let Some(session) = &self.session {
            session.update(&data.plan);
            let target_gen = session.status().generation;
            if let Some(pb) = &self.playback {
                pb.clear_live_params_from_generation(target_gen);
            }
        }
        self.project = Some(data);
        self.apply_live_mutes();
        self.refresh_selected_note_info();
        self.refresh_code_cache();
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
            lead_frames: PLAYBACK_LEAD_FRAMES,
            chunk_frames: PLAYBACK_CHUNK_FRAMES,
            pause_prerender: false,
            preview_preroll_frames: proj.sample_rate_hz as u64 / 4,
            approximate_start: true,
            render_threads: Some(4),
            loop_range: self.loop_range,
            edit_debounce: PLAYBACK_EDIT_DEBOUNCE,
        };
        match session.play(start, config) {
            Ok(pb) => {
                pb.set_muted_nodes(self.effective_muted_nodes(proj));
                pb.set_host_processor(Some(self.host_rack.clone()));
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
            {
                let collab_on = self.show_sidebar && self.show_collab;
                let open_pins = self.collab_pins.iter().filter(|p| !p.resolved).count();
                let ab_active = self.ab_comparison.is_some();
                let collab_lbl = if self.agent_running {
                    "AI · Thinking...".to_owned()
                } else if ab_active {
                    "AI · A/B!".to_owned()
                } else if open_pins > 0 {
                    format!("AI Collab ({open_pins})")
                } else {
                    "AI Collab (G)".to_owned()
                };
                button(text(collab_lbl).size(11.5))
                    .padding([3, 8])
                    .style(move |theme, status| {
                        if ab_active {
                            accent_button_style(Color::from_rgb(0.14, 0.62, 0.68), theme, status)
                        } else if collab_on {
                            accent_button_style(Color::from_rgb(0.16, 0.54, 0.62), theme, status)
                        } else {
                            studio_button_style(theme, status)
                        }
                    })
                    .on_press(Message::ToggleCollabPanel)
            },
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        if !proj.automations.is_empty() {
            let auto_on = self.show_automation;
            zoom_controls = zoom_controls.push(
                button(text("Auto").size(11.5))
                    .padding([3, 7])
                    .style(move |theme, status| {
                        if auto_on {
                            accent_button_style(Color::from_rgb(0.76, 0.54, 0.16), theme, status)
                        } else {
                            studio_button_style(theme, status)
                        }
                    })
                    .on_press(Message::ToggleAutomation),
            );
        }

        zoom_controls = zoom_controls
            .push(
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
            )
            .push(
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
            )
            .push(Space::with_width(4))
            .push(
                button(text("-").size(12))
                    .padding([3, 7])
                    .style(studio_button_style)
                    .on_press(Message::ZoomDelta {
                        delta: -1.0,
                        anchor: 0.5,
                    }),
            )
            .push(
                button(text("Fit").size(11.5))
                    .padding([3, 7])
                    .style(studio_button_style)
                    .on_press(Message::ZoomFit),
            )
            .push(
                button(text("+").size(12))
                    .padding([3, 7])
                    .style(studio_button_style)
                    .on_press(Message::ZoomDelta {
                        delta: 1.0,
                        anchor: 0.5,
                    }),
            )
            .push(
                text(format!("{:.1}x", self.zoom))
                    .size(11)
                    .color(Color::from_rgb(0.66, 0.72, 0.80)),
            )
            .push(Space::with_width(4))
            .push(
                button(text("Recompile").size(11.5))
                    .padding([3, 8])
                    .style(studio_button_style)
                    .on_press(Message::RecompileNow),
            );

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
            let selected_source_path = self
                .selected_note_address
                .as_deref()
                .and_then(|addr| {
                    proj.notes
                        .iter()
                        .find(|n| n.address == addr)
                        .map(|n| n.source_path.as_slice())
                })
                .or_else(|| {
                    self.selected_hit_address.as_deref().and_then(|addr| {
                        proj.hits
                            .iter()
                            .find(|h| h.address == addr)
                            .map(|h| h.source_path.as_slice())
                    })
                });

            Element::from(
                canvas(TimelineView {
                    project: proj,
                    visible_tracks: &self.visible_tracks,
                    audible_tracks,
                    selected_notes: &self.selected_notes,
                    selected_source_path,
                    selected_hit_address: self.selected_hit_address.as_deref(),
                    show_automation: self.show_automation,
                    active_auto_lane: self.active_auto_lane,
                    selected_auto_point: self.selected_auto_point,
                    playhead_frame: self.playhead_frame,
                    checkpoint_frame: self.checkpoint_frame,
                    loop_range: self.loop_range,
                    snap_to_bar: self.snap_to_bar,
                    approximate: self.approximate,
                    stale: self.stale,
                    zoom: self.zoom,
                    scroll: self.scroll,
                    review_pins: &self.collab_pins,
                    ai_highlight: self.ab_comparison.as_ref().map(|ab| &ab.highlight),
                })
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .map(Message::Timeline)
        };

        let mut left_col = column![sub_bar]
            .width(Length::FillPortion(3))
            .height(Length::Fill);

        if let Some(ab) = &self.ab_comparison {
            left_col = left_col.push(self.view_ab_banner(proj, ab));
        }

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

        let open_pins = self.collab_pins.iter().filter(|p| !p.resolved).count();
        let collab_tab_lbl = if self.agent_running {
            "AI Collab ●".to_owned()
        } else if open_pins > 0 {
            format!("AI Collab ({open_pins})")
        } else {
            "AI Collab".to_owned()
        };
        let is_collab = self.show_collab;
        let mode_tabs = row![
            button(text("Inspector").size(11))
                .padding([4, 8])
                .width(Length::FillPortion(1))
                .style(move |t, s| {
                    if !is_collab {
                        accent_button_style(Color::from_rgb(0.26, 0.44, 0.72), t, s)
                    } else {
                        studio_button_style(t, s)
                    }
                })
                .on_press(Message::SetSidebarCollab(false)),
            button(text(collab_tab_lbl).size(11))
                .padding([4, 8])
                .width(Length::FillPortion(1))
                .style(move |t, s| {
                    if is_collab {
                        accent_button_style(Color::from_rgb(0.14, 0.56, 0.64), t, s)
                    } else {
                        studio_button_style(t, s)
                    }
                })
                .on_press(Message::SetSidebarCollab(true)),
        ]
        .spacing(6);
        col = col.push(mode_tabs);

        if self.show_collab {
            return self.view_collab_sidebar(proj, col);
        }

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
        } else if let Some(hit) = self
            .selected_hit_address
            .as_deref()
            .and_then(|addr| proj.hits.iter().find(|h| h.address == addr))
        {
            let track_name = proj
                .tracks
                .get(hit.track_idx)
                .map(|t| t.name.as_str())
                .unwrap_or("drums");
            let track_col = track_color(hit.track_idx);
            let busy = self.editing;
            let mut header = row![
                text("SELECTED HIT")
                    .size(10.5)
                    .color(Color::from_rgb(0.58, 0.66, 0.78)),
                Space::with_width(6),
                text(format!("{track_name} · {}", hit.key))
                    .size(11)
                    .color(track_col),
                horizontal_space(),
            ]
            .align_y(Alignment::Center);
            if hit.shared_occurrences > 1 {
                header = header.push(
                    container(
                        text(format!("×{}", hit.shared_occurrences))
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

            let path_label = if hit.source_path.is_empty() {
                hit.address.clone()
            } else {
                hit.source_path.join(" / ")
            };
            let pos_label = format!(
                "{} · {}",
                proj.format_bar_position(hit.on_frame),
                format_time(hit.on_frame, proj.sample_rate_hz)
            );
            let mut card = Column::new().spacing(6).push(header).push(
                text(format!("{path_label}   ({pos_label})"))
                    .size(11)
                    .color(Color::from_rgb(0.76, 0.82, 0.90)),
            );

            if let Some(info) = &self.selected_note_info {
                let sp = info.path.clone();
                if let Some(keys) = proj.track_hit_keys.get(&hit.track_idx) {
                    let mut key_row = Row::new().spacing(4).align_y(Alignment::Center);
                    for k in keys {
                        let active = info.hit_key.as_deref() == Some(k.as_str());
                        let path = sp.clone();
                        let k_owned = k.clone();
                        key_row = key_row.push(
                            button(text(k).size(10))
                                .padding([2, 6])
                                .style(move |t, s| {
                                    if active {
                                        accent_button_style(Color::from_rgb(0.24, 0.62, 0.44), t, s)
                                    } else {
                                        studio_button_style(t, s)
                                    }
                                })
                                .on_press_maybe((!busy).then_some(Message::ApplyNoteEdit {
                                    source_path: path,
                                    edit: NoteEdit::SetHitKey(k_owned),
                                })),
                        );
                    }
                    card = card.push(scrollable(key_row).direction(
                        scrollable::Direction::Horizontal(
                            scrollable::Scrollbar::new().width(3).scroller_width(3),
                        ),
                    ));
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

                let vel_val = self
                    .velocity_draft
                    .or(info.velocity)
                    .unwrap_or(hit.velocity);
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

                if hit.shared_occurrences > 1 {
                    card = card.push(
                        row![
                            text(format!("Shared ×{} occurrences", hit.shared_occurrences))
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
            }

            col = col.push(container(card).padding(9).width(Length::Fill).style(|_| {
                container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.112, 0.138, 0.150))),
                    border: Border {
                        color: Color::from_rgba(0.45, 0.86, 0.62, 0.40),
                        width: 1.0,
                        radius: 5.0.into(),
                    },
                    ..Default::default()
                }
            }));
        } else if let Some((lane_idx, point_idx)) = self.selected_auto_point {
            if let Some(lane) = proj.automations.get(lane_idx) {
                if let Some(pt) = lane.points.iter().find(|p| p.index == point_idx) {
                    let busy = self.editing;
                    let accent = lane
                        .track_idx
                        .map(track_color)
                        .unwrap_or(Color::from_rgb(0.96, 0.76, 0.30));
                    let header = row![
                        text("AUTOMATION POINT")
                            .size(10.5)
                            .color(Color::from_rgb(0.58, 0.66, 0.78)),
                        Space::with_width(6),
                        text(format!("{}.{}", lane.target_node, lane.target_param))
                            .size(11)
                            .color(accent),
                        horizontal_space(),
                        button(text("×").size(11))
                            .padding([1, 6])
                            .style(studio_button_style)
                            .on_press(Message::ClearSelection),
                    ]
                    .align_y(Alignment::Center);

                    let unit_lbl = if lane.clock_is_seconds { "s" } else { "q" };
                    let mut card = Column::new().spacing(6).push(header).push(
                        text(format!(
                            "{} [pt #{}] @ {:.2}{unit_lbl} ({})",
                            lane.curve_id.as_deref().unwrap_or(&lane.id),
                            point_idx,
                            pt.pos_raw,
                            proj.format_bar_position(pt.frame)
                        ))
                        .size(11)
                        .color(Color::from_rgb(0.76, 0.82, 0.90)),
                    );

                    if proj.automations.len() > 1 {
                        let mut lanes_row = Row::new().spacing(4);
                        for (idx, l) in proj.automations.iter().enumerate() {
                            let active = idx == lane_idx;
                            lanes_row = lanes_row.push(
                                button(
                                    text(format!("{}.{}", l.target_node, l.target_param)).size(10),
                                )
                                .padding([2, 6])
                                .style(move |t, s| {
                                    if active {
                                        accent_button_style(Color::from_rgb(0.76, 0.54, 0.16), t, s)
                                    } else {
                                        studio_button_style(t, s)
                                    }
                                })
                                .on_press(Message::SelectAutoLane(idx)),
                            );
                        }
                        card = card.push(scrollable(lanes_row).direction(
                            scrollable::Direction::Horizontal(
                                scrollable::Scrollbar::new().width(3).scroller_width(3),
                            ),
                        ));
                    }

                    let val = self.auto_value_draft.unwrap_or(pt.value);
                    let step = ((lane.max_val - lane.min_val) / 200.0).max(0.01);
                    let val_slider = slider(
                        lane.min_val..=lane.max_val.max(lane.min_val + 0.01),
                        val.clamp(lane.min_val, lane.max_val),
                        Message::AutoValueSliderChanged,
                    )
                    .step(step)
                    .on_release(Message::CommitAutoPointValue)
                    .width(Length::Fill);

                    card = card.push(
                        row![
                            text("Val")
                                .size(11)
                                .width(Length::Fixed(34.0))
                                .color(Color::from_rgb(0.62, 0.68, 0.78)),
                            val_slider,
                            text(format_param_value(lane.unit, val))
                                .size(11)
                                .color(Color::from_rgb(0.55, 0.86, 0.78)),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    );

                    if lane.editable_in_entry {
                        let shape_btn = |shape: &'static str| {
                            let active = pt.shape == shape;
                            button(text(shape).size(10.5))
                                .padding([2, 7])
                                .style(move |t, s| {
                                    if active {
                                        accent_button_style(Color::from_rgb(0.76, 0.54, 0.16), t, s)
                                    } else {
                                        studio_button_style(t, s)
                                    }
                                })
                                .on_press_maybe((!busy).then_some(Message::ApplyAutoPointEdit {
                                    lane_idx,
                                    point_idx,
                                    value: None,
                                    shape: Some(shape.to_owned()),
                                }))
                        };
                        card = card.push(
                            row![
                                text("Curve")
                                    .size(11)
                                    .width(Length::Fixed(34.0))
                                    .color(Color::from_rgb(0.62, 0.68, 0.78)),
                                shape_btn("step"),
                                shape_btn("linear"),
                                shape_btn("exponential"),
                            ]
                            .spacing(4)
                            .align_y(Alignment::Center),
                        );

                        if lane.points.len() > 1 {
                            card = card.push(row![
                                horizontal_space(),
                                button(text("Delete Point").size(11))
                                    .padding([3, 8])
                                    .style(|t, s| {
                                        accent_button_style(Color::from_rgb(0.68, 0.22, 0.26), t, s)
                                    })
                                    .on_press_maybe((!busy).then_some(Message::DeleteAutoPoint {
                                        lane_idx,
                                        point_idx,
                                    })),
                            ]);
                        }
                    }

                    col = col.push(container(card).padding(9).width(Length::Fill).style(|_| {
                        container::Style {
                            background: Some(Background::Color(Color::from_rgb(
                                0.125, 0.122, 0.152,
                            ))),
                            border: Border {
                                color: Color::from_rgba(0.96, 0.76, 0.30, 0.45),
                                width: 1.0,
                                radius: 5.0.into(),
                            },
                            ..Default::default()
                        }
                    }));
                }
            }
        } else {
            col = col.push(
                text("Click or ⌘-drag notes, click HITS or AUTO points to edit · Double-click empty lane to insert.")
                    .size(10.5)
                    .color(Color::from_rgb(0.50, 0.56, 0.66)),
            );
        }

        // External AU / CLAP Plugin Picker, SF2 Preset Picker, and Hosted Plugin Inspector
        if let Some((target, kind)) = &self.plugin_picker {
            col = col.push(self.view_plugin_picker_card(target, *kind));
        }
        if let Some((path, presets, target_track)) = &self.sf2_picker {
            col = col.push(self.view_sf2_picker_card(proj, path, presets, target_track.as_deref()));
        }
        if let Some(slot) = self
            .selected_plugin_slot
            .and_then(|id| self.plugin_slots.iter().find(|s| s.slot_id == id))
        {
            col = col.push(self.view_plugin_slot_inspector_card(slot));
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

        let plugin_count = self.plugin_catalog.len();
        let header = row![
            text("LIVE MIXER & FX CONSOLE")
                .size(12)
                .color(Color::from_rgb(0.76, 0.84, 0.96)),
            Space::with_width(10),
            text(format!(
                "{} Tracks · {} Bus / FX · {} AU/CLAP/VST3 Plugins",
                instruments.len(),
                bus_fx.len(),
                plugin_count
            ))
            .size(11)
            .color(Color::from_rgb(0.56, 0.64, 0.76)),
            horizontal_space(),
            button(text("+ Master FX").size(11))
                .padding([3, 8])
                .style(|t, s| accent_button_style(Color::from_rgb(0.42, 0.30, 0.72), t, s))
                .on_press(Message::OpenPluginPicker {
                    target: SlotTarget::MasterInsert,
                    kind: PluginKind::Effect,
                }),
            button(text("Import SF2 / SFZ...").size(11))
                .padding([3, 8])
                .style(|t, s| accent_button_style(Color::from_rgb(0.18, 0.56, 0.50), t, s))
                .on_press_maybe((!self.editing).then_some(Message::OpenSamplerImportDialog)),
        ]
        .spacing(6)
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

        let master_slots: Vec<&PluginSlotSummary> = self
            .plugin_slots
            .iter()
            .filter(|s| matches!(s.target, SlotTarget::MasterInsert))
            .collect();
        if !master_slots.is_empty() {
            let mut master_row = Row::new().spacing(6).align_y(Alignment::Center);
            master_row = master_row.push(
                text(format!("MASTER FX ({})", master_slots.len()))
                    .size(10.5)
                    .color(Color::from_rgb(0.76, 0.62, 0.98)),
            );
            for slot in &master_slots {
                let is_sel = self.selected_plugin_slot == Some(slot.slot_id);
                let bypassed = slot.bypassed;
                let label = format!(
                    "[{}] {}{}",
                    slot.descriptor.format.label(),
                    slot.descriptor.name,
                    if bypassed { " (BYP)" } else { "" }
                );
                master_row = master_row.push(
                    button(text(label).size(11))
                        .padding([3, 9])
                        .style(move |t, s| {
                            if is_sel {
                                accent_button_style(Color::from_rgb(0.54, 0.34, 0.86), t, s)
                            } else if bypassed {
                                studio_button_style(t, s)
                            } else {
                                accent_button_style(Color::from_rgb(0.34, 0.24, 0.58), t, s)
                            }
                        })
                        .on_press(Message::SelectPluginSlot(Some(slot.slot_id))),
                );
                if slot.has_native_ui {
                    let ui_open = slot.native_ui_open;
                    master_row = master_row.push(
                        button(text(if ui_open { "UI ●" } else { "UI" }).size(10))
                            .padding([3, 6])
                            .style(move |t, s| {
                                if ui_open {
                                    accent_button_style(Color::from_rgb(0.76, 0.44, 0.16), t, s)
                                } else {
                                    studio_button_style(t, s)
                                }
                            })
                            .on_press(Message::TogglePluginNativeUi(slot.slot_id)),
                    );
                }
            }
            master_row = master_row.push(
                button(
                    text(if self.freezing {
                        "Freezing..."
                    } else {
                        "Freeze Master"
                    })
                    .size(10.5),
                )
                .padding([3, 8])
                .style(|t, s| accent_button_style(Color::from_rgb(0.20, 0.54, 0.72), t, s))
                .on_press_maybe(
                    (!self.freezing && !self.editing)
                        .then_some(Message::FreezePluginTarget(SlotTarget::MasterInsert)),
                ),
            );
            body = body.push(
                container(scrollable(master_row).width(Length::Fill).direction(
                    scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(4).scroller_width(4),
                    ),
                ))
                .padding([5, 10])
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.112, 0.108, 0.160))),
                    border: Border {
                        color: Color::from_rgba(0.68, 0.52, 0.96, 0.38),
                        width: 1.0,
                        radius: 5.0.into(),
                    },
                    ..Default::default()
                }),
            );
        }

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

        // External Master Plugin Rack strip (AU / CLAP master inserts)
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

        // External AU / CLAP instrument & insert FX slots on this strip
        let ext_inst = self
            .plugin_slots
            .iter()
            .find(|s| matches!(&s.target, SlotTarget::TrackInstrument(id) if id == &node.id));
        let ext_inserts: Vec<&PluginSlotSummary> = self
            .plugin_slots
            .iter()
            .filter(|s| matches!(&s.target, SlotTarget::NodeInsert(id) if id == &node.id))
            .collect();

        let mut ext_actions = Row::new().spacing(4);
        if node.category == NodeCategory::Instrument {
            ext_actions = ext_actions.push(
                button(
                    text(if ext_inst.is_some() {
                        "Swap Inst"
                    } else {
                        "+ Inst"
                    })
                    .size(9.5),
                )
                .padding([2, 6])
                .width(Length::FillPortion(1))
                .style(|t, s| accent_button_style(Color::from_rgb(0.32, 0.24, 0.58), t, s))
                .on_press(Message::OpenPluginPicker {
                    target: SlotTarget::TrackInstrument(node.id.clone()),
                    kind: PluginKind::Instrument,
                }),
            );
        }
        ext_actions = ext_actions.push(
            button(text("+ FX").size(9.5))
                .padding([2, 6])
                .width(Length::FillPortion(1))
                .style(|t, s| accent_button_style(Color::from_rgb(0.24, 0.36, 0.54), t, s))
                .on_press(Message::OpenPluginPicker {
                    target: SlotTarget::NodeInsert(node.id.clone()),
                    kind: PluginKind::Effect,
                }),
        );
        strip = strip.push(ext_actions);

        if let Some(slot) = ext_inst {
            let is_sel = self.selected_plugin_slot == Some(slot.slot_id);
            let bypassed = slot.bypassed;
            let lbl = format!(
                "{} {}{}",
                slot.descriptor.format.label(),
                slot.descriptor.name,
                if bypassed { " [BYP]" } else { "" }
            );
            let mut pill_row = Row::new().spacing(4).push(
                button(text(lbl).size(10))
                    .width(Length::Fill)
                    .padding([3, 6])
                    .style(move |t, s| {
                        if is_sel {
                            accent_button_style(Color::from_rgb(0.56, 0.34, 0.88), t, s)
                        } else if bypassed {
                            studio_button_style(t, s)
                        } else {
                            accent_button_style(Color::from_rgb(0.40, 0.24, 0.68), t, s)
                        }
                    })
                    .on_press(Message::SelectPluginSlot(Some(slot.slot_id))),
            );
            if slot.has_native_ui {
                let ui_open = slot.native_ui_open;
                pill_row = pill_row.push(
                    button(text(if ui_open { "UI●" } else { "UI" }).size(9.5))
                        .padding([3, 6])
                        .style(move |t, s| {
                            if ui_open {
                                accent_button_style(Color::from_rgb(0.76, 0.44, 0.16), t, s)
                            } else {
                                studio_button_style(t, s)
                            }
                        })
                        .on_press(Message::TogglePluginNativeUi(slot.slot_id)),
                );
            }
            strip = strip.push(pill_row);
        }
        for slot in ext_inserts {
            let is_sel = self.selected_plugin_slot == Some(slot.slot_id);
            let bypassed = slot.bypassed;
            let lbl = format!(
                "FX: {}{}",
                slot.descriptor.name,
                if bypassed { " [BYP]" } else { "" }
            );
            let mut pill_row = Row::new().spacing(4).push(
                button(text(lbl).size(10))
                    .width(Length::Fill)
                    .padding([3, 6])
                    .style(move |t, s| {
                        if is_sel {
                            accent_button_style(Color::from_rgb(0.32, 0.56, 0.84), t, s)
                        } else if bypassed {
                            studio_button_style(t, s)
                        } else {
                            accent_button_style(Color::from_rgb(0.20, 0.38, 0.58), t, s)
                        }
                    })
                    .on_press(Message::SelectPluginSlot(Some(slot.slot_id))),
            );
            if slot.has_native_ui {
                let ui_open = slot.native_ui_open;
                pill_row = pill_row.push(
                    button(text(if ui_open { "UI●" } else { "UI" }).size(9.5))
                        .padding([3, 6])
                        .style(move |t, s| {
                            if ui_open {
                                accent_button_style(Color::from_rgb(0.76, 0.44, 0.16), t, s)
                            } else {
                                studio_button_style(t, s)
                            }
                        })
                        .on_press(Message::TogglePluginNativeUi(slot.slot_id)),
                );
            }
            strip = strip.push(pill_row);
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

    fn view_plugin_picker_card<'a>(
        &'a self,
        target: &'a SlotTarget,
        kind: PluginKind,
    ) -> Element<'a, Message> {
        let target_label = match target {
            SlotTarget::TrackInstrument(n) => format!("{n} (Instrument)"),
            SlotTarget::NodeInsert(n) => format!("{n} (Insert FX)"),
            SlotTarget::MasterInsert => "Master (Insert FX)".into(),
        };
        let header = row![
            text(format!(
                "ADD {} · {target_label}",
                kind.label().to_uppercase()
            ))
            .size(10.5)
            .color(Color::from_rgb(0.76, 0.62, 0.98)),
            horizontal_space(),
            button(text("×").size(11))
                .padding([1, 6])
                .style(studio_button_style)
                .on_press(Message::ClosePluginPicker),
        ]
        .align_y(Alignment::Center);

        let search = text_input("Filter AU / CLAP / VST3 plugins...", &self.plugin_search)
            .on_input(Message::PluginSearchChanged)
            .size(11.5)
            .padding([4, 7]);

        let q = self.plugin_search.trim().to_ascii_lowercase();
        let matches: Vec<&PluginDescriptor> = self
            .plugin_catalog
            .iter()
            .filter(|d| {
                d.kind == kind
                    && (q.is_empty()
                        || d.name.to_ascii_lowercase().contains(&q)
                        || d.vendor.to_ascii_lowercase().contains(&q)
                        || d.format.label().to_ascii_lowercase().contains(&q))
            })
            .take(60)
            .collect();

        let mut list = Column::new().spacing(3);
        if matches.is_empty() {
            list = list.push(
                text(if self.plugin_catalog.is_empty() {
                    "Scanning installed AU / CLAP / VST3 plugins..."
                } else {
                    "No matching plugins found"
                })
                .size(11)
                .color(Color::from_rgb(0.56, 0.62, 0.72)),
            );
        } else {
            for desc in matches {
                let lbl = format!(
                    "[{}] {}  ·  {}",
                    desc.format.label(),
                    desc.name,
                    desc.vendor
                );
                list = list.push(
                    button(text(lbl).size(11))
                        .width(Length::Fill)
                        .padding([4, 7])
                        .style(studio_button_style)
                        .on_press(Message::AddPluginSlot {
                            target: target.clone(),
                            descriptor: desc.clone(),
                        }),
                );
            }
        }

        let card = column![
            header,
            search,
            scrollable(list)
                .width(Length::Fill)
                .height(Length::Fixed(210.0)),
        ]
        .spacing(6);

        container(card)
            .padding(9)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.118, 0.112, 0.168))),
                border: Border {
                    color: Color::from_rgba(0.70, 0.52, 0.98, 0.45),
                    width: 1.2,
                    radius: 5.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_plugin_slot_inspector_card<'a>(
        &'a self,
        slot: &'a PluginSlotSummary,
    ) -> Element<'a, Message> {
        let target_label = match &slot.target {
            SlotTarget::TrackInstrument(n) => format!("{n} · Inst"),
            SlotTarget::NodeInsert(n) => format!("{n} · Insert FX"),
            SlotTarget::MasterInsert => "Master · FX".into(),
        };
        let bypassed = slot.bypassed;
        let header = row![
            text(format!(
                "[{}] {}",
                slot.descriptor.format.label(),
                slot.descriptor.name
            ))
            .size(11.5)
            .color(Color::from_rgb(0.84, 0.72, 0.99)),
            horizontal_space(),
            button(text("×").size(11))
                .padding([1, 6])
                .style(studio_button_style)
                .on_press(Message::SelectPluginSlot(None)),
        ]
        .align_y(Alignment::Center);

        let ui_open = slot.native_ui_open;
        let mut actions = Row::new().spacing(5).align_y(Alignment::Center).push(
            button(text(if bypassed { "Bypassed" } else { "Active" }).size(10.5))
                .padding([3, 7])
                .style(move |t, s| {
                    if bypassed {
                        studio_button_style(t, s)
                    } else {
                        accent_button_style(Color::from_rgb(0.18, 0.62, 0.42), t, s)
                    }
                })
                .on_press(Message::TogglePluginBypass(slot.slot_id)),
        );
        if slot.has_native_ui {
            actions = actions.push(
                button(text(if ui_open { "Close UI" } else { "Open UI" }).size(10.5))
                    .padding([3, 7])
                    .style(move |t, s| {
                        if ui_open {
                            accent_button_style(Color::from_rgb(0.76, 0.44, 0.16), t, s)
                        } else {
                            accent_button_style(Color::from_rgb(0.46, 0.30, 0.78), t, s)
                        }
                    })
                    .on_press(Message::TogglePluginNativeUi(slot.slot_id)),
            );
        }
        actions = actions
            .push(
                button(
                    text(if self.freezing {
                        "Freezing..."
                    } else {
                        "Freeze"
                    })
                    .size(10.5),
                )
                .padding([3, 7])
                .style(|t, s| accent_button_style(Color::from_rgb(0.20, 0.54, 0.72), t, s))
                .on_press_maybe(
                    (!self.freezing && !self.editing)
                        .then_some(Message::FreezePluginTarget(slot.target.clone())),
                ),
            )
            .push(horizontal_space())
            .push(
                button(text("Remove").size(10.5))
                    .padding([3, 7])
                    .style(|t, s| accent_button_style(Color::from_rgb(0.68, 0.22, 0.26), t, s))
                    .on_press(Message::RemovePluginSlot(slot.slot_id)),
            );

        let mut params_col = Column::new().spacing(5);
        if slot.params.is_empty() {
            params_col = params_col.push(
                text("No exposed writable parameters")
                    .size(10.5)
                    .color(Color::from_rgb(0.56, 0.62, 0.72)),
            );
        } else {
            for p in slot.params.iter().take(32) {
                let current = slot
                    .param_values
                    .iter()
                    .find(|(id, _)| *id == p.id)
                    .map(|(_, v)| *v)
                    .unwrap_or(p.default);
                let pos = p.to_slider_pos(current);
                let val_str = p.format_value(current);
                let p_clone = p.clone();
                let slot_id = slot.slot_id;
                let param_id = p.id;
                params_col = params_col.push(
                    column![
                        row![
                            text(&p.name)
                                .size(10.5)
                                .color(Color::from_rgb(0.84, 0.88, 0.96)),
                            horizontal_space(),
                            text(val_str)
                                .size(10.5)
                                .color(Color::from_rgb(0.58, 0.86, 0.78)),
                        ]
                        .align_y(Alignment::Center),
                        slider(0.0_f32..=1.0_f32, pos, move |new_pos| {
                            Message::PluginParamChanged {
                                slot_id,
                                param_id,
                                value: p_clone.value_at_slider_pos(new_pos),
                            }
                        })
                        .step(0.005_f32)
                        .width(Length::Fill),
                    ]
                    .spacing(2),
                );
            }
        }

        let card = column![
            header,
            text(format!(
                "{target_label} · {} · {} params (live ~64 ms)",
                slot.descriptor.vendor,
                slot.params.len()
            ))
            .size(10)
            .color(Color::from_rgb(0.60, 0.66, 0.78)),
            actions,
            scrollable(params_col.padding(iced::Padding {
                top: 0.0,
                right: 12.0,
                bottom: 0.0,
                left: 0.0,
            }))
            .width(Length::Fill)
            .height(Length::Fixed(220.0)),
        ]
        .spacing(6);

        container(card)
            .padding(9)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.115, 0.112, 0.165))),
                border: Border {
                    color: Color::from_rgba(0.66, 0.48, 0.96, 0.45),
                    width: 1.2,
                    radius: 5.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_sf2_picker_card<'a>(
        &'a self,
        proj: &'a ProjectData,
        path: &'a std::path::Path,
        presets: &'a [Sf2PresetInfo],
        target_track: Option<&'a str>,
    ) -> Element<'a, Message> {
        let fname = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "SoundFont.sf2".into());
        let header = row![
            text(format!("IMPORT SF2 · {fname}"))
                .size(10.5)
                .color(Color::from_rgb(0.42, 0.88, 0.78)),
            horizontal_space(),
            button(text("×").size(11))
                .padding([1, 6])
                .style(studio_button_style)
                .on_press(Message::CloseSf2Picker),
        ]
        .align_y(Alignment::Center);

        let mut track_row = Row::new().spacing(4).align_y(Alignment::Center);
        track_row = track_row.push(
            button(text("Import Only").size(10))
                .padding([2, 6])
                .style(move |t, s| {
                    if target_track.is_none() {
                        accent_button_style(Color::from_rgb(0.18, 0.56, 0.50), t, s)
                    } else {
                        studio_button_style(t, s)
                    }
                })
                .on_press(Message::SelectSf2TargetTrack(None)),
        );
        for tr in &proj.tracks {
            let active = target_track == Some(tr.name.as_str());
            let name = tr.name.clone();
            track_row = track_row.push(
                button(text(&tr.name).size(10))
                    .padding([2, 6])
                    .style(move |t, s| {
                        if active {
                            accent_button_style(Color::from_rgb(0.18, 0.56, 0.50), t, s)
                        } else {
                            studio_button_style(t, s)
                        }
                    })
                    .on_press(Message::SelectSf2TargetTrack(Some(name))),
            );
        }

        let mut preset_col = Column::new().spacing(3);
        for p in presets.iter().take(128) {
            let lbl = format!("[{:03}:{:03}] {}", p.bank, p.program, p.name);
            preset_col = preset_col.push(
                button(text(lbl).size(11))
                    .width(Length::Fill)
                    .padding([4, 7])
                    .style(studio_button_style)
                    .on_press(Message::ImportSf2Preset {
                        bank: p.bank,
                        program: p.program,
                    }),
            );
        }

        let card = column![
            header,
            text("Assign imported MaaC library to track:")
                .size(10)
                .color(Color::from_rgb(0.60, 0.68, 0.78)),
            scrollable(track_row).direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new().width(3).scroller_width(3),
            )),
            scrollable(preset_col)
                .width(Length::Fill)
                .height(Length::Fixed(180.0)),
        ]
        .spacing(6);

        container(card)
            .padding(9)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.105, 0.145, 0.148))),
                border: Border {
                    color: Color::from_rgba(0.32, 0.84, 0.74, 0.45),
                    width: 1.2,
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

    fn view_ab_banner<'a>(
        &'a self,
        proj: &'a ProjectData,
        ab: &'a AbComparison,
    ) -> Element<'a, Message> {
        let is_before = ab.active_side == AbSide::Before;
        let is_after = ab.active_side == AbSide::After;
        let summary = ab.highlight.summary_line(proj);
        let has_span = ab.highlight.bounding_span.is_some();

        let mut bar = row![
            text("AI REVIEW")
                .size(11)
                .color(Color::from_rgb(0.32, 0.94, 0.98)),
            text(format!("{} · {summary}", ab.label))
                .size(11)
                .color(Color::from_rgb(0.84, 0.90, 0.96)),
            horizontal_space(),
            button(text("A: Before").size(10.5))
                .padding([3, 8])
                .style(move |t, s| {
                    if is_before {
                        accent_button_style(Color::from_rgb(0.82, 0.52, 0.16), t, s)
                    } else {
                        studio_button_style(t, s)
                    }
                })
                .on_press(Message::SwitchAbSide(AbSide::Before)),
            button(text("B: After").size(10.5))
                .padding([3, 8])
                .style(move |t, s| {
                    if is_after {
                        accent_button_style(Color::from_rgb(0.14, 0.62, 0.68), t, s)
                    } else {
                        studio_button_style(t, s)
                    }
                })
                .on_press(Message::SwitchAbSide(AbSide::After)),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        if has_span {
            bar = bar.push(
                button(text("Loop Changed").size(10.5))
                    .padding([3, 8])
                    .style(|t, s| accent_button_style(Color::from_rgb(0.28, 0.44, 0.74), t, s))
                    .on_press(Message::JumpAndLoopAiEdit),
            );
        }

        bar = bar
            .push(
                button(text("Accept").size(10.5))
                    .padding([3, 8])
                    .style(|t, s| accent_button_style(Color::from_rgb(0.18, 0.62, 0.40), t, s))
                    .on_press(Message::AcceptAiEdit),
            )
            .push(
                button(text("Reject").size(10.5))
                    .padding([3, 8])
                    .style(|t, s| accent_button_style(Color::from_rgb(0.68, 0.24, 0.28), t, s))
                    .on_press(Message::RejectAiEdit),
            );

        container(bar.padding([5, 12]))
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.085, 0.145, 0.175))),
                border: Border {
                    color: Color::from_rgba(0.24, 0.88, 0.96, 0.42),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_collab_sidebar<'a>(
        &'a self,
        proj: &'a ProjectData,
        mut col: Column<'a, Message>,
    ) -> Element<'a, Message> {
        if let Some(err) = &self.edit_error {
            col = col.push(
                container(text(err).size(11).color(Color::from_rgb(0.98, 0.56, 0.56)))
                    .padding(7)
                    .width(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(Background::Color(Color::from_rgba(
                            0.65, 0.12, 0.14, 0.30,
                        ))),
                        border: Border {
                            color: Color::from_rgb(0.85, 0.30, 0.32),
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    }),
            );
        }

        // 1. Agent Backend & Controls Card
        let mut backend_row = Row::new().spacing(4).align_y(Alignment::Center);
        if self.detected_agents.is_empty() {
            backend_row = backend_row.push(
                text("No local CLI (use Copy Context)")
                    .size(10.5)
                    .color(Color::from_rgb(0.76, 0.64, 0.44)),
            );
        } else {
            for agent in &self.detected_agents {
                let kind = agent.kind;
                let active = self.selected_agent == Some(kind);
                backend_row = backend_row.push(
                    button(text(kind.label()).size(10.5))
                        .padding([2, 7])
                        .style(move |t, s| {
                            if active {
                                accent_button_style(Color::from_rgb(0.16, 0.58, 0.66), t, s)
                            } else {
                                studio_button_style(t, s)
                            }
                        })
                        .on_press(Message::SelectAgentBackend(kind)),
                );
            }
        }
        let auto_loop = self.auto_loop_ai_edit;
        let backend_actions = row![
            button(
                text(if auto_loop {
                    "Auto-Loop: ON"
                } else {
                    "Auto-Loop: OFF"
                })
                .size(10),
            )
            .padding([2, 7])
            .style(move |t, s| {
                if auto_loop {
                    accent_button_style(Color::from_rgb(0.24, 0.52, 0.46), t, s)
                } else {
                    studio_button_style(t, s)
                }
            })
            .on_press(Message::ToggleAutoLoopAiEdit),
            horizontal_space(),
            button(text("Copy Context").size(10))
                .padding([2, 7])
                .style(studio_button_style)
                .on_press(Message::CopyCollabContext),
        ]
        .spacing(5)
        .align_y(Alignment::Center);

        let backend_card = container(
            column![
                row![
                    text("AI CO-PRODUCER")
                        .size(11)
                        .color(Color::from_rgb(0.36, 0.90, 0.96)),
                    horizontal_space(),
                    text(if self.agent_running {
                        "Running..."
                    } else {
                        "Ready"
                    })
                    .size(10)
                    .color(if self.agent_running {
                        Color::from_rgb(0.98, 0.78, 0.30)
                    } else {
                        Color::from_rgb(0.46, 0.84, 0.64)
                    }),
                ],
                backend_row,
                backend_actions,
            ]
            .spacing(6),
        )
        .padding(8)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.100, 0.125, 0.155))),
            border: Border {
                color: Color::from_rgba(0.26, 0.84, 0.92, 0.35),
                width: 1.0,
                radius: 5.0.into(),
            },
            ..Default::default()
        });
        col = col.push(backend_card);

        // 2. Shared Focus & Audio Telemetry Card
        let playhead_bar = proj.format_bar_position(self.playhead_frame);
        let loop_text = match self.loop_range {
            Some(lr) => format!(
                "Loop {}–{}",
                proj.format_bar_position(lr.start),
                proj.format_bar_position(lr.end)
            ),
            None => "No loop".into(),
        };
        let sel_text = if !self.selected_notes.is_empty() {
            format!("{} selected notes", self.selected_notes.len())
        } else if let Some(node) = &self.selected_node_id {
            format!("Node: {node}")
        } else {
            "Whole mix".into()
        };
        let audio_text = match &self.audio_snapshot {
            Some(snap) => snap.compact_label(),
            None if self.audio_snapshot_busy => "Measuring focused audio...".into(),
            None => "Click Measure to share RMS & spectrum".into(),
        };
        let focus_card = container(
            column![
                row![
                    text("SHARED FOCUS & AUDIO")
                        .size(10.5)
                        .color(Color::from_rgb(0.68, 0.78, 0.90)),
                    horizontal_space(),
                    button(
                        text(if self.audio_snapshot_busy {
                            "Measuring..."
                        } else {
                            "Measure"
                        })
                        .size(10),
                    )
                    .padding([2, 6])
                    .style(studio_button_style)
                    .on_press_maybe(
                        (!self.audio_snapshot_busy).then_some(Message::RefreshAudioSnapshot)
                    ),
                ]
                .align_y(Alignment::Center),
                text(format!("{playhead_bar} · {loop_text} · {sel_text}"))
                    .size(10.5)
                    .color(Color::from_rgb(0.84, 0.90, 0.96)),
                text(audio_text)
                    .size(10)
                    .color(Color::from_rgb(0.54, 0.86, 0.76)),
            ]
            .spacing(4),
        )
        .padding(8)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.098, 0.110, 0.138))),
            border: Border {
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                width: 1.0,
                radius: 5.0.into(),
            },
            ..Default::default()
        });
        col = col.push(focus_card);

        // 3. Timeline Review Pins Card
        let open_count = self.collab_pins.iter().filter(|p| !p.resolved).count();
        let pin_input = row![
            text_input(
                &format!("Pin note @ {playhead_bar}..."),
                &self.collab_pin_draft
            )
            .on_input(Message::CollabPinDraftChanged)
            .on_submit(Message::AddReviewPin)
            .size(11)
            .padding([4, 6]),
            button(text("+ Pin").size(10.5))
                .padding([4, 8])
                .style(|t, s| accent_button_style(Color::from_rgb(0.84, 0.52, 0.16), t, s))
                .on_press(Message::AddReviewPin),
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        let mut pin_list = Column::new().spacing(4);
        if self.collab_pins.is_empty() {
            pin_list = pin_list.push(
                text("No review pins yet. Drop a pin on any bar to guide the agent.")
                    .size(10.5)
                    .color(Color::from_rgb(0.54, 0.60, 0.70)),
            );
        } else {
            for pin in &self.collab_pins {
                let resolved = pin.resolved;
                let id = pin.id;
                let hdr = format!(
                    "#{} {} {}",
                    pin.id,
                    pin.bar_label,
                    pin.track.as_deref().unwrap_or("")
                );
                pin_list = pin_list.push(
                    container(
                        column![
                            row![
                                button(text(hdr).size(10))
                                    .padding([1, 5])
                                    .style(move |t, s| {
                                        if resolved {
                                            accent_button_style(
                                                Color::from_rgb(0.20, 0.54, 0.40),
                                                t,
                                                s,
                                            )
                                        } else {
                                            accent_button_style(
                                                Color::from_rgb(0.82, 0.50, 0.16),
                                                t,
                                                s,
                                            )
                                        }
                                    })
                                    .on_press(Message::JumpToReviewPin(id)),
                                horizontal_space(),
                                button(text(if resolved { "✓" } else { "○" }).size(10))
                                    .padding([1, 5])
                                    .style(studio_button_style)
                                    .on_press(Message::TogglePinResolved(id)),
                                button(text("×").size(10))
                                    .padding([1, 5])
                                    .style(studio_button_style)
                                    .on_press(Message::DeleteReviewPin(id)),
                            ]
                            .spacing(3)
                            .align_y(Alignment::Center),
                            text(&pin.text).size(10.5).color(if resolved {
                                Color::from_rgb(0.54, 0.64, 0.60)
                            } else {
                                Color::from_rgb(0.88, 0.90, 0.95)
                            }),
                        ]
                        .spacing(3),
                    )
                    .padding(5)
                    .width(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(Background::Color(Color::from_rgb(0.120, 0.132, 0.164))),
                        border: Border {
                            color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    }),
                );
            }
        }

        let mut pin_header = row![
            text(format!("REVIEW PINS ({open_count} open)"))
                .size(10.5)
                .color(Color::from_rgb(0.96, 0.72, 0.34)),
            horizontal_space(),
        ]
        .align_y(Alignment::Center);
        if open_count > 0 {
            pin_header = pin_header.push(
                button(text("Fix Open Pins").size(10))
                    .padding([2, 6])
                    .style(|t, s| accent_button_style(Color::from_rgb(0.16, 0.56, 0.64), t, s))
                    .on_press_maybe((!self.agent_running).then(|| {
                        Message::SendCollabPrompt(Some(
                            "Please address all Open Timeline Review Pins in the score and mark each resolved pin with [RESOLVED #<id>]."
                                .into(),
                        ))
                    })),
            );
        }

        let pins_card = container(
            column![
                pin_header,
                pin_input,
                scrollable(pin_list.padding(iced::Padding {
                    top: 0.0,
                    right: 10.0,
                    bottom: 0.0,
                    left: 0.0,
                }))
                .width(Length::Fill)
                .height(Length::Fixed(118.0)),
            ]
            .spacing(6),
        )
        .padding(8)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.098, 0.110, 0.138))),
            border: Border {
                color: Color::from_rgba(0.96, 0.66, 0.24, 0.24),
                width: 1.0,
                radius: 5.0.into(),
            },
            ..Default::default()
        });
        col = col.push(pins_card);

        // 4. Live Agent Chat Thread & Prompt Composer
        let mut chat_col = Column::new().spacing(6);
        if self.collab_chat.is_empty() {
            chat_col = chat_col.push(
                text("Ask your AI co-producer to refine chords, add drum fills, balance the mix, or fix open pins. Your playhead, loop, selection, and audio telemetry are shared automatically.")
                    .size(10.5)
                    .color(Color::from_rgb(0.56, 0.64, 0.74)),
            );
        } else {
            for msg in &self.collab_chat {
                let (who, accent, bg) = match msg.role {
                    ChatRole::User => (
                        "You",
                        Color::from_rgb(0.56, 0.78, 0.98),
                        Color::from_rgb(0.125, 0.150, 0.205),
                    ),
                    ChatRole::Agent => (
                        "Agent",
                        Color::from_rgb(0.36, 0.90, 0.86),
                        Color::from_rgb(0.100, 0.145, 0.160),
                    ),
                    ChatRole::System => (
                        "System",
                        Color::from_rgb(0.96, 0.66, 0.42),
                        Color::from_rgb(0.165, 0.125, 0.120),
                    ),
                };
                let mut bubble = Column::new().spacing(3).push(
                    row![
                        text(who).size(10).color(accent),
                        horizontal_space(),
                        text(msg.detail.as_deref().unwrap_or(""))
                            .size(9.5)
                            .color(Color::from_rgb(0.56, 0.64, 0.74)),
                    ]
                    .align_y(Alignment::Center),
                );
                bubble = bubble.push(
                    text(&msg.text)
                        .size(10.5)
                        .color(Color::from_rgb(0.88, 0.92, 0.96)),
                );
                chat_col = chat_col.push(container(bubble).padding(7).width(Length::Fill).style(
                    move |_| container::Style {
                        background: Some(Background::Color(bg)),
                        border: Border {
                            color: Color::from_rgba(accent.r, accent.g, accent.b, 0.22),
                            width: 1.0,
                            radius: 5.0.into(),
                        },
                        ..Default::default()
                    },
                ));
            }
        }

        let quick_row = row![
            button(text("Balance Mix").size(9.5))
                .padding([2, 6])
                .style(studio_button_style)
                .on_press_maybe((!self.agent_running).then(|| {
                    Message::SendCollabPrompt(Some(
                        "Inspect the focused audio telemetry and mix parameters, and tighten the balance so the focused section sounds clearer without clipping."
                            .into(),
                    ))
                })),
            button(text("Drum Fill").size(9.5))
                .padding([2, 6])
                .style(studio_button_style)
                .on_press_maybe((!self.agent_running).then(|| {
                    Message::SendCollabPrompt(Some(
                        "Add a tasteful drum fill right at the end of the currently focused loop or playhead bar."
                            .into(),
                    ))
                })),
            button(text("Brighten").size(9.5))
                .padding([2, 6])
                .style(studio_button_style)
                .on_press_maybe((!self.agent_running).then(|| {
                    Message::SendCollabPrompt(Some(
                        "Make the focused track or section slightly brighter and more expressive while preserving the groove."
                            .into(),
                    ))
                })),
        ]
        .spacing(4);

        let composer = row![
            text_input("Ask AI about this section...", &self.collab_prompt_input)
                .on_input(Message::CollabPromptChanged)
                .on_submit(Message::SendCollabPrompt(None))
                .size(11)
                .padding([5, 7]),
            button(text(if self.agent_running { "..." } else { "Send" }).size(11))
                .padding([5, 10])
                .style(|t, s| accent_button_style(Color::from_rgb(0.16, 0.58, 0.66), t, s))
                .on_press_maybe((!self.agent_running).then_some(Message::SendCollabPrompt(None))),
        ]
        .spacing(5)
        .align_y(Alignment::Center);

        let chat_card = container(
            column![
                text("COLLABORATION CHAT")
                    .size(10.5)
                    .color(Color::from_rgb(0.68, 0.78, 0.90)),
                scrollable(chat_col.padding(iced::Padding {
                    top: 0.0,
                    right: 10.0,
                    bottom: 0.0,
                    left: 0.0,
                }))
                .width(Length::Fill)
                .height(Length::Fixed(240.0)),
                quick_row,
                composer,
            ]
            .spacing(6),
        )
        .padding(8)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.098, 0.110, 0.138))),
            border: Border {
                color: Color::from_rgba(0.24, 0.84, 0.92, 0.24),
                width: 1.0,
                radius: 5.0.into(),
            },
            ..Default::default()
        });
        col = col.push(chat_card);

        scrollable(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
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
