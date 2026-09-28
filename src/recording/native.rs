//! Minimal macOS C ABI bindings, checked against the installed Apple SDK.
//! AudioQueue owns its callback thread; the normal control thread owns all
//! teardown and keeps callback storage alive until synchronous disposal.

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Write;
use std::mem::{self, size_of};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    devices::{self, Provider, ReadFailure, ResolvedInput},
    recording_error,
    stream::{Ring, WaveSink, CHUNK_FRAMES},
    CliError, InputDeviceInfo, InputDevicesResult, RATE,
};

type Queue = *mut c_void;
type CfString = *const c_void;
const fn fourcc(value: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*value)
}
const DEVICES: u32 = fourcc(b"dev#");
const NAME: u32 = fourcc(b"lnam");
const CURRENT_DEVICE: u32 = fourcc(b"aqcd");
const STREAM_DESCRIPTION: u32 = fourcc(b"aqft");
const GLOBAL: u32 = fourcc(b"glob");
const INPUT: u32 = fourcc(b"inpt");
const OUTPUT: u32 = fourcc(b"outp");
#[path = "duplex.rs"]
mod duplex;
const UID: u32 = fourcc(b"uid ");
const ALIVE: u32 = fourcc(b"livn");
const NOMINAL_RATE: u32 = fourcc(b"nsrt");
const STREAM_CONFIGURATION: u32 = fourcc(b"slay");
const DEFAULT_INPUT: u32 = fourcc(b"dIn ");
static DEVICE_CHANGED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Format {
    rate: f64,
    id: u32,
    flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}
#[repr(C)]
struct QueueBuffer {
    capacity: u32,
    data: *mut c_void,
    bytes: u32,
    user: *mut c_void,
    packet_capacity: u32,
    packets: *mut c_void,
    packet_count: u32,
}
#[repr(C)]
struct SmpteTime {
    subframes: i16,
    divisor: i16,
    counter: u32,
    kind: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}
#[repr(C)]
struct TimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock: u64,
    smpte: SmpteTime,
    flags: u32,
    reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

type InputCallback = unsafe extern "C" fn(
    *mut c_void,
    Queue,
    *mut QueueBuffer,
    *const TimeStamp,
    u32,
    *const c_void,
);
type QueueListener = unsafe extern "C" fn(*mut c_void, Queue, u32);
type ObjectListener = unsafe extern "C" fn(u32, u32, *const Address, *mut c_void) -> i32;

