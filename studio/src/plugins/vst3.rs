//! Cross-platform VST3 dynamic plugin scanner and host backend powered by
//! `vst3-host` (MIT, `default-features = false`, built on pre-generated `vst3`
//! COM bindings).
//!
//! All `vst3_host` types stay encapsulated inside this module behind MaaC
//! Studio's format-agnostic [`PluginInstance`] trait and existing macOS
//! [`NativePluginWindow`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use maac::dsp::{HostBlockContext, HostNoteEvent};
use vst3_host::audio::AudioBuffers;
use vst3_host::discovery;
use vst3_host::host::Vst3Host;
use vst3_host::midi::{MidiChannel, MidiEvent};
#[cfg(target_os = "macos")]
use vst3_host::plugin::WindowHandle;
use vst3_host::plugin::{Plugin, TransportPosition};

#[cfg(target_os = "macos")]
use super::native_window::NativePluginWindow;
use super::{PluginDescriptor, PluginFormat, PluginInstance, PluginKind, PluginParamDescriptor};

/// VST3 `ParameterFlags::kIsHidden` (`1 << 4`).
const VST3_PARAM_FLAG_HIDDEN: u32 = 1 << 4;

fn vst3_search_dirs() -> Vec<PathBuf> {
    let mut dirs = discovery::scan_standard_paths();
    if let Ok(extra) = std::env::var("VST3_PATH") {
        for part in std::env::split_paths(&extra) {
            if !dirs.contains(&part) {
                dirs.push(part);
            }
        }
    }
    dirs
}

fn classify_vst3_kind(category: &str, has_midi_input: bool, audio_inputs: u32) -> PluginKind {
    let lower = category.to_ascii_lowercase();
    if lower.contains("instrument")
        || lower.contains("synth")
        || lower.contains("sampler")
        || lower.contains("drum")
    {
        return PluginKind::Instrument;
    }
    if lower.contains("fx") || lower.contains("effect") || lower.contains("spatial") {
        return PluginKind::Effect;
    }
    if has_midi_input && audio_inputs == 0 {
        PluginKind::Instrument
    } else {
        PluginKind::Effect
    }
}

fn normalize_uid(uid: &str) -> String {
    uid.trim().to_ascii_lowercase()
}

fn descriptors_from_bundle(path: &Path) -> Vec<PluginDescriptor> {
    let probed = std::panic::catch_unwind(|| discovery::get_detailed_plugin_info(path));
    match probed {
        Ok(Ok(detailed)) => {
            let default_vendor = if !detailed.info.vendor.trim().is_empty() {
                detailed.info.vendor.clone()
            } else if !detailed.factory.vendor.trim().is_empty() {
                detailed.factory.vendor.clone()
            } else {
                "VST3".to_string()
            };

            let audio_classes: Vec<&discovery::ClassInfo> = detailed
                .classes
                .iter()
                .filter(|c| c.category.eq_ignore_ascii_case("Audio Module Class"))
                .collect();

            if audio_classes.len() > 1 {
                let mut out = Vec::with_capacity(audio_classes.len());
                for cls in audio_classes {
                    let uid = normalize_uid(&cls.class_id);
                    if uid.is_empty() {
                        continue;
                    }
                    let sub_cat = detailed
                        .module_info
                        .as_ref()
                        .and_then(|mi| {
                            mi.classes
                                .iter()
                                .find(|mc| mc.class_id.eq_ignore_ascii_case(&cls.class_id))
                        })
                        .map(|mc| mc.sub_categories.join("|"))
                        .unwrap_or_else(|| detailed.info.category.clone());
                    let kind = classify_vst3_kind(
                        &sub_cat,
                        detailed.info.has_midi_input,
                        detailed.info.audio_inputs,
                    );
                    out.push(PluginDescriptor {
                        id: format!("vst3:{uid}"),
                        name: if cls.name.trim().is_empty() {
                            detailed.info.name.clone()
                        } else {
                            cls.name.clone()
                        },
                        vendor: default_vendor.clone(),
                        format: PluginFormat::Vst3,
                        kind,
                        path: Some(path.to_path_buf()),
                    });
                }
                if !out.is_empty() {
                    return out;
                }
            }

            let uid = normalize_uid(&detailed.info.uid);
            if uid.is_empty() {
                return Vec::new();
            }
            let kind = classify_vst3_kind(
                &detailed.info.category,
                detailed.info.has_midi_input,
                detailed.info.audio_inputs,
            );
            vec![PluginDescriptor {
                id: format!("vst3:{uid}"),
                name: detailed.info.name,
                vendor: default_vendor,
                format: PluginFormat::Vst3,
                kind,
                path: Some(path.to_path_buf()),
            }]
        }
        _ => {
            // Fallback to lightweight basic probing if detailed introspection fails.
            let Ok(Ok(info)) = std::panic::catch_unwind(|| discovery::get_plugin_info(path)) else {
                return Vec::new();
            };
            let uid = normalize_uid(&info.uid);
            if uid.is_empty() {
                return Vec::new();
            }
            let kind = classify_vst3_kind(&info.category, info.has_midi_input, info.audio_inputs);
            vec![PluginDescriptor {
                id: format!("vst3:{uid}"),
                name: info.name,
                vendor: if info.vendor.trim().is_empty() {
                    "VST3".into()
                } else {
                    info.vendor
                },
                format: PluginFormat::Vst3,
                kind,
                path: Some(path.to_path_buf()),
            }]
        }
    }
}

