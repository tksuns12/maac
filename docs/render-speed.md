# Render speed

**Status:** first pass done 2026-10-07. Rendering "Late Window" takes 1.8
times fewer CPU cycles, and every render is byte-identical to before.

The [T2 trial](ai-production-trial.md) found that a render costs about as
long as the music (F7), so every listening iteration of an AI producer costs
a minute. Moving "Late Window" to [std/studio](studio-instruments.md) made it
slower still. This pass removes work the engine did on every frame that never
changed the audio.

## Rule: same bytes

Every change must leave every rendered sample bit-identical. The libraries'
sounds are frozen and plans are replayed from their bytes, so a faster
engine may not round differently. A change qualifies only if it skips work
whose result is already known or reuses a value computed from exactly the
same inputs. Evaluation order and arithmetic are unchanged.

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
  changes. Each voice keeps the control values it last used.
- **Constants are reused.** The reverb keeps its eight feedback gains until
  the decay changes. A curved envelope computes `expm1(-curve)` once. The
  drive reuses the previous sample's `ln cosh` and `tanh(bias)` while drive
  and bias are unchanged.

## Results

Apple M1, release build, measured with `time -l`. Cycles and instructions
retired do not depend on other load on the machine.

| "Late Window" render | Instructions | Cycles |
| --- | ---: | ---: |
| Before | 536.8 billion | 161.5 billion |
| After | 220.5 billion | 88.7 billion |
| Ratio | 2.43× fewer | 1.82× fewer |

On an otherwise idle machine the render took 45.6 s before, and 24.2 s
after the engine and voice-parameter changes, before the constant caches
were added. The std/basic version of the track took 26.7 s before the
palette work. The [std/studio examples](studio-delivery.md#render-cost)
take 1.6–2.4 times fewer cycles.

Both builds rendered the same source to byte-identical WAV files, and the
full test suite passes.

## What is left

- **The graph walk.** Each voice still visits every node once per sample,
  with a dispatch and bounds checks per visit. Processing a block of samples
  per node would remove most of that, but it changes how the engine
  schedules events, automation and feedback delays. It needs its own design.
- **Rendering one section.** `analyze` and `render --start-frame` still
  render from the start, because effects and voices carry state. Rendering a
  window needs the prehistory that §28 describes.