unsafe extern "C" {
    fn maac_microphone_authorize() -> i32;
    fn AudioQueueNewInput(
        format: *const Format,
        callback: InputCallback,
        data: *mut c_void,
        run_loop: *const c_void,
        mode: CfString,
        flags: u32,
        queue: *mut Queue,
    ) -> i32;
    fn AudioQueueAllocateBuffer(queue: Queue, bytes: u32, buffer: *mut *mut QueueBuffer) -> i32;
    fn AudioQueueEnqueueBuffer(
        queue: Queue,
        buffer: *mut QueueBuffer,
        packets: u32,
        descriptions: *const c_void,
    ) -> i32;
    fn AudioQueueStart(queue: Queue, time: *const TimeStamp) -> i32;
    fn AudioQueueStop(queue: Queue, immediate: u8) -> i32;
    fn AudioQueueDispose(queue: Queue, immediate: u8) -> i32;
    fn AudioQueueGetProperty(queue: Queue, property: u32, data: *mut c_void, size: *mut u32)
        -> i32;
    fn AudioQueueSetProperty(queue: Queue, property: u32, data: *const c_void, size: u32) -> i32;
    fn AudioQueueAddPropertyListener(
        queue: Queue,
        property: u32,
        listener: QueueListener,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectAddPropertyListener(
        object: u32,
        address: *const Address,
        listener: ObjectListener,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectRemovePropertyListener(
        object: u32,
        address: *const Address,
        listener: ObjectListener,
        data: *mut c_void,
    ) -> i32;
    fn CFStringGetLength(string: CfString) -> isize;
    fn CFStringGetBytes(
        string: CfString,
        range: CfRange,
        encoding: u32,
        loss: u8,
        external: u8,
        buffer: *mut u8,
        maximum: isize,
        used: *mut isize,
    ) -> isize;
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        count: isize,
        encoding: u32,
        external: u8,
    ) -> CfString;
    fn CFRelease(object: *const c_void);
}

unsafe extern "C" fn input_callback(
    data: *mut c_void,
    queue: Queue,
    buffer: *mut QueueBuffer,
    time: *const TimeStamp,
    _packets: u32,
    _descriptions: *const c_void,
) {
    // SAFETY: AudioQueueNewInput stores this pinned Ring pointer; QueueGuard
    // keeps it alive until AudioQueueDispose synchronously stops all callbacks.
    let ring = unsafe { &*(data as *const Ring) };
    if ring.stopping.load(Ordering::Acquire) {
        return;
    }
    if buffer.is_null() || time.is_null() {
        ring.fail(1);
        return;
    }
    // SAFETY: non-null callback buffer/timestamp are AudioQueue-owned and valid
    // for this callback. Bounds are checked before constructing the sample slice.
    let buffer_ref = unsafe { &*buffer };
    let time = unsafe { &*time };
    if buffer_ref.data.is_null()
        || buffer_ref.bytes == 0
        || buffer_ref.bytes > buffer_ref.capacity
        || buffer_ref.bytes > (CHUNK_FRAMES * 4) as u32
        || buffer_ref.bytes % 4 != 0
        || time.flags & 1 == 0
    {
        ring.fail(1);
        return;
    }
    // SAFETY: AudioQueue float PCM is aligned f32 data with the checked extent.
    let samples = unsafe {
        std::slice::from_raw_parts(buffer_ref.data as *const f32, buffer_ref.bytes as usize / 4)
    };
    if !ring.push(time.sample_time, samples) {
        return;
    }
    if !ring.stopping.load(Ordering::Acquire) {
        // SAFETY: the same live queue and callback-owned buffer are re-enqueued;
        // linear PCM requires no packet descriptions.
        enqueue_result(ring, unsafe {
            AudioQueueEnqueueBuffer(queue, buffer, 0, ptr::null())
        });
    }
}

pub(super) fn enqueue_result(ring: &Ring, result: i32) {
    // A callback may have passed the stopping check just before the control
    // thread begins Stop/Dispose. Only the SDK's expected shutdown errors are
    // harmless once that transition is visible; genuine input failures remain
    // fatal even when teardown overlaps them.
    if result != 0 && !(matches!(result, -66632 | -66685) && ring.stopping.load(Ordering::Acquire))
    {
        ring.fail(4);
    }
}

unsafe extern "C" fn queue_changed(_data: *mut c_void, _queue: Queue, _property: u32) {
    DEVICE_CHANGED.store(true, Ordering::Release);
}
unsafe extern "C" fn object_changed(
    _object: u32,
    _count: u32,
    _addresses: *const Address,
    _data: *mut c_void,
) -> i32 {
    DEVICE_CHANGED.store(true, Ordering::Release);
    0
}

struct QueueGuard {
    queue: Queue,
    ring: Box<Ring>,
    device: u32,
    listeners: Vec<Address>,
}
impl QueueGuard {
    fn shutdown(&mut self) -> Result<(), CliError> {
        if self.queue.is_null() {
            return Ok(());
        }
        self.ring.stopping.store(true, Ordering::Release);
        // These listeners refer only to a static atomic, so even an already
        // scheduled property notification has no freed client data to access.
        for address in self.listeners.drain(..) {
            // SAFETY: listener/address match a successful registration.
            unsafe {
                AudioObjectRemovePropertyListener(
                    self.device,
                    &address,
                    object_changed,
                    ptr::null_mut(),
                );
            }
        }
        // SAFETY: normal control thread owns this live queue. Stop can call
        // pending callbacks, which see stopping=true. Dispose returns only after
        // callbacks finish, before the pinned Ring is released by this guard.
        let stop = unsafe { AudioQueueStop(self.queue, 1) };
        let dispose = unsafe { AudioQueueDispose(self.queue, 1) };
        // If disposal fails, exit the private process without freeing callback
        // state. This exceptional path must never permit use-after-free.
        if dispose != 0 {
            let ring = mem::replace(&mut self.ring, Box::new(Ring::new()));
            let _ = Box::leak(ring);
        }
        self.queue = ptr::null_mut();
        status(dispose, "dispose microphone queue")?;
        status(stop, "stop microphone queue")
    }
}
impl Drop for QueueGuard {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub(super) fn capture(
    duration: u32,
    wave: &Path,
    requested_uid: Option<&str>,
    monitor: bool,
) -> Result<(), CliError> {
    if monitor {
        return duplex::capture(
            duration,
            wave,
            requested_uid
                .ok_or_else(|| recording_error("monitor requires an explicit input UID"))?,
        );
    }
    // Resolve before authorization. This choice survives a long permission
    // prompt; a changed or removed device fails instead of selecting a fallback.
    let mut authorized = None;
    let (selected, provenance) =
        devices::prepare_and_authorize(&mut NativeProvider, duration, requested_uid, || {
            authorized = Some(authorize(wave)?);
            Ok(())
        })?;
    let device = selected.object;
    let uid = selected.info.uid.clone();
    let rate = selected
        .info
        .hardware_rate_hz
        .expect("resolved live input has a rate");
    let (mut lifecycle, authorization_finished) =
        authorized.expect("successful authorization creates lifecycle marker");
    let format = Format {
        rate: f64::from(RATE),
        id: fourcc(b"lpcm"),
        flags: 1 | 8,
        bytes_per_packet: 4,
        frames_per_packet: 1,
        bytes_per_frame: 4,
        channels: 1,
        bits: 32,
        reserved: 0,
    };
    let mut guard = QueueGuard {
        queue: ptr::null_mut(),
        ring: Box::new(Ring::new()),
        device,
        listeners: Vec::new(),
    };
    // SAFETY: format and out pointer are initialized; Box pins callback storage;
    // null runloop selects AudioQueue's internal callback thread.
    status(
        unsafe {
            AudioQueueNewInput(
                &format,
                input_callback,
                (&*guard.ring as *const Ring).cast_mut().cast(),
                ptr::null(),
                ptr::null(),
                0,
                &mut guard.queue,
            )
        },
        "create microphone input queue",
    )?;
    if guard.queue.is_null() {
        return Err(recording_error("macOS returned a null microphone queue"));
    }
    // Use the resolved exact UTF-8 UID, without reacquiring a possibly changed
    // device property. The queue retains its own copy on a successful set.
    let selected_uid = OwnedString::from_utf8(&uid)?;
    // SAFETY: CurrentDevice expects the address of a live CFStringRef.
    status(
        unsafe {
            AudioQueueSetProperty(
                guard.queue,
                CURRENT_DEVICE,
                (&selected_uid.0 as *const CfString).cast(),
                size_of::<CfString>() as u32,
            )
        },
        "pin microphone input device",
    )?;
    check_queue(guard.queue, &format, &uid)?;
    DEVICE_CHANGED.store(false, Ordering::Release);
    for property in [CURRENT_DEVICE, STREAM_DESCRIPTION] {
        // SAFETY: live queue listener uses only static atomic state.
        status(
            unsafe {
                AudioQueueAddPropertyListener(guard.queue, property, queue_changed, ptr::null_mut())
            },
            "monitor microphone queue",
        )?;
    }
    for (selector, scope) in [
        (UID, GLOBAL),
        (ALIVE, GLOBAL),
        (NOMINAL_RATE, GLOBAL),
        (STREAM_CONFIGURATION, INPUT),
    ] {
        let address = Address {
            selector,
            scope,
            element: 0,
        };
        // SAFETY: property address is copied on registration; callback state is static.
        status(
            unsafe {
                AudioObjectAddPropertyListener(device, &address, object_changed, ptr::null_mut())
            },
            "monitor selected microphone",
        )?;
        guard.listeners.push(address);
    }
    for _ in 0..4 {
        let mut buffer = ptr::null_mut();
        // SAFETY: live queue owns the allocated buffer until disposal.
        status(
            unsafe {
                AudioQueueAllocateBuffer(guard.queue, (CHUNK_FRAMES * 4) as u32, &mut buffer)
            },
            "allocate microphone buffer",
        )?;
        status(
            unsafe { AudioQueueEnqueueBuffer(guard.queue, buffer, 0, ptr::null()) },
            "enqueue microphone buffer",
        )?;
    }
    check_selected(&selected)?;
    check_queue(guard.queue, &format, &uid)?;
    if DEVICE_CHANGED.load(Ordering::Acquire) {
        return Err(recording_error(
            "selected microphone device or format changed before capture",
        ));
    }
    let mut sink = WaveSink::create(wave, duration)?;
    // SAFETY: queue has its fixed format, pinned device, and reserved buffers.
    status(
        unsafe { AudioQueueStart(guard.queue, ptr::null()) },
        "start microphone input",
    )?;
    let mut progress = authorization_finished;
    let mut inspection = Instant::now();
    let mut started = false;
    while !sink.complete() {
        guard.ring.check()?;
        if DEVICE_CHANGED.load(Ordering::Acquire) {
            return Err(recording_error(
                "selected microphone device or format changed during capture",
            ));
        }
        if let Some(chunk) = guard.ring.pop() {
            sink.accept(&chunk)?;
            if !started {
                use std::io::{Seek, SeekFrom};
                lifecycle
                    .seek(SeekFrom::Start(0))
                    .map_err(|error| CliError::new("E_IO", error.to_string()))?;
                // Same byte count as "authorized": a single replacement write
                // cannot expose a stale suffix to the supervisor.
                lifecycle
                    .write_all(b"capturing ")
                    .map_err(|error| CliError::new("E_IO", error.to_string()))?;
                started = true;
            }
            progress = Instant::now();
        } else {
            thread::sleep(Duration::from_millis(2));
        }
        if progress.elapsed() > Duration::from_secs(10) {
            return Err(recording_error(
                "microphone delivered no input for the 10-second startup/progress deadline",
            ));
        }
        if inspection.elapsed() >= Duration::from_millis(100) {
            check_queue(guard.queue, &format, &uid)?;
            let alive: u32 = object_property(device, ALIVE, GLOBAL)?;
            let actual_rate: f64 = object_property(device, NOMINAL_RATE, GLOBAL)?;
            if alive == 0 || actual_rate != rate {
                return Err(recording_error(
                    "selected microphone disappeared or changed its nominal rate",
                ));
            }
            inspection = Instant::now();
        }
    }
    guard.ring.check()?;
    if DEVICE_CHANGED.load(Ordering::Acquire) {
        return Err(recording_error(
            "selected microphone device or format changed during capture",
        ));
    }
    check_queue(guard.queue, &format, &uid)?;
    guard.shutdown()?;
    guard.ring.check()?;
    if DEVICE_CHANGED.load(Ordering::Acquire) {
        return Err(recording_error(
            "selected microphone device or format changed during capture",
        ));
    }
    sink.finish(duration, provenance)
}

fn authorize(wave: &Path) -> Result<(std::fs::File, Instant), CliError> {
    // SAFETY: the shim runs real TCC authorization and returns only a status.
    if unsafe { maac_microphone_authorize() } != 0 {
        return Err(CliError::new("E_PERMISSION", "microphone access is denied or restricted; enable MaaC or its launching application in macOS Privacy & Security > Microphone"));
    }
    let authorization_finished = Instant::now();
    let mut lifecycle = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(wave.with_extension("status"))
        .map_err(|error| {
            CliError::new(
                "E_IO",
                format!("cannot create recording lifecycle marker: {error}"),
            )
        })?;
    lifecycle.write_all(b"authorized").map_err(|error| {
        CliError::new(
            "E_IO",
            format!("cannot update recording lifecycle marker: {error}"),
        )
    })?;
    Ok((lifecycle, authorization_finished))
}

fn status(value: i32, operation: &str) -> Result<(), CliError> {
    if value == 0 {
        Ok(())
    } else {
        Err(recording_error(format!(
            "cannot {operation} (macOS status {value})"
        )))
    }
}
fn object_property<T: Copy>(object: u32, selector: u32, scope: u32) -> Result<T, CliError> {
    let address = Address {
        selector,
        scope,
        element: 0,
    };
    let mut value = mem::MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    // SAFETY: typed output reserves exactly size bytes; API receives null qualifiers.
    status(
        unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                ptr::null(),
                &mut size,
                value.as_mut_ptr().cast(),
            )
        },
        "inspect selected microphone",
    )?;
    if size as usize != size_of::<T>() {
        return Err(recording_error(
            "macOS returned an inconsistent microphone property size",
        ));
    }
    // SAFETY: successful exact-size property read initialized the typed result.
    Ok(unsafe { value.assume_init() })
}
fn queue_property<T: Copy>(queue: Queue, property: u32) -> Result<T, CliError> {
    let mut value = mem::MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    // SAFETY: typed output has the requested property capacity.
    status(
        unsafe { AudioQueueGetProperty(queue, property, value.as_mut_ptr().cast(), &mut size) },
        "inspect microphone queue format/device",
    )?;
    if size as usize != size_of::<T>() {
        return Err(recording_error(
            "macOS returned an inconsistent queue property size",
        ));
    }
    // SAFETY: successful exact-size property read initialized the typed result.
    Ok(unsafe { value.assume_init() })
}
fn check_queue(queue: Queue, format: &Format, uid: &str) -> Result<(), CliError> {
    let actual: Format = queue_property(queue, STREAM_DESCRIPTION)?;
    // AudioQueueGetProperty duplicates CF objects even though it is named Get.
    // Apple requires the caller to release them (AudioQueueGetProperty docs).
    let reported = owned_string_property(
        |value, size| {
            // SAFETY: initialized CFString out pointer has exactly the size
            // supplied by the helper; queue is live throughout this call.
            unsafe { AudioQueueGetProperty(queue, CURRENT_DEVICE, value.cast(), size) }
        },
        "inspect microphone queue device",
    )?;
    if actual != *format || reported != uid {
        return Err(recording_error(
            "macOS could not retain the selected microphone and fixed delivered format",
        ));
    }
    Ok(())
}
#[repr(C)]
struct CfRange {
    location: isize,
    length: isize,
}

