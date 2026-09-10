# Native core modulation

The initial sample-rate slice's qualification is recorded in the
[validation report](core-modulation-validation.md). Event-rate support extends
that slice with the capture and release-accounting rules below.
This slice implements existing MaaC/1 §13 and the
`core.lfo/1` and `core.constant/1` definitions in §18.9–10. It adds no syntax,
general computation, library format, or executable plug-in boundary.

```maac
node motion {
  type = "core.lfo/1";
  config = { period = 2q; wave = sine; phase = 0; };
}
modulate opening {
  from = &motion:out;
  target = &filter.params.cutoff;
  amount = 100Hz;
}
```

The fragment assumes an existing `filter` node. The
[complete example](../examples/core-modulation.maac) combines both clocks,
an automated constant, notes, a hit, rate/warp clips, filtering, and reverb:

```sh
maac build examples/core-modulation.maac --project-root . -o core-modulation.wav
```

[Event-rate modulation](../examples/event-rate-modulation.maac) demonstrates
note-on and note-off capture with core sine and instrument public controls.

## Supported controls and targets

An LFO has no inputs or parameters. Its required `period` is positive score q
or physical seconds. `wave` is sine (the default), triangle, saw, or square;
`phase` is an exact dimensionless cycle coordinate, default zero. A constant
has no config or inputs; its `value` parameter is finite and dimensionless,
defaults to zero, and permits global automation and incoming modulation.
Both expose only the dimensionless scalar control port `out`.

A top-level `modulate` requires `from`, `target`, and `amount`. The source must
be a control output; the target must be a continuous numeric sample-rate or
event-rate parameter. The amount uses the target's native unit, including valid equivalent
units already accepted by automation. Existing core, kit, native effect, and
instrument public controls are eligible when their descriptors satisfy these
requirements. `core.constant/1.value` is also eligible.

Event-rate targets include `core.sine/1.attack` and `release`, and instrument
public controls whose descriptors declare note-on or note-off capture. Reset-rate
controls remain unsupported with `E_CAPABILITY`. Config fields, discrete parameters,
and audio-clip transport metadata cannot be modulation targets.
Instrument-internal modulation also supports voice ADSR and phase event parameters under
its [separate capture contract](internal-event-modulation.md). Internal event
contributions are checked only at capture; the top-level contract below continues
to check combined parameters each frame.

The combined parameter is evaluated and range-checked each frame before note-offs
and note-ons. Attack and other note-on parameters are captured at note-on; release
and other note-off parameters are captured at note-off. Later modulation does not
change an already captured value. Coincident events use the same frame's control
values, with note-offs and finished-voice retirement preceding note-ons. Invalid
combined values still fail on frames without events and on disconnected nodes.

Control ports are not audio or event ports. They cannot be project outputs,
audio connections, selected render outputs, or production delivery targets.
No implicit signal conversion is introduced. Unknown fields, wrong units,
invalid references, malformed periods, and unsupported processor fields fail
even on disconnected declarations.

## Evaluation and clocks

For each target, evaluate its base or replacement automation value, then add
`amount * control_output` in modulation-ID unsigned UTF-8 order. Apply the
target descriptor's range policy to the combined value: declared error ranges
fail and declared clamp ranges clamp. Do not clamp each contribution or add
implicit smoothing. Every intermediate product, sum, control output, and audio
result must be finite or fail with `E_NONFINITE`.
Authored base parameters and automation endpoints retain their existing
independent validation. Validating the combined value does not admit otherwise
invalid declarations.

Control dependencies and audio dependencies participate in the same acyclic
graph validation. Constants may form feed-forward modulation chains. Self
modulation and indirect same-sample cycles fail with `E_ALGEBRAIC_LOOP`.
No delay processor or feedback support is added in this slice. Node IDs resolve
topological ties; modulation IDs resolve addition order, independently of
declaration order.

At output frame `n`, rate `R`, and reset score origin `q0`, an LFO's cycle
coordinate is `(n/R)/P + phase` for a seconds period and
`(T^-1(T(q0)+n/R)-q0)/P + phase` for a score period. With fractional part
`f` in `[0,1)`, the outputs are:

| Wave | Output |
| --- | --- |
| sine | `sin(2*pi*f)` |
| saw | `2*f-1` |
| square | `1` for `f < 1/2`, otherwise `-1` |
| triangle | `1-4*abs(f-1/2)` |

The score LFO follows the shared step/ramp tempo map and holds its exact
score-end coordinate during the physical tail. The seconds LFO continues
through that tail. Global automation, including automation of constant values,
retains its existing score-end freeze. Reset starts all LFOs at their configured
phase at the project origin; rendering does not infer prehistory.

