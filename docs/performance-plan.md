# Performance-plan format, versions 1, 2 and 3

This JSON artifact is produced by the MaaC `maac compile` command and consumed
by `maac render`. It is distinct from `syntax-tree.schema.json` and contains
resolved performance data rather than editable source syntax. Recompile after
source changes; the renderer does not reopen the source file or fetch
dependencies.

Version 1 retains the legacy processor contract. Version 2 adds the embedded
instrument resources described below. Bundle compilation, including CLI source
commands, emits version 2 for step-only scores. The parsed-document Rust API
still emits version 1 for legacy-only documents. Versioned compilation emits
version 3 when a tempo point has a `linear` shape, including flat ramps.
All three versions render without the original sources.

## Version 3 timing

Version 3 shares the graph, resource, expression, and output fields below.
It supports both `step` and `linear` tempo segments. BPM is linear in score
position, so physical duration uses a logarithmic integral. Positive BPM,
strictly increasing point positions, and a final `step` shape are required.

Events omit `on_seconds` and `off_seconds`; those fields are rejected. Exact
`score_on_q`, `score_off_q`, onset/release offsets, and the tempo map determine
physical timing. Import validation independently certifies every stored frame
count, including the output duration and release clamp.

Automation `at` is a tagged object: `{"kind":"score","q":"1/1"}` or
`{"kind":"seconds","seconds":"1/2"}`. Score-clock lanes require a score
anchor. Seconds-clock lanes accept either; a score anchor means `T(q)` followed
by the curve's physical offsets. Instrument resources may be omitted for
core-only graphs, but instrument nodes require embedded resources.

The Rust types `PlanV3`, `ResolvedEventV3`, `AutomationV3`, and `VersionedPlan`
are additive. `VersionedPlan` serializes directly with the top-level `version`
field, without an enum wrapper. Use `compile_versioned`,
`compile_bundle_versioned`, `load_plan_versioned`, `render_versioned`, and the
versioned WAV export functions; caller-limited variants are available. Existing
`Plan` types and compile/load/render functions keep their version 1/2 contracts
and reject ramps. Older readers reject version 3 explicitly.

Certification uses bounded interval arithmetic and returns `E_TIME_PRECISION`
when it cannot prove a frame boundary, or `E_RESOURCE_LIMIT` when work is
exhausted. See the [tempo contract](tempo-ramps.md) for timing, budgets, and
delivery manifest version 2. The field descriptions below retain the version
1/2 representation where version 3 differs as described above.

## Top-level fields

| Field | Content |
| --- | --- |
| `version` | Performance-plan version, `1`, `2` or `3` |
| `output` | Score origin/end, explicit tail, sample rate, channel count, total frames and output port |
| `tempo` | Ordered tempo points needed for score/physical conversion |
| `events` | Resolved note occurrences, identities, source mappings, pitches, physical offsets and schedules |
| `nodes` | Processor instances, structural configuration and numeric parameters |
| `connections` | Named explicit audio edges |
| `automation` | Global replacement lanes with resolved targets, clocks, anchors and points |
| `regions` | Named non-rendering score intervals |
| `source_mappings` | Additional source identity metadata |
| `instruments` | Required version 2 resource payload; absent in version 1; version 3 requires it for instrument nodes |
| `production` | Optional native-delivery settings and original-source execution identity; absent for legacy plans |

Rationals serialize as canonical reduced strings such as `"0/1"`, `"1/3"`, or
`"-1/50"`, always with a positive denominator. JSON floating-point numbers are
used for resolved pitch and release velocity. Source pitch spelling remains in
the parsed document; equal-tempered frequency is a binary64 DSP input.

## Output and clocks

`output` contains `score_start_q`, `score_end_q`, `tail_seconds`,
`sample_rate_hz`, `channels`, `total_frames`, and `output`. Port references have
the shape `{"node":"master","port":"out"}`. Foundation output supports
48,000 Hz and one or two channels.

`tempo.points` contains objects with `q`, `bpm`, and `shape`. Versions 1 and 2
execute only `"step"`. `T(0)=0`; the first/last tempo extends beyond the map's
endpoints. The reset origin is `T(score_start_q)`, including negative pickups.

`total_frames` must equal the exact ceiling of
`rate * (T(score_end_q) - T(score_start_q) + tail_seconds)`.

## Events

Each event contains:

- `address`: stable occurrence identity, for example `bass_main/7/n2`.
- `source`: source object, declaration path and optional byte `span`.
- `target`: sine or instrument node's `events` port.
- `kind`: a tagged note value with `kind: "note"`, `pitch_hz`, and rational
  `velocity`; optional `pitch_expression`, `gain_expression`, `timbre_expression`, and
  `pressure_expression` carry per-note cents, amplitude, authored timbre, and
  authored pressure curves.
