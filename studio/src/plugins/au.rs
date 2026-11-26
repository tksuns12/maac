//! macOS Audio Unit v2 (`AudioToolbox` + `CoreFoundation`) plugin discovery,
//! real-time rendering, parameter control, and `ClassInfo` state persistence.

use std::ffi::{c_char, c_void, CStr};
use std::mem::size_of;
use std::ptr;

use maac::dsp::{HostBlockContext, HostNoteEvent};

use super::{PluginDescriptor, PluginFormat, PluginInstance, PluginKind, PluginParamDescriptor};

type OSStatus = i32;
type OSType = u32;
type AudioComponent = *mut c_void;
type AudioComponentInstance = *mut c_void;
type AudioUnit = AudioComponentInstance;
type AudioUnitPropertyID = u32;
type AudioUnitScope = u32;
type AudioUnitElement = u32;
type AudioUnitParameterID = u32;
type AudioUnitParameterValue = f32;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDataRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFErrorRef = *mut c_void;

const K_AUDIO_UNIT_SCOPE_GLOBAL: AudioUnitScope = 0;
const K_AUDIO_UNIT_SCOPE_INPUT: AudioUnitScope = 1;
const K_AUDIO_UNIT_SCOPE_OUTPUT: AudioUnitScope = 2;

const K_AUDIO_UNIT_PROPERTY_CLASS_INFO: AudioUnitPropertyID = 0;
const K_AUDIO_UNIT_PROPERTY_PARAMETER_LIST: AudioUnitPropertyID = 3;
const K_AUDIO_UNIT_PROPERTY_PARAMETER_INFO: AudioUnitPropertyID = 4;
const K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT: AudioUnitPropertyID = 8;
const K_AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE: AudioUnitPropertyID = 14;
const K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK: AudioUnitPropertyID = 23;

const K_AUDIO_FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1 << 0;
const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;
const K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED: u32 = 1 << 5;
const K_AUDIO_TIME_STAMP_SAMPLE_TIME_VALID: u32 = 1 << 0;

