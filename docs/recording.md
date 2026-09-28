# Bounded microphone recording on macOS

The experimental `maac record` command records a fixed duration from the macOS
default or explicitly selected audio input and creates a new editable MaaC project. The delivered stream is mono Float32 at
48 kHz. The operating system may convert the device's native rate and channel
layout; the command does not assert that the hardware itself runs in this format.

## Record and reopen

```sh
maac record --duration-seconds 10 --output-dir vocal-take
maac verify-import vocal-take --disk-media
maac play vocal-take --disk-media
maac build vocal-take --disk-media -o vocal-take.wav
```

Duration is a required whole number from 1 to 1800 seconds. Capture delivers
exactly `duration-seconds * 48000` frames; setup and microphone authorization
are outside that duration. The last callback buffer is trimmed to the requested
frame count. There is no indefinite recording mode. The destination must be a
new directory under an existing parent; existing files, directories, and
symlinks are never replaced. The published recording directory is owner-only
(mode 0700), including when the parent directory is shared.

macOS microphone permission is required. MaaC includes a microphone usage
description; the OS controls authorization and may attribute permission to the
launching application. A denied or restricted request returns an explicit
error. Permission can be reviewed in System Settings → Privacy & Security →
Microphone. Automated tests use synthetic input and do not request microphone
access. Apple documents the required [microphone usage description](https://developer.apple.com/documentation/BundleResources/Information-Property-List/NSMicrophoneUsageDescription)
and [capture authorization](https://developer.apple.com/documentation/avfoundation/requesting-authorization-to-capture-and-save-media).

Use [`maac inputs`](input-devices.md) to list recording inputs without capture
permission, then select one with `record --input-device UID`. Without that flag,
the default input is selected once and pinned for the recording. The command
does not silently move to another input if the device changes or disappears.
Changing the system default preference alone does not stop a still-valid pinned
input. Optional [live monitoring](input-monitoring.md) requires `--monitor`
and an explicit UID for a duplex device already running at 48 kHz. It records
input channel 1 dry while routing that channel to the same device's first two
outputs at fixed reduced gain. Unmonitored capture retains the system format
conversion behavior described above. The command supplies no stereo/multichannel
mode, backing track playback, overdub alignment, or automatic take-group editing.

## Project and recording provenance

Successful capture is validated, then imported through the existing retained
disk-media path. The published project contains:

- `main.maac`: an editable composition with an audio clip;
- `media.pcm`: the delivered native Float32 samples;
- `import.json`: the existing version 2 retained WAV import record; and
- `original.wav`: the exact captured WAV, including its recording provenance.

Recording provenance is a bounded JSON record in the WAV's `maac` RIFF chunk.
Default selection uses `maac.recording/1`; explicit UID selection uses
`maac.recording/2`, with the same fields and an explicit selection policy.
Monitored explicit-device capture uses `maac.recording/3`, adding its route and
gain policy. The [selection contract](input-devices.md) binds the recorded UID
to the request.
The record identifies the selected input, requested
duration, delivered format/frame count, sample-data hash, capture origin, and
failure policy. Input latency is unknown and applied compensation is zero.
Source frame zero means the first delivered capture frame; it does not establish
alignment with an existing composition or another recording.

The importer hashes the complete original WAV, so the provenance bytes travel
with the retained original. Archiving the generated root project automatically
retains its verified import:

```sh
maac archive create vocal-take --output-dir vocal-archive
maac archive verify vocal-archive
maac archive unpack vocal-archive --output-dir reopened-take
maac verify-import reopened-take --disk-media
```

When a larger composition uses several recorded import directories, select
them explicitly with `--retain-import PATH`, relative to the project root.
Import verification checks the original’s exact bytes, decoded samples, and
current project closure. It does not authenticate a microphone or establish
that an untrusted recording-origin claim is true.

## Failure and cancellation

Ctrl-C or SIGTERM aborts the operation during capture or import. The supervisor
stops and reaps the active child, removes owner-only staging and all child
import snapshots, and publishes no
partial project. SIGINT returns exit 130; SIGTERM returns 143. If cleanup is
prevented by filesystem permissions, the error identifies the remaining path.
SIGKILL or power loss cannot guarantee cleanup.

Detected sample-timeline gaps, callback overflow, invalid samples, device
changes/loss, and a stalled capture fail explicitly. No missing samples are
filled with silence and no partial take is published. This is a fail-on-error
policy; dropout recovery remains future work. A silent but otherwise valid
stream is allowed and does not prove that the selected microphone heard sound.

The command uses private disk staging and bounded callback buffers. At the
30-minute limit, mono sample data alone is about 346 MB; retained WAV and PCM
copies require additional disk space. Existing import, graph, and work limits
still apply. The generated mono clip fits the default work allowance through
231 seconds. Durations of 232 seconds or more require `--profile song`; an
over-budget request fails before microphone access. Use the same profile when
reopening a project that requires it.

`--json` emits one terminal result with the output path, frame count, sample
rate, channel count, selected input UID, backend, recording
format `maac.recording/1`, `/2`, or `/3`, and retained WAV digest. Unmonitored
capture uses `macos-audioqueue/1`; monitored capture uses `macos-auhal/1`.
Completion means capture and validated project publication succeeded. It does
not measure audible quality, physical
input latency, or synchronization. Other platforms return `E_CAPABILITY`.
This command belongs to the process CLI; the public `Command` enum and
`execute*` embedding interfaces retain their existing contracts.

## Acceptance boundary

The [verification record](verification.md) separates synthetic lifecycle and
archive evidence from any actual microphone test. Device permission, physical
capture quality, and latency-aligned production acceptance require a deliberate
hardware session. Audible monitoring acceptance, backing-track transport,
multiple capture lanes, overdubbing, and recovery remain
[roadmap](end-to-end-production-plan.md) gates.
