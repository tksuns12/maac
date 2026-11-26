//! Cross-platform CLAP 1.2 dynamic plugin scanner and host backend.

use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::ptr;

use libloading::Library;
use maac::dsp::{HostBlockContext, HostNoteEvent};

use super::clap_abi::*;
use super::{PluginDescriptor, PluginFormat, PluginInstance, PluginKind, PluginParamDescriptor};

/// Standard system and user directories containing `.clap` bundles/libraries.
fn clap_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        dirs.push(PathBuf::from("/usr/lib/clap"));
        dirs.push(PathBuf::from("/usr/local/lib/clap"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join(".clap"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(common) = std::env::var("CommonProgramFiles") {
            dirs.push(PathBuf::from(common).join("CLAP"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs\\Common\\CLAP"));
        }
    }
    if let Ok(extra) = std::env::var("CLAP_PATH") {
        for part in std::env::split_paths(&extra) {
            dirs.push(part);
        }
    }
    dirs
}

/// Resolve the dynamic library binary inside a `.clap` path.
/// On macOS, `.clap` is a bundle directory containing `Contents/MacOS/<binary>`.
fn resolve_clap_binary(clap_path: &Path) -> Option<PathBuf> {
    if clap_path.is_file() {
        return Some(clap_path.to_path_buf());
    }
    #[cfg(target_os = "macos")]
    {
        let macos_dir = clap_path.join("Contents/MacOS");
        if macos_dir.is_dir() {
            // Prefer executable matching bundle stem; otherwise first regular file.
            if let Some(stem) = clap_path.file_stem() {
                let candidate = macos_dir.join(stem);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            if let Ok(entries) = std::fs::read_dir(&macos_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

fn cstr_opt(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

fn classify_clap_features(features: *const *const c_char) -> PluginKind {
    if features.is_null() {
        return PluginKind::Effect;
    }
    let mut i = 0usize;
    loop {
        let feat_ptr = unsafe { *features.add(i) };
        if feat_ptr.is_null() || i > 64 {
            break;
        }
        let s = cstr_opt(feat_ptr);
        if s == "instrument" || s == "synthesizer" || s == "sampler" || s == "drum-machine" {
            return PluginKind::Instrument;
        }
        i += 1;
    }
    PluginKind::Effect
}

/// Enumerate all `.clap` plugins found in standard CLAP directories.
pub fn scan_clap_plugins() -> Vec<PluginDescriptor> {
    let mut out = Vec::new();
    for dir in clap_search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("clap") {
                continue;
            }
            if let Ok(found) = scan_clap_bundle(&path) {
                out.extend(found);
            }
        }
    }
    out
}

fn scan_clap_bundle(clap_path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    let bin_path = resolve_clap_binary(clap_path)
        .ok_or_else(|| format!("no CLAP binary inside {}", clap_path.display()))?;
    let path_cstr = CString::new(clap_path.to_string_lossy().as_bytes())
        .map_err(|_| "CLAP path contains interior NUL".to_string())?;

    let lib = unsafe { Library::new(&bin_path) }
        .map_err(|err| format!("failed to dlopen {}: {err}", bin_path.display()))?;
    let entry_sym: libloading::Symbol<'_, *const clap_plugin_entry_t> =
        unsafe { lib.get(b"clap_entry\0") }
            .map_err(|err| format!("missing clap_entry in {}: {err}", bin_path.display()))?;
    let entry_ptr = *entry_sym;
    if entry_ptr.is_null() {
        return Err("clap_entry is null".into());
    }
    let entry = unsafe { &*entry_ptr };
    if let Some(init) = entry.init {
        if !unsafe { init(path_cstr.as_ptr()) } {
            return Err(format!(
                "clap_entry.init failed for {}",
                clap_path.display()
            ));
        }
    }

    let mut out = Vec::new();
    if let Some(get_factory) = entry.get_factory {
        let factory_ptr =
            unsafe { get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) } as *const clap_plugin_factory_t;
        if !factory_ptr.is_null() {
            let factory = unsafe { &*factory_ptr };
            let count = factory
                .get_plugin_count
                .map_or(0, |f| unsafe { f(factory_ptr) });
            for index in 0..count {
                let desc_ptr = factory
                    .get_plugin_descriptor
                    .map_or(ptr::null(), |f| unsafe { f(factory_ptr, index) });
                if desc_ptr.is_null() {
                    continue;
                }
                let desc = unsafe { &*desc_ptr };
                let raw_id = cstr_opt(desc.id);
                let name = cstr_opt(desc.name);
                let vendor = cstr_opt(desc.vendor);
                if raw_id.is_empty() || name.is_empty() {
                    continue;
                }
                out.push(PluginDescriptor {
                    id: format!("clap:{raw_id}"),
                    name,
                    vendor: if vendor.is_empty() {
                        "CLAP".into()
                    } else {
                        vendor
                    },
                    format: PluginFormat::Clap,
                    kind: classify_clap_features(desc.features),
                    path: Some(clap_path.to_path_buf()),
                });
            }
        }
    }

    if let Some(deinit) = entry.deinit {
        unsafe { deinit() };
    }
    Ok(out)
}

// ---- Host callbacks & event queues ---------------------------------------

unsafe extern "C" fn host_get_extension(
    _host: *const clap_host_t,
    _extension_id: *const c_char,
) -> *const c_void {
    ptr::null()
}

unsafe extern "C" fn host_noop(_host: *const clap_host_t) {}

#[derive(Clone, Copy)]
enum QueuedEvent {
    Note(clap_event_note_t),
    Param(clap_event_param_value_t),
}

impl QueuedEvent {
    fn time(&self) -> u32 {
        match self {
            Self::Note(e) => e.header.time,
            Self::Param(e) => e.header.time,
        }
    }
    fn as_header_ptr(&self) -> *const clap_event_header_t {
        match self {
            Self::Note(e) => &e.header as *const clap_event_header_t,
            Self::Param(e) => &e.header as *const clap_event_header_t,
        }
    }
}

unsafe extern "C" fn input_events_size(list: *const clap_input_events_t) -> u32 {
    let vec = &*((*list).ctx as *const Vec<QueuedEvent>);
    vec.len() as u32
}

unsafe extern "C" fn input_events_get(
    list: *const clap_input_events_t,
    index: u32,
) -> *const clap_event_header_t {
    let vec = &*((*list).ctx as *const Vec<QueuedEvent>);
    vec.get(index as usize)
        .map_or(ptr::null(), QueuedEvent::as_header_ptr)
}

unsafe extern "C" fn output_events_try_push(
    _list: *const clap_output_events_t,
    _event: *const clap_event_header_t,
) -> bool {
    true
}

unsafe extern "C" fn ostream_write(
    stream: *const clap_ostream_t,
    buffer: *const c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let vec = &mut *((*stream).ctx as *mut Vec<u8>);
    let slice = std::slice::from_raw_parts(buffer as *const u8, size as usize);
    vec.extend_from_slice(slice);
    size as i64
}

struct ReadCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

unsafe extern "C" fn istream_read(
    stream: *const clap_istream_t,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let cursor = &mut *((*stream).ctx as *mut ReadCursor<'_>);
    let remaining = cursor.bytes.len().saturating_sub(cursor.pos);
    let take = remaining.min(size as usize);
    if take > 0 {
        ptr::copy_nonoverlapping(
            cursor.bytes.as_ptr().add(cursor.pos),
            buffer as *mut u8,
            take,
        );
        cursor.pos += take;
    }
    take as i64
}

pub struct ClapInstance {
    descriptor: PluginDescriptor,
    /// Dynamic library handle kept alive until after `plugin.destroy` & `entry.deinit`.
    _lib: Option<Library>,
    entry_deinit: Option<unsafe extern "C" fn()>,
    _host: Box<clap_host_t>,
    plugin: *const clap_plugin_t,
    params_ext: *const clap_plugin_params_t,
    state_ext: *const clap_plugin_state_t,
    activated: bool,
    processing: bool,
    params: Vec<PluginParamDescriptor>,
    param_cookies: BTreeMap<u32, *mut c_void>,
    pending_params: Vec<(u32, f64)>,
    in_left: Vec<f32>,
    in_right: Vec<f32>,
    out_left: Vec<f32>,
    out_right: Vec<f32>,
}

// SAFETY: `ClapInstance` owns its plugin pointer & library handle exclusively
// and is synchronized behind `Mutex<HostRack>`.
unsafe impl Send for ClapInstance {}

impl ClapInstance {
    pub fn new(descriptor: &PluginDescriptor) -> Result<Self, String> {
        let clap_path = descriptor
            .path
            .as_deref()
            .ok_or_else(|| format!("missing CLAP bundle path for {}", descriptor.name))?;
        let bin_path = resolve_clap_binary(clap_path)
            .ok_or_else(|| format!("no CLAP binary inside {}", clap_path.display()))?;
        let path_cstr = CString::new(clap_path.to_string_lossy().as_bytes())
            .map_err(|_| "CLAP path contains interior NUL".to_string())?;
        let lib = unsafe { Library::new(&bin_path) }
            .map_err(|err| format!("failed to load {}: {err}", bin_path.display()))?;
        let entry_sym: libloading::Symbol<'_, *const clap_plugin_entry_t> =
            unsafe { lib.get(b"clap_entry\0") }
                .map_err(|err| format!("missing clap_entry: {err}"))?;
        let entry_ptr = *entry_sym;
        if entry_ptr.is_null() {
            return Err("clap_entry is null".into());
        }
        let entry = unsafe { &*entry_ptr };
        if let Some(init) = entry.init {
            if !unsafe { init(path_cstr.as_ptr()) } {
                return Err(format!("clap_entry.init failed for {}", descriptor.name));
            }
        }
        Self::from_entry(descriptor, Some(lib), entry)
    }

    /// Instantiate directly from an in-process `clap_plugin_entry_t` (also used
    /// by unit tests to exercise the complete CLAP 1.2 C ABI without external files).
    pub(crate) fn from_entry(
        descriptor: &PluginDescriptor,
        lib: Option<Library>,
        entry: &clap_plugin_entry_t,
    ) -> Result<Self, String> {
        let raw_id = descriptor
            .id
            .strip_prefix("clap:")
            .unwrap_or(&descriptor.id);
        let id_cstr =
            CString::new(raw_id).map_err(|_| "CLAP plugin id contains interior NUL".to_string())?;
        let get_factory = entry
            .get_factory
            .ok_or_else(|| "CLAP entry missing get_factory".to_string())?;
        let factory_ptr =
            unsafe { get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) } as *const clap_plugin_factory_t;
        if factory_ptr.is_null() {
            return Err("CLAP bundle has no clap.plugin-factory".into());
        }
        let factory = unsafe { &*factory_ptr };
        let create_plugin = factory
            .create_plugin
            .ok_or_else(|| "CLAP factory missing create_plugin".to_string())?;

        let host = Box::new(clap_host_t {
            clap_version: CLAP_VERSION_INIT,
            host_data: ptr::null_mut(),
            name: c"MaaC Studio".as_ptr(),
            vendor: c"MaaC".as_ptr(),
            url: c"https://github.com/tksuns12/maac".as_ptr(),
            version: c"0.1.0".as_ptr(),
            get_extension: Some(host_get_extension),
            request_restart: Some(host_noop),
            request_process: Some(host_noop),
            request_callback: Some(host_noop),
        });

        let plugin_ptr = unsafe { create_plugin(factory_ptr, &*host, id_cstr.as_ptr()) };
        if plugin_ptr.is_null() {
            return Err(format!("CLAP create_plugin failed for {}", descriptor.name));
        }
        let plugin = unsafe { &*plugin_ptr };
        if let Some(init) = plugin.init {
            if !unsafe { init(plugin_ptr) } {
                if let Some(destroy) = plugin.destroy {
                    unsafe { destroy(plugin_ptr) };
                }
                return Err(format!("CLAP plugin.init failed for {}", descriptor.name));
            }
        }

        let params_ext = plugin.get_extension.map_or(ptr::null(), |f| unsafe {
            f(plugin_ptr, CLAP_EXT_PARAMS.as_ptr())
        }) as *const clap_plugin_params_t;
        let state_ext = plugin.get_extension.map_or(ptr::null(), |f| unsafe {
            f(plugin_ptr, CLAP_EXT_STATE.as_ptr())
        }) as *const clap_plugin_state_t;

        let mut inst = Self {
            descriptor: descriptor.clone(),
            _lib: lib,
            entry_deinit: entry.deinit,
            _host: host,
            plugin: plugin_ptr,
            params_ext,
            state_ext,
            activated: false,
            processing: false,
            params: Vec::new(),
            param_cookies: BTreeMap::new(),
            pending_params: Vec::new(),
            in_left: vec![0.0; 2_048],
            in_right: vec![0.0; 2_048],
            out_left: vec![0.0; 2_048],
            out_right: vec![0.0; 2_048],
        };
        inst.discover_parameters();
        Ok(inst)
    }

    fn discover_parameters(&mut self) {
        self.params.clear();
        self.param_cookies.clear();
        if self.params_ext.is_null() {
            return;
        }
        let ext = unsafe { &*self.params_ext };
        let count = ext.count.map_or(0, |f| unsafe { f(self.plugin) });
        for idx in 0..count {
            let mut info: clap_param_info_t = unsafe { std::mem::zeroed() };
            let ok = ext
                .get_info
                .is_some_and(|f| unsafe { f(self.plugin, idx, &mut info) });
            if !ok {
                continue;
            }
            if (info.flags & (CLAP_PARAM_IS_READONLY | CLAP_PARAM_IS_HIDDEN)) != 0 {
                continue;
            }
            if !info.min_value.is_finite()
                || !info.max_value.is_finite()
                || info.max_value <= info.min_value
            {
                continue;
            }
            let name = cstr_opt(info.name.as_ptr());
            let min = info.min_value as f32;
            let max = info.max_value as f32;
            let default = (info.default_value as f32).clamp(min, max);
            self.param_cookies.insert(info.id, info.cookie);
            self.params.push(PluginParamDescriptor {
                id: info.id,
                name: if name.is_empty() {
                    format!("Param {}", info.id)
                } else {
                    name
                },
                unit_label: String::new(),
                min,
                max,
                default,
                logarithmic: false,
            });
        }
    }

    fn run_clap_process(
        &mut self,
        ctx: HostBlockContext,
        note_events: &[HostNoteEvent],
        has_audio_in: bool,
    ) {
        if !self.activated || ctx.frames == 0 {
            return;
        }
        let frames = ctx.frames;
        if self.out_left.len() < frames {
            self.out_left.resize(frames, 0.0);
            self.out_right.resize(frames, 0.0);
        }
        self.out_left[..frames].fill(0.0);
        self.out_right[..frames].fill(0.0);

        let mut events = Vec::with_capacity(note_events.len() + self.pending_params.len());
        for (param_id, value) in self.pending_params.drain(..) {
            let cookie = self
                .param_cookies
                .get(&param_id)
                .copied()
                .unwrap_or(ptr::null_mut());
            events.push(QueuedEvent::Param(clap_event_param_value_t {
                header: clap_event_header_t {
                    size: size_of::<clap_event_param_value_t>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id,
                cookie,
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            }));
        }
        for ev in note_events {
            events.push(QueuedEvent::Note(clap_event_note_t {
                header: clap_event_header_t {
                    size: size_of::<clap_event_note_t>() as u32,
                    time: ev.frame_offset as u32,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: if ev.is_note_on {
                        CLAP_EVENT_NOTE_ON
                    } else {
                        CLAP_EVENT_NOTE_OFF
                    },
                    flags: 0,
                },
                note_id: -1,
                port_index: 0,
                channel: 0,
                key: i16::from(ev.midi_note),
                velocity: f64::from(ev.velocity),
            }));
        }
        events.sort_by_key(QueuedEvent::time);

        let in_events = clap_input_events_t {
            ctx: (&events as *const Vec<QueuedEvent>) as *mut c_void,
            size: Some(input_events_size),
            get: Some(input_events_get),
        };
        let out_events = clap_output_events_t {
            ctx: ptr::null_mut(),
            try_push: Some(output_events_try_push),
        };
        let transport = clap_event_transport_t {
            header: clap_event_header_t {
                size: size_of::<clap_event_transport_t>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: 9,
                flags: 0,
            },
            flags: CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_IS_PLAYING,
            song_pos_beats: 0,
            song_pos_seconds: 0,
            tempo: ctx.tempo_bpm,
            tempo_inc: 0.0,
            loop_start_beats: 0,
            loop_end_beats: 0,
            loop_start_seconds: 0,
            loop_end_seconds: 0,
            bar_start: 0,
            bar_number: 0,
            tsig_num: 4,
            tsig_denom: 4,
        };

        let mut in_ptrs = [self.in_left.as_mut_ptr(), self.in_right.as_mut_ptr()];
        let mut out_ptrs = [self.out_left.as_mut_ptr(), self.out_right.as_mut_ptr()];
        let audio_in = clap_audio_buffer_t {
            data32: in_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut audio_out = clap_audio_buffer_t {
            data32: out_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let proc_data = clap_process_t {
            steady_time: ctx.start_frame as i64,
            frames_count: frames as u32,
            transport: &transport,
            audio_inputs: if has_audio_in { &audio_in } else { ptr::null() },
            audio_outputs: &mut audio_out,
            audio_inputs_count: u32::from(has_audio_in),
            audio_outputs_count: 1,
            in_events: &in_events,
            out_events: &out_events,
        };

        let plugin = unsafe { &*self.plugin };
        if let Some(process) = plugin.process {
            let _ = unsafe { process(self.plugin, &proc_data) };
        }
    }
}

impl PluginInstance for ClapInstance {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn activate(&mut self, sample_rate: f64, max_block_frames: usize) -> Result<(), String> {
        let plugin = unsafe { &*self.plugin };
        if self.processing {
            if let Some(stop) = plugin.stop_processing {
                unsafe { stop(self.plugin) };
            }
            self.processing = false;
        }
        if self.activated {
            if let Some(deact) = plugin.deactivate {
                unsafe { deact(self.plugin) };
            }
            self.activated = false;
        }
        let max_f = max_block_frames.max(64);
        if self.in_left.len() < max_f {
            self.in_left.resize(max_f, 0.0);
            self.in_right.resize(max_f, 0.0);
            self.out_left.resize(max_f, 0.0);
            self.out_right.resize(max_f, 0.0);
        }
        if let Some(act) = plugin.activate {
            if !unsafe { act(self.plugin, sample_rate, 1, max_f as u32) } {
                return Err(format!("CLAP activate failed for {}", self.descriptor.name));
            }
        }
        self.activated = true;
        if let Some(start) = plugin.start_processing {
            self.processing = unsafe { start(self.plugin) };
        }
        Ok(())
    }

    fn reset(&mut self) {
        if !self.activated {
            return;
        }
        let plugin = unsafe { &*self.plugin };
        if let Some(reset) = plugin.reset {
            unsafe { reset(self.plugin) };
        }
    }

    fn params(&self) -> &[PluginParamDescriptor] {
        &self.params
    }

    fn get_param(&self, param_id: u32) -> Option<f32> {
        if self.params_ext.is_null() {
            return None;
        }
        let ext = unsafe { &*self.params_ext };
        let mut val: f64 = 0.0;
        let ok = ext
            .get_value
            .is_some_and(|f| unsafe { f(self.plugin, param_id, &mut val) });
        (ok && val.is_finite()).then_some(val as f32)
    }

    fn set_param(&mut self, param_id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        self.pending_params.push((param_id, f64::from(value)));
        // Also flush immediately if the plugin supports params.flush so `get_param`
        // reflects the updated value even while transport is paused.
        if !self.params_ext.is_null() {
            let ext = unsafe { &*self.params_ext };
            if let Some(flush) = ext.flush {
                let cookie = self
                    .param_cookies
                    .get(&param_id)
                    .copied()
                    .unwrap_or(ptr::null_mut());
                let ev = vec![QueuedEvent::Param(clap_event_param_value_t {
                    header: clap_event_header_t {
                        size: size_of::<clap_event_param_value_t>() as u32,
                        time: 0,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_PARAM_VALUE,
                        flags: 0,
                    },
                    param_id,
                    cookie,
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value: f64::from(value),
                })];
                let in_events = clap_input_events_t {
                    ctx: (&ev as *const Vec<QueuedEvent>) as *mut c_void,
                    size: Some(input_events_size),
                    get: Some(input_events_get),
                };
                let out_events = clap_output_events_t {
                    ctx: ptr::null_mut(),
                    try_push: Some(output_events_try_push),
                };
                unsafe { flush(self.plugin, &in_events, &out_events) };
            }
        }
    }

    fn process_instrument(
        &mut self,
        ctx: HostBlockContext,
        events: &[HostNoteEvent],
        channels: usize,
        out: &mut [f64],
    ) {
        if channels == 0 || ctx.frames == 0 {
            return;
        }
        self.run_clap_process(ctx, events, false);
        for i in 0..ctx.frames {
            let l = f64::from(self.out_left[i]);
            let r = f64::from(self.out_right[i]);
            if channels == 1 {
                out[i] = 0.5 * (l + r);
            } else {
                out[i * channels] = l;
                out[i * channels + 1] = r;
            }
        }
    }

    fn process_effect(&mut self, ctx: HostBlockContext, channels: usize, io: &mut [f64]) {
        if channels == 0 || ctx.frames == 0 {
            return;
        }
        let frames = ctx.frames;
        if self.in_left.len() < frames {
            self.in_left.resize(frames, 0.0);
            self.in_right.resize(frames, 0.0);
        }
        for i in 0..frames {
            let l = io[i * channels] as f32;
            let r = if channels >= 2 {
                io[i * channels + 1] as f32
            } else {
                l
            };
            self.in_left[i] = l;
            self.in_right[i] = r;
        }
        self.run_clap_process(ctx, &[], true);
        for i in 0..frames {
            let l = f64::from(self.out_left[i]);
            let r = f64::from(self.out_right[i]);
            if channels == 1 {
                io[i] = 0.5 * (l + r);
            } else {
                io[i * channels] = l;
                io[i * channels + 1] = r;
            }
        }
    }

    fn save_state(&self) -> Result<Vec<u8>, String> {
        if self.state_ext.is_null() {
            return Ok(Vec::new());
        }
        let ext = unsafe { &*self.state_ext };
        let Some(save) = ext.save else {
            return Ok(Vec::new());
        };
        let mut bytes = Vec::new();
        let ostream = clap_ostream_t {
            ctx: (&mut bytes as *mut Vec<u8>) as *mut c_void,
            write: Some(ostream_write),
        };
        if !unsafe { save(self.plugin, &ostream) } {
            return Err(format!(
                "CLAP state.save failed for {}",
                self.descriptor.name
            ));
        }
        Ok(bytes)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() || self.state_ext.is_null() {
            return Ok(());
        }
        let ext = unsafe { &*self.state_ext };
        let Some(load) = ext.load else {
            return Ok(());
        };
        let mut cursor = ReadCursor { bytes, pos: 0 };
        let istream = clap_istream_t {
            ctx: (&mut cursor as *mut ReadCursor<'_>) as *mut c_void,
            read: Some(istream_read),
        };
        if !unsafe { load(self.plugin, &istream) } {
            return Err(format!(
                "CLAP state.load failed for {}",
                self.descriptor.name
            ));
        }
        Ok(())
    }
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        if !self.plugin.is_null() {
            let plugin = unsafe { &*self.plugin };
            if self.processing {
                if let Some(stop) = plugin.stop_processing {
                    unsafe { stop(self.plugin) };
                }
            }
            if self.activated {
                if let Some(deact) = plugin.deactivate {
                    unsafe { deact(self.plugin) };
                }
            }
            if let Some(destroy) = plugin.destroy {
                unsafe { destroy(self.plugin) };
            }
        }
        if let Some(deinit) = self.entry_deinit {
            unsafe { deinit() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockGainState {
        gain: f64,
    }

    struct SyncWrap<T>(T);
    unsafe impl<T> Sync for SyncWrap<T> {}

    static MOCK_FEATURES: SyncWrap<[*const c_char; 2]> =
        SyncWrap([c"audio-effect".as_ptr(), ptr::null()]);
    static MOCK_DESC: SyncWrap<clap_plugin_descriptor_t> = SyncWrap(clap_plugin_descriptor_t {
        clap_version: CLAP_VERSION_INIT,
        id: c"com.maac.test.gain".as_ptr(),
        name: c"Mock CLAP Gain".as_ptr(),
        vendor: c"MaaC Test".as_ptr(),
        url: ptr::null(),
        manual_url: ptr::null(),
        support_url: ptr::null(),
        version: c"1.0".as_ptr(),
        description: ptr::null(),
        features: MOCK_FEATURES.0.as_ptr(),
    });

    unsafe extern "C" fn mock_params_count(_: *const clap_plugin_t) -> u32 {
        1
    }
    unsafe extern "C" fn mock_params_info(
        _: *const clap_plugin_t,
        index: u32,
        out: *mut clap_param_info_t,
    ) -> bool {
        if index != 0 {
            return false;
        }
        let info = &mut *out;
        *info = std::mem::zeroed();
        info.id = 42;
        for (i, b) in b"Gain\0".iter().enumerate() {
            info.name[i] = *b as c_char;
        }
        info.min_value = 0.0;
        info.max_value = 2.0;
        info.default_value = 1.0;
        true
    }
    unsafe extern "C" fn mock_params_get(
        plugin: *const clap_plugin_t,
        id: clap_id,
        out: *mut f64,
    ) -> bool {
        if id != 42 {
            return false;
        }
        let st = &*((*plugin).plugin_data as *const MockGainState);
        *out = st.gain;
        true
    }
    unsafe extern "C" fn mock_params_flush(
        plugin: *const clap_plugin_t,
        in_events: *const clap_input_events_t,
        _: *const clap_output_events_t,
    ) {
        let st = &mut *((*plugin).plugin_data as *mut MockGainState);
        let n = ((*in_events).size.unwrap())(in_events);
        for i in 0..n {
            let hdr = ((*in_events).get.unwrap())(in_events, i);
            if (*hdr).type_ == CLAP_EVENT_PARAM_VALUE {
                let ev = &*(hdr as *const clap_event_param_value_t);
                if ev.param_id == 42 {
                    st.gain = ev.value;
                }
            }
        }
    }
    unsafe extern "C" fn mock_state_save(
        plugin: *const clap_plugin_t,
        stream: *const clap_ostream_t,
    ) -> bool {
        let st = &*((*plugin).plugin_data as *const MockGainState);
        let bytes = st.gain.to_le_bytes();
        ((*stream).write.unwrap())(stream, bytes.as_ptr() as *const c_void, 8) == 8
    }
    unsafe extern "C" fn mock_state_load(
        plugin: *const clap_plugin_t,
        stream: *const clap_istream_t,
    ) -> bool {
        let st = &mut *((*plugin).plugin_data as *mut MockGainState);
        let mut bytes = [0u8; 8];
        if ((*stream).read.unwrap())(stream, bytes.as_mut_ptr() as *mut c_void, 8) != 8 {
            return false;
        }
        st.gain = f64::from_le_bytes(bytes);
        true
    }

    static MOCK_PARAMS_EXT: clap_plugin_params_t = clap_plugin_params_t {
        count: Some(mock_params_count),
        get_info: Some(mock_params_info),
        get_value: Some(mock_params_get),
        value_to_text: None,
        text_to_value: None,
        flush: Some(mock_params_flush),
    };
    static MOCK_STATE_EXT: clap_plugin_state_t = clap_plugin_state_t {
        save: Some(mock_state_save),
        load: Some(mock_state_load),
    };

    unsafe extern "C" fn mock_init(_: *const clap_plugin_t) -> bool {
        true
    }
    unsafe extern "C" fn mock_destroy(plugin: *const clap_plugin_t) {
        let p = Box::from_raw(plugin as *mut clap_plugin_t);
        let _ = Box::from_raw(p.plugin_data as *mut MockGainState);
    }
    unsafe extern "C" fn mock_activate(_: *const clap_plugin_t, _: f64, _: u32, _: u32) -> bool {
        true
    }
    unsafe extern "C" fn mock_start(_: *const clap_plugin_t) -> bool {
        true
    }
    unsafe extern "C" fn mock_process(
        plugin: *const clap_plugin_t,
        proc: *const clap_process_t,
    ) -> i32 {
        let st = &mut *((*plugin).plugin_data as *mut MockGainState);
        mock_params_flush(plugin, (*proc).in_events, (*proc).out_events);
        let frames = (*proc).frames_count as usize;
        let in_buf = &*(*proc).audio_inputs;
        let out_buf = &mut *(*proc).audio_outputs;
        for ch in 0..2 {
            let src = std::slice::from_raw_parts(*in_buf.data32.add(ch), frames);
            let dst = std::slice::from_raw_parts_mut(*out_buf.data32.add(ch), frames);
            for (d, &s) in dst.iter_mut().zip(src) {
                *d = (f64::from(s) * st.gain) as f32;
            }
        }
        CLAP_PROCESS_CONTINUE
    }
    unsafe extern "C" fn mock_get_ext(_: *const clap_plugin_t, id: *const c_char) -> *const c_void {
        let s = CStr::from_ptr(id);
        if s == CLAP_EXT_PARAMS {
            &MOCK_PARAMS_EXT as *const clap_plugin_params_t as *const c_void
        } else if s == CLAP_EXT_STATE {
            &MOCK_STATE_EXT as *const clap_plugin_state_t as *const c_void
        } else {
            ptr::null()
        }
    }

    unsafe extern "C" fn mock_factory_count(_: *const clap_plugin_factory_t) -> u32 {
        1
    }
    unsafe extern "C" fn mock_factory_desc(
        _: *const clap_plugin_factory_t,
        idx: u32,
    ) -> *const clap_plugin_descriptor_t {
        if idx == 0 {
            &MOCK_DESC.0
        } else {
            ptr::null()
        }
    }
    unsafe extern "C" fn mock_factory_create(
        _: *const clap_plugin_factory_t,
        _: *const clap_host_t,
        _: *const c_char,
    ) -> *const clap_plugin_t {
        let state = Box::into_raw(Box::new(MockGainState { gain: 1.0 }));
        let plugin = Box::new(clap_plugin_t {
            desc: &MOCK_DESC.0,
            plugin_data: state as *mut c_void,
            init: Some(mock_init),
            destroy: Some(mock_destroy),
            activate: Some(mock_activate),
            deactivate: None,
            start_processing: Some(mock_start),
            stop_processing: None,
            reset: None,
            process: Some(mock_process),
            get_extension: Some(mock_get_ext),
            on_main_thread: None,
        });
        Box::into_raw(plugin)
    }

    static MOCK_FACTORY: clap_plugin_factory_t = clap_plugin_factory_t {
        get_plugin_count: Some(mock_factory_count),
        get_plugin_descriptor: Some(mock_factory_desc),
        create_plugin: Some(mock_factory_create),
    };

    unsafe extern "C" fn mock_entry_factory(id: *const c_char) -> *const c_void {
        if CStr::from_ptr(id) == CLAP_PLUGIN_FACTORY_ID {
            &MOCK_FACTORY as *const clap_plugin_factory_t as *const c_void
        } else {
            ptr::null()
        }
    }

    #[test]
    fn clap_abi_lifecycle_params_audio_and_state_roundtrip() {
        let entry = clap_plugin_entry_t {
            clap_version: CLAP_VERSION_INIT,
            init: None,
            deinit: None,
            get_factory: Some(mock_entry_factory),
        };
        let desc = PluginDescriptor {
            id: "clap:com.maac.test.gain".into(),
            name: "Mock CLAP Gain".into(),
            vendor: "MaaC Test".into(),
            format: PluginFormat::Clap,
            kind: PluginKind::Effect,
            path: None,
        };
        let mut inst = ClapInstance::from_entry(&desc, None, &entry).expect("create CLAP");
        inst.activate(48_000.0, 512).expect("activate CLAP");
        assert_eq!(inst.params().len(), 1);
        assert_eq!(inst.params()[0].id, 42);
        assert_eq!(inst.get_param(42), Some(1.0));

        inst.set_param(42, 0.25);
        assert_eq!(inst.get_param(42), Some(0.25));

        let mut io = vec![0.8_f64; 64 * 2];
        let ctx = HostBlockContext {
            start_frame: 0,
            frames: 64,
            sample_rate_hz: 48_000.0,
            tempo_bpm: 120.0,
        };
        inst.process_effect(ctx, 2, &mut io);
        for s in &io {
            assert!(
                (*s - 0.2).abs() < 1e-5,
                "expected 0.8 * 0.25 = 0.2, got {s}"
            );
        }

        let state = inst.save_state().expect("save CLAP state");
        assert_eq!(state.len(), 8);
        let mut inst2 = ClapInstance::from_entry(&desc, None, &entry).expect("second CLAP");
        inst2.activate(48_000.0, 512).expect("activate second");
        inst2.load_state(&state).expect("load CLAP state");
        assert_eq!(inst2.get_param(42), Some(0.25));
    }
}