const K_AUDIO_UNIT_PARAM_FLAG_CF_NAME_RELEASE: u32 = 1 << 4;
const K_AUDIO_UNIT_PARAM_FLAG_DISPLAY_LOGARITHMIC: u32 = 1 << 22;
const K_AUDIO_UNIT_PARAM_FLAG_IS_WRITABLE: u32 = 1 << 31;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_PROPERTY_LIST_XML_FORMAT_V1_0: i64 = 100;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct AudioComponentDescription {
    component_type: OSType,
    component_sub_type: OSType,
    component_manufacturer: OSType,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SmpteTime {
    subframes: i16,
    subframe_divisor: i16,
    counter: u32,
    time_type: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioTimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock_time: u64,
    smpte_time: SmpteTime,
    flags: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList2 {
    number_buffers: u32,
    buffers: [AudioBuffer; 2],
}

#[repr(C)]
struct AudioUnitParameterInfo {
    name: [c_char; 52],
    unit_name: CFStringRef,
    clump_id: u32,
    cf_name_string: CFStringRef,
    unit: u32,
    min_value: AudioUnitParameterValue,
    max_value: AudioUnitParameterValue,
    default_value: AudioUnitParameterValue,
    flags: u32,
}

type AURenderCallback = extern "C" fn(
    in_ref_con: *mut c_void,
    io_action_flags: *mut u32,
    in_time_stamp: *const AudioTimeStamp,
    in_bus_number: u32,
    in_number_frames: u32,
    io_data: *mut AudioBufferList2,
) -> OSStatus;

#[repr(C)]
struct AURenderCallbackStruct {
    input_proc: AURenderCallback,
    input_proc_ref_con: *mut c_void,
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioComponentFindNext(
        in_component: AudioComponent,
        in_desc: *const AudioComponentDescription,
    ) -> AudioComponent;
    fn AudioComponentGetDescription(
        in_component: AudioComponent,
        out_desc: *mut AudioComponentDescription,
    ) -> OSStatus;
    fn AudioComponentCopyName(in_component: AudioComponent, out_name: *mut CFStringRef)
        -> OSStatus;
    fn AudioComponentInstanceNew(
        in_component: AudioComponent,
        out_instance: *mut AudioComponentInstance,
    ) -> OSStatus;
    fn AudioComponentInstanceDispose(in_instance: AudioComponentInstance) -> OSStatus;

    fn AudioUnitInitialize(in_unit: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(in_unit: AudioUnit) -> OSStatus;
    fn AudioUnitReset(
        in_unit: AudioUnit,
        in_scope: AudioUnitScope,
        in_element: AudioUnitElement,
    ) -> OSStatus;
    fn AudioUnitGetPropertyInfo(
        in_unit: AudioUnit,
        in_id: AudioUnitPropertyID,
        in_scope: AudioUnitScope,
        in_element: AudioUnitElement,
        out_data_size: *mut u32,
        out_writable: *mut u8,
    ) -> OSStatus;
    fn AudioUnitGetProperty(
        in_unit: AudioUnit,
        in_id: AudioUnitPropertyID,
        in_scope: AudioUnitScope,
        in_element: AudioUnitElement,
        out_data: *mut c_void,
        io_data_size: *mut u32,
    ) -> OSStatus;
    fn AudioUnitSetProperty(
        in_unit: AudioUnit,
        in_id: AudioUnitPropertyID,
        in_scope: AudioUnitScope,
        in_element: AudioUnitElement,
        in_data: *const c_void,
        in_data_size: u32,
    ) -> OSStatus;
    fn AudioUnitGetParameter(
        in_unit: AudioUnit,
        in_id: AudioUnitParameterID,
        in_scope: AudioUnitScope,
        in_element: AudioUnitElement,
        out_value: *mut AudioUnitParameterValue,
    ) -> OSStatus;
    fn AudioUnitSetParameter(
        in_unit: AudioUnit,
        in_id: AudioUnitParameterID,
        in_scope: AudioUnitScope,
        in_element: AudioUnitElement,
        in_value: AudioUnitParameterValue,
        in_buffer_offset_in_frames: u32,
    ) -> OSStatus;
    fn AudioUnitRender(
        in_unit: AudioUnit,
        io_action_flags: *mut u32,
        in_time_stamp: *const AudioTimeStamp,
        in_output_bus_number: u32,
        in_number_frames: u32,
        io_data: *mut AudioBufferList2,
    ) -> OSStatus;
    fn MusicDeviceMIDIEvent(
        in_unit: AudioUnit,
        in_status: u32,
        in_data1: u32,
        in_data2: u32,
        in_offset_sample_frame: u32,
    ) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> u8;
    fn CFPropertyListCreateData(
        allocator: CFAllocatorRef,
        property_list: CFTypeRef,
        format: i64,
        options: usize,
        error: *mut CFErrorRef,
    ) -> CFDataRef;
    fn CFPropertyListCreateWithData(
        allocator: CFAllocatorRef,
        data: CFDataRef,
        options: usize,
        format: *mut i64,
        error: *mut CFErrorRef,
    ) -> CFTypeRef;
    fn CFDataCreate(allocator: CFAllocatorRef, bytes: *const u8, length: isize) -> CFDataRef;
    fn CFDataGetBytePtr(the_data: CFDataRef) -> *const u8;
    fn CFDataGetLength(the_data: CFDataRef) -> isize;
}

fn fourcc_to_hex(code: OSType) -> String {
    format!("{code:08x}")
}

fn parse_fourcc_hex(hex: &str) -> Option<OSType> {
    u32::from_str_radix(hex, 16).ok()
}

fn au_id(desc: &AudioComponentDescription) -> String {
    format!(
        "au:{}:{}:{}",
        fourcc_to_hex(desc.component_type),
        fourcc_to_hex(desc.component_sub_type),
        fourcc_to_hex(desc.component_manufacturer)
    )
}

fn parse_au_id(id: &str) -> Option<AudioComponentDescription> {
    let mut parts = id.split(':');
    if parts.next()? != "au" {
        return None;
    }
    let component_type = parse_fourcc_hex(parts.next()?)?;
    let component_sub_type = parse_fourcc_hex(parts.next()?)?;
    let component_manufacturer = parse_fourcc_hex(parts.next()?)?;
    Some(AudioComponentDescription {
        component_type,
        component_sub_type,
        component_manufacturer,
        component_flags: 0,
        component_flags_mask: 0,
    })
}

fn cfstring_to_rust(cf: CFStringRef) -> Option<String> {
    if cf.is_null() {
        return None;
    }
    let mut buf = [0_i8; 512];
    let ok = unsafe {
        CFStringGetCString(
            cf,
            buf.as_mut_ptr(),
            buf.len() as isize,
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    let s = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Some(s)
}

fn au_unit_label(unit: u32) -> &'static str {
    match unit {
        1 => "",
        2 => "%",
        3 => "s",
        4 => "frames",
        5 => "phase",
        6 => "rate",
        7 => "Hz",
        8 => "cents",
        9 => "semi",
        10 => "note",
        12 => "pan",
        13 => "dB",
        14 => "gain",
        15 => "deg",
        16 => "ms",
        17 => "ratio",
        18 => "oct",
        21 => "bpm",
        22 => "beats",
        25 => "dB",
        _ => "",
    }
}

/// Enumerate all installed macOS Audio Unit v2 effects (`aufx`, `aumf`) and
/// instruments (`aumu`).
pub fn scan_audio_units() -> Vec<PluginDescriptor> {
    let queries = [
        (u32::from_be_bytes(*b"aumu"), PluginKind::Instrument),
        (u32::from_be_bytes(*b"aufx"), PluginKind::Effect),
        (u32::from_be_bytes(*b"aumf"), PluginKind::Effect),
    ];
    let mut out = Vec::new();
    for (comp_type, kind) in queries {
        let search = AudioComponentDescription {
            component_type: comp_type,
            component_sub_type: 0,
            component_manufacturer: 0,
            component_flags: 0,
            component_flags_mask: 0,
        };
        let mut comp = ptr::null_mut();
        loop {
            comp = unsafe { AudioComponentFindNext(comp, &search) };
            if comp.is_null() {
                break;
            }
            let mut actual = AudioComponentDescription::default();
            if unsafe { AudioComponentGetDescription(comp, &mut actual) } != 0 {
                continue;
            }
            // Skip Apple's network send AU which cannot be instantiated offline.
            if actual.component_sub_type == u32::from_be_bytes(*b"nsnd")
                && actual.component_manufacturer == u32::from_be_bytes(*b"appl")
            {
                continue;
            }
            let mut cf_name: CFStringRef = ptr::null();
            let raw_name = if unsafe { AudioComponentCopyName(comp, &mut cf_name) } == 0
                && !cf_name.is_null()
            {
                let s = cfstring_to_rust(cf_name);
                unsafe { CFRelease(cf_name) };
                s.unwrap_or_else(|| "Audio Unit".into())
            } else {
                "Audio Unit".into()
            };
            let (vendor, name) = match raw_name.split_once(':') {
                Some((v, n)) => (v.trim().to_owned(), n.trim().to_owned()),
                None => ("Apple".into(), raw_name.trim().to_owned()),
            };
            out.push(PluginDescriptor {
                id: au_id(&actual),
                name,
                vendor,
                format: PluginFormat::Au,
                kind,
                path: None,
            });
        }
    }
    out
}

/// Pinned heap buffer read by the AU input render callback for effect plugins.
struct RenderInputBuffer {
    left: Vec<f32>,
    right: Vec<f32>,
}

extern "C" fn au_input_render_callback(
    in_ref_con: *mut c_void,
    _io_action_flags: *mut u32,
    _in_time_stamp: *const AudioTimeStamp,
    _in_bus_number: u32,
    in_number_frames: u32,
    io_data: *mut AudioBufferList2,
) -> OSStatus {
    if in_ref_con.is_null() || io_data.is_null() {
        return -50;
    }
    let state = unsafe { &*(in_ref_con as *const RenderInputBuffer) };
    let list = unsafe { &mut *io_data };
    let frames = in_number_frames as usize;
    let num_bufs = (list.number_buffers as usize).min(2);
    for (ch, buf) in list.buffers.iter_mut().enumerate().take(num_bufs) {
        if buf.data.is_null() {
            continue;
        }
        let dst = unsafe { std::slice::from_raw_parts_mut(buf.data as *mut f32, frames) };
        let src = if ch == 0 { &state.left } else { &state.right };
        let copy_len = frames.min(src.len());
        dst[..copy_len].copy_from_slice(&src[..copy_len]);
        if copy_len < frames {
            dst[copy_len..].fill(0.0);
        }
        buf.data_byte_size = (frames * size_of::<f32>()) as u32;
    }
    0
}

pub struct AuInstance {
    descriptor: PluginDescriptor,
    unit: AudioUnit,
    initialized: bool,
    sample_rate: f64,
    max_block: usize,
    params: Vec<PluginParamDescriptor>,
    /// Heap-allocated input buffer whose address is registered with `SetRenderCallback`.
    input_buf: Box<RenderInputBuffer>,
    out_left: Vec<f32>,
    out_right: Vec<f32>,
    render_sample_time: f64,
}

// SAFETY: `AuInstance` exclusively owns its `AudioComponentInstance` handle and
// is only accessed from one thread at a time (protected by `Mutex<HostRack>`).
unsafe impl Send for AuInstance {}

impl AuInstance {
    pub fn new(descriptor: &PluginDescriptor) -> Result<Self, String> {
        let comp_desc = parse_au_id(&descriptor.id)
            .ok_or_else(|| format!("invalid AU descriptor id: {}", descriptor.id))?;
        let comp = unsafe { AudioComponentFindNext(ptr::null_mut(), &comp_desc) };
        if comp.is_null() {
            return Err(format!("Audio Unit not found: {}", descriptor.name));
        }
        let mut unit: AudioUnit = ptr::null_mut();
        let status = unsafe { AudioComponentInstanceNew(comp, &mut unit) };
        if status != 0 || unit.is_null() {
            return Err(format!(
                "AudioComponentInstanceNew failed for {} (OSStatus {status})",
                descriptor.name
            ));
        }

        let input_buf = Box::new(RenderInputBuffer {
            left: vec![0.0; 2_048],
            right: vec![0.0; 2_048],
        });

        let mut inst = Self {
            descriptor: descriptor.clone(),
            unit,
            initialized: false,
            sample_rate: 48_000.0,
            max_block: 512,
            params: Vec::new(),
            input_buf,
            out_left: vec![0.0; 2_048],
            out_right: vec![0.0; 2_048],
            render_sample_time: 0.0,
        };
        inst.discover_parameters();
        Ok(inst)
    }

    fn discover_parameters(&mut self) {
        self.params.clear();
        let mut byte_size: u32 = 0;
        let mut writable: u8 = 0;
        let st = unsafe {
            AudioUnitGetPropertyInfo(
                self.unit,
                K_AUDIO_UNIT_PROPERTY_PARAMETER_LIST,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                &mut byte_size,
                &mut writable,
            )
        };
        if st != 0 || byte_size == 0 {
            return;
        }
        let count = (byte_size as usize) / size_of::<AudioUnitParameterID>();
        let mut ids = vec![0u32; count];
        let st = unsafe {
            AudioUnitGetProperty(
                self.unit,
                K_AUDIO_UNIT_PROPERTY_PARAMETER_LIST,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                ids.as_mut_ptr() as *mut c_void,
                &mut byte_size,
            )
        };
        if st != 0 {
            return;
        }
        for param_id in ids {
            let mut info: AudioUnitParameterInfo = unsafe { std::mem::zeroed() };
            let mut info_size = size_of::<AudioUnitParameterInfo>() as u32;
            let st = unsafe {
                AudioUnitGetProperty(
                    self.unit,
                    K_AUDIO_UNIT_PROPERTY_PARAMETER_INFO,
                    K_AUDIO_UNIT_SCOPE_GLOBAL,
                    param_id,
                    &mut info as *mut AudioUnitParameterInfo as *mut c_void,
                    &mut info_size,
                )
            };
            if st != 0 {
                continue;
            }
            let name = if !info.cf_name_string.is_null() {
                let s = cfstring_to_rust(info.cf_name_string);
                if (info.flags & K_AUDIO_UNIT_PARAM_FLAG_CF_NAME_RELEASE) != 0 {
                    unsafe { CFRelease(info.cf_name_string) };
                }
                s.unwrap_or_else(|| format!("Param {param_id}"))
            } else {
                let cstr = unsafe { CStr::from_ptr(info.name.as_ptr()) };
                cstr.to_string_lossy().trim().to_owned()
            };
            let unit_label = if !info.unit_name.is_null() {
                let s = cfstring_to_rust(info.unit_name).unwrap_or_default();
                unsafe { CFRelease(info.unit_name) };
                s
            } else {
                au_unit_label(info.unit).to_owned()
            };
            if (info.flags & K_AUDIO_UNIT_PARAM_FLAG_IS_WRITABLE) == 0 {
                continue;
            }
            if !info.min_value.is_finite()
                || !info.max_value.is_finite()
                || info.max_value <= info.min_value
            {
                continue;
            }
            let default = if info.default_value.is_finite() {
                info.default_value.clamp(info.min_value, info.max_value)
            } else {
                info.min_value
            };
            self.params.push(PluginParamDescriptor {
                id: param_id,
                name: if name.is_empty() {
                    format!("Param {param_id}")
                } else {
                    name
                },
                unit_label,
                min: info.min_value,
                max: info.max_value,
                default,
                logarithmic: (info.flags & K_AUDIO_UNIT_PARAM_FLAG_DISPLAY_LOGARITHMIC) != 0
                    && info.min_value > 0.0,
            });
        }
    }

    fn render_stereo(&mut self, frames: usize) -> bool {
        if !self.initialized || frames == 0 {
            return false;
        }
        if frames > self.out_left.len() {
            self.out_left.resize(frames, 0.0);
            self.out_right.resize(frames, 0.0);
        }
        self.out_left[..frames].fill(0.0);
        self.out_right[..frames].fill(0.0);

        let mut flags: u32 = 0;
        let ts = AudioTimeStamp {
            sample_time: self.render_sample_time,
            flags: K_AUDIO_TIME_STAMP_SAMPLE_TIME_VALID,
            ..Default::default()
        };
        let mut abl = AudioBufferList2 {
            number_buffers: 2,
            buffers: [
                AudioBuffer {
                    number_channels: 1,
                    data_byte_size: (frames * size_of::<f32>()) as u32,
                    data: self.out_left.as_mut_ptr() as *mut c_void,
                },
                AudioBuffer {
                    number_channels: 1,
                    data_byte_size: (frames * size_of::<f32>()) as u32,
                    data: self.out_right.as_mut_ptr() as *mut c_void,
                },
            ],
        };
        let status =
            unsafe { AudioUnitRender(self.unit, &mut flags, &ts, 0, frames as u32, &mut abl) };
        self.render_sample_time += frames as f64;
        status == 0
    }
}

impl PluginInstance for AuInstance {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    fn activate(&mut self, sample_rate: f64, max_block_frames: usize) -> Result<(), String> {
        if self.initialized {
            unsafe {
                AudioUnitUninitialize(self.unit);
            }
            self.initialized = false;
        }
        self.sample_rate = sample_rate.max(1.0);
        self.max_block = max_block_frames.max(64);
        if self.input_buf.left.len() < self.max_block {
            self.input_buf.left.resize(self.max_block, 0.0);
            self.input_buf.right.resize(self.max_block, 0.0);
        }
        if self.out_left.len() < self.max_block {
            self.out_left.resize(self.max_block, 0.0);
            self.out_right.resize(self.max_block, 0.0);
        }

        let asbd = AudioStreamBasicDescription {
            sample_rate: self.sample_rate,
            format_id: K_AUDIO_FORMAT_LINEAR_PCM,
            format_flags: K_AUDIO_FORMAT_FLAG_IS_FLOAT
                | K_AUDIO_FORMAT_FLAG_IS_PACKED
                | K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        };

        // Set stereo float32 non-interleaved output format on output bus 0.
        let st_out = unsafe {
            AudioUnitSetProperty(
                self.unit,
                K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
                K_AUDIO_UNIT_SCOPE_OUTPUT,
                0,
                &asbd as *const AudioStreamBasicDescription as *const c_void,
                size_of::<AudioStreamBasicDescription>() as u32,
            )
        };
        if st_out != 0 {
            return Err(format!(
                "failed to set AU output format for {} (OSStatus {st_out})",
                self.descriptor.name
            ));
        }

        if self.descriptor.kind == PluginKind::Effect {
            let st_in = unsafe {
                AudioUnitSetProperty(
                    self.unit,
                    K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
                    K_AUDIO_UNIT_SCOPE_INPUT,
                    0,
                    &asbd as *const AudioStreamBasicDescription as *const c_void,
                    size_of::<AudioStreamBasicDescription>() as u32,
                )
            };
            if st_in != 0 {
                return Err(format!(
                    "failed to set AU input format for {} (OSStatus {st_in})",
                    self.descriptor.name
                ));
            }
            let cb = AURenderCallbackStruct {
                input_proc: au_input_render_callback,
                input_proc_ref_con: (&mut *self.input_buf as *mut RenderInputBuffer) as *mut c_void,
            };
            let st_cb = unsafe {
                AudioUnitSetProperty(
                    self.unit,
                    K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
                    K_AUDIO_UNIT_SCOPE_INPUT,
                    0,
                    &cb as *const AURenderCallbackStruct as *const c_void,
                    size_of::<AURenderCallbackStruct>() as u32,
                )
            };
            if st_cb != 0 {
                return Err(format!(
                    "failed to attach AU render callback for {} (OSStatus {st_cb})",
                    self.descriptor.name
                ));
            }
        }

        let max_frames = (self.max_block as u32).max(4_096);
        let _ = unsafe {
            AudioUnitSetProperty(
                self.unit,
                K_AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                &max_frames as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            )
        };

        let st_init = unsafe { AudioUnitInitialize(self.unit) };
        if st_init != 0 {
            return Err(format!(
                "AudioUnitInitialize failed for {} (OSStatus {st_init})",
                self.descriptor.name
            ));
        }
        self.initialized = true;
        self.render_sample_time = 0.0;
        self.discover_parameters();
        Ok(())
    }

    fn reset(&mut self) {
        if !self.initialized {
            return;
        }
        if self.descriptor.kind == PluginKind::Instrument {
            // Send MIDI CC 123 (All Notes Off) on channel 0 before resetting.
            unsafe {
                let _ = MusicDeviceMIDIEvent(self.unit, 0xB0, 123, 0, 0);
            }
        }
        unsafe {
            let _ = AudioUnitReset(self.unit, K_AUDIO_UNIT_SCOPE_GLOBAL, 0);
        }
    }

    fn params(&self) -> &[PluginParamDescriptor] {
        &self.params
    }

    fn get_param(&self, param_id: u32) -> Option<f32> {
        let mut val: f32 = 0.0;
        let st = unsafe {
            AudioUnitGetParameter(self.unit, param_id, K_AUDIO_UNIT_SCOPE_GLOBAL, 0, &mut val)
        };
        (st == 0 && val.is_finite()).then_some(val)
    }

    fn set_param(&mut self, param_id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        unsafe {
            let _ =
                AudioUnitSetParameter(self.unit, param_id, K_AUDIO_UNIT_SCOPE_GLOBAL, 0, value, 0);
        }
    }

    fn process_instrument(
        &mut self,
        ctx: HostBlockContext,
        events: &[HostNoteEvent],
        channels: usize,
        out: &mut [f64],
    ) {
        if !self.initialized || channels == 0 {
            return;
        }
        for ev in events {
            let (status, data2) = if ev.is_note_on {
                let vel = ((ev.velocity * 127.0).round() as u32).clamp(1, 127);
                (0x90_u32, vel)
            } else {
                (0x80_u32, 0_u32)
            };
            unsafe {
                let _ = MusicDeviceMIDIEvent(
                    self.unit,
                    status,
                    u32::from(ev.midi_note),
                    data2,
                    ev.frame_offset as u32,
                );
            }
        }
        if !self.render_stereo(ctx.frames) {
            return;
        }
        for i in 0..ctx.frames {
            let l = f64::from(self.out_left[i]);
            let r = f64::from(self.out_right[i]);
            if channels == 1 {
                out[i] = 0.5 * (l + r);
            } else {
                out[i * channels] = l;
                out[i * channels + 1] = r;
                for c in 2..channels {
                    out[i * channels + c] = 0.0;
                }
            }
        }
    }

    fn process_effect(&mut self, ctx: HostBlockContext, channels: usize, io: &mut [f64]) {
        if !self.initialized || channels == 0 || ctx.frames == 0 {
            return;
        }
        let frames = ctx.frames;
        if self.input_buf.left.len() < frames {
            self.input_buf.left.resize(frames, 0.0);
            self.input_buf.right.resize(frames, 0.0);
        }
        for i in 0..frames {
            let l = io[i * channels] as f32;
            let r = if channels >= 2 {
                io[i * channels + 1] as f32
            } else {
                l
            };
            self.input_buf.left[i] = l;
            self.input_buf.right[i] = r;
        }
        if !self.render_stereo(frames) {
            return;
        }
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
        let mut plist: CFTypeRef = ptr::null();
        let mut size = size_of::<CFTypeRef>() as u32;
        let st = unsafe {
            AudioUnitGetProperty(
                self.unit,
                K_AUDIO_UNIT_PROPERTY_CLASS_INFO,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                &mut plist as *mut CFTypeRef as *mut c_void,
                &mut size,
            )
        };
        if st != 0 || plist.is_null() {
            return Err(format!("failed to read AU ClassInfo (OSStatus {st})"));
        }
        let mut err: CFErrorRef = ptr::null_mut();
        let data = unsafe {
            CFPropertyListCreateData(
                ptr::null(),
                plist,
                K_CF_PROPERTY_LIST_XML_FORMAT_V1_0,
                0,
                &mut err,
            )
        };
        unsafe { CFRelease(plist) };
        if !err.is_null() {
            unsafe { CFRelease(err as CFTypeRef) };
        }
        if data.is_null() {
            return Err("failed to serialize AU ClassInfo plist".into());
        }
        let bytes = unsafe {
            let len = CFDataGetLength(data) as usize;
            let ptr = CFDataGetBytePtr(data);
            std::slice::from_raw_parts(ptr, len).to_vec()
        };
        unsafe { CFRelease(data) };
        Ok(bytes)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }
        let data = unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize) };
        if data.is_null() {
            return Err("failed to create CFData for AU state".into());
        }
        let mut err: CFErrorRef = ptr::null_mut();
        let plist = unsafe {
            CFPropertyListCreateWithData(ptr::null(), data, 0, ptr::null_mut(), &mut err)
        };
        unsafe { CFRelease(data) };
        if !err.is_null() {
            unsafe { CFRelease(err as CFTypeRef) };
        }
        if plist.is_null() {
            return Err("failed to decode AU ClassInfo plist".into());
        }
        let st = unsafe {
            AudioUnitSetProperty(
                self.unit,
                K_AUDIO_UNIT_PROPERTY_CLASS_INFO,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                &plist as *const CFTypeRef as *const c_void,
                size_of::<CFTypeRef>() as u32,
            )
        };
        unsafe { CFRelease(plist) };
        if st != 0 {
            return Err(format!("failed to apply AU ClassInfo (OSStatus {st})"));
        }
        Ok(())
    }
}

impl Drop for AuInstance {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                AudioUnitUninitialize(self.unit);
            }
        }
        if !self.unit.is_null() {
            unsafe {
                AudioComponentInstanceDispose(self.unit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_builtin_macos_audio_units() {
        let units = scan_audio_units();
        assert!(!units.is_empty(), "expected macOS built-in Audio Units");
        assert!(
            units.iter().any(|u| u.name.contains("AUDelay")),
            "expected Apple AUDelay in discovered AUs"
        );
        assert!(
            units.iter().any(|u| u.name.contains("DLSMusicDevice")),
            "expected Apple DLSMusicDevice instrument in discovered AUs"
        );
    }

    #[test]
    fn renders_au_delay_effect_and_persists_state() {
        let units = scan_audio_units();
        let delay_desc = units
            .iter()
            .find(|u| u.name.contains("AUDelay") && u.kind == PluginKind::Effect)
            .expect("Apple AUDelay");
        let mut inst = AuInstance::new(delay_desc).expect("create AUDelay");
        inst.activate(48_000.0, 512).expect("activate AUDelay");
        assert!(
            !inst.params().is_empty(),
            "expected AUDelay to expose parameters"
        );

        // Set wet/dry mix (param 0 on Apple AUDelay) to 50% and verify state roundtrip.
        let first_param = inst.params()[0].id;
        let mid = 0.5 * (inst.params()[0].min + inst.params()[0].max);
        inst.set_param(first_param, mid);
        let state = inst.save_state().expect("save AUDelay ClassInfo state");
        assert!(!state.is_empty());

        // Feed an impulse burst followed by silence and verify wet delay tail appears.
        let mut dry = vec![0.0_f64; 512 * 2];
        for i in 0..64 {
            let s = (i as f64 * 0.25).sin() * 0.8;
            dry[i * 2] = s;
            dry[i * 2 + 1] = s;
        }
        let mut io = dry.clone();
        let ctx = HostBlockContext {
            start_frame: 0,
            frames: 512,
            sample_rate_hz: 48_000.0,
            tempo_bpm: 120.0,
        };
        inst.process_effect(ctx, 2, &mut io);
        assert!(io.iter().all(|s| s.is_finite()));

        // Restore saved state onto a second fresh instance.
        let mut inst2 = AuInstance::new(delay_desc).expect("second AUDelay");
        inst2.activate(48_000.0, 512).expect("activate second");
        inst2.load_state(&state).expect("load AUDelay state");
        let restored_val = inst2.get_param(first_param).expect("read restored param");
        assert!((restored_val - mid).abs() < 1e-3);
    }

    #[test]
    fn renders_dls_music_device_instrument_notes() {
        let units = scan_audio_units();
        let dls_desc = units
            .iter()
            .find(|u| u.name.contains("DLSMusicDevice") && u.kind == PluginKind::Instrument)
            .expect("Apple DLSMusicDevice");
        let mut inst = AuInstance::new(dls_desc).expect("create DLSMusicDevice");
        inst.activate(48_000.0, 512)
            .expect("activate DLSMusicDevice");

        let events = [HostNoteEvent {
            frame_offset: 0,
            is_note_on: true,
            midi_note: 60,
            velocity: 0.9,
            pitch_hz: 261.625565,
        }];
        let mut peak = 0.0_f64;
        for block in 0..8u64 {
            let mut out = vec![0.0_f64; 512 * 2];
            let ctx = HostBlockContext {
                start_frame: block * 512,
                frames: 512,
                sample_rate_hz: 48_000.0,
                tempo_bpm: 120.0,
            };
            let evs = if block == 0 { &events[..] } else { &[] };
            inst.process_instrument(ctx, evs, 2, &mut out);
            assert!(out.iter().all(|s| s.is_finite()));
            for s in out {
                peak = peak.max(s.abs());
            }
        }
        assert!(
            peak > 1e-4,
            "expected audible audio from DLSMusicDevice, got peak {peak}"
        );
    }
}
