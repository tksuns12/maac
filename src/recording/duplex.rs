//! One AUHAL instance owns both directions of one explicit logical device.
//! C ABI declarations match the installed macOS SDK headers.
use super::*;
use crate::recording::{
    monitor::{self, MonitorState},
    MonitorLatency, Monitoring,
};
use std::io::{Seek, SeekFrom};

type Unit = *mut c_void;
const UNIT_GLOBAL: u32 = 0;
const UNIT_INPUT: u32 = 1;
const UNIT_OUTPUT: u32 = 2;
const FORMAT: u32 = 8;
const MAX_FRAMES: u32 = 14;
const RENDER_CALLBACK: u32 = 23;
const ALLOCATE_BUFFER: u32 = 51;
const UNIT_DEVICE: u32 = 2000;
const CHANNEL_MAP: u32 = 2002;
const ENABLE_IO: u32 = 2003;
const BUFFER_FRAMES: u32 = fourcc(b"fsiz");
const LATENCY: u32 = fourcc(b"ltnc");
const SAFETY_OFFSET: u32 = fourcc(b"saft");
const SILENCE: u32 = 16;

#[repr(C)]
struct ComponentDescription {
    kind: u32,
    subtype: u32,
    manufacturer: u32,
    flags: u32,
    mask: u32,
}
#[repr(C)]
struct AudioBuffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}
#[repr(C)]
struct BufferList {
    count: u32,
    buffer: AudioBuffer,
}
#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}
type RenderCallback =
    unsafe extern "C" fn(*mut c_void, *mut u32, *const TimeStamp, u32, u32, *mut BufferList) -> i32;
#[repr(C)]
struct Callback {
    proc: RenderCallback,
    data: *mut c_void,
}
type UnitListener = unsafe extern "C" fn(*mut c_void, Unit, u32, u32, u32);

