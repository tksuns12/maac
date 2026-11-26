//! External DAW plugin hosting (macOS Audio Unit v2 & CLAP 1.2) for MaaC Studio.
//!
//! All plugin discovery, instantiation, parameter control, state persistence,
//! and audio rendering live strictly inside `studio/`, keeping `maac` core pure
//! Rust and bit-identical. `HostRack` implements `maac::dsp::HostProcessor`
//! so hosted instruments and insert effects run inline on `Playback`'s producer
//! thread (512-frame chunks) while keeping `Playback::read` 0-alloc.

#[cfg(target_os = "macos")]
pub mod au;
pub mod clap;
pub mod clap_abi;
pub mod freeze;
#[cfg(target_os = "macos")]
pub mod native_window;
pub mod rack_file;

use std::collections::BTreeMap;
use std::path::PathBuf;

use maac::dsp::{HostBlockContext, HostNoteEvent, HostProcessor};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum PluginFormat {
    Au,
    Clap,
}

impl PluginFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Au => "AU",
            Self::Clap => "CLAP",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum PluginKind {
    Instrument,
    Effect,
}

impl PluginKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Instrument => "Inst",
            Self::Effect => "FX",
        }
    }
}

/// Stable metadata describing one discovered external plugin.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    /// Stable identifier (e.g. `au:aufx:dely:appl` or `clap:org.surge-synth-team.surge-xt`).
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub format: PluginFormat,
    pub kind: PluginKind,
    /// Filesystem bundle path for CLAP plugins (empty for macOS AU components).
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// Metadata for one automatable parameter exposed by a hosted plugin.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginParamDescriptor {
    pub id: u32,
    pub name: String,
    pub unit_label: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub logarithmic: bool,
}

impl PluginParamDescriptor {
    pub fn to_slider_pos(&self, val: f32) -> f32 {
        let clamped = val.clamp(self.min, self.max);
        if self.logarithmic && self.min > 0.0 && self.max > self.min {
            let lo = self.min.ln();
            let hi = self.max.ln();
            ((clamped.ln() - lo) / (hi - lo)).clamp(0.0, 1.0)
        } else {
            let span = (self.max - self.min).max(1e-6);
            ((clamped - self.min) / span).clamp(0.0, 1.0)
        }
    }

    pub fn value_at_slider_pos(&self, pos: f32) -> f32 {
        let t = pos.clamp(0.0, 1.0);
        if self.logarithmic && self.min > 0.0 && self.max > self.min {
            let lo = self.min.ln();
            let hi = self.max.ln();
            (lo + t * (hi - lo)).exp().clamp(self.min, self.max)
        } else {
            (self.min + t * (self.max - self.min)).clamp(self.min, self.max)
        }
    }

    pub fn format_value(&self, val: f32) -> String {
        if self.unit_label.is_empty() {
            if self.max - self.min >= 50.0 {
                format!("{val:.1}")
            } else {
                format!("{val:.2}")
            }
        } else if self.max - self.min >= 50.0 {
            format!("{val:.1} {}", self.unit_label)
        } else {
            format!("{val:.2} {}", self.unit_label)
        }
    }
}

/// Unified runtime trait for a loaded AU or CLAP plugin instance.
pub trait PluginInstance: Send {
    fn descriptor(&self) -> &PluginDescriptor;
    fn activate(&mut self, sample_rate: f64, max_block_frames: usize) -> Result<(), String>;
    fn reset(&mut self);
    fn params(&self) -> &[PluginParamDescriptor];
    fn get_param(&self, param_id: u32) -> Option<f32>;
    fn set_param(&mut self, param_id: u32, value: f32);
    fn process_instrument(
        &mut self,
        ctx: HostBlockContext,
        events: &[HostNoteEvent],
        channels: usize,
        out: &mut [f64],
    );
    fn process_effect(&mut self, ctx: HostBlockContext, channels: usize, io: &mut [f64]);
    fn save_state(&self) -> Result<Vec<u8>, String>;
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String>;

