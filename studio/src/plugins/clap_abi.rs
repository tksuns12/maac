//! Self-contained MIT-compatible CLAP 1.2 C ABI definitions.
//!
//! Reference: https://github.com/free-audio/clap (MIT License).
//! Only the core host/plugin lifecycle, audio/note ports, params, events, and
//! state stream structs are included so `studio/` compiles cleanly offline
//! with zero external C SDK dependencies.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_void};

pub const CLAP_VERSION_MAJOR: u32 = 1;
pub const CLAP_VERSION_MINOR: u32 = 2;
pub const CLAP_VERSION_REVISION: u32 = 0;

pub const CLAP_VERSION_INIT: clap_version_t = clap_version_t {
    major: CLAP_VERSION_MAJOR,
    minor: CLAP_VERSION_MINOR,
    revision: CLAP_VERSION_REVISION,
};

pub const CLAP_CORE_EVENT_SPACE_ID: u16 = 0;
pub const CLAP_EVENT_NOTE_ON: u16 = 0;
pub const CLAP_EVENT_NOTE_OFF: u16 = 1;
pub const CLAP_EVENT_PARAM_VALUE: u16 = 5;

pub const CLAP_PROCESS_ERROR: i32 = 0;
pub const CLAP_PROCESS_CONTINUE: i32 = 1;
pub const CLAP_PROCESS_CONTINUE_IF_NOT_QUIET: i32 = 2;
pub const CLAP_PROCESS_TAIL: i32 = 3;
pub const CLAP_PROCESS_SLEEP: i32 = 4;

pub const CLAP_TRANSPORT_HAS_TEMPO: u32 = 1 << 0;
pub const CLAP_TRANSPORT_IS_PLAYING: u32 = 1 << 5;

pub const CLAP_PARAM_IS_READONLY: u32 = 1 << 3;
pub const CLAP_PARAM_IS_HIDDEN: u32 = 1 << 4;

pub const CLAP_PLUGIN_FACTORY_ID: &std::ffi::CStr = c"clap.plugin-factory";
pub const CLAP_EXT_PARAMS: &std::ffi::CStr = c"clap.params";
pub const CLAP_EXT_STATE: &std::ffi::CStr = c"clap.state";
pub const CLAP_EXT_AUDIO_PORTS: &std::ffi::CStr = c"clap.audio-ports";