- `score_on_q`, `score_off_q`: final musical coordinates after inherited
  transformations, repetition cuts, and occurrence edits.
- `onset_offset_seconds`, `release_offset_seconds`: physical offsets.
- `on_seconds`, `off_seconds`: absolute physical times after effective
  project-end truncation.
- `on_frame`, `off_frame`: exact ceiling schedules relative to reset origin.
- `release_velocity` and integer `order`.

Validation recomputes the physical times from score coordinates and offsets,
checks effective score bounds, and recomputes the frame ceilings. A positive gate
that collapses to one frame boundary fails. At a shared frame, note-offs precede
note-ons; equal-class events sort by `order`, then unsigned UTF-8 event address.

`pitch_expression` is a strict object with `clock` (`score`, `seconds`, or
`normalized`) and `points`. Each point has canonical rational `position` and
`cents`, plus `shape` (`step` or `linear`; the final shape is `step`). Score
positions already include inherited musical stretch. The
[pitch-expression contract](pitch-expression.md) defines evaluation over the
scheduled frame gate, release holding, and effective-domain frequency checks.
`core.sine/1` and reusable mono/stereo instruments receive pitch expression.
The [instrument pitch contract](instrument-pitch.md) validates the expressed
base across the reachable curve domain, including the gate end; node effective
frequency bounds depend on live controls and modulation and are checked at render
time, even for silent voices.

`gain_expression` uses the same clocks and a strict `points` array with
canonical rational `position` and dimensionless `gain`, plus `shape`. Gains
are nonnegative finite engine values with no upper limit of 1. Step and linear
segments allow zero; exponential segments require strictly positive endpoints.
The [gain-expression contract](gain-expression.md) defines sample evaluation,
zero-gain voice state, release holding, and simultaneous pitch/gain expression.
`core.sine/1` and reusable mono/stereo instruments receive gain expression.
The [instrument gain contract](instrument-gain.md) places gain after each
complete voice contribution, before summation and shared effects, and adds a
point-count-dependent charge to existing execution work. This receiver extension
adds no wire fields or plan version. Pitch, gain, timbre, and pressure share the global
automation-point budget, including after pattern expansion. Instrument pitch adds
`17 + ceil(log2(point_count))` work units per conservative active voice frame,
additively with gain, timbre, and pressure; the 65,536-point and 4,096-bit rational limits are unchanged.

`timbre_expression` is a strict object with `clock` and `points`. Each point
has canonical rational `position` and dimensionless `value` in `[0, 1]`, plus
`shape` (`step`, `linear`, or `exponential`; the final shape is `step`).
Exponential endpoints must be strictly positive. The
[timbre contract](timbre-expression.md) defines timing, release holding, mapping,
and the additive `17 + ceil(log2(point_count))` per-voice-frame work charge.
Only a graph whose voice stage declares `synth.timbre/1` receives timbre;
capability is derived from its embedded program, even for an unconnected source.
There is no separate capability flag. Graphs without that source, including
frozen basic/acoustic graphs, and `core.sine/1` reject the expression.

`pressure_expression` uses the same strict clock/points structure and exact
`[0, 1]` bounds as timbre. Unknown fields are rejected at every payload level.
The public Rust types are `PressureExpression` and `PressureExpressionPoint`.
The [pressure contract](pressure-expression.md) requires a `synth.pressure/1`
voice source independently of timbre opt-in. Absent pressure is zero; its gate-end
value holds through release. All four kinds may coexist on an opted-in graph.
Each pressure curve adds `17 + ceil(log2(point_count))` work units per conservative
active voice frame, including release and silent voices. Automation and all four
expression kinds share the 65,536 expanded-point limit and 4,096-bit rational bound.
The public runtime `note_on` signature is unchanged.

These optional note fields are supported in all three plan versions. Absent fields
are omitted, preserving previous JSON shapes; older readers reject fields they
do not support. Rust `EventKind::Note` literals add `pitch_expression: None`,
`gain_expression: None`, `timbre_expression: None`, and
`pressure_expression: None` when absent. The shared `ExpressionClock` retains
`PitchExpressionClock` as a compatibility type alias.

## Processors and automation

A node has `id`, a tagged `processor`, and a `params` map. Processor tags are
`sine` with `voices`, `one_pole` with `channels`, `gain` with `channels`, `pan`,
and `sum` with `channels`. These correspond to the reference algorithms named in the
capability matrix. Parameter values use canonical units: seconds, Hz, decibels, or
dimensionless numbers.