    /// Whether this plugin instance can open a native floating OS window.
    fn has_native_ui(&self) -> bool {
        false
    }
    /// Whether this plugin's native floating window is currently open.
    fn is_native_ui_open(&self) -> bool {
        false
    }
    /// Open (or bring to front) this plugin's native floating UI window.
    /// Must be called on the main GUI thread.
    fn open_native_ui(&mut self, _title: &str) -> Result<(), String> {
        Err("native plugin UI is not supported on this platform".into())
    }
    /// Close and destroy this plugin's native floating UI window if open.
    /// Must be called on the main GUI thread.
    fn close_native_ui(&mut self) {}
    /// Main-thread tick hook (dispatches CLAP `on_main_thread`, resizes, and
    /// cleans up windows closed by the user). Returns `true` if UI state changed
    /// or a native window is open and parameters may have changed.
    fn poll_main_thread(&mut self) -> bool {
        false
    }
}

/// Where a hosted plugin slot is attached in the project graph.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum SlotTarget {
    /// Replaces the built-in synth on an instrument track node (`node_id`).
    TrackInstrument(String),
    /// Post-node insert effect on any non-delay-cycle node (`node_id`).
    NodeInsert(String),
    /// Post-mix insert effect on the final stereo master output.
    MasterInsert,
}

/// Snapshot of a hosted slot for rendering Studio's Mixer & Inspector UI
/// without holding the audio rack lock during `iced::view`.
#[derive(Clone, Debug)]
pub struct PluginSlotSummary {
    pub slot_id: u64,
    pub target: SlotTarget,
    pub descriptor: PluginDescriptor,
    pub bypassed: bool,
    pub has_native_ui: bool,
    pub native_ui_open: bool,
    pub params: Vec<PluginParamDescriptor>,
    pub param_values: Vec<(u32, f32)>,
}

struct HostedSlot {
    slot_id: u64,
    target: SlotTarget,
    bypassed: bool,
    instance: Box<dyn PluginInstance>,
}

/// Scan all installed plugins (macOS AUv2 + CLAP) sorted cleanly by kind & name.
pub fn scan_installed_plugins() -> Vec<PluginDescriptor> {
    let mut all = Vec::new();
    #[cfg(target_os = "macos")]
    {
        all.extend(au::scan_audio_units());
    }
    all.extend(clap::scan_clap_plugins());
    all.sort_by(|a, b| {
        (
            a.kind == PluginKind::Effect,
            a.name.to_ascii_lowercase(),
            a.vendor.to_ascii_lowercase(),
        )
            .cmp(&(
                b.kind == PluginKind::Effect,
                b.name.to_ascii_lowercase(),
                b.vendor.to_ascii_lowercase(),
            ))
    });
    all
}

/// Instantiate a discovered plugin descriptor and activate it for audio rendering.
pub fn instantiate_plugin(
    desc: &PluginDescriptor,
    sample_rate: f64,
    max_block_frames: usize,
) -> Result<Box<dyn PluginInstance>, String> {
    let mut instance: Box<dyn PluginInstance> = match desc.format {
        #[cfg(target_os = "macos")]
        PluginFormat::Au => Box::new(au::AuInstance::new(desc)?),
        #[cfg(not(target_os = "macos"))]
        PluginFormat::Au => {
            return Err("Audio Units are only supported on macOS".into());
        }
        PluginFormat::Clap => Box::new(clap::ClapInstance::new(desc)?),
    };
    instance.activate(sample_rate, max_block_frames)?;
    Ok(instance)
}

/// Thread-safe plugin rack attached to `Playback` via `maac::dsp::HostProcessor`.
pub struct HostRack {
    sample_rate: f64,
    max_block_frames: usize,
    next_slot_id: u64,
    slots: Vec<HostedSlot>,
}