pub type clap_id = u32;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct clap_version_t {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

#[repr(C)]
pub struct clap_plugin_entry_t {
    pub clap_version: clap_version_t,
    pub init: Option<unsafe extern "C" fn(plugin_path: *const c_char) -> bool>,
    pub deinit: Option<unsafe extern "C" fn()>,
    pub get_factory: Option<unsafe extern "C" fn(factory_id: *const c_char) -> *const c_void>,
}

#[repr(C)]
pub struct clap_plugin_descriptor_t {
    pub clap_version: clap_version_t,
    pub id: *const c_char,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub manual_url: *const c_char,
    pub support_url: *const c_char,
    pub version: *const c_char,
    pub description: *const c_char,
    pub features: *const *const c_char,
}

#[repr(C)]
pub struct clap_plugin_factory_t {
    pub get_plugin_count:
        Option<unsafe extern "C" fn(factory: *const clap_plugin_factory_t) -> u32>,
    pub get_plugin_descriptor: Option<
        unsafe extern "C" fn(
            factory: *const clap_plugin_factory_t,
            index: u32,
        ) -> *const clap_plugin_descriptor_t,
    >,
    pub create_plugin: Option<
        unsafe extern "C" fn(
            factory: *const clap_plugin_factory_t,
            host: *const clap_host_t,
            plugin_id: *const c_char,
        ) -> *const clap_plugin_t,
    >,
}

#[repr(C)]
pub struct clap_host_t {
    pub clap_version: clap_version_t,
    pub host_data: *mut c_void,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub version: *const c_char,
    pub get_extension: Option<
        unsafe extern "C" fn(
            host: *const clap_host_t,
            extension_id: *const c_char,
        ) -> *const c_void,
    >,
    pub request_restart: Option<unsafe extern "C" fn(host: *const clap_host_t)>,
    pub request_process: Option<unsafe extern "C" fn(host: *const clap_host_t)>,
    pub request_callback: Option<unsafe extern "C" fn(host: *const clap_host_t)>,
}

#[repr(C)]
pub struct clap_plugin_t {
    pub desc: *const clap_plugin_descriptor_t,
    pub plugin_data: *mut c_void,
    pub init: Option<unsafe extern "C" fn(plugin: *const clap_plugin_t) -> bool>,
    pub destroy: Option<unsafe extern "C" fn(plugin: *const clap_plugin_t)>,
    pub activate: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin_t,
            sample_rate: f64,
            min_frames_count: u32,
            max_frames_count: u32,
        ) -> bool,
    >,
    pub deactivate: Option<unsafe extern "C" fn(plugin: *const clap_plugin_t)>,
    pub start_processing: Option<unsafe extern "C" fn(plugin: *const clap_plugin_t) -> bool>,
    pub stop_processing: Option<unsafe extern "C" fn(plugin: *const clap_plugin_t)>,
    pub reset: Option<unsafe extern "C" fn(plugin: *const clap_plugin_t)>,
    pub process: Option<
        unsafe extern "C" fn(plugin: *const clap_plugin_t, process: *const clap_process_t) -> i32,
    >,
    pub get_extension: Option<
        unsafe extern "C" fn(plugin: *const clap_plugin_t, id: *const c_char) -> *const c_void,
    >,
    pub on_main_thread: Option<unsafe extern "C" fn(plugin: *const clap_plugin_t)>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct clap_event_header_t {
    pub size: u32,
    pub time: u32,
    pub space_id: u16,
    pub type_: u16,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct clap_event_note_t {
    pub header: clap_event_header_t,
    pub note_id: i32,
    pub port_index: i16,
    pub channel: i16,
    pub key: i16,
    pub velocity: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct clap_event_param_value_t {
    pub header: clap_event_header_t,
    pub param_id: clap_id,
    pub cookie: *mut c_void,
    pub note_id: i32,
    pub port_index: i16,
    pub channel: i16,
    pub key: i16,
    pub value: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct clap_event_transport_t {
    pub header: clap_event_header_t,
    pub flags: u32,
    pub song_pos_beats: i64,
    pub song_pos_seconds: i64,
    pub tempo: f64,
    pub tempo_inc: f64,
    pub loop_start_beats: i64,
    pub loop_end_beats: i64,
    pub loop_start_seconds: i64,
    pub loop_end_seconds: i64,
    pub bar_start: i64,
    pub bar_number: i32,
    pub tsig_num: u16,
    pub tsig_denom: u16,
}

#[repr(C)]
pub struct clap_input_events_t {
    pub ctx: *mut c_void,
    pub size: Option<unsafe extern "C" fn(list: *const clap_input_events_t) -> u32>,
    pub get: Option<
        unsafe extern "C" fn(
            list: *const clap_input_events_t,
            index: u32,
        ) -> *const clap_event_header_t,
    >,
}

#[repr(C)]
pub struct clap_output_events_t {
    pub ctx: *mut c_void,
    pub try_push: Option<
        unsafe extern "C" fn(
            list: *const clap_output_events_t,
            event: *const clap_event_header_t,
        ) -> bool,
    >,
}

#[repr(C)]
pub struct clap_audio_buffer_t {
    pub data32: *mut *mut f32,
    pub data64: *mut *mut f64,
    pub channel_count: u32,
    pub latency: u32,
    pub constant_mask: u64,
}

#[repr(C)]
pub struct clap_process_t {
    pub steady_time: i64,
    pub frames_count: u32,
    pub transport: *const clap_event_transport_t,
    pub audio_inputs: *const clap_audio_buffer_t,
    pub audio_outputs: *mut clap_audio_buffer_t,
    pub audio_inputs_count: u32,
    pub audio_outputs_count: u32,
    pub in_events: *const clap_input_events_t,
    pub out_events: *const clap_output_events_t,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct clap_param_info_t {
    pub id: clap_id,
    pub flags: u32,
    pub cookie: *mut c_void,
    pub name: [c_char; 256],
    pub module: [c_char; 1024],
    pub min_value: f64,
    pub max_value: f64,
    pub default_value: f64,
}

#[repr(C)]
pub struct clap_plugin_params_t {
    pub count: Option<unsafe extern "C" fn(plugin: *const clap_plugin_t) -> u32>,
    pub get_info: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin_t,
            param_index: u32,
            param_info: *mut clap_param_info_t,
        ) -> bool,
    >,
    pub get_value: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin_t,
            param_id: clap_id,
            out_value: *mut f64,
        ) -> bool,
    >,
    pub value_to_text: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin_t,
            param_id: clap_id,
            value: f64,
            out_buffer: *mut c_char,
            out_buffer_capacity: u32,
        ) -> bool,
    >,
    pub text_to_value: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin_t,
            param_id: clap_id,
            param_value_text: *const c_char,
            out_value: *mut f64,
        ) -> bool,
    >,
    pub flush: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin_t,
            in_events: *const clap_input_events_t,
            out_events: *const clap_output_events_t,
        ),
    >,
}

#[repr(C)]
pub struct clap_istream_t {
    pub ctx: *mut c_void,
    pub read: Option<
        unsafe extern "C" fn(stream: *const clap_istream_t, buffer: *mut c_void, size: u64) -> i64,
    >,
}

#[repr(C)]
pub struct clap_ostream_t {
    pub ctx: *mut c_void,
    pub write: Option<
        unsafe extern "C" fn(
            stream: *const clap_ostream_t,
            buffer: *const c_void,
            size: u64,
        ) -> i64,
    >,
}

#[repr(C)]
pub struct clap_plugin_state_t {
    pub save: Option<
        unsafe extern "C" fn(plugin: *const clap_plugin_t, stream: *const clap_ostream_t) -> bool,
    >,
    pub load: Option<
        unsafe extern "C" fn(plugin: *const clap_plugin_t, stream: *const clap_istream_t) -> bool,
    >,
}
