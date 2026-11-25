//! Interactive iced Canvas for the bar/region ruler, editable piano roll,
//! drum strip, checkpoint frontier, playhead, and draggable bar-snapped loop.

use std::collections::HashSet;
use std::time::Instant;

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};
use maac::dsp::RenderSpan;

use crate::editor::midi_to_pitch_symbol;
use crate::project::{format_time, ProjectData, RollNote};

pub const RULER_HEIGHT: f32 = 38.0;
const DRUM_STRIP_HEIGHT: f32 = 54.0;
const CHECKPOINT_BAR_HEIGHT: f32 = 5.0;

pub const TRACK_PALETTE: [Color; 10] = [
    Color::from_rgb(0.35, 0.72, 0.98), // sky
    Color::from_rgb(0.98, 0.56, 0.34), // coral
    Color::from_rgb(0.45, 0.86, 0.62), // mint
    Color::from_rgb(0.84, 0.54, 0.98), // violet
    Color::from_rgb(0.98, 0.80, 0.30), // amber
    Color::from_rgb(0.96, 0.44, 0.64), // rose
    Color::from_rgb(0.32, 0.84, 0.84), // teal
    Color::from_rgb(0.72, 0.88, 0.36), // lime
    Color::from_rgb(0.56, 0.64, 0.98), // indigo
    Color::from_rgb(0.96, 0.68, 0.50), // peach
];

pub fn track_color(index: usize) -> Color {
    TRACK_PALETTE[index % TRACK_PALETTE.len()]
}

#[derive(Clone, Debug)]
pub enum TimelineAction {
    Seek(u64),
    SetLoop(Option<RenderSpan>),
    LoopRegion(RenderSpan),
    SelectNote(Option<String>),
    ToggleSelectNote(String),
    SelectNotesBox(Vec<String>),
    AddNoteAt {
        frame: u64,
        midi_pitch: i32,
    },
    DragNotePitch {
        source_path: Vec<String>,
        delta_semitones: i32,
    },
    DragNoteTiming {
        source_path: Vec<String>,
        delta_quarter_16ths: i64,
    },
    DragNoteDuration {
        source_path: Vec<String>,
        delta_quarter_16ths: i64,
    },
    Zoom {
        delta: f32,
        anchor: f32,
    },
    Pan(f32),
}

#[derive(Clone, Debug)]
enum NoteDragMode {
    Move,
    ResizeRight,
}

#[derive(Clone, Debug)]
struct NoteDrag {
    source_path: Vec<String>,
    mode: NoteDragMode,
    start_pos: Point,
    current_pos: Point,
    orig_midi: f32,
    orig_on: u64,
    orig_off: u64,
    moved: bool,
}

#[derive(Default)]
pub struct InteractionState {
    dragging_loop: Option<(u64, u64)>,
    drag_moved: bool,
    marquee: Option<(Point, Point)>,
    note_drag: Option<NoteDrag>,
    shift_held: bool,
    alt_held: bool,
    cmd_held: bool,
    last_ruler_click: Option<(Instant, f32)>,
    last_empty_click: Option<(Instant, Point)>,
}

pub struct TimelineView<'a> {
    pub project: &'a ProjectData,
    pub visible_tracks: &'a [bool],
    pub audible_tracks: Vec<bool>,
    pub selected_notes: &'a HashSet<String>,
    pub selected_source_path: Option<&'a [String]>,
    pub playhead_frame: u64,
    pub checkpoint_frame: u64,
    pub loop_range: Option<RenderSpan>,
    pub snap_to_bar: bool,
    pub approximate: bool,
    pub stale: bool,
    pub zoom: f32,
    pub scroll: f32,
}

impl<'a> TimelineView<'a> {
    fn view_window(&self) -> (f64, f64) {
        let total = self.project.total_frames.max(1) as f64;
        let visible_frac = (1.0 / self.zoom.max(1.0)) as f64;
        let max_start = (1.0 - visible_frac).max(0.0);
        let start_frac = (self.scroll as f64).clamp(0.0, max_start);
        let start = start_frac * total;
        let end = (start_frac + visible_frac).min(1.0) * total;
        (start, end.max(start + 1.0))
    }

    fn x_to_frame(&self, x: f32, width: f32) -> u64 {
        let (v0, v1) = self.view_window();
        let frac = (x / width.max(1.0)).clamp(0.0, 1.0) as f64;
        let frame = v0 + frac * (v1 - v0);
        (frame.round() as u64).min(self.project.total_frames.saturating_sub(1))
    }

