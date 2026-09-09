# Tempo ramps: implementation contract

Status: implemented and validated. See the [validation record](tempo-ramps-validation.md)
for test, installed CLI, compatibility, and independent review evidence.

This work implements the existing MaaC/1 sections 5.1 and 6 timing semantics.
It follows the [design principles](design-principles.md): the tempo map is a
shared musical coordinate system, with no new source syntax, library export
kind, or general-purpose expression evaluation.

## Musical behavior

```maac
tempo clock {
  points = [(0q, 120bpm, linear), (8q, 180bpm, step)];
}
```

The outgoing `linear` segment changes BPM linearly with score position, not
elapsed seconds. Both increasing and decreasing ramps are supported. Equal
endpoint BPM uses the constant-tempo formula. Points remain strictly ordered,
all BPM values are positive, the last shape is `step`, and endpoint tempos hold
outside the map. Exponential tempo shapes remain invalid.

The standalone [example](../examples/tempo-ramps.maac) combines acceleration,
deceleration, and score-clock dynamics. Build it directly, or retain its plan:

```sh
maac build examples/tempo-ramps.maac -o tempo-ramps.wav
maac compile examples/tempo-ramps.maac -o tempo-ramps.json
maac render tempo-ramps.json -o tempo-ramps-retained.wav
```

The absolute clock is `T(q) = integral(0..q, 60/B(x) dx)`. A nonconstant ramp
uses the specification's logarithmic integral. Negative pickups, nonzero reset
origins, mixed step/ramp maps, and origins inside ramps use the same clock.

## Scheduling and numerical evidence

An event frame is `ceil(rate * (T(q) + offset - T(score.start)))`. Physical
offsets remain independent of musical stretch. Release offset is applied before
project-end truncation; the physical gate must remain positive. A positive gate
that collapses to a single frame boundary produces `E_SUBSAMPLE_NOTE`.

Step-tempo rational timing stays exact. Ramp frame boundaries require enclosing
numerical bounds and increasing precision until the mathematical ceiling is
established. An unresolved boundary produces `E_TIME_PRECISION`; a rounded
floating-point timestamp is insufficient scheduling evidence. Resource limits
must bound the precision work. Timing differences should be evaluated directly
where possible to avoid loss from subtracting large absolute timestamps.

The same scheduling authority governs note on/off, automation knot activation,
tempo segment boundaries, score end, engine render length, and production
delivery length at each selected output rate. Continuous score-clock automation
uses the inverse tempo map at physical sample instants. Its numerical DSP
evaluation is separate from certified discrete transition frames. Automation
holds its score-end value through the tail.

Per-note expression keeps its existing scheduled-gate interpretation and clock
transformations. Tempo ramps do not introduce a different expression clock.

## Saved-plan compatibility

Ramp-enabled compositions use performance-plan version 3, while
existing version 1 and 2 plans remain readable with their current meaning.
Existing step-only compilation retains its format and scheduling behavior.
The source header remains `maac 1`.

The new representation must retain sufficient exact musical coordinates and
physical offsets to independently recompute timing and validate saved frame
counts without source files. It must not label a rational approximation as an
exact absolute time. Older readers must fail explicitly on an unsupported plan
version. A legacy reader may report a missing legacy timing field rather than
`E_VERSION`; it must not accept the artifact or render it as a step map.
Existing public Rust plan types and legacy compile/load/render entry
points retain their current contracts. Additive versioned entry points provide
ramp support; the CLI selects the appropriate plan version automatically.

Version 3 events retain exact score positions and physical offsets, with
independently verified frame counts. Their physical times are derived from those
values and the tempo map, including the specified release clamp. They do not
carry the legacy rational `on_seconds` and `off_seconds` fields. Seconds-clock
automation retains whether its anchor is a score position or absolute seconds;
a bar anchor is resolved to an exact score position. This preserves its meaning
without storing a rounded timestamp.

Ramp delivery metadata identifies the score interval, exact tail, and tempo
map needed to derive duration. Engine and delivery frame counts are independently
certified at their respective rates. A duration approximation must never replace
the legacy exact `duration_seconds` value under the same contract.

Named delivery from a version 3 plan uses manifest schema
`maac.production.delivery-manifest/2`. Its `interval` contains canonical
`score_start_q`, `score_end_q`, `tail_seconds`, the full `tempo` object, and
`engine_frames`/`delivery_frames`. The first four fields are the exact duration
recipe; `duration_seconds` is absent. Delivery from legacy plans retains
manifest schema version 1 and its exact rational duration field.

### Version 3 wire and API boundary

The version 3 top-level fields retain the existing output, tempo, graph,
resources, region, and source-mapping representations. Event objects retain all
legacy fields except `on_seconds` and `off_seconds`; those two fields are
rejected in version 3. Instruments must be embedded when instrument nodes are
used. Legacy-only graphs may omit instrument resources.