/// Enumerate all `.vst3` plugins found in standard VST3 directories (+ `VST3_PATH`).
pub fn scan_vst3_plugins() -> Vec<PluginDescriptor> {
    let dirs = vst3_search_dirs();
    let Ok(bundles) = discovery::scan_directories(&dirs) else {
        return Vec::new();
    };
    let mut seen_ids = BTreeSet::new();
    let mut out = Vec::new();
    for bundle_path in bundles {
        for desc in descriptors_from_bundle(&bundle_path) {
            if seen_ids.insert(desc.id.clone()) {
                out.push(desc);
            }
        }
    }
    out
}

pub struct Vst3Instance {
    descriptor: PluginDescriptor,
    plugin: Plugin,
    buffers: AudioBuffers,
    params: Vec<PluginParamDescriptor>,
    sample_rate: f64,
    max_block_frames: usize,
    active: bool,
    #[cfg(target_os = "macos")]
    native_window: Option<NativePluginWindow>,
}

impl Vst3Instance {
    pub fn new(descriptor: &PluginDescriptor) -> Result<Self, String> {
        let bundle_path = descriptor
            .path
            .as_deref()
            .ok_or_else(|| format!("missing VST3 bundle path for {}", descriptor.name))?;
        let class_id = descriptor
            .id
            .strip_prefix("vst3:")
            .unwrap_or(&descriptor.id);

        let sample_rate = 48_000.0;
        let max_block_frames = 512;
        let mut host = Vst3Host::builder()
            .sample_rate(sample_rate)
            .block_size(max_block_frames)
            .input_channels(2)
            .output_channels(2)
            .build()
            .map_err(|e| format!("failed to initialize VST3 host: {e}"))?;

        let plugin = if !class_id.is_empty() {
            host.load_plugin_class(bundle_path, class_id)
                .or_else(|_| host.load_plugin(bundle_path))
        } else {
            host.load_plugin(bundle_path)
        }
        .map_err(|e| format!("failed to load VST3 {}: {e}", descriptor.name))?;

        let params = Self::collect_params(&plugin);
        let buffers = AudioBuffers::new(2, 2, max_block_frames, sample_rate);

        Ok(Self {
            descriptor: descriptor.clone(),
            plugin,
            buffers,
            params,
            sample_rate,
            max_block_frames,
            active: false,
            #[cfg(target_os = "macos")]
            native_window: None,
        })
    }

