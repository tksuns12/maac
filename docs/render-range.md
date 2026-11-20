# Reset-correct WAV range export

`maac render PLAN -o WAV` accepts an optional frame interval for exporting a
selected excerpt:

```text
maac render PLAN -o WAV --start-frame N --end-frame M
```

The flags are a pair. `N` and `M` are unsigned, reset-origin engine frame
indices and select the half-open interval `[N, M)`. The bounds must satisfy
`0 <= N <= M <= plan.output.total_frames`. The plan's complete duration
includes its declared tail. `N == M` is valid and produces a zero-frame WAV.

Both flags omitted retain the existing render behavior and result shape. A
single flag, an invalid bound, or a range beyond the plan duration fails
before the destination is published. Existing destination protection and
`--force` behavior continue to apply.

Range export is a transport operation at the final WAV boundary. MaaC still
loads and validates the complete plan, charges the complete execution work,
prepares a reset-state engine, and executes every frame through the plan's
declared end. DSP state, event history, automation, processor validation,
finite-value checks, and PCM16 overload checks therefore include frames before
and after the selected interval. Sample conversion and validation run for all
frames; only samples in `[N, M)` are written to the WAV payload.
The generic `write_wav_artifact_range*` helpers stream selected frames as
execution proceeds, so callers should discard a partial sink when an error is
returned. The `render_wav_to_path_artifact_range*` helpers retain the existing
temporary-file and atomic-publication behavior.

Consequently, for a successful render, the crop's decoded sample payload is
the exact corresponding frame slice of a successful full render in the same
format and environment. It does not crop source files, shorten the plan,
change delivery duration, seek into processor state, or alter the render
origin. A stateful effect's history before `N` remains part of the excerpt's
calculation.

The structured result keeps the existing fields and reports `frames = M - N`.
When a range was explicitly supplied it additionally includes `start_frame`
and `end_frame`; these fields are absent from the default result. Human output
continues to report the exported frame count.

The reusable artifact helpers in `maac::export` accept a `FrameRange` with
caller limits. Existing no-range helpers remain available and retain their
signatures.

This contract covers offline WAV excerpts only. It does not add source/build
cropping, retained-plan schema fields, seek checkpoints, realtime playback, or
new language syntax.

## Previews: `--preroll-frames`

An exact excerpt still executes the whole plan, so a two-second excerpt of a
song takes as long as the song. For listening while editing, add
`--preroll-frames K`:

```text
maac render PLAN -o WAV --start-frame N --end-frame M --preroll-frames K
```

`--preroll-frames` needs `--start-frame`. The engine starts at frame
`max(0, N - K)` from reset state and stops at `M`:

- A note still sounding at the start restarts there with a fresh envelope and
  continues its pitch, gain, timbre and pressure curves from where the full
  render would be. A note that ended at or before the start, including one
  still in its release tail, is dropped. A note that joins a dropped one
  starts its own voice.
- Effects start empty, so reverb and delay tails from before the start are
  missing.
- Frames before `N` are not written, and an overloaded PCM16 sample among them
  does not fail the preview. Frames from `M` on are not rendered or validated.

The result is **a preview, not a slice of the full render**, and its JSON
result says so: `approximate` is `true` and `render_start_frame` is the frame
the engine began at. When `N - K <= 0` the engine starts at frame 0 and the
written samples equal the exact excerpt's, but a plan that fails before `N`
(for example a PCM16 overload) or after `M` still succeeds here, where the
exact excerpt reports the failure.

How much preroll is enough depends on the plan's longest effect tails and
sustained notes, and on the window. Two measurements on "Afterlight"
(a two-second window, Apple M1, release build with the `jit` feature; the
difference is the RMS of preview minus exact excerpt, relative to the
excerpt's RMS):

| Preroll | Render time | Difference, window at 1:02 | Difference, window at 1:18 |
| --- | ---: | ---: | ---: |
| none (exact) | 41 s | 0 | 0 |
| 1 s | 0.7 s | -16 dB | -29 dB |
| 2 s | 0.9 s | -73 dB | -67 dB |
| 4 s | 2.3 s | -110 dB | not measured |
| 8 s | 5.2 s | bit-identical in 99% of samples | not measured |

The library exposes the same as `DspEngine::render_span` and
`FrameRange::with_preroll`.
