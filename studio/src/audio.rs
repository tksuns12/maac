//! Real-time audio device output (`cpal`) pulling from `maac::dsp::Playback`.
//!
//! The audio callback never allocates, never blocks on the UI thread (`try_lock`),
//! and publishes stereo peak levels atomically for the 60fps UI meter.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, Stream, StreamConfig};
use maac::dsp::Playback;

const SCRATCH_FRAMES: usize = 4_096;

struct AudioState {
    playback: Mutex<Option<Arc<Playback>>>,
    playing: AtomicBool,
    gain_bits: AtomicU32,
    peak_left_bits: AtomicU32,
    peak_right_bits: AtomicU32,
    plan_rate: AtomicU32,
    plan_channels: AtomicUsize,
}

pub struct AudioEngine {
    _stream: Option<Stream>,
    state: Arc<AudioState>,
    device_label: String,
    device_rate: u32,
    device_channels: usize,
    init_error: Option<String>,
}

impl AudioEngine {
    pub fn new(preferred_rate: u32) -> Self {
        let state = Arc::new(AudioState {
            playback: Mutex::new(None),
            playing: AtomicBool::new(false),
            gain_bits: AtomicU32::new(0.85_f32.to_bits()),
            peak_left_bits: AtomicU32::new(0.0_f32.to_bits()),
            peak_right_bits: AtomicU32::new(0.0_f32.to_bits()),
            plan_rate: AtomicU32::new(preferred_rate.max(1)),
            plan_channels: AtomicUsize::new(2),
        });

        match open_stream(Arc::clone(&state), preferred_rate) {
            Ok((stream, label, rate, channels)) => Self {
                _stream: Some(stream),
                state,
                device_label: label,
                device_rate: rate,
                device_channels: channels,
                init_error: None,
            },
            Err(err) => Self {
                _stream: None,
                state,
                device_label: "No audio output".into(),
                device_rate: preferred_rate,
                device_channels: 2,
                init_error: Some(err),
            },
        }
    }

    pub fn set_playback(&self, playback: Option<Arc<Playback>>, rate: u32, channels: usize) {
        self.state.plan_rate.store(rate.max(1), Ordering::Release);
        self.state
            .plan_channels
            .store(channels.max(1), Ordering::Release);
        if let Ok(mut guard) = self.state.playback.lock() {
            *guard = playback;
        }
    }

    pub fn set_playing(&self, playing: bool) {
        self.state.playing.store(playing, Ordering::Release);
        if !playing {
            self.state
                .peak_left_bits
                .store(0.0_f32.to_bits(), Ordering::Relaxed);
            self.state
                .peak_right_bits
                .store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.state.playing.load(Ordering::Acquire)
    }

    pub fn set_gain(&self, gain: f32) {
        let clamped = gain.clamp(0.0, 1.5);
        self.state
            .gain_bits
            .store(clamped.to_bits(), Ordering::Relaxed);
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.state.gain_bits.load(Ordering::Relaxed))
    }

    pub fn peaks(&self) -> (f32, f32) {
        let left = f32::from_bits(self.state.peak_left_bits.load(Ordering::Relaxed));
        let right = f32::from_bits(self.state.peak_right_bits.load(Ordering::Relaxed));
        (left, right)
    }

    pub fn device_summary(&self) -> String {
        if let Some(err) = &self.init_error {
            format!("Audio unavailable: {err}")
        } else {
            format!(
                "{} ({} Hz, {} ch)",
                self.device_label, self.device_rate, self.device_channels
            )
        }
    }
}