impl HostRack {
    pub fn new(sample_rate: f64, max_block_frames: usize) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            max_block_frames: max_block_frames.max(64),
            next_slot_id: 1,
            slots: Vec::new(),
        }
    }

    pub fn set_format(&mut self, sample_rate: f64, max_block_frames: usize) {
        let rate = sample_rate.max(1.0);
        let block = max_block_frames.max(64);
        if (self.sample_rate - rate).abs() < 1e-6 && self.max_block_frames == block {
            return;
        }
        self.sample_rate = rate;
        self.max_block_frames = block;
        for slot in &mut self.slots {
            let _ = slot.instance.activate(rate, block);
        }
    }

    /// Add (or replace, for `TrackInstrument`) a hosted plugin slot and return its `slot_id`.
    pub fn add_slot(&mut self, target: SlotTarget, desc: &PluginDescriptor) -> Result<u64, String> {
        let instance = instantiate_plugin(desc, self.sample_rate, self.max_block_frames)?;
        if let SlotTarget::TrackInstrument(ref node_id) = target {
            for slot in &mut self.slots {
                if matches!(&slot.target, SlotTarget::TrackInstrument(existing) if existing == node_id)
                {
                    slot.instance.close_native_ui();
                }
            }
            self.slots.retain(|s| {
                !matches!(&s.target, SlotTarget::TrackInstrument(existing) if existing == node_id)
            });
        }
        let slot_id = self.next_slot_id;
        self.next_slot_id += 1;
        self.slots.push(HostedSlot {
            slot_id,
            target,
            bypassed: false,
            instance,
        });
        Ok(slot_id)
    }

    pub fn remove_slot(&mut self, slot_id: u64) -> bool {
        let before = self.slots.len();
        for slot in &mut self.slots {
            if slot.slot_id == slot_id {
                slot.instance.close_native_ui();
            }
        }
        self.slots.retain(|s| s.slot_id != slot_id);
        self.slots.len() != before
    }

    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.instance.close_native_ui();
        }
        self.slots.clear();
    }

    pub fn toggle_native_ui(&mut self, slot_id: u64) -> Result<bool, String> {
        let Some(slot) = self.slots.iter_mut().find(|s| s.slot_id == slot_id) else {
            return Err("plugin slot no longer exists".into());
        };
        if slot.instance.is_native_ui_open() {
            slot.instance.close_native_ui();
            return Ok(false);
        }
        let target_label = match &slot.target {
            SlotTarget::TrackInstrument(n) => format!("{n} · Inst"),
            SlotTarget::NodeInsert(n) => format!("{n} · FX"),
            SlotTarget::MasterInsert => "Master · FX".into(),
        };
        let title = format!(
            "[{}] {} — {target_label}",
            slot.instance.descriptor().format.label(),
            slot.instance.descriptor().name
        );
        slot.instance.open_native_ui(&title)?;
        Ok(true)
    }

    /// Main-thread UI pump: dispatches CLAP `on_main_thread`, resizes, and
    /// detects closed floating windows. Returns `(any_ui_open, state_changed)`.
    pub fn poll_main_thread(&mut self) -> (bool, bool) {
        let mut any_open = false;
        let mut changed = false;
        for slot in &mut self.slots {
            let was_open = slot.instance.is_native_ui_open();
            let polled = slot.instance.poll_main_thread();
            let now_open = slot.instance.is_native_ui_open();
            any_open |= now_open;
            if was_open != now_open || polled {
                changed = true;
            }
        }
        (any_open, changed)
    }

    pub fn set_bypassed(&mut self, slot_id: u64, bypassed: bool) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.slot_id == slot_id) {
            slot.bypassed = bypassed;
            if bypassed {
                slot.instance.reset();
            }
        }
    }

    pub fn toggle_bypass(&mut self, slot_id: u64) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.slot_id == slot_id) {
            slot.bypassed = !slot.bypassed;
            if slot.bypassed {
                slot.instance.reset();
            }
        }
    }

    pub fn set_param(&mut self, slot_id: u64, param_id: u32, value: f32) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.slot_id == slot_id) {
            slot.instance.set_param(param_id, value);
        }
    }

    pub fn summaries(&self) -> Vec<PluginSlotSummary> {
        self.slots
            .iter()
            .map(|slot| {
                let params = slot.instance.params().to_vec();
                let param_values = params
                    .iter()
                    .map(|p| {
                        let v = slot.instance.get_param(p.id).unwrap_or(p.default);
                        (p.id, v)
                    })
                    .collect();
                PluginSlotSummary {
                    slot_id: slot.slot_id,
                    target: slot.target.clone(),
                    descriptor: slot.instance.descriptor().clone(),
                    bypassed: slot.bypassed,
                    has_native_ui: slot.instance.has_native_ui(),
                    native_ui_open: slot.instance.is_native_ui_open(),
                    params,
                    param_values,
                }
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn to_rack_file(&self) -> rack_file::RackFile {
        let entries = self
            .slots
            .iter()
            .map(|slot| {
                let mut params = BTreeMap::new();
                for p in slot.instance.params() {
                    if let Some(v) = slot.instance.get_param(p.id) {
                        if (v - p.default).abs() > 1e-5 {
                            params.insert(p.id, v);
                        }
                    }
                }
                let state_b64 = slot
                    .instance
                    .save_state()
                    .ok()
                    .filter(|bytes| !bytes.is_empty())
                    .map(|bytes| rack_file::base64_encode(&bytes));
                rack_file::RackSlotRecord {
                    target: slot.target.clone(),
                    plugin_id: slot.instance.descriptor().id.clone(),
                    plugin_name: slot.instance.descriptor().name.clone(),
                    format: slot.instance.descriptor().format,
                    bypassed: slot.bypassed,
                    params,
                    state_b64,
                }
            })
            .collect();
        rack_file::RackFile {
            version: 1,
            slots: entries,
        }
    }

    /// Restore slots from a saved `RackFile` using `catalog` of installed plugins.
    /// Returns any non-fatal warnings (e.g. missing plugins on this machine).
    pub fn load_rack_file(
        &mut self,
        file: &rack_file::RackFile,
        catalog: &[PluginDescriptor],
    ) -> Vec<String> {
        self.slots.clear();
        let mut warnings = Vec::new();
        for rec in &file.slots {
            let Some(desc) = catalog.iter().find(|d| d.id == rec.plugin_id) else {
                warnings.push(format!(
                    "Plugin not installed: {} ({})",
                    rec.plugin_name, rec.plugin_id
                ));
                continue;
            };
            match self.add_slot(rec.target.clone(), desc) {
                Ok(slot_id) => {
                    if let Some(slot) = self.slots.iter_mut().find(|s| s.slot_id == slot_id) {
                        slot.bypassed = rec.bypassed;
                        if let Some(b64) = &rec.state_b64 {
                            if let Ok(bytes) = rack_file::base64_decode(b64) {
                                let _ = slot.instance.load_state(&bytes);
                            }
                        }
                        for (&pid, &val) in &rec.params {
                            slot.instance.set_param(pid, val);
                        }
                    }
                }
                Err(err) => warnings.push(format!("Failed to load {}: {err}", rec.plugin_name)),
            }
        }
        warnings
    }
}

impl HostProcessor for HostRack {
    fn has_any_active(&self) -> bool {
        self.slots.iter().any(|s| !s.bypassed)
    }

    fn has_instrument_override(&self, node_id: &str) -> bool {
        self.slots.iter().any(|s| {
            !s.bypassed && matches!(&s.target, SlotTarget::TrackInstrument(id) if id == node_id)
        })
    }

    fn has_node_insert(&self, node_id: &str) -> bool {
        self.slots.iter().any(|s| {
            !s.bypassed && matches!(&s.target, SlotTarget::NodeInsert(id) if id == node_id)
        })
    }

    fn has_master_insert(&self) -> bool {
        self.slots
            .iter()
            .any(|s| !s.bypassed && matches!(s.target, SlotTarget::MasterInsert))
    }

    fn process_instrument(
        &mut self,
        node_id: &str,
        ctx: HostBlockContext,
        events: &[HostNoteEvent],
        channels: usize,
        out: &mut [f64],
    ) {
        if let Some(slot) = self.slots.iter_mut().find(|s| {
            !s.bypassed && matches!(&s.target, SlotTarget::TrackInstrument(id) if id == node_id)
        }) {
            slot.instance.process_instrument(ctx, events, channels, out);
        }
    }

    fn process_node_insert(
        &mut self,
        node_id: &str,
        ctx: HostBlockContext,
        channels: usize,
        io: &mut [f64],
    ) {
        for slot in &mut self.slots {
            if !slot.bypassed && matches!(&slot.target, SlotTarget::NodeInsert(id) if id == node_id)
            {
                slot.instance.process_effect(ctx, channels, io);
            }
        }
    }

    fn process_master_insert(&mut self, ctx: HostBlockContext, channels: usize, io: &mut [f64]) {
        for slot in &mut self.slots {
            if !slot.bypassed && matches!(slot.target, SlotTarget::MasterInsert) {
                slot.instance.process_effect(ctx, channels, io);
            }
        }
    }

    fn reset(&mut self, sample_rate_hz: f64, max_block_frames: usize) {
        self.set_format(sample_rate_hz, max_block_frames);
        for slot in &mut self.slots {
            slot.instance.reset();
        }
    }
}