struct OwnedString(CfString);
impl Drop for OwnedString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: CoreAudio/AQ property and Create results are caller-owned.
            unsafe {
                CFRelease(self.0);
            }
        }
    }
}
impl OwnedString {
    fn from_utf8(value: &str) -> Result<Self, CliError> {
        // SAFETY: bounded UTF-8 slice and count are valid for the duration of Create.
        let string = Self(unsafe {
            CFStringCreateWithBytes(
                ptr::null(),
                value.as_ptr(),
                value.len() as isize,
                0x0800_0100,
                0,
            )
        });
        if string.0.is_null() {
            return Err(recording_error("cannot encode input device UID"));
        }
        Ok(string)
    }
    fn utf8(&self) -> Result<String, CliError> {
        if self.0.is_null() {
            return Err(recording_error(
                "macOS returned an empty input device string",
            ));
        }
        // SAFETY: this owner holds the CFString throughout conversion. GetBytes
        // preserves embedded NUL so validation cannot silently truncate a UID.
        unsafe {
            let length = CFStringGetLength(self.0);
            if !(1..=4096).contains(&length) {
                return Err(recording_error(
                    "input device string exceeds its bounded UTF-8 envelope",
                ));
            }
            let mut bytes = [0u8; 4096];
            let mut used = 0isize;
            let converted = CFStringGetBytes(
                self.0,
                CfRange {
                    location: 0,
                    length,
                },
                0x0800_0100,
                0,
                0,
                bytes.as_mut_ptr(),
                bytes.len() as isize,
                &mut used,
            );
            if converted != length || !(1..=4096).contains(&used) {
                return Err(recording_error(
                    "input device string exceeds its bounded UTF-8 envelope",
                ));
            }
            std::str::from_utf8(&bytes[..used as usize])
                .map(str::to_owned)
                .map_err(|_| recording_error("input device string is not UTF-8"))
        }
    }
}
fn owned_string_property(
    read: impl FnOnce(*mut CfString, *mut u32) -> i32,
    operation: &str,
) -> Result<String, CliError> {
    let mut value = ptr::null();
    let mut size = size_of::<CfString>() as u32;
    status(read(&mut value, &mut size), operation)?;
    // Successful CF property reads return a caller-owned object. Establish
    // ownership before validating the returned size or converting the string,
    // so every subsequent error releases the returned reference.
    let value = OwnedString(value);
    if size as usize != size_of::<CfString>() {
        return Err(recording_error(
            "macOS returned an inconsistent input string property size",
        ));
    }
    value.utf8()
}
fn object_string(object: u32, selector: u32) -> Result<String, CliError> {
    let address = Address {
        selector,
        scope: GLOBAL,
        element: 0,
    };
    owned_string_property(
        |value, size| {
            // SAFETY: fixed CFString storage and address are initialized;
            // CoreAudio returns a retained reference on success.
            unsafe {
                AudioObjectGetPropertyData(object, &address, 0, ptr::null(), size, value.cast())
            }
        },
        "inspect input device string",
    )
}

