# Implemented capability set

The foundation implements a bounded subset of MaaC/1. It does **not** claim
full Document, Performance, Core Audio, or Locked Render conformance. The language
specification remains authoritative; this page describes the implementation scope.

| Feature | Foundation scope |
| --- | --- |
| Source | UTF-8, span-aware parsing, exact rational quantities, comments and source text retained in the parsed document |
| Patterns | Finite notes and hits, nested uses, repetition, stretching, cents transposition for notes, cut and spill |
| Arrangement | Explicit tracks and placements, stable occurrence addresses, final-state overrides, deletion and inserts |
| Time | Step and linear-in-score tempo maps, meter maps, global `bar` positions, independent physical offsets, certified ceiling scheduling |
| Pitch | Letter pitches, `key`, `degree`, `ratio`, Hz/kHz, explicit tunings |
| Per-note pitch | `core.sine/1` and reusable mono/stereo instruments (basic, acoustic, custom); cents curves; normalized, seconds and score clocks; step/linear interpolation and gate-end release holding |
| Per-note gain | `core.sine/1` and reusable mono/stereo instruments (basic, acoustic, custom); nonnegative amplitude curves on all three clocks; step/linear/exponential interpolation; simultaneous independent pitch |
| Per-note timbre | Voice graphs explicitly declaring `synth.timbre/1`; exact 0…1 curves on all three clocks; step/linear/exponential interpolation; independent pitch/gain coexistence and release holding |
| Per-note pressure | Voice graphs explicitly declaring `synth.pressure/1`; exact 0…1 curves on all three clocks; step/linear/exponential interpolation; independent pitch/gain/timbre coexistence and release holding |
| Automation | Global score/seconds clocks; step, linear and exponential interpolation; sample/event parameter rates |
| Routing | Explicit mono/stereo audio graph and note/hit targets; no implicit channel conversions or mixers |
| Processors | `core.sine/1`, `core.kit/1`, `core.onepole/1`, `core.pan/1`, `core.sum/1`, `core.gain/1`, `core.fader/1`, `core.matrix/1` (mono/stereo) |
| Sample kits | Pinned raw float32 mono/stereo assets, native-rate one-shot playback, linear interpolation, natural tails and sample-rate level automation |
| Arranged audio | Top-level rate and warp-rate clips; source-frame slicing, musical warp anchors, rate-mode speed/reverse/physical placement, gain, linear/equal-power fades, explicit routing and tempo-aware tails |
| Reusable instruments | Named libraries, typed public controls, presets, independent polyphonic instances, voice and shared graphs |
| Sound graphs | Versioned sine/saw/square/triangle/wavetable oscillators, deterministic noise and recirculating plucked strings, linear ADSR, sine LFO, gain, low/high-pass one-pole filtering, mixing and panning |
| Modulation | Top-level typed control modulation of continuous sample-rate, note-on/note-off, and instrument reset parameters, score/seconds LFOs and automated constants; instrument feed-forward graph modulation, ADSR, voice phase, and shared-LFO reset capture, and sample-wise through-zero linear FM; no oversampling |
| Built-in instruments | 24 stereo exports in `std/basic/1.0.0` with four common controls; three separate `std/acoustic/1.0.0` guitars with six controls; exact-version CLI/Rust discovery |
| Local dependencies | Explicit namespace aliases, transitive declaring-file resolution, SHA-256 source/WAV pins, project containment |
| Wavetables | Explicit mono WAV cycles, cyclic interpolation, adjacent-frame morphing and harmonic-limited banks |
| Regions | Named score intervals retained as non-rendering metadata |
| Render | Reset-state offline rendering at 48 kHz, score-end releases, explicit tail |
| Export | Legacy build/render: Float32 WAV or overload-rejecting PCM16; production delivery also adds PCM24 and explicit seeded TPDF |
| Native production | Project-level EQ, linked peak compression with external sidechains, eight-delay reverb; required `maac.production/1` |
| Named deliveries | Complete-graph master/stem capture; 44.1/48/96 kHz conversion; final-artifact loudness/sample-peak/experimental true-peak analysis |
| Interchange | Independently validated standalone plans: version 1 legacy, version 2 embedded graph/data/provenance, version 3 exact ramp timing recipes, version 4 embedded audio assets and kit nodes, version 5 rate clips, version 6 warp-rate clips, version 7 core control modulation |

Recognized deferred features fail with `E_CAPABILITY`: messages,
other processors, pitched sample instruments,
preserve-pitch audio warping, external plug-ins and other extensions. Transactional editing, full render locks,
MIDI transport, GUI and real-time playback are outside this release's interfaces.
No deferred feature is approximated silently.