`core.gain/1` uses the strict processor object `{"kind":"gain","channels":2}`
(or channels 1) and the ordinary node parameter map. Its only parameter is
`gain`: finite, dimensionless, nonnegative, default `"1/1"`, sampled every
frame. For example, `"params":{"gain":"7/10"}` multiplies each input channel
by 0.7. There is no `level` alias, upper gain limit of 16, or implicit smoothing.
It requires exactly one matching audio input. Missing or unsupported channels,
unknown fields, invalid gain values, and nonfinite output fail explicitly.
This implements the existing MaaC/1 §18.2 algorithm in all three plan
versions. Its introduction required no schema version bump; older renderers may reject the newly
supported `gain` kind.

Versions 2 and 3 also support `{"kind":"instrument","program":"program_0",
"voices":64,"channels":2}`. `program` identifies an embedded reusable program;
`channels` must match its output. The node's parameter map contains public
control values. Missing values use the program's control defaults. Compiled
source instances carry the values resolved from defaults, preset, and instance
overrides. Presets are authoring data, so rendering does not need preset lookup.

A connection has `id`, `from`, and `to` port references. Channel counts must
match. Single-input ports require one incoming edge; summing inputs accept zero
or more. Graph execution orders available nodes by ID and sums inputs by
connection ID. Cycles are rejected.

An automation lane has `id`, parameter `target` (`node` plus parameter name in
`port`), `clock` (`score` or `seconds`), rational `at`, and `points`. Each point
has rational `position`, rational `value`, and outgoing `shape` (`step`, `linear`,
or `exponential`). Score anchors are absolute q; seconds anchors are absolute
physical seconds under `T`. Relative point positions begin at zero. Before the
anchor, the authored base parameter applies. During the tail, all automation
holds its exact score-end value.

Instrument automation targets only public controls and inherits each underlying
parameter's unit, range, and rate. Sample-rate parameters evaluate every frame;
note-on parameters are captured at note-on, and release is captured at note-off.
Reset-rate parameters cannot be automated. Internal graph nodes are private.

## Native production and named deliveries

All three versions additionally support these strict processor objects:

```json
{"kind":"fx.eq/1","channels":2,"mode":"peak"}
{"kind":"fx.compressor/1","channels":2,"sidechain_channels":1}
{"kind":"fx.reverb/1","channels":2,"predelay_frames":960,"damping":"1/2"}
```

EQ mode is `peak`, `low_shelf`, `high_shelf`, `low_pass`, or `high_pass`.
Omitted compressor `sidechain_channels` selects the internal detector; a value
of 1 or 2 requires exactly one matching `sidechain` connection. Main `in` and
`out` widths are independently set by `channels`, always 1 or 2. Detector audio
orders graph execution and participates in cycle checks but does not enter the
audible sum. Reverb predelay is the exact ceiling of source seconds times
48000, bounded to 0–12000 frames; damping is a canonical rational in [0,1].
No mutable filter, detector, or delay state is serialized.

Native parameters live in the ordinary rational `params` map, with applicable
defaults resolved by the source compiler and renderer. EQ has `frequency` in
Hz, `q` dimensionless, and `gain` in dB; shelves forbid `q`, and pass filters
forbid `gain`. Compression has dB `threshold`, `knee`, `makeup`, dimensionless
`ratio`, and seconds `attack`/`release`. Reverb has seconds `decay` and
unitless `mix`. All are sampled every frame, retain state during automation and
tail, and apply the bounds in the [native contract](production.md).
Exponential interpolation is rejected for dB parameters even when positive
endpoints were supplied directly in JSON. Structural config cannot be automated.
Unknown fields, invalid layouts, disconnected required inputs, ranges, and
nonfinite values fail at the same independent plan boundary as core processors.

Optional `production` contains `schema_hash`, `extension_id`, `deliveries`, and
`execution_identity`. Delivery and target records retain stable source IDs;
rates become integer Hz, output references use the ordinary `{node,port}`
shape, encodings retain their `wav_*` identifiers, and dither is a tagged
`{"type":"none"}` or `{"type":"tpdf","seed":...}` record. Limit units remain explicit
and bounds become canonical rational strings. Each delivery must contain one
master matching `output.output`; all other targets are stems. Original source
extension/descriptor objects are validated before removal from compilation.
The retained identity includes its algorithm, execution hash, additional
source-input hash, and normalized source JSON evidence. Rendering never follows
source paths in that evidence or retrieves the schema again.

