//! Main Elm-architecture `iced` application for MaaC Studio.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::keyboard::{self, key::Named, Key};
use iced::widget::{
    button, canvas, column, container, horizontal_space, row, scrollable, slider, text, toggler,
    Column, Row, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Subscription, Task, Theme};
use maac::dsp::{BackgroundSession, Playback, PlaybackConfig, RenderSpan, SessionPolicy};

use crate::analysis::{run_analysis, AnalysisSummary};
use crate::audio::AudioEngine;
use crate::project::{format_time, load_and_compile, ProjectData};
use crate::timeline::{track_color, TimelineAction, TimelineView};
use crate::watcher::ProjectWatcher;

const WATCH_DEBOUNCE: Duration = Duration::from_millis(120);
const PLAYBACK_EDIT_DEBOUNCE: Duration = Duration::from_millis(80);
const CHECKPOINT_SECONDS: u64 = 4;

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
    ToggleWatch(bool),
    ToggleTrack(usize),
    GainChanged(f32),
    ZoomDelta {
        delta: f32,
        anchor: f32,
    },
    ZoomFit,
    PanDelta(f32),
    ScrollChanged(f32),
    RecompileNow,
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
    compiling: bool,
    compile_error: Option<String>,
    last_reload_note: Option<String>,

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
    visible_tracks: Vec<bool>,
    zoom: f32,
    scroll: f32,
    peaks: (f32, f32),

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
            session: None,
            playback: None,
            watcher: None,
            auto_watch: true,
            pending_watch_at: None,
            compiling: false,
            compile_error: None,
            last_reload_note: None,
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
            visible_tracks: Vec::new(),
            zoom: 1.0,
            scroll: 0.0,
            peaks: (0.0, 0.0),
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

    pub fn title(&self) -> String {
        match &self.project {
            Some(proj) => format!("MaaC Studio — {}", proj.title),
            None => "MaaC Studio".into(),
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let tick = iced::time::every(Duration::from_millis(16)).map(|_| Message::Tick);
        let keys = keyboard::on_key_press(|key, modifiers| {
            if modifiers.command() || modifiers.control() || modifiers.alt() {
                return None;
            }
            match key {
                Key::Named(Named::Space) => Some(Message::TogglePlay),
                Key::Named(Named::Home) => Some(Message::StopToStart),
                Key::Character(ref c) if c.eq_ignore_ascii_case("l") => Some(Message::ToggleLoop),
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
                        self.apply_compiled_project(*data, is_reload);
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

            Message::ToggleWatch(on) => {
                self.auto_watch = on;
                self.pending_watch_at = None;
                Task::none()
            }

            Message::ToggleTrack(idx) => {
                if let Some(vis) = self.visible_tracks.get_mut(idx) {
                    *vis = !*vis;
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

            Message::RunAnalysis => {
                if self.analyzing {
                    return Task::none();
                }
                let Some(proj) = &self.project else {
                    return Task::none();
                };
                self.analyzing = true;
                self.analysis_error = None;
                let plan = proj.plan.clone();
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

                // 2. Poll project file watcher (debounced) for seamless live reloads.
                if self.auto_watch {
                    if let Some(watcher) = &self.watcher {
                        if let Some(at) = watcher.poll_latest() {
                            self.pending_watch_at = Some(at);
                        }
                    }
                    if let Some(at) = self.pending_watch_at {
                        if at.elapsed() >= WATCH_DEBOUNCE && !self.compiling {
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

    fn apply_compiled_project(&mut self, data: ProjectData, is_reload: bool) {
        let same_format = self.project.as_ref().is_some_and(|prev| {
            prev.sample_rate_hz == data.sample_rate_hz && prev.channels == data.channels
        });

        let compile_ms = data.compile_ms;
        let track_count = data.tracks.len();
        if self.visible_tracks.len() != track_count {
            self.visible_tracks = vec![true; track_count];
        }

        if is_reload && same_format {
            if let Some(session) = &self.session {
                session.update(&data.plan);
                self.last_reload_note =
                    Some(format!("Reloaded in {compile_ms} ms — live update sent"));
                self.project = Some(data);
                return;
            }
        }

        // Fresh project open (or audio format changed): rebuild session, watcher, playback.
        let was_playing = self.audio.is_playing();
        self.audio.set_playing(false);
        self.audio
            .set_playback(None, data.sample_rate_hz, data.channels);
        self.playback = None;

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

        self.ensure_playback_started();
        if was_playing {
            self.audio.set_playing(true);
        }
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
            Some(proj) => format!(
                "{} / {}",
                format_time(self.playhead_frame, proj.sample_rate_hz),
                format_time(proj.total_frames, proj.sample_rate_hz)
            ),
            None => "--:--.- / --:--.-".into(),
        };

        let (badge_text, badge_color) =
            if self.compile_error.is_some() || self.playback_error.is_some() {
                ("ERROR", Color::from_rgb(0.92, 0.30, 0.32))
            } else if self.compiling {
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

        let meter_text = format!(
            "L {:>4.0}%  R {:>4.0}%",
            (self.peaks.0 * 100.0).min(100.0),
            (self.peaks.1 * 100.0).min(100.0)
        );

        let vol_slider = slider(0.0..=1.2_f32, self.audio.gain(), Message::GainChanged)
            .step(0.01_f32)
            .width(Length::Fixed(88.0));

        let watch_toggle = toggler(self.auto_watch)
            .label("Watch")
            .on_toggle(Message::ToggleWatch)
            .size(16.0);

        let jit_tag = if cfg!(feature = "jit") { "JIT" } else { "INT" };
        let sidebar_btn = button(
            text(if self.show_sidebar {
                "Hide Panel"
            } else {
                "Analysis"
            })
            .size(12),
        )
        .padding([5, 10])
        .style(studio_button_style)
        .on_press(Message::ToggleSidebar);

        let bar = row![
            open_btn,
            play_btn,
            stop_btn,
            loop_btn,
            Space::with_width(8),
            text(time_display).size(14),
            Space::with_width(8),
            status_badge,
            Space::with_width(8),
            text(ckpt_text)
                .size(12)
                .color(Color::from_rgb(0.66, 0.72, 0.80)),
            horizontal_space(),
            text(meter_text)
                .size(11)
                .color(Color::from_rgb(0.55, 0.85, 0.72)),
            Space::with_width(8),
            text("Vol")
                .size(11)
                .color(Color::from_rgb(0.70, 0.74, 0.82)),
            vol_slider,
            Space::with_width(8),
            watch_toggle,
            Space::with_width(6),
            text(jit_tag)
                .size(11)
                .color(Color::from_rgb(0.56, 0.66, 0.82)),
            Space::with_width(6),
            sidebar_btn,
        ]
        .spacing(6)
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
            .push(button(text("Open .maac Project").size(15)).on_press(Message::OpenDialog))
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
        // Track chips + zoom bar
        let mut track_row = Row::new().spacing(6).align_y(Alignment::Center);
        for (idx, tr) in proj.tracks.iter().enumerate() {
            let visible = self.visible_tracks.get(idx).copied().unwrap_or(true);
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
            let chip_color = if visible {
                c
            } else {
                Color::from_rgb(0.38, 0.42, 0.48)
            };
            track_row = track_row.push(
                button(text(label).size(11.5).color(chip_color))
                    .padding([3, 9])
                    .style(move |theme, status| track_chip_style(c, visible, theme, status))
                    .on_press(Message::ToggleTrack(idx)),
            );
        }

        let zoom_controls = row![
            button(text("-").size(12))
                .padding([3, 8])
                .style(studio_button_style)
                .on_press(Message::ZoomDelta {
                    delta: -1.0,
                    anchor: 0.5
                }),
            button(text("Fit").size(12))
                .padding([3, 8])
                .style(studio_button_style)
                .on_press(Message::ZoomFit),
            button(text("+").size(12))
                .padding([3, 8])
                .style(studio_button_style)
                .on_press(Message::ZoomDelta {
                    delta: 1.0,
                    anchor: 0.5
                }),
            text(format!("{:.1}x", self.zoom))
                .size(11)
                .color(Color::from_rgb(0.66, 0.72, 0.80)),
            Space::with_width(8),
            button(text("Recompile").size(12))
                .padding([3, 9])
                .style(studio_button_style)
                .on_press(Message::RecompileNow),
        ]
        .spacing(5)
        .align_y(Alignment::Center);

        let sub_bar = container(
            row![
                scrollable(track_row).direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(4).scroller_width(4),
                )),
                horizontal_space(),
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

        let timeline_canvas: Element<'a, Message> = Element::from(
            canvas(TimelineView {
                project: proj,
                visible_tracks: &self.visible_tracks,
                playhead_frame: self.playhead_frame,
                checkpoint_frame: self.checkpoint_frame,
                loop_range: self.loop_range,
                approximate: self.approximate,
                stale: self.stale,
                zoom: self.zoom,
                scroll: self.scroll,
            })
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .map(Message::Timeline);

        let mut left_col = column![sub_bar, timeline_canvas]
            .width(Length::FillPortion(3))
            .height(Length::Fill);

        if self.zoom > 1.01 {
            let max_scroll = (1.0 - 1.0 / self.zoom).max(0.001);
            let pan_bar = container(
                slider(0.0_f32..=max_scroll, self.scroll, Message::ScrollChanged)
                    .step(0.002_f32)
                    .width(Length::Fill),
            )
            .padding([2, 12]);
            left_col = left_col.push(pan_bar);
        }

        // Bottom status strip
        let loop_desc = match self.loop_range {
            Some(span) => format!(
                "Loop {}–{}  (Right-click canvas to clear)",
                format_time(span.start, proj.sample_rate_hz),
                format_time(span.end, proj.sample_rate_hz)
            ),
            None => {
                "Click to seek · Drag ruler or Shift+drag to loop · Double-click region to loop"
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

        // Declared regions quick jump & loop
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
