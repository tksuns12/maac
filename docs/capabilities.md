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
| Per-note pitch | `core.sine/1` cents curves; normalized, seconds and score clocks; step/linear interpolation and gate-end release holding |
| Automation | Global score/seconds clocks; step, linear and exponential interpolation; sample/event parameter rates |
| Routing | Explicit mono/stereo audio graph and note targets; no implicit channel conversions or mixers |
| Processors | `core.sine/1`, `core.onepole/1`, `core.pan/1`, `core.sum/1`, `core.gain/1` (mono/stereo) |
| Reusable instruments | Named libraries, typed public controls, presets, independent polyphonic instances, voice and shared graphs |
| Sound graphs | Versioned sine/saw/square/triangle/wavetable oscillators, deterministic noise and recirculating plucked strings, linear ADSR, sine LFO, gain, low/high-pass one-pole filtering, mixing and panning |
| Modulation | Feed-forward graph modulation and sample-wise through-zero linear FM; no oversampling |
| Built-in instruments | 24 stereo exports in `std/basic/1.0.0` with four common controls; three separate `std/acoustic/1.0.0` guitars with six controls; exact-version CLI/Rust discovery |
| Local dependencies | Explicit namespace aliases, transitive declaring-file resolution, SHA-256 source/WAV pins, project containment |
| Wavetables | Explicit mono WAV cycles, cyclic interpolation, adjacent-frame morphing and harmonic-limited banks |
| Regions | Named score intervals retained as non-rendering metadata |
| Render | Reset-state offline rendering at 48 kHz, score-end releases, explicit tail |
| Export | Legacy build/render: Float32 WAV or overload-rejecting PCM16; production delivery also adds PCM24 and explicit seeded TPDF |
| Native production | Project-level EQ, linked peak compression with external sidechains, eight-delay reverb; required `maac.production/1` |
| Named deliveries | Complete-graph master/stem capture; 44.1/48/96 kHz conversion; final-artifact loudness/sample-peak/experimental true-peak analysis |
| Interchange | Independently validated standalone plans: version 1 legacy and version 2 embedded graph/data/provenance |

Recognized deferred features fail with `E_CAPABILITY`: tempo ramps, per-note
gain/pressure/timbre expression, pitch expression on graph instruments,
hits/messages, top-level core modulation, other processors, recorded
sample instruments, arranged audio, external plug-ins and other extensions. Transactional editing, full render locks,
MIDI transport, GUI and real-time playback are outside this release's interfaces.
No deferred feature is approximated silently.

The [per-note pitch guide](pitch-expression.md) defines the supported receiver,
curve clocks, instance transformations, and standalone-plan compatibility.

The [native production contract](production.md), required capability
`maac.production/1`, has an **experimental implementation**. Native `fx.eq/1`,
`fx.compressor/1`, and `fx.reverb/1` support mono/stereo project-level nodes,
sample automation, private reset state, and explicit tails. Named master/stem
outputs retain sidechain and shared-effect context. Delivery supports 44.1/48/96
kHz, Float32/PCM24/PCM16 WAV, explicit none/seeded TPDF dither, and measurements
of the reconstructed final artifact. Failed requested limits retain completed
audio and produce a failed check result; they do not trigger automatic gain.

Analyzer `maac.analysis.bs1770-5/2` uses the approved single-stage four-times
Annex 2 true-peak profile. The [metering evidence](production-metering-evidence.md)
records applicable current fixtures and the historical 16× profile comparison.
These results do not establish full ITU/EBU compliance or listening acceptance.
`check_production.py` remains a separate syntax/schema/arithmetic smoke checker;
Rust tests and external metering/SRC gates provide separate implementation
evidence. The [production delivery report](production-delivery.md) records the
installed source/retained-plan, selection, publication, and failed-check audio
retention boundary. Core Audio obligations, grammar, generic syntax-tree schema,
and performance-plan version numbers remain unchanged.

