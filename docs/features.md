# Feature tour

A tour of what MaaC can do today, with a runnable example for most features.
The [README](../README.md) has the quick start; the
[capability matrix](capabilities.md) lists exact limits.

## Analysis for AI producers

[`maac analyze`](analyze.md) gives an AI producer, or anyone without
ears on the render, a way to hear it: per-section and per-source loudness,
peaks, octave bands and stereo measurements, findings named by the authored
IDs, and spectrogram and piano-roll images. Try
`maac analyze examples/showcase/showcase.maac --project-root . --disk-media --profile song --images analysis`.
The [AI producer guide](ai-producer-guide.md) shows the whole loop, and
[`examples/lofi`](../examples/lofi/) is a track an AI produced with it.

## Built-in instruments

The installed executable includes [24 basic instruments](basic-instruments.md):
keys, guitars, basses, drums, strings, flute, bell, pad and lead. Import
`std/basic/1.0.0` without downloading sounds or supplying a library directory.
These are synthesized interpretations, with stereo output and common controls.

The separate [acoustic guitar collection](acoustic-guitars.md) supplies
three MaaC-native string instruments under `std/acoustic/1.0.0`, with bends
and vibrato. The [delivery report](reports/acoustic-delivery.md) records numerical
and installed-example checks, and the owner approved all three by ear. Basic
sounds and default discovery remain unchanged.

The [studio instruments](studio-instruments.md) under `std/studio/1.1.0`
are an electric piano, an acoustic piano, an electric guitar, a bass, a pad
and a drum kit whose sound changes with velocity: harder notes bark, pluck,
strike or hit brighter. Their defaults share one loudness scale (−20 LUFS at
velocity 0.7), and the drum kit uses the std/basic/1.1.0 kit's keys. The
owner approved every one of them by ear.
Try `maac build examples/studio/drums.maac -o studio-drums.wav`.

## Sound design

Reusable [sound libraries](instruments.md) add code-authored instruments,
presets, sample-wise FM, and morphing wavetables. Import local source files with
SHA-256 pins, instantiate their instruments, and automate the exposed controls.
Compiled plans embed everything needed for offline rendering.

