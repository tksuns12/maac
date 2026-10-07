# Render speed

**Status:** done 2026-10-07. A "Late Window" render takes 3.1 times fewer CPU
cycles than before, every render is byte-identical to before, and `analyze`
can measure one section, exactly or as a quick preview.

The [T2 trial](ai-production-trial.md) found that a render takes about as
long as the music (F7), so every listening iteration of an AI producer costs
a minute, and that `analyze` could not render just one section. Moving
"Late Window" to [std/studio](studio-instruments.md) made it slower still.

## Rule: same bytes

Every change to rendering must leave every rendered sample bit-identical.
The libraries' sounds are frozen and plans are replayed from their bytes, so a
faster engine may not round differently. A change qualifies only if it skips
work whose result is already known, reuses a value computed from exactly the
same inputs, or performs the same operations on each sample in another
order that no sample depends on. The one deliberate exception is
`analyze --preroll`, which is opt-in and marked approximate.

## What the profile showed

A 30-second sample of the "Late Window" render showed that most of the time
was per-frame bookkeeping, not signal processing:

| Cost | Share | Cause |
| --- | ---: | --- |
| Heap allocation and freeing | 21% | Each frame cloned every node's parameter map, the instrument processors and their names, and the topological order |
| String comparison | 8% | Parameter maps keyed by name, compared and searched every frame |
| Instrument control updates | 10% | Every instrument's controls rebuilt from those maps every frame, kits per piece |
| Parameter validation | 4% | Every parameter of every node checked again every frame |

## What changed

### Per-frame work

- **Fixed nodes are evaluated once.** A node with no automation, no incoming
  modulation and no control role has the same parameters on every frame. The
  engine evaluates, validates and applies them on the first frame after a
  reset, then skips them.
- **Automated parameters are written in place.** The other parameters
  already hold their base values, so the map is no longer cloned.
- **No per-frame clones.** The frame loop walks the topological order by
  index; instruments render before the processor dispatch, so their
  processor and name are not cloned; connection lists are borrowed.
- **Voice parameters are cached.** Inside an instrument, a node without
  sample-rate modulation recomputes its parameters only when a control
  changes.
- **Constants are reused.** The reverb keeps its eight feedback gains until
  the decay changes. A curved envelope computes `expm1(-curve)` once. The
  drive reuses the previous sample's `ln cosh` and `tanh(bias)` while drive
  and bias are unchanged.

### Block rendering

An instrument has no audio inputs: only its notes, hits and controls change
what it plays. When its parameters are fixed, nothing reaches it between two
of its events, so the engine lets it render ahead to its next note, hit or
release, up to 128 frames at a time, and hands out one buffered frame per
engine frame.

Inside the block, each voice runs its graph node by node: one node over all
the block's frames, then the next. Every node performs exactly the
operations it performed frame by frame, on the same inputs, so every sample
is identical; only the interleaving between nodes changes, and nodes share
no state. The voices and the shared graph are then combined frame by frame
in voice order, as before. A voice whose release ends inside the block stops
contributing at the frame where pruning would have removed it. A kit renders
its pieces ahead to the next hit or gate end.

Errors keep their frame and identity. When a node fails at some frame, later
nodes stop before that frame, so the error reported is the one frame-by-frame
rendering meets first. An instrument whose controls are automated or
modulated still renders one frame at a time. A note, release or control
change that would land inside a block rendered ahead is refused as an
internal error, which the tests use to check the engine's event horizons.

### Windows

`maac analyze --window region:ID|bars:A-B` measures one section. By default
it renders from the start, because effects and sounding notes carry state
into the window, but stops at the window's end, so the frames it measures
are exactly the full render's. `--preroll SECONDS` starts rendering that many
seconds before the window from reset state instead: notes still sounding
there restart, earlier hits are dropped, effects start empty. It is fast but
approximate, and the report says so. See [windows](analyze.md#windows-2026-10-07).

The engine exposes the same as `RenderSpan` in the Rust API: a span from
frame 0 is an exact prefix of the full render, and a later start is a
preview.

## Results

Apple M1, release build, measured with `time -l` on an idle machine. The
three builds ran interleaved, twice each; the table gives the better run.
Cycles depend on whether macOS runs the process on a performance or an
efficiency core, so comparisons are only made between runs taken together.

| "Late Window" render | Instructions | Cycles | CPU time |
| --- | ---: | ---: | ---: |
| Before | 528.9 billion | 113.9 billion | 48.5 s |
| Per-frame work removed | 215.6 billion | 46.7 billion | 18.4 s |
| Block rendering | 212.2 billion | 36.5 billion | 14.8 s |
| Overall | 2.49× fewer | 3.12× fewer | 3.27× less |

All three builds rendered the same source to byte-identical WAV files.
Block rendering removes few instructions but runs them faster: one node
over many frames keeps its code and state in cache and its branches
predictable. The [std/studio examples](studio-delivery.md#render-cost) gain
similarly.

Analysing one section of "Late Window" (CPU time, same build; the whole
piece takes 45.2 s):

| Window | Exact | `--preroll 4` | Master loudness, exact / preroll |
| --- | ---: | ---: | --- |
| `region:intro` (bars 1–4) | 5.6 s | same, no room for a preroll | −16.22 / −16.22 LUFS |
| `region:b` (bars 13–20) | 21.8 s | 16.7 s | −14.08 / −14.08 LUFS |
| `region:outro` (bars 25–28) | 18.7 s | 4.0 s | −16.63 / −16.63 LUFS |

Here the 4-second preroll, longer than the 2.2-second reverb, matched the
exact window to the reported precision; a shorter preroll or a longer tail
would not.

An earlier version of this page reported 1.82 times fewer cycles for the
per-frame changes. That compared runs taken under different load, partly on
efficiency cores; measured together, those changes cut cycles 2.44 times.

## What is left

- **Measuring costs more than rendering.** A whole-piece `analyze` takes
  about three times as long as a build, because it meters every reaching
  port with K-weighting, octave bands and 4× true-peak interpolation.
  `--source master:out` measures one port.
- **Instruments with automated controls** still render frame by frame.
  Rendering ahead would need the automation for the whole block first.
- **A preview's start.** A preroll restarts sounding notes with fresh
  envelopes and empty effects; a checkpoint of the full state at section
  boundaries would make windows both fast and exact, at the cost of storing
  every node's state.