fn open_stream(
    state: Arc<AudioState>,
    preferred_rate: u32,
) -> Result<(Stream, String, u32, usize), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output audio device found".to_string())?;
    let name = device
        .name()
        .unwrap_or_else(|_| "Default Output".to_string());

    let supported = device
        .supported_output_configs()
        .map_err(|e| format!("failed to query output configs: {e}"))?;

    // Prefer f32 stereo at `preferred_rate` (48 kHz), falling back to default config.
    let chosen = supported
        .filter(|cfg| cfg.sample_format() == SampleFormat::F32)
        .find_map(|range| {
            let rate = SampleRate(preferred_rate);
            if range.min_sample_rate() <= rate
                && rate <= range.max_sample_rate()
                && range.channels() >= 2
            {
                return Some(range.with_sample_rate(rate));
            }
            None
        })
        .or_else(|| device.default_output_config().ok())
        .ok_or_else(|| "no usable output stream config".to_string())?;

    let sample_format = chosen.sample_format();
    let config: StreamConfig = chosen.into();
    let dev_rate = config.sample_rate.0;
    let dev_channels = usize::from(config.channels);

    let err_fn = |err| {
        eprintln!("[maac-studio] audio stream error: {err}");
    };

    let stream = match sample_format {
        SampleFormat::F32 => {
            let mut runner = CallbackRunner::new(state, dev_rate, dev_channels);
            device
                .build_output_stream(
                    &config,
                    move |data: &mut [f32], _| runner.fill_f32(data),
                    err_fn,
                    None,
                )
                .map_err(|e| format!("failed to build f32 output stream: {e}"))?
        }
        SampleFormat::I16 => {
            let mut runner = CallbackRunner::new(state, dev_rate, dev_channels);
            let mut tmp = vec![0.0_f32; SCRATCH_FRAMES * dev_channels.max(2)];
            device
                .build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        let len = data.len().min(tmp.len());
                        runner.fill_f32(&mut tmp[..len]);
                        for (dst, &src) in data[..len].iter_mut().zip(&tmp[..len]) {
                            *dst = (src.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                        if len < data.len() {
                            data[len..].fill(0);
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("failed to build i16 output stream: {e}"))?
        }
        other => return Err(format!("unsupported audio sample format: {other:?}")),
    };

    stream
        .play()
        .map_err(|e| format!("failed to start audio stream: {e}"))?;
    Ok((stream, name, dev_rate, dev_channels))
}

struct CallbackRunner {
    state: Arc<AudioState>,
    dev_rate: u32,
    dev_channels: usize,
    scratch: Vec<f64>,
    // Smooth 5 ms de-click envelope on play/pause transitions.
    env: f64,
    env_step: f64,
    // Simple fractional resampler state when device rate != plan rate.
    frac_pos: f64,
    prev_l: f64,
    prev_r: f64,
    curr_l: f64,
    curr_r: f64,
}

impl CallbackRunner {
    fn new(state: Arc<AudioState>, dev_rate: u32, dev_channels: usize) -> Self {
        let rate = dev_rate.max(1);
        let fade_frames = ((rate as f64) * 0.005).max(1.0);
        Self {
            state,
            dev_rate: rate,
            dev_channels: dev_channels.max(1),
            // Pre-allocated for up to 8 channels * SCRATCH_FRAMES: zero allocs in callback.
            scratch: vec![0.0; SCRATCH_FRAMES * 8],
            env: 0.0,
            env_step: 1.0 / fade_frames,
            frac_pos: 1.0,
            prev_l: 0.0,
            prev_r: 0.0,
            curr_l: 0.0,
            curr_r: 0.0,
        }
    }

    fn fill_f32(&mut self, out: &mut [f32]) {
        let want_play = self.state.playing.load(Ordering::Acquire);
        if !want_play && self.env <= 0.0 {
            out.fill(0.0);
            return;
        }
        let Ok(guard) = self.state.playback.try_lock() else {
            out.fill(0.0);
            return;
        };
        let Some(playback) = guard.as_ref() else {
            self.env = 0.0;
            out.fill(0.0);
            return;
        };

        let gain = f32::from_bits(self.state.gain_bits.load(Ordering::Relaxed)) as f64;
        let plan_rate = self.state.plan_rate.load(Ordering::Acquire).max(1);
        let plan_ch = playback.channels().max(1);
        let dev_ch = self.dev_channels;
        let out_frames = out.len() / dev_ch;

        let mut peak_l = 0.0_f32;
        let mut peak_r = 0.0_f32;

        if plan_rate == self.dev_rate {
            // Fast 1:1 path (e.g. 48 kHz -> 48 kHz).
            let mut offset = 0usize;
            while offset < out_frames {
                let max_chunk = (self.scratch.len() / plan_ch).min(out_frames - offset);
                let slice = &mut self.scratch[..max_chunk * plan_ch];
                let _got = playback.read(slice);
                for f in 0..max_chunk {
                    self.env = if want_play {
                        (self.env + self.env_step).min(1.0)
                    } else {
                        (self.env - self.env_step).max(0.0)
                    };
                    let eff_gain = gain * self.env;
                    let base = f * plan_ch;
                    let l = (slice[base] * eff_gain).clamp(-1.0, 1.0) as f32;
                    let r = if plan_ch >= 2 {
                        (slice[base + 1] * eff_gain).clamp(-1.0, 1.0) as f32
                    } else {
                        l
                    };
                    peak_l = peak_l.max(l.abs());
                    peak_r = peak_r.max(r.abs());
                    let dst = (offset + f) * dev_ch;
                    if dev_ch == 1 {
                        out[dst] = 0.5 * (l + r);
                    } else {
                        out[dst] = l;
                        out[dst + 1] = r;
                        for extra in 2..dev_ch {
                            out[dst + extra] = 0.0;
                        }
                    }
                }
                offset += max_chunk;
            }
        } else {
            // Linear resampling path when device rate differs from plan rate.
            let ratio = plan_rate as f64 / self.dev_rate as f64;
            let one_frame_len = plan_ch.min(self.scratch.len());
            for f in 0..out_frames {
                while self.frac_pos >= 1.0 {
                    self.frac_pos -= 1.0;
                    self.prev_l = self.curr_l;
                    self.prev_r = self.curr_r;
                    let slice = &mut self.scratch[..one_frame_len];
                    let _ = playback.read(slice);
                    self.curr_l = slice[0];
                    self.curr_r = if plan_ch >= 2 { slice[1] } else { slice[0] };
                }
                self.env = if want_play {
                    (self.env + self.env_step).min(1.0)
                } else {
                    (self.env - self.env_step).max(0.0)
                };
                let eff_gain = gain * self.env;
                let t = self.frac_pos;
                let l = ((self.prev_l + (self.curr_l - self.prev_l) * t) * eff_gain)
                    .clamp(-1.0, 1.0) as f32;
                let r = ((self.prev_r + (self.curr_r - self.prev_r) * t) * eff_gain)
                    .clamp(-1.0, 1.0) as f32;
                self.frac_pos += ratio;
                peak_l = peak_l.max(l.abs());
                peak_r = peak_r.max(r.abs());
                let dst = f * dev_ch;
                if dev_ch == 1 {
                    out[dst] = 0.5 * (l + r);
                } else {
                    out[dst] = l;
                    out[dst + 1] = r;
                    for extra in 2..dev_ch {
                        out[dst + extra] = 0.0;
                    }
                }
            }
        }

        // Smooth decay toward new peaks for readable metering.
        let prev_pl = f32::from_bits(self.state.peak_left_bits.load(Ordering::Relaxed));
        let prev_pr = f32::from_bits(self.state.peak_right_bits.load(Ordering::Relaxed));
        let next_pl = peak_l.max(prev_pl * 0.88);
        let next_pr = peak_r.max(prev_pr * 0.88);
        self.state
            .peak_left_bits
            .store(next_pl.to_bits(), Ordering::Relaxed);
        self.state
            .peak_right_bits
            .store(next_pr.to_bits(), Ordering::Relaxed);
    }
}