struct NativeProvider;
impl Provider for NativeProvider {
    fn ids(&mut self) -> Result<Vec<u32>, CliError> {
        devices::parse_ids(&variable_property(1, DEVICES, GLOBAL)?)
    }
    fn default_input(&mut self) -> Result<u32, CliError> {
        object_property(1, DEFAULT_INPUT, GLOBAL)
    }
    fn info(&mut self, object: u32) -> Result<Option<InputDeviceInfo>, CliError> {
        let channels =
            devices::parse_channels(&variable_property(object, STREAM_CONFIGURATION, INPUT)?)?;
        if channels == 0 {
            return Ok(None);
        }
        let uid = object_string(object, UID)?;
        if uid.contains('\0') {
            return Err(recording_error(
                "macOS returned an input device UID containing NUL",
            ));
        }
        let name = object_string(object, NAME)?;
        let available: u32 = object_property(object, ALIVE, GLOBAL)?;
        let rate = if available != 0 {
            Some(object_property(object, NOMINAL_RATE, GLOBAL)?)
        } else {
            None
        };
        Ok(Some(InputDeviceInfo {
            uid,
            name,
            input_channels: channels,
            output_channels: devices::parse_channels(&variable_property(
                object,
                STREAM_CONFIGURATION,
                OUTPUT,
            )?)?,
            is_default: false,
            hardware_rate_hz: rate,
            available: available != 0,
        }))
    }
}