The [kit contract](core-kit.md) defines native hit scheduling, raw sample assets,
voice capacity, interpolation and standalone replay. The additive `*_artifact`
Rust APIs support versions 1–7 through opaque `PlanArtifact`; existing APIs keep
their supported versions. WAV importing and reusable sample-kit library exports
remain outside this slice.

The [audio clip contract](audio-clips.md) defines rate-mode transport through
the same clock and graph. Clips remain independent of pattern note/hit events.
Tracks may group clips without an event target; grouping creates no routing.
The [warp-rate contract](warp-rate.md) adds ordered musical source-frame anchors
through the full tempo map, including the tail. Preserve-pitch warping and
placement inside patterns remain unsupported.

The [core modulation contract](core-modulation.md) implements explicit control
edges, `core.lfo/1`, and `core.constant/1`. Score LFOs follow tempo and hold at
score end; seconds LFOs continue through the tail. Additive contributions follow
modulation-ID order and the target's final range policy. Controls remain distinct
from audio outputs. Event-rate targets capture the combined value at note-on or
note-off; instrument reset controls capture during preparation. Direct reset-control
automation and same-sample cycles are rejected.

The [tempo ramp contract](tempo-ramps.md) defines linear BPM in score position,
inverse-clock automation, and version 3 interchange. The CLI automatically
selects the plan version. Existing Rust entry points retain their step-only
contracts; additive `*_versioned` entry points support ramps. Exponential tempo
shapes remain invalid. Timing certification can fail with `E_TIME_PRECISION`
or `E_RESOURCE_LIMIT` instead of guessing a frame boundary.

The [per-note pitch guide](pitch-expression.md) defines the supported receivers,
curve clocks, instance transformations, and standalone-plan compatibility.
The [gain guide](gain-expression.md) adds independent swells and fades while
preserving voice state at zero gain. The [instrument gain guide](instrument-gain.md)
covers reusable graphs and the additional execution-work charge. The
[instrument pitch guide](instrument-pitch.md) explains bends, vibrato, and
base versus live node frequency validation. The [timbre guide](timbre-expression.md)
defines explicit graph opt-in and authored mappings; `core.sine/1`, graphs without
a source, and the frozen basic/acoustic libraries reject timbre with `E_CAPABILITY`.
The [pressure guide](pressure-expression.md) adds independent authored pressure
mappings with separate `synth.pressure/1` opt-in. All four expression kinds may
coexist when the graph declares both sources; frozen libraries remain unchanged.

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
retention boundary. Native production leaves Core Audio obligations, grammar,
and the generic syntax-tree schema unchanged. Tempo ramps separately introduce
performance-plan version 3.

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
100,000 expanded note/hit events, 256 nodes, 4,096 bits per rational component, and 30 minutes
of rendered duration including tail. The engine supports 48 kHz and one or two
audio channels. Exceeding a bound is an explicit `E_RESOURCE_LIMIT` (unsupported
rates/channel capabilities use `E_CAPABILITY`). Additional bounds are:

| Resource | Limit |
| --- | --- |
| Source/aggregate plan objects | 200,000 |
| Audio connections and top-level modulation edges | 4,096 combined |
| Tempo points | 4,096 |
| Global automation, expanded per-note pitch/gain/timbre/pressure points, and warp anchors | 65,536 combined |
| Warp anchors per clip | 4,096 |
| Regions | 16,384 |
| Additional source mappings | 100,000 |
| Identifier | 128 ASCII bytes |
| Individual plan metadata string | 4,096 bytes |
| Aggregate plan strings | 4 MiB |
| Declared voices per sine node | 1,000,000 |
| Sum of target voice capacities over note/hit events | 5,000,000 |
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
| Instrument expression execution charge | `17 + ceil(log2(point_count))` units per active voice frame for each attached pitch, gain, timbre, or pressure curve, additive when combined, including conservative release |
| Pluck execution charge | 16 units per sample visit plus 2402 initialization units per note per pluck node |

Execution work includes every note's gate and maximum possible release, bounded
by the render endpoint, plus shared effects throughout the full output. Caller
`PlanLimits` can tighten graph, table, state, and work limits. The 4 MiB plan
artifact limit applies alongside embedded-sample limits.

Core audio assets share the 4 MiB per-file, 16 MiB aggregate asset and 64-asset
bounds. Raw bytes count toward structural work; mapped assets and keys count
toward object/string limits. Kit execution charges each node for the complete
render interval and each hit for its natural lifetime clipped at render end,
including silent hits. See the [exact accounting](core-kit.md#kit-playback).

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