Source compilation reduces phase exactly modulo one; retained plans require
canonical phase in `[0,1)`. Preparation partitions the clock into exact
half-cycle cells, merging actual tempo boundaries for score-clock LFOs.
Half-cycle parity determines square polarity and saw wrap ownership. All cuts
use the shared certified frame ceiling; a sample exactly on a cut belongs to
the following cell. Coincident-frame empty spans remain validated and charged
before omission. Score-tail holding uses a separately exact-reduced end phase.

Each nonempty prepared span uses a local frame and half-cycle coordinate, with
combined coefficients formed before floating conversion. Actual tempo intervals
provide canonical clock prefixes; half-cycle cuts do not accumulate logarithmic
subintervals. First/last sample containment is checked in both forward and
independently formed backward coordinates. Exact zero at a left boundary is
allowed. Nonzero elapsed times, progress, and right residuals must retain finite
values with the correct sign; subtracting the backward residual must leave a
representable position strictly below the right boundary. Multiple samples must
retain ordered representable progress.

Unrepresentable bounded recipes fail with `E_TIME_PRECISION`; excessive cells
or exhausted numerical work fail with `E_RESOURCE_LIMIT`. Runtime evaluates
directly from the frame offset with no phase accumulation, arbitrary-precision
arithmetic, or certification. Ordinary IEEE evaluation of interior samples and
libm rounding are part of the reference engine; cross-platform bit identity is
not claimed. There is no alias suppression or hidden smoothing.

## Retained plans and resources

Sources containing either control processor or top-level modulation emit private
performance-plan version 7 through `PlanArtifact`. It retains all V6 musical,
asset, graph, production, and timing data and adds typed control processors and
explicit modulation records. Public closed Rust enums and existing plan structs
retain their contracts. Sources without the new feature retain their previous
version and bytes. Older executables reject V7 explicitly.

V7 loaders, encoders, compilation, rendering, and delivery independently enforce
the same structure, typed ports, target admission, causality, and resource rules.
Plans retain control configuration, base parameters, automation, exact amounts,
and modulation identities. Production identity retains these authored recipes;
no generated controller events or hidden audio nodes stand in for modulation.

Control nodes count against existing node limits. Audio connections and
top-level modulation edges share the existing connection allowance. IDs,
strings, exact quantities, and objects count toward existing aggregate limits;
silent or disconnected controls are not exempt. Execution charges cover control
evaluation and every modulation contribution for the complete render and tail.

Let `p` be canonical phase, `N` total render frames, `R` engine rate, and `P`
the period. For score clocks, define `c_end=p+(score.end-score.start)/P`.
For seconds clocks with `N>0`, define `c_end=p+(N-1)/(R*P)`. The candidate cell
count is `floor(2*c_end)-floor(2*p)+1`; zero-frame seconds renders conservatively
count one cell. Score clocks additionally count all declared tempo points as
a conservative bound on merged cuts. Call this combined bound `M`.

Before allocating any spans, checked exact arithmetic proves that existing
structural work plus `sum(128*M_i)` across all LFOs fits `max_work`. An individual
LFO fitting the allowance is not sufficient. Certified clock operations
also consume the shared timing context's numerical budget. Additional execution
charges per rendered frame are 8 for a constant, `128+ceil(log2(max(1,M_i)))`
for an LFO, and 8 for every modulation edge. Existing graph overhead and other
processor charges remain in force. These are conservative resource units, not
performance measurements; no new public limit field is introduced.

When modulation targets an instrument's public amplitude-envelope release
control, voice and expression work use the descriptor's maximum release duration,
clipped to the declared render endpoint. This includes zero-amount edges and avoids
undercharging tails whose duration depends on a control signal. Unmodulated
release controls retain their existing base/automation bounds. This conservative
bound can reject long, dense arrangements even when the actual modulation is
small. It does not extend the render tail or change voice-capacity rules.

## Acceptance

1. Analytical control/audio tests prove all waves, phase/reset, both clocks,
   step and ramp tempo, negative origins, score-end holding, and seconds-tail
   continuation. Automation plus sorted modulation follows range policy;
   feed-forward controls and invalid cycles behave deterministically.
2. Source and hostile retained-plan tests prove unit/port/rate admission,
   malformed fields, nonfinite values, resource limits, and explicit precision
   failures. Legacy JSON, audio, diagnostics, and voice-work behavior remain
   compatible.
3. Installed source/retained workflows match with their inputs removed, including
   mixed notes, kits, rate/warp clips, effects, sidechains, and named deliveries.
   Full tests, formatting, Clippy, release/install, all three metering audits,
   specification/SRC tooling, and the orchestrator's final evidence and
   acceptance checks pass on the frozen revision. Human listening remains a
   separately reported check.