[Note sources](instruments.md#note-sources) let a voice graph read the
note's velocity and key, so harder notes can be brighter and higher notes
decay sooner. An envelope's `curve` makes decays and releases exponential in
shape while keeping their durations.
Try `maac build examples/note-sources.maac -o note-sources.wav`.
A [resonant filter](instruments.md#resonant-filter) that can track the
key and anti-aliased [saturation](instruments.md#saturation) give
instruments filter character and warmth.
Try `maac build examples/resonant-drive.maac -o resonant-drive.wav`.

## Reusable musical material

[Musical libraries](instruments.md#reusable-musical-declarations) also
export pinned patterns, curves, and tunings. Their references stay owned by
the library while each composition supplies its own tempo, placement, and
targets. [Source module artifacts](musical-module-artifact.md) preserve
these libraries and their dependency closure for reuse.

## Timing

[Tempo ramps](tempo-ramps.md) use the existing `linear` tempo shape to
accelerate or slow down across score positions. Try
`maac build examples/tempo-ramps.maac -o tempo-ramps.wav`.
Ramp scores without sample kits or arranged clips produce version 3 plans; other existing scores
retain versions 1 and 2.

## Recorded audio

[Native hits and sample kits](core-kit.md) play pinned mono/stereo audio
assets through `core.kit/1`. Hits use the existing pattern, placement, override,
and tempo machinery. Version 4 plans embed the samples for offline replay.
Sample preparation stays in external tools; this feature adds no language syntax.
Try `maac build examples/core-kit.maac --project-root . -o core-kit.wav`.

[Arranged audio clips](audio-clips.md) use existing top-level `audio` objects
for placement, source slicing, playback speed, reverse and explicit fades.
Connect each clip's `out` port to the processing graph. Version 5 plans retain
the samples and transport settings for standalone replay; sample importing
remains external tooling.
Try `maac build examples/audio-clips.maac --project-root . -o audio-clips.wav`.

[Warp-rate clips](warp-rate.md) map source frames to musical anchors using
`mode = warp_rate`. Playback follows the tempo map, changing pitch with speed,
including tempo changes during the tail. Version 6 plans retain the exact warp
recipe. `mode = warp_preserve` with `processor = "core.stretch.ola/1"` keeps the
recorded pitch through the core overlap-add reference stretch; module-asset
stretchers remain an explicit extension capability.
Try `maac build examples/warp-rate.maac --project-root . -o warp-rate.wav`.

The [take and comp capability](takes-and-comping.md) records alternate
audio assets, a shared physical origin, and comp regions checked against
explicit clips. Selection changes use existing atomic edits, inverses, and
archive history. Try the [synthetic take example](../examples/take-comp.maac);
[v2 take groups](grouped-takes.md) coordinate all lanes of a take.
Takes are imported audio; MaaC itself does not record, play or monitor audio
devices. Capture and listen with any audio tool, and bring recordings in with
`maac import-wav`.

## Signal processing and modulation

[Core fader](core-fader.md) provides explicit dB gain with sample-rate
automation and modulation. Try `maac build examples/core-fader.maac -o core-fader.wav`.

[Core matrix](core-matrix.md) provides explicit mono/stereo channel mapping,
duplication, and downmixing. Try `maac build examples/core-matrix.maac -o core-matrix.wav`.

[Core delay](core-delay.md) provides exact frame delays and causal feedback.
Try `maac build examples/core-delay.maac -o core-delay.wav`.

[Core reference noise](core-noise.md) provides reproducible seeded mono/stereo
noise. Try `maac build examples/core-noise.maac -o core-noise.wav`.

The [Core Audio conformance audit](reports/core-audio-conformance-audit.md) tracks
remaining correctness and profile gaps after the reference-processor slices.

[Core modulation](core-modulation.md) connects score- or seconds-clock LFOs
and automated constants to continuous sample-rate controls such as cutoff, pan,
and gain, plus event-rate attack/release and instrument public controls.
Event-rate values are captured at note-on, note-off, or reset. Existing `modulate` objects
add their contributions to base automation;
version 7 plans retain the control graph for standalone replay.
Try `maac build examples/core-modulation.maac --project-root . -o core-modulation.wav`.
Reset capture: `maac build examples/reset-rate-modulation.maac -o reset-rate.wav`.

[Internal event modulation](internal-event-modulation.md) captures per-voice
signals at note-on, note-off, and shared-LFO reset without advancing their audio state twice.
Try `maac build examples/internal-event-modulation.maac -o internal-event.wav`
or `maac build examples/internal-phase-modulation.maac -o internal-phase.wav`.
Shared reset capture: `maac build examples/internal-reset-modulation.maac -o internal-reset.wav`.

## Projects

The [project-entrypoint guide](project-entrypoint.md) specifies conventional
`main.maac` discovery and the explicit finite `--profile song` allowance for
larger projects. Existing explicit-file commands remain compatible.

## Mixing and delivery

The experimental [native mixing and delivery implementation](production.md)
adds EQ, compression, algorithmic reverb, and named master/stem deliveries under
`maac.production/1`. The graph runs at 48 kHz; `maac deliver` exports 44.1, 48,
or 96 kHz WAV with explicit encoding and dither, then analyzes the final files.
The [production example](../examples/production.maac),
[delivery schema](../schemas/production.schema.json), and
[bounded fixtures](../conformance/production-conformance.json) describe the contract. The
[production delivery report](reports/production-delivery.md) records installed-CLI
acceptance separately from the specification fixtures.
Metering uses analyzer `maac.analysis.bs1770-5/2` and the approved single-stage
four-times Annex 2 true-peak profile. See the applicable fixture results and
historical profile comparison in the [metering evidence](reports/production-metering-evidence.md).
No professional sound-quality or full ITU/EBU conformance claim is made.