    fn frame_to_x(&self, frame: u64, width: f32) -> f32 {
        let (v0, v1) = self.view_window();
        (((frame as f64 - v0) / (v1 - v0)) as f32) * width
    }

    fn snap_if_enabled(&self, frame: u64, alt_held: bool) -> u64 {
        if self.snap_to_bar && !alt_held {
            self.project.snap_frame_to_bar(frame)
        } else {
            frame
        }
    }

    fn roll_bounds(&self, h: f32) -> (f32, f32, f32, f32) {
        let has_hits = !self.project.hits.is_empty();
        let drum_h = if has_hits { DRUM_STRIP_HEIGHT } else { 0.0 };
        let roll_top = RULER_HEIGHT;
        let roll_bottom = (h - drum_h - CHECKPOINT_BAR_HEIGHT).max(roll_top + 20.0);
        let roll_h = roll_bottom - roll_top;
        let pitch_span = (self.project.max_pitch - self.project.min_pitch).max(1.0);
        let note_h = ((roll_h / pitch_span) * 0.85).clamp(2.5, 11.0);
        (roll_top, roll_bottom, roll_h, note_h)
    }

    fn note_rect(&self, note: &RollNote, w: f32, h: f32) -> Rectangle {
        let (roll_top, roll_bottom, roll_h, note_h) = self.roll_bounds(h);
        let pitch_span = (self.project.max_pitch - self.project.min_pitch).max(1.0);
        let x0 = self.frame_to_x(note.on_frame, w);
        let x1 = self.frame_to_x(note.off_frame, w);
        let nw = (x1 - x0).max(4.0);
        let norm = ((note.midi_pitch - self.project.min_pitch) / pitch_span).clamp(0.0, 1.0);
        let y = (roll_bottom - norm * roll_h - note_h * 0.5).clamp(roll_top, roll_bottom - note_h);
        Rectangle {
            x: x0,
            y,
            width: nw,
            height: note_h,
        }
    }

    fn hit_test_note(&self, pos: Point, w: f32, h: f32) -> Option<(&RollNote, NoteDragMode)> {
        let (roll_top, roll_bottom, _, _) = self.roll_bounds(h);
        if pos.y < roll_top || pos.y > roll_bottom {
            return None;
        }
        let (v0, v1) = self.view_window();
        // Search in reverse so topmost drawn notes match first; prefer exact hit,
        // with a small vertical pad for thin zoomed-out notes.
        for note in self.project.notes.iter().rev() {
            if !self
                .visible_tracks
                .get(note.track_idx)
                .copied()
                .unwrap_or(true)
            {
                continue;
            }
            if (note.off_frame as f64) < v0 || (note.on_frame as f64) > v1 {
                continue;
            }
            let r = self.note_rect(note, w, h);
            let pad_y = (6.0 - r.height * 0.5).max(2.0);
            if pos.x >= r.x - 2.0
                && pos.x <= r.x + r.width + 3.0
                && pos.y >= r.y - pad_y
                && pos.y <= r.y + r.height + pad_y
            {
                let resize = r.width >= 10.0 && pos.x >= r.x + r.width - 5.0;
                let mode = if resize {
                    NoteDragMode::ResizeRight
                } else {
                    NoteDragMode::Move
                };
                return Some((note, mode));
            }
        }
        None
    }

    fn y_to_midi(&self, y: f32, h: f32) -> i32 {
        let (_, roll_bottom, roll_h, _) = self.roll_bounds(h);
        let pitch_span = (self.project.max_pitch - self.project.min_pitch).max(1.0);
        let norm = ((roll_bottom - y) / roll_h.max(1.0)).clamp(0.0, 1.0);
        (self.project.min_pitch + norm * pitch_span)
            .round()
            .clamp(12.0, 119.0) as i32
    }

    fn notes_in_marquee(&self, a: Point, b: Point, w: f32, h: f32) -> Vec<String> {
        let min_x = a.x.min(b.x);
        let max_x = a.x.max(b.x);
        let min_y = a.y.min(b.y);
        let max_y = a.y.max(b.y);
        let (v0, v1) = self.view_window();
        let mut out = Vec::new();
        for note in &self.project.notes {
            if !self
                .visible_tracks
                .get(note.track_idx)
                .copied()
                .unwrap_or(true)
            {
                continue;
            }
            if (note.off_frame as f64) < v0 || (note.on_frame as f64) > v1 {
                continue;
            }
            let r = self.note_rect(note, w, h);
            let overlaps =
                r.x + r.width >= min_x && r.x <= max_x && r.y + r.height >= min_y && r.y <= max_y;
            if overlaps {
                out.push(note.address.clone());
            }
        }
        out
    }

