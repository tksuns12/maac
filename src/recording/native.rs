//! Minimal macOS C ABI bindings, checked against the installed Apple SDK.
//! AudioQueue owns its callback thread; the normal control thread owns all
//! teardown and keeps callback storage alive until synchronous disposal.

use std::ffi::{c_char, c_void};
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
    recording_error,
    stream::{Ring, WaveSink, CHUNK_FRAMES},
    CliError, InputDevice, RATE,
};

type Queue = *mut c_void;
type CfString = *const c_void;
const fn fourcc(value: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*value)
}
const CURRENT_DEVICE: u32 = fourcc(b"aqcd");
const STREAM_DESCRIPTION: u32 = fourcc(b"aqft");
const GLOBAL: u32 = fourcc(b"glob");
const INPUT: u32 = fourcc(b"inpt");
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
    fn CFStringGetCString(string: CfString, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
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

pub(super) fn capture(duration: u32, wave: &Path) -> Result<(), CliError> {
    // SAFETY: the shim runs real TCC authorization and returns only a status.
    if unsafe { maac_microphone_authorize() } != 0 {
        return Err(CliError::new("E_PERMISSION", "microphone access is denied or restricted; enable MaaC or its launching application in macOS Privacy & Security > Microphone"));
    }
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
    let authorization_finished = Instant::now();
    let device: u32 = object_property(1, DEFAULT_INPUT, GLOBAL)?;
    if device == 0 {
        return Err(recording_error("macOS has no default input device"));
    }
    let uid_ref: CfString = object_property(device, UID, GLOBAL)?;
    let uid = string_and_release(uid_ref)?;
    let rate: f64 = object_property(device, NOMINAL_RATE, GLOBAL)?;
    if !rate.is_finite() || rate <= 0.0 {
        return Err(recording_error(
            "selected input device has an invalid nominal rate",
        ));
    }
    let alive: u32 = object_property(device, ALIVE, GLOBAL)?;
    if alive == 0 {
        return Err(recording_error("selected input device is unavailable"));
    }
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
    // Reacquire a CFString UID solely for the property call. The queue retains
    // its own copy; CoreAudio returns a retained property value to this caller.
    let selected_uid: CfString = object_property(device, UID, GLOBAL)?;
    // SAFETY: CurrentDevice expects the address of a live CFStringRef.
    let selected = unsafe {
        AudioQueueSetProperty(
            guard.queue,
            CURRENT_DEVICE,
            (&selected_uid as *const CfString).cast(),
            size_of::<CfString>() as u32,
        )
    };
    // SAFETY: the UID is a retained CoreAudio property result.
    unsafe {
        CFRelease(selected_uid);
    }
    status(selected, "pin microphone input device")?;
    check_queue(guard.queue, &format)?;
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
            check_queue(guard.queue, &format)?;
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
    check_queue(guard.queue, &format)?;
    guard.shutdown()?;
    guard.ring.check()?;
    if DEVICE_CHANGED.load(Ordering::Acquire) {
        return Err(recording_error(
            "selected microphone device or format changed during capture",
        ));
    }
    sink.finish(
        duration,
        InputDevice {
            selection: "system-default".into(),
            uid,
            hardware_rate_hz: Some(rate),
        },
    )
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
fn check_queue(queue: Queue, format: &Format) -> Result<(), CliError> {
    let actual: Format = queue_property(queue, STREAM_DESCRIPTION)?;
    // CurrentDevice is explicitly set and subsequently monitored by its
    // change listener. Avoid assuming ownership of undocumented CFString
    // out-parameters from AudioQueueGetProperty.
    if actual != *format {
        return Err(recording_error(
            "macOS could not retain the selected microphone and fixed delivered format",
        ));
    }
    Ok(())
}
fn string_and_release(string: CfString) -> Result<String, CliError> {
    if string.is_null() {
        return Err(recording_error("macOS returned an empty input device UID"));
    }
    // SAFETY: retained CoreAudio/AudioQueue property is a CFStringRef; conversion
    // storage is bounded and the owned reference is released on every path.
    let result = unsafe {
        let mut bytes = [0i8; 4097];
        if CFStringGetLength(string) <= 0
            || CFStringGetLength(string) > 4096
            || CFStringGetCString(
                string,
                bytes.as_mut_ptr(),
                bytes.len() as isize,
                0x0800_0100,
            ) == 0
        {
            Err(recording_error(
                "input device UID exceeds its bounded UTF-8 envelope",
            ))
        } else {
            std::ffi::CStr::from_ptr(bytes.as_ptr())
                .to_str()
                .map(str::to_owned)
                .map_err(|_| recording_error("input device UID is not UTF-8"))
        }
    };
    // SAFETY: release the owned property reference after conversion.
    unsafe {
        CFRelease(string);
    }
    result
}