    fn collect_params(plugin: &Plugin) -> Vec<PluginParamDescriptor> {
        let Ok(raw_params) = plugin.get_parameters() else {
            return Vec::new();
        };
        raw_params
            .into_iter()
            .filter(|p| {
                !p.is_read_only
                    && !p.is_bypass
                    && (p.flags & VST3_PARAM_FLAG_HIDDEN) == 0
                    // Skip synthetic per-channel MIDI CC proxy params exposed by JUCE VST3 wrappers
                    // (16 channels x 128 CCs = 2048 noisy entries).
                    && !p.name.starts_with("MIDI CC ")
            })
            .map(|p| {
                let name = if p.name.trim().is_empty() {
                    format!("Param {}", p.id)
                } else {
                    p.name
                };
                let default = if p.default.is_finite() {
                    (p.default as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                PluginParamDescriptor {
                    id: p.id,
                    name,
                    unit_label: p.unit,
                    min: 0.0,
                    max: 1.0,
                    default,
                    logarithmic: false,
                }
            })
            .collect()
    }

    fn sync_transport(&mut self, ctx: HostBlockContext) {
        let tempo = ctx.tempo_bpm.clamp(1.0, 999.0);
        let _ = self.plugin.set_tempo(tempo);
        let _ = self.plugin.set_playing(true);
        let quarter_notes = if self.sample_rate > 0.0 {
            (ctx.start_frame as f64 / self.sample_rate) * (tempo / 60.0)
        } else {
            0.0
        };
        let _ = self.plugin.set_transport_position(TransportPosition {
            samples: ctx.start_frame as i64,
            quarter_notes,
        });
    }

    fn ensure_buffer_capacity(&mut self, frames: usize) {
        if frames > self.max_block_frames {
            let next_max = frames.next_power_of_two().max(64);
            let _ = self.activate(self.sample_rate, next_max);
        }
        self.buffers.block_size = frames;
        for ch in &mut self.buffers.inputs {
            if ch.len() < frames {
                ch.resize(frames, 0.0);
            }
        }
        for ch in &mut self.buffers.outputs {
            if ch.len() < frames {
                ch.resize(frames, 0.0);
            }
        }
    }

    fn write_outputs_interleaved(&self, frames: usize, channels: usize, dst: &mut [f64]) {
        let out_l = self.buffers.outputs.first();
        let out_r = self.buffers.outputs.get(1).or(out_l);
        for i in 0..frames {
            let l = f64::from(out_l.and_then(|c| c.get(i)).copied().unwrap_or(0.0));
            let r = f64::from(out_r.and_then(|c| c.get(i)).copied().unwrap_or(l as f32));
            let l = if l.is_finite() { l } else { 0.0 };
            let r = if r.is_finite() { r } else { 0.0 };
            if channels == 1 {
                dst[i] = 0.5 * (l + r);
            } else {
                dst[i * channels] = l;
                dst[i * channels + 1] = r;
            }
        }
    }
}

impl PluginInstance for Vst3Instance {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn activate(&mut self, sample_rate: f64, max_block_frames: usize) -> Result<(), String> {
        let sr = sample_rate.max(1.0);
        let max_f = max_block_frames.max(64);
        if self.active {
            let _ = self.plugin.stop_processing();
            self.active = false;
        }
        self.plugin
            .reconfigure(sr, max_f)
            .map_err(|e| format!("VST3 reconfigure failed for {}: {e}", self.descriptor.name))?;
        self.plugin.start_processing().map_err(|e| {
            format!(
                "VST3 start_processing failed for {}: {e}",
                self.descriptor.name
            )
        })?;
        self.sample_rate = sr;
        self.max_block_frames = max_f;
        self.buffers = AudioBuffers::new(2, 2, max_f, sr);
        self.active = true;
        Ok(())
    }

    fn reset(&mut self) {
        let _ = self.plugin.midi_panic();
        if self.active {
            let _ = self.plugin.stop_processing();
            let _ = self.plugin.start_processing();
        }
    }

    fn params(&self) -> &[PluginParamDescriptor] {
        &self.params
    }

    fn get_param(&self, param_id: u32) -> Option<f32> {
        self.plugin
            .get_parameter(param_id)
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| (v as f32).clamp(0.0, 1.0))
    }

    fn set_param(&mut self, param_id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let _ = self
            .plugin
            .set_parameter(param_id, f64::from(value.clamp(0.0, 1.0)));
    }

    fn process_instrument(
        &mut self,
        ctx: HostBlockContext,
        events: &[HostNoteEvent],
        channels: usize,
        out: &mut [f64],
    ) {
        if !self.active || channels == 0 || ctx.frames == 0 {
            return;
        }
        let frames = ctx.frames;
        self.ensure_buffer_capacity(frames);
        self.sync_transport(ctx);

        let channel = MidiChannel::from_index(0).expect("MIDI channel 1 (index 0) is valid");
        let max_offset = frames.saturating_sub(1);
        for ev in events {
            let note = ev.midi_note.min(127);
            let vel = (ev.velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
            let midi_ev = if ev.is_note_on {
                MidiEvent::NoteOn {
                    channel,
                    note,
                    velocity: vel.max(1),
                }
            } else {
                MidiEvent::NoteOff {
                    channel,
                    note,
                    velocity: vel,
                }
            };
            let offset = ev.frame_offset.min(max_offset) as i32;
            let _ = self.plugin.send_midi_event_at(midi_ev, offset);
        }

        for ch in &mut self.buffers.inputs {
            ch[..frames].fill(0.0);
        }
        for ch in &mut self.buffers.outputs {
            ch[..frames].fill(0.0);
        }
        let _ = self.plugin.process_audio(&mut self.buffers);
        self.write_outputs_interleaved(frames, channels, out);
    }

    fn process_effect(&mut self, ctx: HostBlockContext, channels: usize, io: &mut [f64]) {
        if !self.active || channels == 0 || ctx.frames == 0 {
            return;
        }
        let frames = ctx.frames;
        self.ensure_buffer_capacity(frames);
        self.sync_transport(ctx);

        for i in 0..frames {
            let l = io[i * channels] as f32;
            let r = if channels >= 2 {
                io[i * channels + 1] as f32
            } else {
                l
            };
            self.buffers.inputs[0][i] = l;
            self.buffers.inputs[1][i] = r;
        }
        for ch in &mut self.buffers.outputs {
            ch[..frames].fill(0.0);
        }
        let _ = self.plugin.process_audio(&mut self.buffers);
        self.write_outputs_interleaved(frames, channels, io);
    }

    fn save_state(&self) -> Result<Vec<u8>, String> {
        self.plugin
            .save_state()
            .map_err(|e| format!("VST3 save_state failed for {}: {e}", self.descriptor.name))
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.plugin
            .load_state(bytes)
            .map_err(|e| format!("VST3 load_state failed for {}: {e}", self.descriptor.name))
    }

    fn has_native_ui(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.plugin.has_editor()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    fn is_native_ui_open(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.native_window
                .as_ref()
                .is_some_and(NativePluginWindow::is_visible)
                && self.plugin.is_editor_open()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    fn open_native_ui(&mut self, title: &str) -> Result<(), String> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = title;
            Err("VST3 native UI is currently implemented for macOS Cocoa".into())
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(win) = &self.native_window {
                if win.is_visible() && self.plugin.is_editor_open() {
                    win.show();
                    return Ok(());
                }
            }
            self.close_native_ui();

            if !self.plugin.has_editor() {
                return Err(format!(
                    "{} does not expose a VST3 GUI",
                    self.descriptor.name
                ));
            }
            let (w, h) = self.plugin.get_editor_size().unwrap_or((800, 520));
            let resizable = self.plugin.editor_can_resize();
            let mut win = NativePluginWindow::new(
                title,
                f64::from(w.max(240)),
                f64::from(h.max(160)),
                resizable,
            )?;
            let view_ptr = win.content_view();
            if view_ptr.is_null() {
                win.close();
                return Err("macOS NSWindow has no contentView".into());
            }
            let handle = unsafe { WindowHandle::from_nsview(view_ptr) };
            if let Err(err) = self.plugin.open_editor(handle) {
                win.close();
                return Err(format!(
                    "VST3 open_editor failed for {}: {err}",
                    self.descriptor.name
                ));
            }
            if let Ok(Some((actual_w, actual_h))) = self.plugin.attached_editor_size() {
                if actual_w > 0 && actual_h > 0 {
                    win.set_content_size(f64::from(actual_w), f64::from(actual_h));
                }
            }
            win.show();
            self.native_window = Some(win);
            Ok(())
        }
    }

    fn close_native_ui(&mut self) {
        #[cfg(target_os = "macos")]
        {
            // Detach the plugin's IPlugView BEFORE releasing the host NSWindow/NSView.
            if self.plugin.is_editor_open() {
                let _ = self.plugin.close_editor();
            }
            if let Some(mut win) = self.native_window.take() {
                win.close();
            }
        }
    }

    fn poll_main_thread(&mut self) -> bool {
        let mut changed = false;
        let _ = self.plugin.service_host_requests();
        if !self.plugin.take_parameter_edits().is_empty() {
            changed = true;
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(win) = &self.native_window {
                if !win.is_visible() || !self.plugin.is_editor_open() {
                    self.close_native_ui();
                    return true;
                }
                if let Some((w, h)) = self.plugin.take_editor_resize_request() {
                    if w > 0 && h > 0 {
                        win.set_content_size(f64::from(w), f64::from(h));
                        let _ = self.plugin.resize_editor(w, h);
                    }
                }
                // Keep Studio's sidebar sliders synchronized while the native window is open.
                return true;
            }
        }
        changed
    }
}

impl Drop for Vst3Instance {
    fn drop(&mut self) {
        self.close_native_ui();
        if self.active {
            let _ = self.plugin.stop_processing();
            self.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::rack_file::{RackFile, RackSlotRecord};
    use crate::plugins::SlotTarget;
    use std::collections::BTreeMap;

    #[test]
    fn vst3_category_classification_handles_instruments_and_effects() {
        assert_eq!(
            classify_vst3_kind("Instrument|Synth", true, 0),
            PluginKind::Instrument
        );
        assert_eq!(
            classify_vst3_kind("Fx|Reverb", false, 1),
            PluginKind::Effect
        );
        assert_eq!(
            classify_vst3_kind("Fx|Dynamics", true, 1),
            PluginKind::Effect
        );
        assert_eq!(classify_vst3_kind("", true, 0), PluginKind::Instrument);
        assert_eq!(classify_vst3_kind("", false, 1), PluginKind::Effect);
    }

    #[test]
    fn vst3_rack_file_json_round_trip() {
        let rack = RackFile {
            version: 1,
            slots: vec![RackSlotRecord {
                target: SlotTarget::TrackInstrument("picking".into()),
                plugin_id: "vst3:abcdef0123456789abcdef0123456789".into(),
                plugin_name: "Dexed".into(),
                format: PluginFormat::Vst3,
                bypassed: false,
                params: BTreeMap::from([(1u32, 0.75f32)]),
                state_b64: Some("AQID".into()),
            }],
        };
        let json = serde_json::to_string(&rack).expect("serialize VST3 rack");
        assert!(json.contains("\"Vst3\""));
        let decoded: RackFile = serde_json::from_str(&json).expect("deserialize VST3 rack");
        assert_eq!(decoded.slots.len(), 1);
        assert_eq!(decoded.slots[0].format, PluginFormat::Vst3);
        assert_eq!(decoded.slots[0].plugin_name, "Dexed");
    }

    #[test]
    fn vst3_real_plugin_smoke_if_installed() {
        // Exercises real installed VST3 bundles (single-component instruments
        // and split-component effects) + Freeze → .maac when present on the machine.
        let found = scan_vst3_plugins();
        if found.is_empty() {
            return;
        }
        eprintln!("[vst3 smoke] discovered {} VST3 plugin(s):", found.len());
        for d in &found {
            eprintln!("  - [{:?}] {} ({}) id={}", d.kind, d.name, d.vendor, d.id);
        }
        let mut tested_inst = false;
        let mut tested_fx = false;
        let mut first_inst_desc: Option<PluginDescriptor> = None;
        for desc in &found {
            if desc.kind == PluginKind::Instrument && !tested_inst {
                let mut inst = Vst3Instance::new(desc).expect("load VST3 instrument");
                inst.activate(48_000.0, 512).expect("activate VST3 inst");
                let mut out = vec![0.0f64; 512 * 2];
                let note_on = [HostNoteEvent {
                    frame_offset: 0,
                    midi_note: 60,
                    pitch_hz: 261.6256,
                    velocity: 0.9,
                    is_note_on: true,
                }];
                let ctx = HostBlockContext {
                    start_frame: 0,
                    frames: 512,
                    sample_rate_hz: 48_000.0,
                    tempo_bpm: 120.0,
                };
                inst.process_instrument(ctx, &note_on, 2, &mut out);
                // Render a couple more blocks (including a partial tail block) so synths wake up.
                let mut energy = out.iter().map(|s| s * s).sum::<f64>();
                for b in 1..6 {
                    out.fill(0.0);
                    let frames = if b == 5 { 137 } else { 512 };
                    inst.process_instrument(
                        HostBlockContext {
                            start_frame: b * 512,
                            frames,
                            sample_rate_hz: 48_000.0,
                            tempo_bpm: 120.0,
                        },
                        &[],
                        2,
                        &mut out[..frames * 2],
                    );
                    energy += out[..frames * 2].iter().map(|s| s * s).sum::<f64>();
                }
                assert!(
                    energy > 1e-6,
                    "expected audible output from VST3 instrument {}",
                    desc.name
                );
                if let Some(p) = inst.params().first().cloned() {
                    let orig = inst.get_param(p.id).unwrap_or(p.default);
                    let target = if (orig - 0.5).abs() > 0.1 { 0.5 } else { 0.75 };
                    inst.set_param(p.id, target);
                    let after = inst.get_param(p.id).unwrap_or(orig);
                    assert!(
                        (after - target).abs() < 0.05,
                        "expected VST3 param {} to move toward {target}, got {after}",
                        p.name
                    );
                }
                let state = inst.save_state().expect("save VST3 inst state");
                assert!(!state.is_empty());
                inst.load_state(&state).expect("load VST3 inst state");
                inst.reset();
                eprintln!(
                    "[vst3 smoke] inst={} params={} has_ui={} energy={energy:.4} state_bytes={}",
                    desc.name,
                    inst.params().len(),
                    inst.has_native_ui(),
                    state.len()
                );
                first_inst_desc = Some(desc.clone());
                tested_inst = true;
            } else if desc.kind == PluginKind::Effect && !tested_fx {
                let mut fx = Vst3Instance::new(desc).expect("load VST3 effect");
                fx.activate(48_000.0, 512).expect("activate VST3 fx");
                // Feed a short stereo impulse and verify finite, non-zero wet/dry output.
                let mut io = vec![0.0f64; 512 * 2];
                for i in 0..64 {
                    let s = (i as f64 * 0.1).sin() * 0.5;
                    io[i * 2] = s;
                    io[i * 2 + 1] = s;
                }
                fx.process_effect(
                    HostBlockContext {
                        start_frame: 0,
                        frames: 512,
                        sample_rate_hz: 48_000.0,
                        tempo_bpm: 120.0,
                    },
                    2,
                    &mut io,
                );
                let energy: f64 = io.iter().map(|s| s * s).sum();
                assert!(
                    energy.is_finite() && energy > 1e-6,
                    "expected finite non-zero output from VST3 effect {}",
                    desc.name
                );
                // Partial block check
                let mut tail = vec![0.0f64; 137 * 2];
                fx.process_effect(
                    HostBlockContext {
                        start_frame: 512,
                        frames: 137,
                        sample_rate_hz: 48_000.0,
                        tempo_bpm: 120.0,
                    },
                    2,
                    &mut tail,
                );
                assert!(tail.iter().all(|s| s.is_finite()));
                let state = fx.save_state().expect("save VST3 fx state");
                assert!(!state.is_empty());
                fx.load_state(&state).expect("load VST3 fx state");
                fx.reset();
                eprintln!(
                    "[vst3 smoke] fx={} params={} has_ui={} energy={energy:.4} state_bytes={}",
                    desc.name,
                    fx.params().len(),
                    fx.has_native_ui(),
                    state.len()
                );
                tested_fx = true;
            }
            if tested_inst && tested_fx {
                break;
            }
        }

        // Verify Freeze → .maac on examples/hello.maac using a real VST3 instrument if present.
        if let Some(inst_desc) = first_inst_desc {
            let entry = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/hello.maac");
            let data = crate::project::load_and_compile(&entry).expect("compile hello");
            let mut rack = crate::plugins::HostRack::new(f64::from(data.sample_rate_hz), 512);
            rack.add_slot(SlotTarget::TrackInstrument("piano".into()), &inst_desc)
                .expect("mount VST3 instrument on piano");
            let rack_file = rack.to_rack_file();
            let tmp_dir = std::env::temp_dir().join(format!(
                "maac-studio-vst3-freeze-test-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&tmp_dir);
            std::fs::create_dir_all(&tmp_dir).unwrap();
            let outcome = crate::plugins::freeze::freeze_target_to_maac(
                &data.bundle,
                &data.plan,
                &data.limits,
                &tmp_dir,
                &rack_file,
                &found,
                &SlotTarget::TrackInstrument("piano".into()),
                Some(maac::dsp::RenderSpan {
                    start: 0,
                    end: 48_000,
                }),
            )
            .expect("freeze VST3 instrument to pure .maac");
            let pure_plan = maac::compiler::compile_bundle_artifact_with_limits(
                &outcome.edited.bundle,
                &data.limits,
            )
            .expect("pure MaaC compiles frozen VST3 bundle without plugins");
            let mut engine =
                maac::dsp::DspEngine::new_artifact_with_limits(&pure_plan, &data.limits).unwrap();
            let mut peak = 0.0_f64;
            engine
                .render_span(
                    maac::dsp::RenderSpan {
                        start: 0,
                        end: 8_192,
                    },
                    |frame| {
                        for &s in frame {
                            peak = peak.max(s.abs());
                        }
                        Ok(())
                    },
                )
                .unwrap();
            assert!(
                peak > 1e-4,
                "expected audible frozen VST3 stem, peak={peak}"
            );
            eprintln!(
                "[vst3 smoke] Freeze → .maac OK: {} -> {} ({} frames, pure peak={peak:.4})",
                inst_desc.name, outcome.pcm_rel_path, outcome.frames
            );
            let _ = std::fs::remove_dir_all(&tmp_dir);
        }
    }
}
