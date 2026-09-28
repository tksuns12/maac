# Recording input selection on macOS

Use `maac inputs` to inspect recording inputs without opening a microphone or
requesting capture permission:

```sh
maac inputs
maac --json inputs
maac record --input-device='EXACT UID FROM THE LIST' \
  --duration-seconds 10 --output-dir new-take
```

The command lists input devices only. It does not change system preferences,
select a playback output, or start monitoring. Recording still delivers the
[bounded mono 48 kHz Float32 stream](recording.md), even when the selected
hardware advertises more input channels or a different nominal rate.

## Device listing

JSON identifies `command="inputs"`, `backend="macos-coreaudio/1"`, and a
`devices` array sorted by exact UID. An empty array is a successful empty listing.
Each entry contains:

| Field | Meaning |
| --- | --- |
| `uid` | Exact Core Audio identifier accepted by `record --input-device` |
| `name` | Display name; it is not a selector and need not be unique |
| `input_channels` | Advertised input-channel count, not the recorded channel count |
| `is_default` | Whether this input matches the inspected system-default input |
| `hardware_rate_hz` | Observed nominal rate; may be null for an unavailable input |
| `available` | Reported device-alive state, not microphone permission or proof that capture will work |

Human output escapes names and UIDs so embedded control characters are not
interpreted as terminal commands. For scripting, read the JSON UID and pass it
as one argument. The `--input-device=VALUE` form also handles UIDs beginning
with a dash.

The listing is an observation of current device metadata. Devices can disappear
or change after it is printed. Property reads are bounded and inconsistent or
malformed results fail explicitly. Duplicate display names are allowed;
ambiguous duplicate UIDs cannot be selected. Listing does not establish capture
quality, timing, supported conversion, or permission status.

Core Audio exposes the [device UID](https://developer.apple.com/documentation/coreaudio/kaudiodevicepropertydeviceuid)
and [input stream configuration](https://developer.apple.com/documentation/coreaudio/kaudiodevicepropertystreamconfiguration)
used for these queries. UIDs are opaque; do not assume an index, name, or USB
port-independent identity.

## Exact selection and failure

Omitting `--input-device` keeps the existing default-input behavior. Supplying
it requires an exact, nonempty UTF-8 UID of at most 4096 bytes, without NUL.
There is no trimming, case folding, name search, numeric-index lookup, or
special `default` alias. Other control characters remain part of the exact UID.

The recorder resolves an explicit UID before requesting microphone permission.
Unknown, ambiguous, unavailable, or output-only selections fail with
`E_RECORDING`. It never falls back to the default input. After authorization,
it revalidates the selected input before opening the capture queue. A device
that changes or disappears fails explicitly. Changing the system's default
preference does not retarget an already selected input.

Invalid selector syntax is `E_USAGE`. Native inspection failures are
`E_RECORDING`; unsupported platforms return `E_CAPABILITY`. Existing duration,
work-profile, destination, interruption, and owner-only publication rules remain
unchanged. The final capture can still fail after a successful listing—for
example if authorization is denied or the device becomes unavailable.

## Recording provenance

| Selection | Retained WAV recording record | Selection identity |
| --- | --- | --- |
| Omitted selector | `maac.recording/1` | `input_device.selection="system-default"` and the resolved UID |
| Explicit UID | `maac.recording/2` | `input_device.selection="explicit-uid"` and the exact requested/resolved UID |

Version 2 keeps the version 1 field layout and changes the version and selection
policy. The supervisor requires the captured UID to match the explicit request
exactly; mismatched version, policy, or UID fails before project publication.
The metadata encoding limit is checked before microphone authorization, including
JSON escaping. Result `recording_format` reports the applicable version.

The original WAV retains the record, and existing import verification and
archive retention preserve its bytes. Existing default-input recordings need
no migration. These are local capture provenance records, not authenticated
hardware identity or measured latency certificates.

## Acceptance boundary

Synthetic tests verify lookup, request binding, metadata compatibility, and
failure paths. Read-only native enumeration can be checked without recording
sound. Actual explicit-device capture still requires deliberate microphone
acceptance. Monitoring, output-device selection, multichannel capture, shared
capture/playback clocks, and latency-aligned overdubbing remain separate work.
