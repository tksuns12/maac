# Implemented capability set

The foundation implements a bounded subset of MaaC/1. It does **not** claim
full Document, Performance, Core Audio, or Locked Render conformance. The language
specification remains authoritative; this page describes the implementation scope.

| Feature | Foundation scope |
| --- | --- |
| Source | UTF-8, span-aware parsing, exact rational quantities, comments and source text retained in the parsed document |
| Patterns | Finite notes, nested uses, repetition, stretching, cents transposition, cut and spill |
| Arrangement | Explicit tracks and placements, stable occurrence addresses, final-state overrides, deletion and inserts |
| Time | Step tempo maps, meter maps, global `bar` positions, independent physical offsets, exact ceiling scheduling |
| Pitch | Letter pitches, `key`, `degree`, `ratio`, Hz/kHz, explicit tunings |
| Automation | Global score/seconds clocks; step, linear and exponential interpolation; sample/event parameter rates |
| Routing | Explicit mono/stereo audio graph and note targets; no implicit channel conversions or mixers |
| Processors | `core.sine/1`, `core.onepole/1`, `core.pan/1`, `core.sum/1` |
| Reusable instruments | Named libraries, typed public controls, presets, independent polyphonic instances, voice and shared graphs |
| Sound graphs | Versioned sine/saw/square/triangle/wavetable oscillators, linear ADSR, sine LFO, gain, one-pole, mixing and panning |
| Modulation | Feed-forward graph modulation and sample-wise through-zero linear FM; no oversampling |
| Local dependencies | Explicit namespace aliases, transitive declaring-file resolution, SHA-256 source/WAV pins, project containment |
| Wavetables | Explicit mono WAV cycles, cyclic interpolation, adjacent-frame morphing and harmonic-limited banks |
| Regions | Named score intervals retained as non-rendering metadata |
| Render | Reset-state offline rendering at 48 kHz, score-end releases, explicit tail |
| Export | Float32 WAV or overload-rejecting PCM16; no normalization, limiting or dithering |
| Interchange | Independently validated standalone plans: version 1 legacy and version 2 embedded graph/data/provenance |

Recognized deferred features fail with `E_CAPABILITY`: tempo ramps, per-note
expression, hits/messages, top-level core modulation, other processors, recorded
sample instruments, arranged audio, external plug-ins and extensions. Transactional editing, full render locks,
MIDI transport, GUI and real-time playback are outside this release's interfaces.
No deferred feature is approximated silently.

## Resource bounds

The baseline hard limits are 4 MiB per source or plan input, 64 nesting levels,
100,000 expanded notes, 256 nodes, 4,096 bits per rational component, and 30 minutes
of rendered duration including tail. The engine supports 48 kHz and one or two
audio channels. Exceeding a bound is an explicit `E_RESOURCE_LIMIT` (unsupported
rates/channel capabilities use `E_CAPABILITY`). Additional bounds are:

| Resource | Limit |
| --- | --- |
| Source/aggregate plan objects | 200,000 |
| Connections | 4,096 |
| Tempo points | 4,096 |
| Automation points | 65,536 |
| Regions | 16,384 |
| Additional source mappings | 100,000 |
| Identifier | 128 ASCII bytes |
| Individual plan metadata string | 4,096 bytes |
| Aggregate plan strings | 4 MiB |
| Declared voices per sine node | 1,000,000 |
| Sum of target voice capacities over note events | 5,000,000 |
| Pattern expansion work | 20,000,000 visits/repetitions |
| Bar, pitch and tuning indices | Signed 64-bit integers |
| Event `order` | Signed 32-bit integer |

The byte and aggregate limits apply together; reaching a note-count limit does
not guarantee that its serialized plan fits the file-size limit. Oversized
unreduced numeric operands are rejected before expensive integer parsing, even
when they would later cancel. The rational limit also applies during exact
arithmetic. `PlanLimits` permits callers to tighten the foundation limits.

The [instrument extension](instruments.md) additionally limits each source or
WAV file to 4 MiB, aggregate source bytes to 16 MiB, aggregate asset bytes to
16 MiB, source files and assets to 64 each, and import depth to 32. The source
object limit applies across the bundle.
Path keys and references are limited to 4,096 UTF-8 bytes; library version,
creator, and license metadata each have the same byte limit.

| Instrument resource | Limit |
| --- | --- |
| Reusable programs | 128 |
| Nodes / audio-plus-modulation edges per graph | 64 / 256 |
| Public controls per instrument | 64 |
| Aggregate graph nodes / edges | 1,024 / 4,096 |
| Embedded wavetables / raw samples | 64 / 262,144 |
| Cycle length / frames per table | Power of two, 8–2,048 / 1–32 |
| Voice capacity per instrument instance | 4,096 (default 64) |
| Aggregate declared voice graph node states | 262,144 |
| Conservative sample execution work | 500,000,000 node/edge visits |

Execution work includes every note's gate and maximum possible release, bounded
by the render endpoint, plus shared effects throughout the full output. Caller
`PlanLimits` can tighten graph, table, state, and work limits. The 4 MiB plan
artifact limit applies alongside embedded-sample limits.

## Fidelity

Source timing and frame ceilings use exact rational arithmetic. DSP uses
binary64 values; WAV conversion is a separate export step. Determinism is tested
within one executable and environment. Cross-platform bitwise identity is not
claimed. Finite, non-silent samples are automated checks and do not substitute
for a listening review.