    fn quarter_frames(&self) -> f64 {
        if self.project.bars.len() >= 2 {
            let b0 = self.project.bars[0];
            let b1 = self.project.bars[1];
            let beats = b0.beats.max(1) as f64;
            (b1.frame.saturating_sub(b0.frame) as f64 / beats).max(1.0)
        } else {
            self.project.sample_rate_hz as f64 * 0.5
        }
    }
}

impl canvas::Program<TimelineAction> for TimelineView<'_> {
    type State = InteractionState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<TimelineAction>) {
        match event {
            canvas::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) => {
                state.shift_held = mods.shift();
                state.alt_held = mods.alt();
                state.cmd_held = mods.command() || mods.control();
                return (canvas::event::Status::Ignored, None);
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(pos) = cursor.position_in(bounds) {
                    let (dx, dy) = match delta {
                        mouse::ScrollDelta::Lines { x, y } => (x, y),
                        mouse::ScrollDelta::Pixels { x, y } => (x / 40.0, y / 40.0),
                    };
                    if state.shift_held || dx.abs() > dy.abs() {
                        let pan = if dx.abs() > 0.01 { -dx } else { -dy };
                        return (
                            canvas::event::Status::Captured,
                            Some(TimelineAction::Pan(pan * 0.06)),
                        );
                    } else if dy.abs() > 0.01 {
                        let anchor = (pos.x / bounds.width.max(1.0)).clamp(0.0, 1.0);
                        return (
                            canvas::event::Status::Captured,
                            Some(TimelineAction::Zoom { delta: dy, anchor }),
                        );
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if cursor.is_over(bounds) {
                    state.dragging_loop = None;
                    state.note_drag = None;
                    state.marquee = None;
                    return (
                        canvas::event::Status::Captured,
                        Some(TimelineAction::SetLoop(None)),
                    );
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(pos) = cursor.position_in(bounds) {
                    let raw_frame = self.x_to_frame(pos.x, bounds.width);
                    let in_ruler = pos.y <= RULER_HEIGHT;
                    if in_ruler {
                        let now = Instant::now();
                        if let Some((prev_t, prev_x)) = state.last_ruler_click {
                            if now.duration_since(prev_t).as_millis() < 360
                                && (pos.x - prev_x).abs() < 12.0
                            {
                                state.last_ruler_click = None;
                                if let Some(region) =
                                    self.project.regions.iter().find(|r| {
                                        raw_frame >= r.start_frame && raw_frame < r.end_frame
                                    })
                                {
                                    return (
                                        canvas::event::Status::Captured,
                                        Some(TimelineAction::LoopRegion(RenderSpan {
                                            start: region.start_frame,
                                            end: region.end_frame,
                                        })),
                                    );
                                }
                            }
                        }
                        state.last_ruler_click = Some((now, pos.x));
                        let frame = self.snap_if_enabled(raw_frame, state.alt_held);
                        state.dragging_loop = Some((frame, frame));
                        state.drag_moved = false;
                        return (canvas::event::Status::Captured, None);
                    }

                    // Hit-test piano roll notes for direct selection & drag-editing.
                    if let Some((note, mode)) = self.hit_test_note(pos, bounds.width, bounds.height)
                    {
                        let addr = note.address.clone();
                        state.last_empty_click = None;
                        if state.cmd_held || state.shift_held {
                            return (
                                canvas::event::Status::Captured,
                                Some(TimelineAction::ToggleSelectNote(addr)),
                            );
                        }
                        let already_in_multi =
                            self.selected_notes.len() > 1 && self.selected_notes.contains(&addr);
                        state.note_drag = Some(NoteDrag {
                            source_path: note.source_path.clone(),
                            mode,
                            start_pos: pos,
                            current_pos: pos,
                            orig_midi: note.midi_pitch,
                            orig_on: note.on_frame,
                            orig_off: note.off_frame,
                            moved: false,
                        });
                        if already_in_multi {
                            return (canvas::event::Status::Captured, None);
                        }
                        return (
                            canvas::event::Status::Captured,
                            Some(TimelineAction::SelectNote(Some(addr))),
                        );
                    }

                    // Shift+drag on empty roll drags a loop range.
                    if state.shift_held {
                        let frame = self.snap_if_enabled(raw_frame, state.alt_held);
                        state.dragging_loop = Some((frame, frame));
                        state.drag_moved = false;
                        return (canvas::event::Status::Captured, None);
                    }

                    // Cmd/Ctrl+drag on empty roll starts a marquee multi-note selection box.
                    if state.cmd_held {
                        state.marquee = Some((pos, pos));
                        return (canvas::event::Status::Captured, None);
                    }

                    // Double-click empty piano roll space to add a new note at (frame, pitch).
                    let (roll_top, roll_bottom, _, _) = self.roll_bounds(bounds.height);
                    if pos.y >= roll_top && pos.y <= roll_bottom {
                        let now = Instant::now();
                        if let Some((prev_t, prev_p)) = state.last_empty_click {
                            if now.duration_since(prev_t).as_millis() < 360
                                && (pos.x - prev_p.x).abs() < 10.0
                                && (pos.y - prev_p.y).abs() < 10.0
                            {
                                state.last_empty_click = None;
                                let midi_pitch = self.y_to_midi(pos.y, bounds.height);
                                return (
                                    canvas::event::Status::Captured,
                                    Some(TimelineAction::AddNoteAt {
                                        frame: raw_frame,
                                        midi_pitch,
                                    }),
                                );
                            }
                        }
                        state.last_empty_click = Some((now, pos));
                    }

                    // Single-click empty space: seek playhead.
                    let frame = self.snap_if_enabled(raw_frame, state.alt_held);
                    return (
                        canvas::event::Status::Captured,
                        Some(TimelineAction::Seek(frame)),
                    );
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some((_, end)) = state.marquee.as_mut() {
                    if let Some(pos) = cursor.position_in(bounds) {
                        *end = pos;
                        return (canvas::event::Status::Captured, None);
                    }
                }
                if let Some(drag) = state.note_drag.as_mut() {
                    if let Some(pos) = cursor.position_in(bounds) {
                        if (pos.x - drag.start_pos.x).abs() > 4.0
                            || (pos.y - drag.start_pos.y).abs() > 4.0
                        {
                            drag.moved = true;
                        }
                        drag.current_pos = pos;
                        return (canvas::event::Status::Captured, None);
                    }
                }
                if let Some((anchor, _)) = state.dragging_loop {
                    if let Some(pos) = cursor.position_in(bounds) {
                        let raw = self.x_to_frame(pos.x, bounds.width);
                        let current = self.snap_if_enabled(raw, state.alt_held);
                        if raw.abs_diff(anchor) > self.project.sample_rate_hz as u64 / 20 {
                            state.drag_moved = true;
                        }
                        state.dragging_loop = Some((anchor, current));
                        return (canvas::event::Status::Captured, None);
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Some((a, b)) = state.marquee.take() {
                    if (a.x - b.x).abs() > 4.0 || (a.y - b.y).abs() > 4.0 {
                        let selected = self.notes_in_marquee(a, b, bounds.width, bounds.height);
                        return (
                            canvas::event::Status::Captured,
                            Some(TimelineAction::SelectNotesBox(selected)),
                        );
                    }
                    return (canvas::event::Status::Captured, None);
                }
                if let Some(drag) = state.note_drag.take() {
                    if !drag.moved {
                        return (canvas::event::Status::Captured, None);
                    }
                    let (_, _, roll_h, _) = self.roll_bounds(bounds.height);
                    let pitch_span = (self.project.max_pitch - self.project.min_pitch).max(1.0);
                    let dx = drag.current_pos.x - drag.start_pos.x;
                    let dy = drag.start_pos.y - drag.current_pos.y; // up = positive semitones
                    let (v0, v1) = self.view_window();
                    let df = (dx as f64 / bounds.width.max(1.0) as f64) * (v1 - v0);
                    let delta_q16 = (df / (self.quarter_frames() / 4.0)).round() as i64;

                    match drag.mode {
                        NoteDragMode::ResizeRight => {
                            if delta_q16 != 0 {
                                return (
                                    canvas::event::Status::Captured,
                                    Some(TimelineAction::DragNoteDuration {
                                        source_path: drag.source_path,
                                        delta_quarter_16ths: delta_q16,
                                    }),
                                );
                            }
                        }
                        NoteDragMode::Move => {
                            if dy.abs() >= dx.abs() {
                                let semis = ((dy / roll_h) * pitch_span).round() as i32;
                                if semis != 0 {
                                    return (
                                        canvas::event::Status::Captured,
                                        Some(TimelineAction::DragNotePitch {
                                            source_path: drag.source_path,
                                            delta_semitones: semis,
                                        }),
                                    );
                                }
                            } else if delta_q16 != 0 {
                                return (
                                    canvas::event::Status::Captured,
                                    Some(TimelineAction::DragNoteTiming {
                                        source_path: drag.source_path,
                                        delta_quarter_16ths: delta_q16,
                                    }),
                                );
                            }
                        }
                    }
                    return (canvas::event::Status::Captured, None);
                }

                if let Some((a, b)) = state.dragging_loop.take() {
                    let moved = state.drag_moved;
                    state.drag_moved = false;
                    let (start, mut end) = if a <= b { (a, b) } else { (b, a) };
                    if moved && self.snap_to_bar && !state.alt_held && end == start {
                        end = self.project.next_bar_after(start);
                    }
                    if moved && end > start + 256 {
                        return (
                            canvas::event::Status::Captured,
                            Some(TimelineAction::SetLoop(Some(RenderSpan { start, end }))),
                        );
                    } else {
                        return (
                            canvas::event::Status::Captured,
                            Some(TimelineAction::Seek(start)),
                        );
                    }
                }
            }
            _ => {}
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let (v0, v1) = self.view_window();

        // Main dark studio canvas backdrop.
        frame.fill_rectangle(
            Point::ORIGIN,
            Size::new(w, h),
            Color::from_rgb(0.082, 0.092, 0.115),
        );

        let has_hits = !self.project.hits.is_empty();
        let drum_h = if has_hits { DRUM_STRIP_HEIGHT } else { 0.0 };
        let (roll_top, roll_bottom, roll_h, note_h) = self.roll_bounds(h);

        // 1. Ruler background & declared regions.
        frame.fill_rectangle(
            Point::ORIGIN,
            Size::new(w, RULER_HEIGHT),
            Color::from_rgb(0.115, 0.130, 0.165),
        );

        for (idx, region) in self.project.regions.iter().enumerate() {
            if (region.end_frame as f64) < v0 || (region.start_frame as f64) > v1 {
                continue;
            }
            let x0 = self.frame_to_x(region.start_frame, w).clamp(0.0, w);
            let x1 = self.frame_to_x(region.end_frame, w).clamp(0.0, w);
            if x1 <= x0 + 1.0 {
                continue;
            }
            let tint = if idx % 2 == 0 {
                Color::from_rgba(0.24, 0.30, 0.42, 0.42)
            } else {
                Color::from_rgba(0.18, 0.24, 0.34, 0.30)
            };
            frame.fill_rectangle(Point::new(x0, 0.0), Size::new(x1 - x0, RULER_HEIGHT), tint);
            frame.stroke(
                &Path::line(Point::new(x0, 0.0), Point::new(x0, roll_bottom)),
                Stroke::default()
                    .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.11))
                    .with_width(1.0),
            );
            if x1 - x0 > 34.0 {
                let caption = region.label.as_deref().unwrap_or(&region.id);
                frame.fill_text(Text {
                    content: caption.to_owned(),
                    position: Point::new(x0 + 6.0, 4.0),
                    color: Color::from_rgb(0.88, 0.92, 0.98),
                    size: iced::Pixels(11.5),
                    ..Text::default()
                });
            }
        }

        // Bar grid lines & bar labels along the bottom half of the ruler.
        if !self.project.bars.is_empty() {
            let first_vis = self.project.bars.partition_point(|b| (b.frame as f64) < v0);
            let last_vis = self
                .project
                .bars
                .partition_point(|b| (b.frame as f64) <= v1);
            let vis_bars = last_vis.saturating_sub(first_vis).max(1);
            let label_stride = choose_bar_stride(vis_bars);
            for b in &self.project.bars[first_vis..last_vis] {
                let x = self.frame_to_x(b.frame, w);
                if x < 0.0 || x > w {
                    continue;
                }
                let is_major = (b.bar - 1) % 4 == 0;
                let line_alpha = if is_major { 0.085 } else { 0.03 };
                if vis_bars <= 96 || is_major {
                    frame.stroke(
                        &Path::line(Point::new(x, roll_top), Point::new(x, roll_bottom)),
                        Stroke::default()
                            .with_color(Color::from_rgba(1.0, 1.0, 1.0, line_alpha))
                            .with_width(1.0),
                    );
                }
                if (b.bar - 1) % label_stride == 0 {
                    frame.stroke(
                        &Path::line(
                            Point::new(x, RULER_HEIGHT - 8.0),
                            Point::new(x, RULER_HEIGHT),
                        ),
                        Stroke::default()
                            .with_color(Color::from_rgba(0.75, 0.80, 0.88, 0.40))
                            .with_width(1.0),
                    );
                    let label = if label_stride == 1 && vis_bars <= 18 {
                        format!(
                            "{} · {}",
                            b.bar,
                            format_time(b.frame, self.project.sample_rate_hz)
                        )
                    } else {
                        format!("{}", b.bar)
                    };
                    frame.fill_text(Text {
                        content: label,
                        position: Point::new(x + 3.0, RULER_HEIGHT - 16.0),
                        color: Color::from_rgba(0.72, 0.78, 0.88, 0.78),
                        size: iced::Pixels(10.0),
                        ..Text::default()
                    });
                }
            }
        }

        // 2. Piano roll horizontal octave grid lines (every C: MIDI % 12 == 0).
        let min_p = self.project.min_pitch;
        let max_p = self.project.max_pitch;
        let pitch_span = (max_p - min_p).max(1.0);
        let first_oct = ((min_p / 12.0).ceil() as i32) * 12;
        let mut midi = first_oct;
        while (midi as f32) <= max_p {
            let norm = (midi as f32 - min_p) / pitch_span;
            let y = roll_bottom - norm * roll_h;
            frame.stroke(
                &Path::line(Point::new(0.0, y), Point::new(w, y)),
                Stroke::default()
                    .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.07))
                    .with_width(1.0),
            );
            let octave = (midi / 12) - 1;
            frame.fill_text(Text {
                content: format!("C{octave}"),
                position: Point::new(4.0, y - 11.0),
                color: Color::from_rgba(0.60, 0.66, 0.76, 0.55),
                size: iced::Pixels(10.0),
                ..Text::default()
            });
            midi += 12;
        }

        // 3. Active loop overlay (and live drag preview).
        let preview_loop = state.dragging_loop.and_then(|(a, b)| {
            let (s, mut e) = if a <= b { (a, b) } else { (b, a) };
            if state.drag_moved && self.snap_to_bar && !state.alt_held && e == s {
                e = self.project.next_bar_after(s);
            }
            (state.drag_moved && e > s + 256).then_some(RenderSpan { start: s, end: e })
        });
        if let Some(span) = preview_loop.or(self.loop_range) {
            let lx0 = self.frame_to_x(span.start, w).clamp(0.0, w);
            let lx1 = self.frame_to_x(span.end, w).clamp(0.0, w);
            if lx1 > lx0 {
                let is_preview = preview_loop.is_some();
                let band_color = if is_preview {
                    Color::from_rgba(0.35, 0.85, 0.98, 0.16)
                } else {
                    Color::from_rgba(0.98, 0.78, 0.25, 0.12)
                };
                let bar_color = if is_preview {
                    Color::from_rgba(0.35, 0.85, 0.98, 0.85)
                } else {
                    Color::from_rgba(0.98, 0.78, 0.25, 0.85)
                };
                frame.fill_rectangle(
                    Point::new(lx0, roll_top),
                    Size::new(lx1 - lx0, h - roll_top - CHECKPOINT_BAR_HEIGHT),
                    band_color,
                );
                frame.fill_rectangle(Point::new(lx0, 0.0), Size::new(lx1 - lx0, 4.0), bar_color);
                for edge in [lx0, lx1] {
                    frame.stroke(
                        &Path::line(
                            Point::new(edge, 0.0),
                            Point::new(edge, h - CHECKPOINT_BAR_HEIGHT),
                        ),
                        Stroke::default().with_color(bar_color).with_width(1.5),
                    );
                }
            }
        }

        // 4. Piano roll notes (dimmed when muted; highlighted when selected or sharing source).
        for note in &self.project.notes {
            if !self
                .visible_tracks
                .get(note.track_idx)
                .copied()
                .unwrap_or(true)
            {
                continue;
            }
            if (note.off_frame as f64) < v0 || (note.on_frame as f64) > v1 {
                continue;
            }
            let r = self.note_rect(note, w, h);
            let x0 = r.x.clamp(0.0, w);
            let x1 = (r.x + r.width).clamp(0.0, w);
            let nw = (x1 - x0).max(2.0);
            let base = track_color(note.track_idx);
            let audible = self
                .audible_tracks
                .get(note.track_idx)
                .copied()
                .unwrap_or(true);
            let is_selected = self.selected_notes.contains(&note.address);
            let is_sibling = !is_selected
                && self.selected_notes.len() == 1
                && self
                    .selected_source_path
                    .is_some_and(|sp| !sp.is_empty() && sp == note.source_path.as_slice());
            let alpha = if is_selected {
                1.0
            } else if audible {
                (0.35 + 0.65 * note.velocity).clamp(0.3, 1.0)
            } else {
                0.15
            };
            frame.fill_rectangle(
                Point::new(x0, r.y),
                Size::new(nw, note_h),
                Color::from_rgba(base.r, base.g, base.b, alpha),
            );
            if is_selected {
                frame.stroke(
                    &Path::rectangle(
                        Point::new(x0 - 1.0, r.y - 1.0),
                        Size::new(nw + 2.0, note_h + 2.0),
                    ),
                    Stroke::default().with_color(Color::WHITE).with_width(1.8),
                );
            } else if is_sibling {
                frame.stroke(
                    &Path::rectangle(Point::new(x0, r.y), Size::new(nw, note_h)),
                    Stroke::default()
                        .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.45))
                        .with_width(1.0),
                );
            }
        }

        // Marquee multi-selection rectangle (Cmd/Ctrl+drag).
        if let Some((a, b)) = state.marquee {
            let x = a.x.min(b.x);
            let y = a.y.min(b.y);
            let mw = (a.x - b.x).abs();
            let mh = (a.y - b.y).abs();
            if mw > 2.0 || mh > 2.0 {
                frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new(mw, mh),
                    Color::from_rgba(0.36, 0.78, 0.98, 0.16),
                );
                frame.stroke(
                    &Path::rectangle(Point::new(x, y), Size::new(mw, mh)),
                    Stroke::default()
                        .with_color(Color::from_rgba(0.42, 0.86, 1.0, 0.85))
                        .with_width(1.2),
                );
            }
        }

        // Live drag ghost preview for pitch/timing/duration edits.
        if let Some(drag) = state.note_drag.as_ref().filter(|d| d.moved) {
            let dx = drag.current_pos.x - drag.start_pos.x;
            let dy = drag.start_pos.y - drag.current_pos.y;
            let mut gx0 = self.frame_to_x(drag.orig_on, w);
            let mut gx1 = self.frame_to_x(drag.orig_off, w);
            let mut gmidi = drag.orig_midi;
            let label = match drag.mode {
                NoteDragMode::ResizeRight => {
                    gx1 = (gx1 + dx).max(gx0 + 4.0);
                    let df = (dx as f64 / w.max(1.0) as f64) * (v1 - v0);
                    let q16 = (df / (self.quarter_frames() / 4.0)).round() as i64;
                    format!("dur {:+}/4q", q16)
                }
                NoteDragMode::Move => {
                    if dy.abs() >= dx.abs() {
                        let semis = ((dy / roll_h) * pitch_span).round() as i32;
                        gmidi = (drag.orig_midi + semis as f32).clamp(min_p, max_p);
                        format!(
                            "{} ({:+})",
                            midi_to_pitch_symbol(gmidi.round() as i32),
                            semis
                        )
                    } else {
                        gx0 += dx;
                        gx1 += dx;
                        let df = (dx as f64 / w.max(1.0) as f64) * (v1 - v0);
                        let q16 = (df / (self.quarter_frames() / 4.0)).round() as i64;
                        format!("at {:+}/4q", q16)
                    }
                }
            };
            let norm = ((gmidi - min_p) / pitch_span).clamp(0.0, 1.0);
            let gy =
                (roll_bottom - norm * roll_h - note_h * 0.5).clamp(roll_top, roll_bottom - note_h);
            let gw = (gx1 - gx0).max(4.0);
            frame.fill_rectangle(
                Point::new(gx0, gy),
                Size::new(gw, note_h),
                Color::from_rgba(1.0, 1.0, 1.0, 0.35),
            );
            frame.stroke(
                &Path::rectangle(Point::new(gx0, gy), Size::new(gw, note_h)),
                Stroke::default()
                    .with_color(Color::from_rgb(0.40, 0.90, 1.0))
                    .with_width(1.6),
            );
            frame.fill_text(Text {
                content: label,
                position: Point::new(gx0 + 4.0, (gy - 14.0).max(roll_top + 2.0)),
                color: Color::from_rgb(0.90, 0.98, 1.0),
                size: iced::Pixels(11.0),
                ..Text::default()
            });
        }

        // 5. Bottom drum / percussion hit strip.
        if has_hits {
            let strip_top = roll_bottom;
            frame.fill_rectangle(
                Point::new(0.0, strip_top),
                Size::new(w, drum_h),
                Color::from_rgb(0.068, 0.075, 0.095),
            );
            frame.stroke(
                &Path::line(Point::new(0.0, strip_top), Point::new(w, strip_top)),
                Stroke::default()
                    .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.14))
                    .with_width(1.0),
            );
            frame.fill_text(Text {
                content: "HITS".into(),
                position: Point::new(6.0, strip_top + 4.0),
                color: Color::from_rgba(0.65, 0.70, 0.78, 0.55),
                size: iced::Pixels(9.5),
                ..Text::default()
            });
            for hit in &self.project.hits {
                if !self
                    .visible_tracks
                    .get(hit.track_idx)
                    .copied()
                    .unwrap_or(true)
                {
                    continue;
                }
                if (hit.on_frame as f64) < v0 || (hit.on_frame as f64) > v1 {
                    continue;
                }
                let x = self.frame_to_x(hit.on_frame, w);
                let bar_h = (hit.velocity * (drum_h - 14.0)).max(6.0);
                let y = strip_top + drum_h - 4.0 - bar_h;
                let base = track_color(hit.track_idx);
                let audible = self
                    .audible_tracks
                    .get(hit.track_idx)
                    .copied()
                    .unwrap_or(true);
                let alpha = if audible { 0.85 } else { 0.16 };
                frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new(2.5, bar_h),
                    Color::from_rgba(base.r, base.g, base.b, alpha),
                );
            }
        }

        // 6. Checkpoint frontier bar at the very bottom.
        let ck_y = h - CHECKPOINT_BAR_HEIGHT;
        frame.fill_rectangle(
            Point::new(0.0, ck_y),
            Size::new(w, CHECKPOINT_BAR_HEIGHT),
            Color::from_rgb(0.05, 0.06, 0.08),
        );
        if self.checkpoint_frame > 0 {
            let ck_x = self.frame_to_x(self.checkpoint_frame, w).clamp(0.0, w);
            frame.fill_rectangle(
                Point::new(0.0, ck_y),
                Size::new(ck_x, CHECKPOINT_BAR_HEIGHT),
                Color::from_rgb(0.22, 0.74, 0.68),
            );
        }

        // Ruler bottom divider.
        frame.stroke(
            &Path::line(Point::new(0.0, RULER_HEIGHT), Point::new(w, RULER_HEIGHT)),
            Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.16))
                .with_width(1.0),
        );

        // 7. Playhead needle.
        if (self.playhead_frame as f64) >= v0 && (self.playhead_frame as f64) <= v1 {
            let px = self.frame_to_x(self.playhead_frame, w);
            let needle_color = if self.approximate {
                Color::from_rgb(0.98, 0.74, 0.24)
            } else if self.stale {
                Color::from_rgb(0.40, 0.78, 0.98)
            } else {
                Color::from_rgb(0.98, 0.36, 0.36)
            };
            frame.stroke(
                &Path::line(
                    Point::new(px, 0.0),
                    Point::new(px, h - CHECKPOINT_BAR_HEIGHT),
                ),
                Stroke::default().with_color(needle_color).with_width(2.0),
            );
            let head = Path::new(|b| {
                b.move_to(Point::new(px - 5.0, 0.0));
                b.line_to(Point::new(px + 5.0, 0.0));
                b.line_to(Point::new(px, 8.0));
                b.close();
            });
            frame.fill(&head, needle_color);
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if let Some(drag) = &state.note_drag {
            return match drag.mode {
                NoteDragMode::ResizeRight => mouse::Interaction::ResizingHorizontally,
                NoteDragMode::Move => mouse::Interaction::Grabbing,
            };
        }
        if state.dragging_loop.is_some() {
            return mouse::Interaction::ResizingHorizontally;
        }
        if let Some(pos) = cursor.position_in(bounds) {
            if pos.y <= RULER_HEIGHT || state.shift_held {
                return mouse::Interaction::Crosshair;
            }
            if let Some((_, mode)) = self.hit_test_note(pos, bounds.width, bounds.height) {
                return match mode {
                    NoteDragMode::ResizeRight => mouse::Interaction::ResizingHorizontally,
                    NoteDragMode::Move => mouse::Interaction::Grab,
                };
            }
            return mouse::Interaction::Pointer;
        }
        mouse::Interaction::default()
    }
}

fn choose_bar_stride(visible_bars: usize) -> i64 {
    const STRIDES: [i64; 6] = [1, 2, 4, 8, 16, 32];
    for stride in STRIDES {
        if (visible_bars as i64) / stride <= 24 {
            return stride;
        }
    }
    32
}
