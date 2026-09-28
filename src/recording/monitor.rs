//! Bounded dry capture and monitor routing on one AUHAL render timeline.
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{
    recording_error,
    stream::{Ring, CHUNK_FRAMES},
    CliError, MonitorLatency, MonitorOutputDevice, Monitoring,
};

pub(super) const GAIN: f32 = 0.125;
const MAX_EXACT_TIME: f64 = 9_007_199_254_740_992.0 - CHUNK_FRAMES as f64;

pub(super) fn valid_time(time: f64, frames: usize) -> bool {
    time.is_finite()
        && time.abs() <= MAX_EXACT_TIME
        && time.fract() == 0.0
        && frames != 0
        && frames <= CHUNK_FRAMES
}

pub(super) fn output_map(channels: u32) -> Vec<i32> {
    (0..channels)
        .map(|channel| if channel < 2 { 0 } else { -1 })
        .collect()
}

impl Monitoring {
    pub(super) fn initial(
        uid: &str,
        channels: u32,
        latency: MonitorLatency,
        numer: u32,
        denom: u32,
    ) -> Self {
        Self {
            output_device: MonitorOutputDevice {
                selection: "same-as-input".into(),
                uid: uid.into(),
                channels,
            },
            input_channel: 1,
            output_channels: (1..=channels.min(2)).collect(),
            gain: GAIN,
            clip_policy: "clamp-to-unit".into(),
            clock_policy: "single-auhal-render-timeline".into(),
            first_render_sample_time: 0.0,
            first_render_host_time_ticks: 0,
            host_timebase_numer: numer,
            host_timebase_denom: denom,
            latency,
        }
    }

    pub(super) fn reserve_anchor_bytes(&self) -> Self {
        let mut bounded = self.clone();
        // These widths cover every accepted sample-time and host-tick anchor.
        bounded.first_render_sample_time = -MAX_EXACT_TIME;
        bounded.first_render_host_time_ticks = u64::MAX;
        bounded
    }

    pub(super) fn matches_request(&self, uid: &str) -> bool {
        self.output_device.selection == "same-as-input"
            && self.output_device.uid == uid
            && self.input_channel == 1
            && (1..=4096).contains(&self.output_device.channels)
            && self
                .output_channels
                .iter()
                .copied()
                .eq(1..=self.output_device.channels.min(2))
            && self.gain == GAIN
            && self.clip_policy == "clamp-to-unit"
            && self.clock_policy == "single-auhal-render-timeline"
            && valid_time(self.first_render_sample_time, 1)
            && self.first_render_host_time_ticks != 0
            && self.host_timebase_numer != 0
            && self.host_timebase_denom != 0
            && (1..=CHUNK_FRAMES as u32).contains(&self.latency.device_buffer_frames)
            && self.latency.application_buffer_frames == 0
            && self.latency.measured_round_trip_frames.is_none()
    }
}

#[repr(C, align(16))]
struct InputScratch([f32; CHUNK_FRAMES]);

struct Progress {
    frames: u64,
    expected_time: Option<f64>,
    last_host: Option<u64>,
    input: InputScratch,
}

/// The whole callback guard owns scratch and timeline state. The disk thread
/// touches only the SPSC ring and atomic failure/anchor fields.
pub(super) struct MonitorState {
    pub ring: Ring,
    target: u64,
    busy: AtomicBool,
    pub stopping: AtomicBool,
    done: AtomicBool,
    progress: UnsafeCell<Progress>,
    first_time: AtomicU64,
    first_host: AtomicU64,
}
// SAFETY: progress is accessed only by a successful busy guard; all shared
// control/consumer state uses atomics or the ring's SPSC handoff.
unsafe impl Sync for MonitorState {}

impl MonitorState {
    pub(super) fn new(target: u64) -> Self {
        Self {
            ring: Ring::new(),
            target,
            busy: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            done: AtomicBool::new(false),
            progress: UnsafeCell::new(Progress {
                frames: 0,
                expected_time: None,
                last_host: None,
                input: InputScratch([0.0; CHUNK_FRAMES]),
            }),
            first_time: AtomicU64::new(0),
            first_host: AtomicU64::new(0),
        }
    }

    pub(super) fn enter(&self) -> Option<CallbackGuard<'_>> {
        if self.stopping.load(Ordering::Acquire)
            || self.ring.failure.load(Ordering::Acquire) != 0
            || self.done.load(Ordering::Acquire)
        {
            return None;
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.ring.fail(6);
            return None;
        }
        Some(CallbackGuard { state: self })
    }

    pub(super) fn anchor(&self) -> Result<(f64, u64), CliError> {
        let host = self.first_host.load(Ordering::Acquire);
        if host == 0 {
            return Err(recording_error(
                "monitor produced no valid render timestamp",
            ));
        }
        Ok((
            f64::from_bits(self.first_time.load(Ordering::Relaxed)),
            host,
        ))
    }

    #[cfg(test)]
    pub(super) fn render(&self, time: f64, host: u64, samples: &[f32], output: &mut [f32]) {
        output.fill(0.0);
        if let Some(mut guard) = self.enter() {
            if samples.len() > CHUNK_FRAMES || output.len() != samples.len() {
                self.ring.fail(1);
                return;
            }
            guard.input()[..samples.len()].copy_from_slice(samples);
            guard.route(time, host, output);
        }
    }
}

