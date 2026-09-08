# Performance-plan format, version 1

This JSON artifact is produced by the MaaC `maac compile` command and consumed
by `maac render`. It is distinct from `syntax-tree.schema.json` and contains
resolved performance data rather than editable source syntax. Recompile after
source changes; the renderer does not reopen the source file or fetch
dependencies.

## Top-level fields

| Field | Content |
| --- | --- |
| `version` | Performance-plan version, currently `1` |
| `output` | Score origin/end, explicit tail, sample rate, channel count, total frames and output port |
| `tempo` | Ordered step-tempo points needed for exact score/physical conversion |
| `events` | Resolved note occurrences, identities, source mappings, pitches, physical offsets and schedules |
| `nodes` | Processor instances, structural configuration and numeric parameters |
| `connections` | Named explicit audio edges |
| `automation` | Global replacement lanes with resolved targets, clocks, anchors and points |
| `regions` | Named non-rendering score intervals |
| `source_mappings` | Additional source identity metadata |

Rationals serialize as canonical reduced strings such as `"0/1"`, `"1/3"`, or
`"-1/50"`, always with a positive denominator. JSON floating-point numbers are
used for resolved pitch and release velocity. Source pitch spelling remains in
the parsed document; equal-tempered frequency is a binary64 DSP input.

## Output and clocks

`output` contains `score_start_q`, `score_end_q`, `tail_seconds`,
`sample_rate_hz`, `channels`, `total_frames`, and `output`. Port references have
the shape `{"node":"master","port":"out"}`. Foundation output supports
48,000 Hz and one or two channels.

`tempo.points` contains objects with `q`, `bpm`, and `shape`. Only `"step"` is
executable in this format. `T(0)=0`; the first/last tempo extends beyond the map's
endpoints. The reset origin is `T(score_start_q)`, including negative pickups.

`total_frames` must equal the exact ceiling of
`rate * (T(score_end_q) - T(score_start_q) + tail_seconds)`.

## Events

Each event contains:

- `address`: stable occurrence identity, for example `bass_main/7/n2`.
- `source`: source object, declaration path and optional byte `span`.
- `target`: sine node's `events` port.
- `kind`: a tagged note value with `kind: "note"`, `pitch_hz`, and rational
  `velocity`.
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

## Processors and automation

A node has `id`, a tagged `processor`, and a `params` map. Processor tags are
`sine` with `voices`, `one_pole` with `channels`, `pan`, and `sum` with `channels`.
These correspond to the four version-1 reference algorithms named in the
capability matrix. Parameter values use canonical units: seconds, Hz, or
dimensionless numbers.

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

## Import boundary

`Plan::from_json` enforces the input byte limit before deserializing and validates
the plan independently. `Plan::validate` also checks in-memory plans. Unknown
fields, unsupported versions/features, duplicate identities, malformed rational
strings, invalid references, ranges, intervals, schedules, graphs and resource
overruns are errors. Rendering validates again before allocating execution state.
Source mappings are metadata; they do not authorize file access or execution.