fn variable_property(object: u32, selector: u32, scope: u32) -> Result<Vec<u8>, CliError> {
    let address = Address {
        selector,
        scope,
        element: 0,
    };
    devices::read_variable(
        || {
            let mut size = 0;
            // SAFETY: initialized out size and valid property address; no qualifiers.
            status(
                unsafe {
                    AudioObjectGetPropertyDataSize(object, &address, 0, ptr::null(), &mut size)
                },
                "size input device property",
            )?;
            Ok(size as usize)
        },
        |storage, capacity| {
            let mut size = capacity as u32;
            // SAFETY: word-aligned initialized storage reserves the bounded
            // requested byte capacity, even for a size/data hotplug race.
            let result = unsafe {
                AudioObjectGetPropertyData(
                    object,
                    &address,
                    0,
                    ptr::null(),
                    &mut size,
                    storage.as_mut_ptr().cast(),
                )
            };
            if result == fourcc(b"!siz") as i32 {
                return Err(ReadFailure::Resized);
            }
            status(result, "inspect input device property").map_err(ReadFailure::Failed)?;
            Ok(size as usize)
        },
    )
}

fn check_selected(selected: &ResolvedInput) -> Result<(), CliError> {
    devices::revalidate(&mut NativeProvider, selected)
}

pub(super) fn inputs() -> Result<InputDevicesResult, CliError> {
    let devices = devices::snapshot(&mut NativeProvider)?
        .into_iter()
        .map(|device| device.info)
        .collect();
    Ok(InputDevicesResult {
        ok: true,
        command: "inputs",
        backend: "macos-coreaudio/1",
        devices,
    })
}