pub(super) struct CallbackGuard<'a> {
    state: &'a MonitorState,
}
impl CallbackGuard<'_> {
    pub(super) fn input(&mut self) -> &mut [f32; CHUNK_FRAMES] {
        // SAFETY: this guard exclusively owns the whole callback's scratch.
        unsafe { &mut (*self.state.progress.get()).input.0 }
    }

    pub(super) fn route(&mut self, time: f64, host: u64, output: &mut [f32]) {
        output.fill(0.0);
        // SAFETY: the busy guard excludes all concurrent progress access.
        let progress = unsafe { &mut *self.state.progress.get() };
        let frames = output.len();
        if !valid_time(time, frames)
            || host == 0
            || progress
                .expected_time
                .is_some_and(|expected| expected != time)
            || progress.last_host.is_some_and(|previous| host <= previous)
        {
            self.state.ring.fail(1);
            return;
        }
        if progress.input.0[..frames]
            .iter()
            .any(|sample| !sample.is_finite())
        {
            self.state.ring.fail(7);
            return;
        }
        if self.state.stopping.load(Ordering::Acquire)
            || self.state.ring.failure.load(Ordering::Acquire) != 0
        {
            return;
        }
        let count = frames.min((self.state.target - progress.frames) as usize);
        if count == 0 {
            return;
        }
        if !self.state.ring.push(time, &progress.input.0[..count]) {
            return;
        }
        if progress.frames == 0 {
            self.state
                .first_time
                .store(time.to_bits(), Ordering::Relaxed);
            self.state.first_host.store(host, Ordering::Release);
        }
        progress.frames += count as u64;
        progress.expected_time = Some(time + frames as f64);
        progress.last_host = Some(host);
        if self.state.ring.failure.load(Ordering::Acquire) != 0
            || self.state.stopping.load(Ordering::Acquire)
        {
            return;
        }
        for (dry, monitor) in progress.input.0[..count].iter().zip(&mut output[..count]) {
            *monitor = (*dry * GAIN).clamp(-1.0, 1.0);
        }
        if progress.frames == self.state.target {
            self.state.done.store(true, Ordering::Release);
        }
    }
}
impl Drop for CallbackGuard<'_> {
    fn drop(&mut self) {
        self.state.busy.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dry_capture_gain_clamp_channel_map_and_exact_final_silence() {
        let state = MonitorState::new(3);
        let dry = [0.25, 16.0, -16.0, 0.5];
        let mut output = [99.0; 4];
        state.render(900.0, 10, &dry, &mut output);
        assert_eq!(output, [0.03125, 1.0, -1.0, 0.0]);
        let captured = state.ring.pop().unwrap();
        assert_eq!(captured.frames, 3);
        assert_eq!(&captured.samples[..3], &dry[..3]);
        assert_eq!(state.anchor().unwrap(), (900.0, 10));
        assert_eq!(output_map(1), [0]);
        assert_eq!(output_map(2), [0, 0]);
        assert_eq!(output_map(4), [0, 0, -1, -1]);
        state.render(904.0, 11, &dry, &mut output);
        assert_eq!(output, [0.0; 4]);
        assert!(state.ring.pop().is_none());
    }
    #[test]
    fn malformed_input_timeline_overflow_and_overlap_abort_to_silence() {
        for (time, host, dry) in [
            (f64::NAN, 1, 1.0),
            (MAX_EXACT_TIME + 1.0, 1, 1.0),
            (0.5, 1, 1.0),
            (0.0, 0, 1.0),
            (0.0, 1, f32::NAN),
        ] {
            let state = MonitorState::new(10);
            let mut out = [1.0];
            state.render(time, host, &[dry], &mut out);
            assert_eq!(out, [0.0]);
            assert!(state.ring.check().is_err());
            assert!(state.ring.pop().is_none());
        }
        for (time, host) in [(2.0, 2), (1.0, 1)] {
            let state = MonitorState::new(10);
            let mut out = [1.0];
            state.render(0.0, 1, &[0.25], &mut out);
            state.render(time, host, &[0.25], &mut out);
            assert_eq!(out, [0.0]);
            assert!(state.ring.check().is_err());
        }
        let state = MonitorState::new(100);
        let mut out = [1.0];
        for n in 0..16 {
            state.render(n as f64, n + 1, &[0.25], &mut out);
        }
        state.render(16.0, 17, &[0.25], &mut out);
        assert_eq!(out, [0.0]);
        assert!(state.ring.check().is_err());
        let state = MonitorState::new(10);
        let _held = state.enter().unwrap();
        state.render(0.0, 1, &[0.25], &mut out);
        assert_eq!(out, [0.0]);
        assert!(state.ring.check().is_err());
    }
    #[test]
    fn stopping_and_nonfinite_final_excess_are_silent() {
        let state = MonitorState::new(1);
        let mut output = [1.0; 2];
        state.render(0.0, 1, &[0.25, f32::INFINITY], &mut output);
        assert_eq!(output, [0.0; 2]);
        assert!(state.ring.check().is_err());
        let state = MonitorState::new(2);
        state.stopping.store(true, Ordering::Release);
        state.render(0.0, 1, &[0.25; 2], &mut output);
        assert_eq!(output, [0.0; 2]);
        assert!(state.ring.pop().is_none());
    }
}
