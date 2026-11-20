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
`max(0, N - K)` from reset state, restarts any note still sounding there with
a fresh envelope, drops earlier hits, starts effects empty, and stops at `M`.
The result is **a preview, not a slice of the full render**: it differs
wherever the missing history was still audible, and it skips the validation
of frames before `N - K` and from `M` on. When `N - K <= 0` the preview starts
at frame 0 and equals the exact excerpt.

How much preroll is enough depends on the plan's longest effect tails and
sustained notes. On "Afterlight" (a two-second window in the middle of the
song, Apple M1, release build with the `jit` feature):

| Preroll | Render time | Difference from the exact excerpt |
| --- | ---: | --- |
| none (exact) | 41 s | 0 |
| 1 s | 0.7 s | -16 dB (audible) |
| 2 s | 0.9 s | -73 dB |
| 4 s | 2.3 s | -110 dB |
| 8 s | 5.2 s | bit-identical in 99% of samples |

The library exposes the same as `DspEngine::render_span` and
`FrameRange::with_preroll`.