The implemented [project-entrypoint extension](project-entrypoint.md) adds
conventional `main.maac` discovery for source commands and explicit default/song
execution profiles. All 323 integrated tests and the installed entrypoint
checks passed. Full native song build and source-free retained rendering
produced identical WAV bytes; the [delivery report](project-entrypoint-delivery.md)
records the evidence. It changes no import or source grammar
rules. The existing normative `core.gain/1` algorithm is implemented for
visible mono/stereo master gain. Its parameter is `gain`, not `level`; finite
gains above 1 are valid, subject to finite output and separate export headroom checks.

The [plucked-string contract](plucked-string.md) defines mono voice-only
`synth.pluck/1` and the separate three-guitar `std/acoustic/1.0.0` collection.
The [acoustic guide](acoustic-guitars.md) records authoring behavior and the
[delivery report](acoustic-delivery.md) records numerical and installed checks.
Its private delay state does
not permit feedback edges in public audio or modulation graphs.

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
| Global automation plus expanded per-note pitch points | 65,536 combined |
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
object limit applies across the bundle. Embedded built-in source bytes, files,
objects and import depth count toward these limits; the reserved `@builtin/`
namespace cannot be supplied or shadowed by the caller.
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
| Conservative execution work | Default 500,000,000 normalized units; explicit song profile maximum 10,000,000,000 |
| Pluck delay cells | 8,388,608 f64 cells / 64 MiB payload; 2402 cells per pluck voice-node |
| Pluck execution charge | 16 units per sample visit plus 2402 initialization units per note per pluck node |

Execution work includes every note's gate and maximum possible release, bounded
by the render endpoint, plus shared effects throughout the full output. Caller
`PlanLimits` can tighten graph, table, state, and work limits. The 4 MiB plan
artifact limit applies alongside embedded-sample limits.

The explicit song profile changes only execution work. Plan bytes, duration,
events, channels, rate, graph/voice states and pluck memory retain their existing
bounds. Composition checks validate the chosen work budget. No source/plan field
can elevate it, and retained plans require caller selection again. Execution
profiles are resource policies, not the language's conformance profiles.

Pluck memory counts declared instance voice capacities, including unconnected
pluck nodes, and is checked before runtime/ring allocation. Direct runtimes
independently enforce the same per-runtime ceiling. Muting does not discount
memory, initialization, or sample work. The new
`PlanLimits::max_pluck_delay_cells` field can tighten the ceiling and is not
serialized into plans; exhaustive Rust struct literals need the new field or
`..PlanLimits::default()`. Existing processor work weights remain unchanged.

## Production resource bounds

Native DSP uses the existing execution-work allowance. Per frame, EQ charges
`32 + 10*C` units, compression `24 + C + detector_channels`, and reverb
`96 + 40*C`, where `C` is the main width. Internal detection uses `C` detector
channels. Charges include every node throughout the complete interval and tail.
Reverb history is `15562 + C*(predelay_frames + 360) + 8` binary64 cells per
node. `PlanLimits::max_production_delay_cells` defaults to at most 4,194,304
cells (32 MiB payload) across all reverbs and may be tightened by the caller.

Explicit multi-port capture accepts at most 256 port selections, including
duplicates, and charges `total_frames * sum(selected_channel_counts)` additional
copy-work units against the selected plan allowance before allocating capture
buffers. The ordinary single-master render API retains its existing budget
behavior. Production records allow at most 64 deliveries, 256 targets per
delivery, and 1024 targets in total. Production data participates in aggregate
plan object/string bounds and selected rational/identifier limits; source
identity evidence has a separate bounded payload within the plan byte budget.

`DeliveryLimits` separately defaults to two billion conversion/analysis work
units; `song` permits at most 100 billion. Both profiles retain 256 selected
targets, 2 GiB of spool data, 4 GiB aggregate disk use, and the 30-minute,
96-kHz maximum output-frame bound. Source/plan data cannot authorize larger
budgets. `--profile song` selects both the plan and delivery work allowances.

## Fidelity

Source timing and frame ceilings use exact rational arithmetic. DSP uses
binary64 values; WAV conversion is a separate export step. Determinism is tested
within one executable and environment. Cross-platform bitwise identity is not
claimed. Finite, non-silent samples are automated checks and do not substitute
for a listening review. The basic instrument names describe synthesized musical
roles, not recorded acoustic instruments. Noise uses a fixed, independently
reset per-voice xorshift32 sequence; see the [processor contract](instruments.md).