#[cfg(test)]
mod string_tests {
    use super::*;
    unsafe extern "C" {
        fn CFRetain(object: *const c_void) -> *const c_void;
        fn CFGetRetainCount(object: *const c_void) -> isize;
    }
    #[test]
    fn cfstring_conversion_preserves_nul_and_bounds_actual_utf8_bytes() {
        let value = "device\0hidden\n한글";
        assert_eq!(
            OwnedString::from_utf8(value).unwrap().utf8().unwrap(),
            value
        );
        let exact = "한".repeat(1365) + "x";
        assert_eq!(exact.len(), 4096);
        assert_eq!(
            OwnedString::from_utf8(&exact).unwrap().utf8().unwrap(),
            exact
        );
        assert!(OwnedString::from_utf8(&(exact + "x"))
            .unwrap()
            .utf8()
            .is_err());
    }

    #[test]
    fn successful_cf_properties_release_owned_results_on_validation_errors() {
        for (text, wrong_size) in [
            ("runtime UID ".repeat(100), true),
            ("x".repeat(4097), false),
        ] {
            let owner = OwnedString::from_utf8(&text).unwrap();
            // SAFETY: the live owner keeps this runtime-created object valid;
            // retain emulates the caller-owned copy returned by a CF property.
            let before = unsafe { CFGetRetainCount(owner.0) };
            unsafe {
                CFRetain(owner.0);
            }
            let result = owned_string_property(
                |value, size| {
                    // SAFETY: helper supplies initialized, correctly sized outputs.
                    unsafe {
                        *value = owner.0;
                        if wrong_size {
                            *size -= 1;
                        }
                    }
                    0
                },
                "test owned input property",
            );
            assert!(result.is_err());
            assert_eq!(unsafe { CFGetRetainCount(owner.0) }, before);
        }
    }
}