unsafe extern "C" {
    fn AudioComponentFindNext(
        component: *mut c_void,
        description: *const ComponentDescription,
    ) -> *mut c_void;
    fn AudioComponentInstanceNew(component: *mut c_void, unit: *mut Unit) -> i32;
    fn AudioComponentInstanceDispose(unit: Unit) -> i32;
    fn AudioUnitInitialize(unit: Unit) -> i32;
    fn AudioUnitUninitialize(unit: Unit) -> i32;
    fn AudioUnitSetProperty(
        unit: Unit,
        property: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> i32;
    fn AudioUnitGetProperty(
        unit: Unit,
        property: u32,
        scope: u32,
        element: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioUnitAddPropertyListener(
        unit: Unit,
        property: u32,
        listener: UnitListener,
        data: *mut c_void,
    ) -> i32;
    fn AudioUnitRemovePropertyListenerWithUserData(
        unit: Unit,
        property: u32,
        listener: UnitListener,
        data: *mut c_void,
    ) -> i32;
    fn AudioOutputUnitStart(unit: Unit) -> i32;
    fn AudioOutputUnitStop(unit: Unit) -> i32;
    fn AudioUnitRender(
        unit: Unit,
        flags: *mut u32,
        time: *const TimeStamp,
        bus: u32,
        frames: u32,
        output: *mut BufferList,
    ) -> i32;
    fn AudioObjectHasProperty(object: u32, address: *const Address) -> u8;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

struct State {
    unit: Unit,
    callback: MonitorState,
}
unsafe extern "C" fn render(
    data: *mut c_void,
    flags: *mut u32,
    time: *const TimeStamp,
    bus: u32,
    frames: u32,
    output: *mut BufferList,
) -> i32 {
    // SAFETY: forward exactly the native callback's pointers and extents.
    unsafe {
        render_with(
            data,
            flags,
            time,
            bus,
            frames,
            output,
            |unit, flags, time, bus, frames, output| {
                AudioUnitRender(unit, flags, time, bus, frames, output)
            },
        )
    }
}

unsafe fn render_with(
    data: *mut c_void,
    flags: *mut u32,
    time: *const TimeStamp,
    bus: u32,
    frames: u32,
    output: *mut BufferList,
    pull: impl FnOnce(Unit, *mut u32, *const TimeStamp, u32, u32, *mut BufferList) -> i32,
) -> i32 {
    // SAFETY: Guard owns this pinned state until successful synchronous disposal.
    let state = unsafe { &*(data as *const State) };
    if output.is_null() || flags.is_null() {
        state.callback.ring.fail(1);
        return -50;
    }
    // SAFETY: AUHAL supplies the list and flags for this render invocation.
    let output = unsafe { &mut *output };
    unsafe {
        *flags |= SILENCE;
    }
    if output.count != 1
        || output.buffer.channels != 1
        || output.buffer.data.is_null()
        || !(output.buffer.data as usize).is_multiple_of(mem::align_of::<f32>())
        || output.buffer.bytes > (CHUNK_FRAMES * 4) as u32
        || !output.buffer.bytes.is_multiple_of(4)
    {
        state.callback.ring.fail(1);
        return -50;
    }
    // SAFETY: the checked native mono Float32 buffer extent is callback-owned.
    let samples = unsafe {
        std::slice::from_raw_parts_mut(
            output.buffer.data.cast::<f32>(),
            output.buffer.bytes as usize / 4,
        )
    };
    samples.fill(0.0);
    if frames == 0
        || frames as usize > CHUNK_FRAMES
        || samples.len() != frames as usize
        || bus != 0
        || time.is_null()
    {
        state.callback.ring.fail(1);
        return -50;
    }
    if DEVICE_CHANGED.load(Ordering::Acquire) {
        state.callback.ring.fail(3);
    }
    // SAFETY: timestamp is non-null and valid for this callback; sample and host
    // validity are checked before it reaches AudioUnitRender or timeline math.
    let stamp = unsafe { &*time };
    if stamp.flags & 3 != 3
        || stamp.host_time == 0
        || !monitor::valid_time(stamp.sample_time, frames as usize)
    {
        state.callback.ring.fail(1);
        return -50;
    }
    let Some(mut callback) = state.callback.enter() else {
        return 0;
    };
    let input = callback.input();
    let input_pointer = input.as_mut_ptr().cast();
    let mut captured = BufferList {
        count: 1,
        buffer: AudioBuffer {
            channels: 1,
            bytes: frames * 4,
            data: input_pointer,
        },
    };
    let mut input_flags = 0;
    // SAFETY: the whole-callback guard owns the preallocated scratch. One pull
    // on bus1 uses this same AUHAL instance and the output render timestamp.
    let result = pull(state.unit, &mut input_flags, time, 1, frames, &mut captured);
    if result != 0 {
        state.callback.ring.fail(8);
        return result;
    }
    if captured.count != 1
        || captured.buffer.channels != 1
        || captured.buffer.bytes != frames * 4
        || captured.buffer.data != input_pointer
    {
        state.callback.ring.fail(1);
        return -50;
    }
    callback.route(stamp.sample_time, stamp.host_time, samples);
    // OutputIsSilence is a hint; always supply actual zeroes on failure.
    finalize_output(&state.callback, samples, unsafe { &mut *flags });
    0
}

fn finalize_output(state: &MonitorState, samples: &mut [f32], flags: &mut u32) {
    if DEVICE_CHANGED.load(Ordering::Acquire) {
        state.ring.fail(3);
    }
    if state.ring.failure.load(Ordering::Acquire) != 0 || state.stopping.load(Ordering::Acquire) {
        samples.fill(0.0);
        *flags |= SILENCE;
    } else if samples.iter().any(|sample| *sample != 0.0) {
        *flags &= !SILENCE;
    } else {
        *flags |= SILENCE;
    }
}
unsafe extern "C" fn unit_changed(
    _data: *mut c_void,
    _unit: Unit,
    _property: u32,
    _scope: u32,
    _element: u32,
) {
    DEVICE_CHANGED.store(true, Ordering::Release);
}

struct Guard {
    unit: Unit,
    state: Option<Box<State>>,
    device: u32,
    device_listeners: Vec<Address>,
    unit_listeners: Vec<u32>,
    initialized: bool,
    started: bool,
}
trait Teardown {
    fn stop(&mut self, unit: Unit) -> i32;
    fn uninitialize(&mut self, unit: Unit) -> i32;
    fn dispose(&mut self, unit: Unit) -> i32;
}
struct NativeTeardown;
impl Teardown for NativeTeardown {
    fn stop(&mut self, unit: Unit) -> i32 {
        unsafe { AudioOutputUnitStop(unit) }
    }
    fn uninitialize(&mut self, unit: Unit) -> i32 {
        unsafe { AudioUnitUninitialize(unit) }
    }
    fn dispose(&mut self, unit: Unit) -> i32 {
        unsafe { AudioComponentInstanceDispose(unit) }
    }
}
impl Guard {
    fn state(&self) -> &State {
        self.state.as_ref().expect("live callback state")
    }
    fn shutdown(&mut self) -> Result<(), CliError> {
        self.shutdown_with(&mut NativeTeardown)
    }
    fn shutdown_with(&mut self, teardown: &mut impl Teardown) -> Result<(), CliError> {
        if self.unit.is_null() {
            return Ok(());
        }
        self.state()
            .callback
            .stopping
            .store(true, Ordering::Release);
        // All notifications refer only to a static atomic, including late calls.
        let mut listener_failure = None;
        for address in self.device_listeners.drain(..) {
            let result = unsafe {
                AudioObjectRemovePropertyListener(
                    self.device,
                    &address,
                    object_changed,
                    ptr::null_mut(),
                )
            };
            if result != 0 {
                listener_failure = Some(result);
            }
        }
        for property in self.unit_listeners.drain(..) {
            let result = unsafe {
                AudioUnitRemovePropertyListenerWithUserData(
                    self.unit,
                    property,
                    unit_changed,
                    ptr::null_mut(),
                )
            };
            if result != 0 {
                listener_failure = Some(result);
            }
        }
        let stop = if self.started {
            teardown.stop(self.unit)
        } else {
            0
        };
        let uninitialize = if self.initialized {
            teardown.uninitialize(self.unit)
        } else {
            0
        };
        let dispose = teardown.dispose(self.unit);
        self.unit = ptr::null_mut();
        if dispose != 0 {
            // The private child will exit; keep every callback reference alive
            // if native disposal cannot prove that callbacks have ceased.
            let _ = Box::leak(self.state.take().expect("callback state"));
        }
        status(dispose, "dispose duplex AUHAL")?;
        status(stop, "stop duplex AUHAL")?;
        status(uninitialize, "uninitialize duplex AUHAL")?;
        status(
            listener_failure.unwrap_or(0),
            "remove duplex device listeners",
        )?;
        self.state().callback.ring.check()
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn unit_get<T: Copy>(unit: Unit, property: u32, scope: u32, element: u32) -> Result<T, CliError> {
    let mut value = mem::MaybeUninit::<T>::uninit();
    let mut bytes = size_of::<T>() as u32;
    status(
        unsafe {
            AudioUnitGetProperty(
                unit,
                property,
                scope,
                element,
                value.as_mut_ptr().cast(),
                &mut bytes,
            )
        },
        "inspect duplex AUHAL property",
    )?;
    if bytes != size_of::<T>() as u32 {
        return Err(recording_error(
            "AUHAL returned an inconsistent property size",
        ));
    }
    Ok(unsafe { value.assume_init() })
}
fn unit_set<T>(
    unit: Unit,
    property: u32,
    scope: u32,
    element: u32,
    value: &T,
) -> Result<(), CliError> {
    status(
        unsafe {
            AudioUnitSetProperty(
                unit,
                property,
                scope,
                element,
                (value as *const T).cast(),
                size_of::<T>() as u32,
            )
        },
        "configure duplex AUHAL client",
    )
}
fn map_set(unit: Unit, scope: u32, element: u32, map: &[i32]) -> Result<(), CliError> {
    status(
        unsafe {
            AudioUnitSetProperty(
                unit,
                CHANNEL_MAP,
                scope,
                element,
                map.as_ptr().cast(),
                mem::size_of_val(map) as u32,
            )
        },
        "map duplex AUHAL channels",
    )
}
fn map_check(unit: Unit, scope: u32, element: u32, expected: &[i32]) -> Result<(), CliError> {
    let mut actual = vec![0i32; expected.len()];
    let mut bytes = mem::size_of_val(expected) as u32;
    status(
        unsafe {
            AudioUnitGetProperty(
                unit,
                CHANNEL_MAP,
                scope,
                element,
                actual.as_mut_ptr().cast(),
                &mut bytes,
            )
        },
        "inspect duplex AUHAL channel map",
    )?;
    if bytes != mem::size_of_val(expected) as u32 || actual != expected {
        return Err(recording_error(
            "AUHAL channel map differs from the monitor route",
        ));
    }
    Ok(())
}
fn optional_frames(device: u32, selector: u32, scope: u32) -> Result<Option<u32>, CliError> {
    let address = Address {
        selector,
        scope,
        element: 0,
    };
    if unsafe { AudioObjectHasProperty(device, &address) } == 0 {
        return Ok(None);
    }
    object_property(device, selector, scope).map(Some)
}
fn latency(device: u32) -> Result<MonitorLatency, CliError> {
    let frames = object_property(device, BUFFER_FRAMES, GLOBAL)?;
    if !(1..=CHUNK_FRAMES as u32).contains(&frames) {
        return Err(recording_error(
            "duplex device buffer exceeds the bounded monitor callback capacity",
        ));
    }
    Ok(MonitorLatency {
        device_buffer_frames: frames,
        reported_input_device_frames: optional_frames(device, LATENCY, INPUT)?,
        reported_output_device_frames: optional_frames(device, LATENCY, OUTPUT)?,
        reported_input_safety_offset_frames: optional_frames(device, SAFETY_OFFSET, INPUT)?,
        reported_output_safety_offset_frames: optional_frames(device, SAFETY_OFFSET, OUTPUT)?,
        application_buffer_frames: 0,
        measured_round_trip_frames: None,
    })
}
fn check(
    guard: &Guard,
    selected: &ResolvedInput,
    format: &Format,
    monitoring: &Monitoring,
) -> Result<(), CliError> {
    devices::revalidate_monitor(&mut NativeProvider, selected)?;
    if latency(selected.object)? != monitoring.latency
        || unit_get::<u32>(guard.unit, UNIT_DEVICE, UNIT_GLOBAL, 0)? != selected.object
        || unit_get::<Format>(guard.unit, FORMAT, UNIT_OUTPUT, 1)? != *format
        || unit_get::<Format>(guard.unit, FORMAT, UNIT_INPUT, 0)? != *format
    {
        return Err(recording_error(
            "duplex device, format, buffer or latency properties changed",
        ));
    }
    for (scope, element, channels) in [
        (UNIT_INPUT, 1, selected.info.input_channels),
        (UNIT_OUTPUT, 0, selected.info.output_channels),
    ] {
        let hardware: Format = unit_get(guard.unit, FORMAT, scope, element)?;
        if hardware.rate != f64::from(RATE) || hardware.channels != channels {
            return Err(recording_error(
                "AUHAL hardware directions must remain at 48000 Hz on the selected duplex device",
            ));
        }
    }
    let maximum: u32 = unit_get(guard.unit, MAX_FRAMES, UNIT_GLOBAL, 0)?;
    if maximum == 0 || maximum as usize > CHUNK_FRAMES {
        return Err(recording_error(
            "AUHAL maximum slice exceeds the bounded monitor callback capacity",
        ));
    }
    map_check(guard.unit, UNIT_OUTPUT, 1, &[0])?;
    map_check(
        guard.unit,
        UNIT_INPUT,
        0,
        &monitor::output_map(selected.info.output_channels),
    )?;
    if DEVICE_CHANGED.load(Ordering::Acquire) {
        return Err(recording_error(
            "duplex device or AUHAL configuration changed",
        ));
    }
    Ok(())
}

pub(super) fn capture(duration: u32, wave: &Path, uid: &str) -> Result<(), CliError> {
    let mut authorization = None;
    let (selected, provenance, mut monitoring) = devices::prepare_monitor_and_authorize(
        &mut NativeProvider,
        duration,
        uid,
        |selected| {
            let latency = latency(selected.object)?;
            let mut timebase = Timebase { numer: 0, denom: 0 };
            status(
                unsafe { mach_timebase_info(&mut timebase) },
                "inspect host timestamp timebase",
            )?;
            if timebase.numer == 0 || timebase.denom == 0 {
                return Err(recording_error("invalid host timestamp timebase"));
            }
            Ok(Monitoring::initial(
                uid,
                selected.info.output_channels,
                latency,
                timebase.numer,
                timebase.denom,
            ))
        },
        || {
            authorization = Some(authorize(wave)?);
            Ok(())
        },
    )?;
    let (mut lifecycle, authorization_finished) =
        authorization.expect("successful authorization has lifecycle state");
    let description = ComponentDescription {
        kind: fourcc(b"auou"),
        subtype: fourcc(b"ahal"),
        manufacturer: fourcc(b"appl"),
        flags: 0,
        mask: 0,
    };
    let component = unsafe { AudioComponentFindNext(ptr::null_mut(), &description) };
    if component.is_null() {
        return Err(recording_error("macOS has no AUHAL component"));
    }
    let mut unit = ptr::null_mut();
    let created = unsafe { AudioComponentInstanceNew(component, &mut unit) };
    // Install ownership before checking status, including unusual non-null failure.
    let mut guard = Guard {
        unit,
        state: Some(Box::new(State {
            unit,
            callback: MonitorState::new(u64::from(duration) * u64::from(RATE)),
        })),
        device: selected.object,
        device_listeners: Vec::new(),
        unit_listeners: Vec::new(),
        initialized: false,
        started: false,
    };
    status(created, "create duplex AUHAL")?;
    if unit.is_null() {
        return Err(recording_error("macOS returned a null duplex AUHAL"));
    }
    unit_set(unit, ENABLE_IO, UNIT_INPUT, 1, &1u32)?;
    unit_set(unit, ENABLE_IO, UNIT_OUTPUT, 0, &1u32)?;
    unit_set(unit, UNIT_DEVICE, UNIT_GLOBAL, 0, &selected.object)?;
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
    // Only client properties are set. Device defaults, nominal rates and the
    // hardware buffer size are never changed.
    unit_set(unit, FORMAT, UNIT_OUTPUT, 1, &format)?;
    unit_set(unit, FORMAT, UNIT_INPUT, 0, &format)?;
    map_set(unit, UNIT_OUTPUT, 1, &[0])?;
    map_set(
        unit,
        UNIT_INPUT,
        0,
        &monitor::output_map(selected.info.output_channels),
    )?;
    unit_set(unit, ALLOCATE_BUFFER, UNIT_OUTPUT, 1, &0u32)?;
    unit_set(unit, MAX_FRAMES, UNIT_GLOBAL, 0, &(CHUNK_FRAMES as u32))?;
    let callback = Callback {
        proc: render,
        data: (guard.state() as *const State).cast_mut().cast(),
    };
    unit_set(unit, RENDER_CALLBACK, UNIT_INPUT, 0, &callback)?;
    DEVICE_CHANGED.store(false, Ordering::Release);
    for property in [UNIT_DEVICE, FORMAT, MAX_FRAMES, CHANNEL_MAP, ENABLE_IO] {
        status(
            unsafe { AudioUnitAddPropertyListener(unit, property, unit_changed, ptr::null_mut()) },
            "watch duplex AUHAL",
        )?;
        guard.unit_listeners.push(property);
    }
    for (selector, scope) in [
        (UID, GLOBAL),
        (ALIVE, GLOBAL),
        (NOMINAL_RATE, GLOBAL),
        (STREAM_CONFIGURATION, INPUT),
        (STREAM_CONFIGURATION, OUTPUT),
        (BUFFER_FRAMES, GLOBAL),
        (LATENCY, INPUT),
        (LATENCY, OUTPUT),
        (SAFETY_OFFSET, INPUT),
        (SAFETY_OFFSET, OUTPUT),
    ] {
        let address = Address {
            selector,
            scope,
            element: 0,
        };
        if unsafe { AudioObjectHasProperty(selected.object, &address) } == 0 {
            continue;
        }
        status(
            unsafe {
                AudioObjectAddPropertyListener(
                    selected.object,
                    &address,
                    object_changed,
                    ptr::null_mut(),
                )
            },
            "watch duplex device",
        )?;
        guard.device_listeners.push(address);
    }
    status(
        unsafe { AudioUnitInitialize(unit) },
        "initialize duplex AUHAL",
    )?;
    guard.initialized = true;
    check(&guard, &selected, &format, &monitoring)?;
    let mut sink = WaveSink::create(wave, duration)?;
    guard.started = true;
    status(unsafe { AudioOutputUnitStart(unit) }, "start duplex AUHAL")?;
    let mut progress = authorization_finished;
    let mut inspection = Instant::now();
    let mut started = false;
    while !sink.complete() {
        guard.state().callback.ring.check()?;
        if DEVICE_CHANGED.load(Ordering::Acquire) {
            return Err(recording_error(
                "duplex device or AUHAL configuration changed during capture",
            ));
        }
        if let Some(chunk) = guard.state().callback.ring.pop() {
            sink.accept(&chunk)?;
            if !started {
                lifecycle
                    .seek(SeekFrom::Start(0))
                    .map_err(|error| CliError::new("E_IO", error.to_string()))?;
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
                "duplex device delivered no input for the 10-second startup/progress deadline",
            ));
        }
        if inspection.elapsed() >= Duration::from_millis(100) {
            check(&guard, &selected, &format, &monitoring)?;
            inspection = Instant::now();
        }
    }
    guard.state().callback.ring.check()?;
    check(&guard, &selected, &format, &monitoring)?;
    guard.shutdown()?;
    guard.state().callback.ring.check()?;
    devices::revalidate_monitor(&mut NativeProvider, &selected)?;
    if latency(selected.object)? != monitoring.latency || DEVICE_CHANGED.load(Ordering::Acquire) {
        return Err(recording_error(
            "duplex device changed before recording finalization",
        ));
    }
    let (sample, host) = guard.state().callback.anchor()?;
    monitoring.first_render_sample_time = sample;
    monitoring.first_render_host_time_ticks = host;
    sink.finish_with_monitor(duration, provenance, Some(monitoring))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn guard() -> Guard {
        let unit = ptr::dangling_mut::<u8>().cast();
        Guard {
            unit,
            state: Some(Box::new(State {
                unit,
                callback: MonitorState::new(10),
            })),
            device: 0,
            device_listeners: Vec::new(),
            unit_listeners: Vec::new(),
            initialized: true,
            started: true,
        }
    }
    struct FakeTeardown {
        calls: Vec<&'static str>,
        stop: i32,
        uninitialize: i32,
        dispose: i32,
        late_failure: *const MonitorState,
    }
    impl Teardown for FakeTeardown {
        fn stop(&mut self, _: Unit) -> i32 {
            self.calls.push("stop");
            self.stop
        }
        fn uninitialize(&mut self, _: Unit) -> i32 {
            self.calls.push("uninitialize");
            self.uninitialize
        }
        fn dispose(&mut self, _: Unit) -> i32 {
            self.calls.push("dispose");
            if !self.late_failure.is_null() {
                unsafe {
                    (*self.late_failure).ring.fail(8);
                }
            }
            self.dispose
        }
    }
    #[test]
    fn teardown_errors_and_late_callback_failures_abort_and_failed_dispose_retains_state() {
        for failed in 0..4 {
            let mut guard = guard();
            let state = &guard.state().callback as *const MonitorState;
            let mut teardown = FakeTeardown {
                calls: Vec::new(),
                stop: if failed == 0 { -50 } else { 0 },
                uninitialize: if failed == 1 { -50 } else { 0 },
                dispose: if failed == 2 { -50 } else { 0 },
                late_failure: if failed == 3 { state } else { ptr::null() },
            };
            assert!(guard.shutdown_with(&mut teardown).is_err());
            assert_eq!(teardown.calls, ["stop", "uninitialize", "dispose"]);
            assert!(guard.unit.is_null());
            if failed == 2 {
                assert!(guard.state.is_none());
            }
            assert!(unsafe { (*state).stopping.load(Ordering::Acquire) });
            // A failed native dispose deliberately leaks; prove callback state
            // remains usable after guard destruction, until private child exit.
            if failed == 2 {
                drop(guard);
                unsafe {
                    (*state).ring.fail(8);
                    assert!((*state).ring.check().is_err());
                }
            }
        }
    }
    #[test]
    fn native_callback_stopping_and_malformed_inputs_produce_actual_silence() {
        let state = State {
            unit: ptr::null_mut(),
            callback: MonitorState::new(10),
        };
        state.callback.stopping.store(true, Ordering::Release);
        let mut samples = [1.0f32; 4];
        let mut flags = 0;
        let mut output = BufferList {
            count: 1,
            buffer: AudioBuffer {
                channels: 1,
                bytes: 16,
                data: samples.as_mut_ptr().cast(),
            },
        };
        let mut time: TimeStamp = unsafe { mem::zeroed() };
        time.flags = 3;
        time.host_time = 1;
        assert_eq!(
            unsafe {
                render(
                    (&state as *const State).cast_mut().cast(),
                    &mut flags,
                    &time,
                    0,
                    4,
                    &mut output,
                )
            },
            0
        );
        assert_eq!(samples, [0.0; 4]);
        assert_ne!(flags & SILENCE, 0);
        for (sample_time, flags_value) in [
            (f64::NAN, 3),
            (0.5, 3),
            (0.0, 1),
            (9_007_199_254_740_992.0, 3),
        ] {
            let state = State {
                unit: ptr::null_mut(),
                callback: MonitorState::new(1),
            };
            samples.fill(1.0);
            time.sample_time = sample_time;
            time.flags = flags_value;
            assert_eq!(
                unsafe {
                    render(
                        (&state as *const State).cast_mut().cast(),
                        &mut flags,
                        &time,
                        0,
                        4,
                        &mut output,
                    )
                },
                -50
            );
            assert_eq!(samples, [0.0; 4]);
            assert!(state.callback.ring.check().is_err());
        }
    }
    #[test]
    fn synthetic_native_pull_keeps_same_frame_dry_route_and_final_suffix_silent() {
        let state = State {
            unit: ptr::null_mut(),
            callback: MonitorState::new(3),
        };
        let dry = [0.25f32, 16.0, -16.0, 0.5];
        let mut samples = [99.0f32; 4];
        let mut flags = 0;
        let mut output = BufferList {
            count: 1,
            buffer: AudioBuffer {
                channels: 1,
                bytes: 16,
                data: samples.as_mut_ptr().cast(),
            },
        };
        let mut time: TimeStamp = unsafe { mem::zeroed() };
        time.flags = 3;
        time.host_time = 10;
        time.sample_time = 900.0;
        let result = unsafe {
            render_with(
                (&state as *const State).cast_mut().cast(),
                &mut flags,
                &time,
                0,
                4,
                &mut output,
                |unit, _, stamp, bus, frames, input| {
                    assert_eq!(unit, state.unit);
                    assert!(ptr::eq(stamp, &time));
                    assert_eq!(bus, 1);
                    assert_eq!(frames, 4);
                    assert_eq!((*input).buffer.data as usize % 16, 0);
                    std::slice::from_raw_parts_mut(
                        (*input).buffer.data.cast::<f32>(),
                        frames as usize,
                    )
                    .copy_from_slice(&dry);
                    0
                },
            )
        };
        assert_eq!(result, 0);
        assert_eq!(samples, [0.03125, 1.0, -1.0, 0.0]);
        assert_eq!(flags & SILENCE, 0);
        let captured = state.callback.ring.pop().unwrap();
        assert_eq!(captured.frames, 3);
        assert_eq!(&captured.samples[..3], &dry[..3]);
        for (channel, mapping) in monitor::output_map(4).into_iter().enumerate() {
            let routed: Vec<_> = samples
                .iter()
                .map(|sample| if mapping == 0 { *sample } else { 0.0 })
                .collect();
            assert_eq!(
                routed,
                if channel < 2 {
                    samples.to_vec()
                } else {
                    vec![0.0; 4]
                }
            );
        }
        samples.fill(99.0);
        time.sample_time += 4.0;
        time.host_time += 1;
        assert_eq!(
            unsafe {
                render_with(
                    (&state as *const State).cast_mut().cast(),
                    &mut flags,
                    &time,
                    0,
                    4,
                    &mut output,
                    |_, _, _, _, _, _| panic!("completed take must not pull more input"),
                )
            },
            0
        );
        assert_eq!(samples, [0.0; 4]);
        assert_ne!(flags & SILENCE, 0);
        assert!(state.callback.ring.pop().is_none());
    }
    #[test]
    fn synthetic_native_pull_error_aborts_to_silence() {
        let state = State {
            unit: ptr::null_mut(),
            callback: MonitorState::new(1),
        };
        let mut samples = [99.0f32];
        let mut flags = 0;
        let mut output = BufferList {
            count: 1,
            buffer: AudioBuffer {
                channels: 1,
                bytes: 4,
                data: samples.as_mut_ptr().cast(),
            },
        };
        let mut time: TimeStamp = unsafe { mem::zeroed() };
        time.flags = 3;
        time.host_time = 1;
        assert_eq!(
            unsafe {
                render_with(
                    (&state as *const State).cast_mut().cast(),
                    &mut flags,
                    &time,
                    0,
                    1,
                    &mut output,
                    |_, _, _, _, _, _| -50,
                )
            },
            -50
        );
        assert_eq!(samples, [0.0]);
        assert_ne!(flags & SILENCE, 0);
        assert!(state.callback.ring.check().is_err());
        assert!(state.callback.ring.pop().is_none());
    }

    #[test]
    fn input_silence_hint_preserves_dry_bits_and_cannot_hide_nonfinite_pcm() {
        for dry in [-0.0f32, f32::NAN, f32::INFINITY] {
            let state = State {
                unit: ptr::null_mut(),
                callback: MonitorState::new(1),
            };
            let mut sample = [99.0f32];
            let mut flags = 0;
            let mut output = BufferList {
                count: 1,
                buffer: AudioBuffer {
                    channels: 1,
                    bytes: 4,
                    data: sample.as_mut_ptr().cast(),
                },
            };
            let mut time: TimeStamp = unsafe { mem::zeroed() };
            time.flags = 3;
            time.host_time = 1;
            assert_eq!(
                unsafe {
                    render_with(
                        (&state as *const State).cast_mut().cast(),
                        &mut flags,
                        &time,
                        0,
                        1,
                        &mut output,
                        |_, input_flags, _, _, _, input| {
                            *input_flags |= SILENCE;
                            *(*input).buffer.data.cast::<f32>() = dry;
                            0
                        },
                    )
                },
                0
            );
            assert_eq!(sample[0], 0.0);
            assert_ne!(flags & SILENCE, 0);
            if dry.is_finite() {
                state.callback.ring.check().unwrap();
                assert_eq!(
                    state.callback.ring.pop().unwrap().samples[0].to_bits(),
                    dry.to_bits()
                );
            } else {
                assert!(state.callback.ring.check().is_err());
                assert!(state.callback.ring.pop().is_none());
            }
        }
    }

    #[test]
    fn failure_latched_after_routing_zeroes_output_and_silence_hint() {
        let state = MonitorState::new(10);
        let mut output = [0.0; 2];
        state.render(0.0, 1, &[0.25, -0.25], &mut output);
        assert_ne!(output, [0.0; 2]);
        state.ring.fail(6);
        let mut flags = 0;
        finalize_output(&state, &mut output, &mut flags);
        assert_eq!(output, [0.0; 2]);
        assert_ne!(flags & SILENCE, 0);
    }
}
