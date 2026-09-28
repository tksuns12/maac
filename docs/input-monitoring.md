# Live input monitoring on macOS

The experimental `record --monitor` path records a dry mono take while sending
that input to outputs on the same Core Audio device. Monitoring requires an
explicit input UID and a device with both input and output channels that is
already running at 48 kHz.

```sh
maac --json inputs
maac record --input-device='EXACT DUPLEX DEVICE UID' --monitor \
  --duration-seconds 10 --output-dir monitored-take
maac verify-import monitored-take --disk-media
```

Use the listing's `input_channels`, `output_channels`, `hardware_rate_hz`, and
`available` fields to identify a candidate. The recorder checks the route again
before capture; a listing is not a guarantee of successful authorization or
device setup. Built-in microphones and speakers can be separate Core Audio
devices and therefore may not support this route.

## Signal and clock contract

- Record input channel 1 as unchanged 48 kHz mono Float32.
- Send that channel to outputs 1 and 2, or output 1 on a mono output device.
- Multiply monitored samples by `0.125` (about -18 dB), then clamp them to
  `[-1, 1]`. This affects listening only; the retained take stays dry.
- Keep every other output channel silent.
- Use one AUHAL instance and one selected Core Audio device for capture and
  output. Never change the system default, device sample rate, or buffer size.

Use headphones connected to the selected device and start with its hardware
volume low. Software attenuation does not prevent acoustic feedback when a
microphone can hear the monitored speakers. The command's fixed monitor gain
does not adjust hardware volume.

The route shares a logical Core Audio device clock. Aggregate and virtual
devices can contain additional routing and clock behavior; this does not claim
that they share a physical oscillator. Separate input and output device UIDs,
automatic aggregate creation, and cross-device clock correction are unsupported.
Apple describes the one-device AUHAL model and its matching sample-rate
requirement in [Technical Note TN2091](https://developer.apple.com/library/archive/technotes/tn2091/_index.html).

## Recording and failure behavior

The existing [recording](recording.md) duration, work-profile, permission,
owner-only staging, and atomic retained-import publication rules apply.
Monitoring is off unless `--monitor` is present. An explicit UID is required
with that flag; there is no fallback to another input or output.

The device must already provide a valid 48 kHz duplex route. Callback buffers
are bounded to 2048 frames; incompatible device configurations fail rather
than changing system buffer settings. Unsupported routes fail explicitly.
Detected device changes, invalid samples, callback overflow,
timestamp discontinuities, or a stalled capture abort the take. No partial
project is published. Ctrl-C or SIGTERM stops the child and discards private
staging. Monitor output stops when capture completes or fails.

Monitored recordings use backend `macos-auhal/1` and retain version 3 recording
provenance, identifying the monitor route, gain policy, first render timestamp,
and reported device timing. Hardware-reported latency components are distinct
from measured physical round-trip latency, which remains unknown; applied
compensation is zero. Reported timing components are not added together into a
round-trip estimate: they may omit stream, converter, or physical-device delay.
Unmonitored default and explicit-UID recordings keep versions 1 and 2. The
supervisor checks that the captured route and policy match the request before
importing. The original WAV and its provenance travel through the existing
import verification and archive workflow.

The v3 `monitoring` object also appears in the successful `--json` result:

| Field | Meaning |
| --- | --- |
| `output_device` | `selection="same-as-input"`, the exact input UID, and the device's total output `channels` |
| `input_channel`, `output_channels` | One-based device channel routing |
| `gain`, `clip_policy` | `0.125` and `"clamp-to-unit"`, for monitoring only |
| `clock_policy` | `"single-auhal-render-timeline"` |
| `first_render_sample_time`, `first_render_host_time_ticks` | Render timestamp corresponding to source frame zero |
| `host_timebase_numer`, `host_timebase_denom` | Host tick-to-nanosecond ratio |
| `latency.device_buffer_frames` | Observed device buffer size |
| `latency.reported_*` | Separate device-latency and safety-offset components for each direction, or explicit null when unavailable |
| `latency.application_buffer_frames` | Zero: no additional monitor FIFO |
| `latency.measured_round_trip_frames` | Explicit null: no physical measurement |

## Acceptance boundary

This is live input monitoring during a bounded recording. It does not play a
backing track, process live effects, compensate input/output latency, or align
an overdub to a composition. Source frame zero remains the first delivered
capture frame. Software buffering and device latency are not an acoustic
round-trip measurement.

Synthetic checks establish routing, dry capture, bounds, failure handling, and
retained provenance. Real microphone permission, audible output, feedback
behavior, and round-trip latency still require a deliberate hardware session.
See the [verification record](verification.md) and
[production roadmap](end-to-end-production-plan.md).