Automation uses the existing `at` field with a tagged exact anchor:

```json
{"kind":"score","q":"1/1"}
```

or:

```json
{"kind":"seconds","seconds":"1/2"}
```

Score-clock lanes require a score anchor. Seconds-clock lanes accept either
form; a score anchor means absolute time `T(q)`, followed by the lane's physical
point offsets. Unknown fields, duplicate fields, invalid rationals, and
unsupported versions remain errors. Version 3 may represent step maps as well;
ordinary step-only source compilation continues to emit the legacy version
appropriate to its existing entry point.

Add `PlanV3`, `ResolvedEventV3`, `AutomationV3`, and `VersionedPlan`. Versioned
JSON uses the ordinary top-level `version` discriminator, without an enum
wrapper. Existing `Plan` and its public event/automation types remain unchanged.
Internal borrowed views share validation and DSP initialization across versions.
They must not construct a legacy plan with invented rational timestamps.

Add versioned compile, load, render, export, and delivery functions, including
caller-limited variants where the legacy API provides them. Legacy functions
retain their existing step-only behavior and explicitly reject unsupported
ramps. No existing caller must change a public struct literal merely to keep
using step tempos.

## Implementation boundaries

1. Add an isolated ramp-capable timing API with bounded numerical evaluation,
   preserving the existing rational-only musical helpers.
2. Share structural validation through internal borrowed views and add strict
   version 3 wire types. Recompute all derived scheduling values on import.
3. Share source traversal while preserving exact anchor provenance, then expose
   additive versioned compilation and loading APIs.
4. Share the DSP engine with a precomputed ramp clock and certified transitions;
   add versioned render/export and CLI integration.
5. Integrate named deliveries, document authoring and compatibility, and run the
   installed boundary and regression checks.

The numerical evaluator may use the range-reduced logarithm series documented
in [NIST DLMF 4.6.4](https://dlmf.nist.gov/4.6.E4), with outward rounding and an
explicit remainder bound. The chosen finite precision/work limits and proof of
the enclosing bounds require implementation evidence and independent review.

The additive numerical helper accepts 4096-bit rational inputs and at most
4096 tempo points/logarithm terms. Reduced intermediate rationals are bounded
to 32768 bits, with arithmetic temporaries bounded separately. Certification
refines through 64, 128, 256, 512, and 1024 fractional bits; a logarithm uses
at most 1024 series terms at a refinement. These helper limits do not change
the legacy exact-rational API's limits. The DSP-only approximation has a
separate error bound and cannot determine event frames.

Before evaluating a version 3 plan's clock, conservatively charge timing work as
`(tempo_points + 1) * (8 + 8 * events + 3 * (tempo_points + automation_lanes + automation_points))`
alongside existing structural work against the caller's `max_work` allowance.
These units bound segment
traversals and repeated timing operations, including runtime preparation of
tempo boundaries; each numerical operation also has
the finite refinement limits above. The accounting is a resource bound, not
a wall-clock performance guarantee, and adds no fields to public `PlanLimits`.

Each compilation, imported-plan validation, and renderer-preparation stage also
shares an actual numerical budget, capped at the caller's `max_work` and at
20,000,000 logarithm-series terms. Before a refinement, charge
`(logarithm_terms + 1) * fractional_bits`, including evaluation of `ln(2)`.
Exhausting that shared budget produces `E_RESOURCE_LIMIT` before evaluating the
next series. Reaching the precision ceiling without proving a boundary instead
produces `E_TIME_PRECISION`. Exact rational comparisons and frame ceilings need
no logarithm terms. These budgets are local to an operation, with no global or
thread-local mutable state. All certified transitions are prepared before the
audio sample loop.

## Acceptance evidence

- Analytical and independently bounded timing cases cover acceleration,
  deceleration, flat ramps, mixed maps, negative positions, large origins, and
  boundaries just before/on/after an integer frame. Unresolvable precision and
  resource exhaustion have explicit diagnostics.
- Source compilation and imported-plan validation agree on frames, physical
  offsets, end truncation, positive gates, and duration limits. Tampered plans
  fail independently; unsupported tempo shapes are never reinterpreted.
- Rendered automation follows the inverse ramp, switches at certified knots,
  and freezes during tails. Per-note expression and event ordering retain their
  existing behavior.
- Installed CLI builds and source-free retained renders produce identical WAV
  bytes in the tested environment. Named deliveries verify duration at 44.1,
  48, and 96 kHz. Legacy step-only timing and audio remain unchanged.

Targeted regression evidence precedes implementation. Formatting, Clippy, the
full Rust suite, release/install checks, and independent review are required
before claiming the feature complete. Numerical evidence does not establish
human listening acceptance or cross-platform bitwise identity.
