//! Interactive iced Canvas for the region ruler, piano roll, drum strip,
//! checkpoint frontier, playhead, and draggable loop range.

use std::time::Instant;

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};
use maac::dsp::RenderSpan;

use crate::project::{format_time, ProjectData};

pub const RULER_HEIGHT: f32 = 36.0;
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
    Zoom { delta: f32, anchor: f32 },
    Pan(f32),
}

#[derive(Default)]
pub struct InteractionState {
    dragging_loop: Option<(u64, u64)>,
    drag_moved: bool,
    shift_held: bool,
    last_ruler_click: Option<(Instant, f32)>,
}

pub struct TimelineView<'a> {
    pub project: &'a ProjectData,
    pub visible_tracks: &'a [bool],
    pub playhead_frame: u64,
    pub checkpoint_frame: u64,
    pub loop_range: Option<RenderSpan>,
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
                    return (
                        canvas::event::Status::Captured,
                        Some(TimelineAction::SetLoop(None)),
                    );
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(pos) = cursor.position_in(bounds) {
                    let frame = self.x_to_frame(pos.x, bounds.width);
                    let in_ruler = pos.y <= RULER_HEIGHT;
                    // Double-click on a region in the ruler loops that region.
                    if in_ruler {
                        let now = Instant::now();
                        if let Some((prev_t, prev_x)) = state.last_ruler_click {
                            if now.duration_since(prev_t).as_millis() < 360
                                && (pos.x - prev_x).abs() < 12.0
                            {
                                state.last_ruler_click = None;
                                if let Some(region) = self
                                    .project
                                    .regions
                                    .iter()
                                    .find(|r| frame >= r.start_frame && frame < r.end_frame)
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
                    }
                    if in_ruler || state.shift_held {
                        state.dragging_loop = Some((frame, frame));
                        state.drag_moved = false;
                        return (canvas::event::Status::Captured, None);
                    }
                    return (
                        canvas::event::Status::Captured,
                        Some(TimelineAction::Seek(frame)),
                    );
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some((anchor, _)) = state.dragging_loop {
                    if let Some(pos) = cursor.position_in(bounds) {
                        let current = self.x_to_frame(pos.x, bounds.width);
                        if current.abs_diff(anchor) > self.project.sample_rate_hz as u64 / 20 {
                            state.drag_moved = true;
                        }
                        state.dragging_loop = Some((anchor, current));
                        return (canvas::event::Status::Captured, None);
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Some((a, b)) = state.dragging_loop.take() {
                    let moved = state.drag_moved;
                    state.drag_moved = false;
                    let (start, end) = if a <= b { (a, b) } else { (b, a) };
                    if moved && end > start + 256 {
                        return (
                            canvas::event::Status::Captured,
                            Some(TimelineAction::SetLoop(Some(RenderSpan { start, end }))),
                        );
                    } else {
                        return (
                            canvas::event::Status::Captured,
                            Some(TimelineAction::Seek(a)),
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
        let roll_top = RULER_HEIGHT;
        let roll_bottom = (h - drum_h - CHECKPOINT_BAR_HEIGHT).max(roll_top + 20.0);
        let roll_h = roll_bottom - roll_top;

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
            // Subtle vertical region divider down through the piano roll.
            frame.stroke(
                &Path::line(Point::new(x0, 0.0), Point::new(x0, roll_bottom)),
                Stroke::default()
                    .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.09))
                    .with_width(1.0),
            );
            if x1 - x0 > 34.0 {
                let caption = region.label.as_deref().unwrap_or(&region.id);
                frame.fill_text(Text {
                    content: caption.to_owned(),
                    position: Point::new(x0 + 6.0, 4.0),
                    color: Color::from_rgb(0.86, 0.90, 0.96),
                    size: iced::Pixels(11.5),
                    ..Text::default()
                });
            }
        }

        // Time ticks along the bottom half of the ruler.
        let rate = self.project.sample_rate_hz.max(1) as f64;
        let vis_secs = (v1 - v0) / rate;
        let tick_step_s = choose_tick_step(vis_secs);
        let first_tick = ((v0 / rate) / tick_step_s).ceil() as i64;
        let last_tick = ((v1 / rate) / tick_step_s).floor() as i64;
        for t in first_tick..=last_tick {
            let sec = (t as f64) * tick_step_s;
            let f = (sec * rate).round() as u64;
            let x = self.frame_to_x(f, w);
            if x < 0.0 || x > w {
                continue;
            }
            frame.stroke(
                &Path::line(
                    Point::new(x, RULER_HEIGHT - 8.0),
                    Point::new(x, RULER_HEIGHT),
                ),
                Stroke::default()
                    .with_color(Color::from_rgba(0.75, 0.80, 0.88, 0.35))
                    .with_width(1.0),
            );
            frame.stroke(
                &Path::line(Point::new(x, roll_top), Point::new(x, roll_bottom)),
                Stroke::default()
                    .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.035))
                    .with_width(1.0),
            );
            frame.fill_text(Text {
                content: format_time(f, self.project.sample_rate_hz),
                position: Point::new(x + 3.0, RULER_HEIGHT - 16.0),
                color: Color::from_rgba(0.70, 0.76, 0.85, 0.72),
                size: iced::Pixels(10.0),
                ..Text::default()
            });
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
            let (s, e) = if a <= b { (a, b) } else { (b, a) };
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

        // 4. Piano roll notes.
        let note_h = ((roll_h / pitch_span) * 0.85).clamp(2.5, 11.0);
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
            let x0 = self.frame_to_x(note.on_frame, w).clamp(0.0, w);
            let x1 = self.frame_to_x(note.off_frame, w).clamp(0.0, w);
            let nw = (x1 - x0).max(2.0);
            let norm = ((note.midi_pitch - min_p) / pitch_span).clamp(0.0, 1.0);
            let y =
                (roll_bottom - norm * roll_h - note_h * 0.5).clamp(roll_top, roll_bottom - note_h);
            let base = track_color(note.track_idx);
            let alpha = (0.35 + 0.65 * note.velocity).clamp(0.3, 1.0);
            frame.fill_rectangle(
                Point::new(x0, y),
                Size::new(nw, note_h),
                Color::from_rgba(base.r, base.g, base.b, alpha),
            );
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
                frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new(2.5, bar_h),
                    Color::from_rgba(base.r, base.g, base.b, 0.85),
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
        if state.dragging_loop.is_some() {
            return mouse::Interaction::ResizingHorizontally;
        }
        if let Some(pos) = cursor.position_in(bounds) {
            if pos.y <= RULER_HEIGHT || state.shift_held {
                return mouse::Interaction::Crosshair;
            }
            return mouse::Interaction::Pointer;
        }
        mouse::Interaction::default()
    }
}

fn choose_tick_step(visible_seconds: f64) -> f64 {
    const STEPS: [f64; 10] = [0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0];
    for step in STEPS {
        if visible_seconds / step <= 14.0 {
            return step;
        }
    }
    300.0
}