Production data is validated in memory and on JSON import, including schema
identity, limits, references, and caller resource budgets. The complete
extension contributes to the execution identity; selected delivery/targets,
converter/dither/analyzer identities, and numeric environment also enter the
render identity. The final-artifact manifest keeps artifact completion separate
from requested-check status. Metering remains experimental; consult the
[current external-gate evidence](production-metering-evidence.md).

These tags and the optional field were introduced without changing versions 1
and 2, and are also supported in version 3. Older readers may reject the new
features. Plans without production settings omit `production` and keep their prior wire shape.
Rust `Plan` struct literals add `production: None` for that case. Native history,
execution, and selected-port copy budgets are described in the
[resource bounds](capabilities.md#production-resource-bounds).

## Version 2 instrument resources

Version 3 reuses the instrument resource representation introduced in version 2.
`instruments` has these fields, all validated even when unused by an instance:

| Field | Content |
| --- | --- |
| `entry_source` | Normalized project-relative entry path |
| `programs` | Named instrument programs with voice graph, optional shared graph, public controls, and source provenance |
| `wavetables` | Table ID, explicit cycle length, and finite mono raw samples, grouped into contiguous frames |
| `wavetable_sources` | Table ID, declaring file/object, original asset path, and SHA-256 pin |
| `source_files` | Source paths and exact byte hashes |
| `dependencies` | Declaring source, namespace alias, imported path, and matching source hash |
| `libraries` | Declaring file/object, nonempty version, and optional creator/license metadata |

A program has `id`, `voice`, optional `shared`, `controls`, and `source`.
Its graph contains `channels`, `nodes`, `connections`, `modulations`, `output`,
and a designated `amplitude` node for the voice graph. Graph processors use
versioned `kind` tags such as `synth.sine/1` and `synth.wavetable/1`; a wavetable
processor references an embedded table ID. A control maps its name to a
`target` (`graph`, `node`, `parameter`) and rational `default`. A modulation has
an ID, source port, parameter target, and rational depth. Both audio and
modulation edges participate in cycle detection.

The [instrument contract](instruments.md) specifies processor behavior, graph
ports, units, rates, and source syntax. The [resource bounds](capabilities.md#resource-bounds)
apply before rendering, including graph capacity and conservative execution
work. Source and asset provenance records identify dependencies; they are not
retrieval instructions or a cryptographic attestation of an edited plan's audio.
The renderer constructs harmonic banks from the embedded raw table data.
It never opens the recorded paths. Version 1 rejects instrument processors and
non-null instrument resources; version 2 requires the resource payload.

The [plucked-string implementation contract](plucked-string.md) defines an additive
version 2/3 graph processor, `{"kind":"synth.pluck/1","seed":1831565813}`, with
strict resolved-seed validation and separate delay-memory and weighted-work
bounds. It does not change plan versions or serialize mutable string state.
Its approved design is not a claim of current renderer support.

## Import boundary

`Plan::from_json` enforces the input byte limit before deserializing and validates
the plan independently. `Plan::validate` also checks in-memory plans. Unknown
fields, unsupported versions/features, duplicate identities, malformed rational
strings, invalid references, ranges, intervals, schedules, graphs and resource
overruns are errors. Rendering validates again before allocating execution state.
Source mappings are metadata; they do not authorize file access or execution.

The approved [entrypoint and execution-profile extension](project-entrypoint.md)
adds explicit-limit loading, validation, serialization, DSP preparation and
export APIs without changing this wire format or plan version. Default wrappers,
including generic Serde `Plan` decoding, retain the 500-million work budget.
Caller-selected `song` allows up to 10 billion execution-work units while all
other ceilings remain unchanged. A large retained plan needs that explicit
allowance on load and render; it carries no profile or authority to elevate
limits. Implementations must propagate selected limits through every nested
boundary. The implementation passed the integrated and installed checks
recorded in the [entrypoint delivery report](project-entrypoint-delivery.md).

## Timbre and pressure graph sources

Version 2 and 3 instrument graphs additionally accept the exact processor object
`{"kind":"synth.timbre/1"}` or `{"kind":"synth.pressure/1"}` with no
additional fields. Each is voice-only,
parameterless, inputless, and mono; its node `params` map is empty. All sources
of each kind in one voice share that voice's evaluated value, zero when absent.
Source `config` is rejected even when empty; empty `params` is allowed.
Ordinary graph connections and sample-rate modulation retain their existing
rules and costs. Instrument graphs require version 2 or 3; the timbre/pressure
extension itself introduced no plan-version bump. Older readers reject the new processor or
note field, and absent timbre/pressure fields remain omitted from existing plans.
