# Instrument-internal event modulation

Instrument voice graphs can modulate the event-rate parameters of `synth.adsr/1`:
`attack`, `decay`, and `sustain` are captured at note-on; `release` is captured at
note-off. This extends the existing internal `modulate { from; to; depth; }`
syntax. Voice sine, saw, square, triangle, wavetable, and LFO `phase` parameters
also capture at note-on. Shared LFO `phase` captures at render reset. Shared
graphs still cannot contain ADSRs. Top-level modulation can also supply an
instrument's public reset controls under its [own contract](core-modulation.md).

```maac
node touch { type = "synth.pressure/1"; }
node amp { type = "synth.adsr/1"; params = { attack = 5ms; }; }
modulate onset {
  from = &touch:out;
  to = &amp.params.attack;
  depth = 25ms;
}
```

The fragment belongs inside a voice graph. The
[complete example](../examples/internal-event-modulation.maac) combines per-note
pressure with an internal LFO. No new plan schema or language syntax is required;
the existing instrument program retains the modulation recipe for source-free
replay.

The [phase example](../examples/internal-phase-modulation.maac) captures initial
per-note expression into oscillator phase. Phase contributions are dimensionless
cycles. The final value must lie in the inclusive range `0..=1`; one cycle is
equivalent to zero. Values outside that range fail with `E_RANGE` rather than
wrapping. After capture, phase advances normally with oscillator frequency;
later source changes do not reset it.
Captured phase exactly equal to one is canonicalized to zero before advancement.
Legacy wavetable initialization without a phase modulation edge remains unchanged;
its raw authored phase of one can accumulate slightly different floating-point
rounding than the newly canonicalized capture path.

## Capture and evaluation

For each captured parameter, start with its authored base value or current public
control value, then add `depth * source` contributions in modulation-ID unsigned
UTF-8 order. Validate finite products and intermediate sums, then the final
descriptor range. Signed contributions may cancel before the final range check;
there is no clipping or smoothing. Authored bases, controls, and depths retain
their independent validation.

Internal event-rate contributions are evaluated and range-checked only at their
capture event. Later signal changes do not change the captured values or cause
an unused event-rate sum to fail between events. Sample-rate modulation and its
sources continue to run and validate each audio frame, including at zero gain or
velocity. Top-level modulation retains its separate per-frame evaluation and
range-checking contract.

At note-on, a read-only preview visits the required source dependencies in the
graph's stable topological order. Each ADSR or phase target captures and initializes
its onset state before its initial output supplies another capture. This also
applies to chains mixing phase and ADSR targets. Initial pitch, timbre, and pressure
expression belong to the new note and are available during this preview.

At reset, a shared graph captures LFO phase before any notes start, using silent
shared input and resolved control defaults, presets, and instance values.
Top-level reset-control modulation, when present, supplies its captured frame-zero
values before this internal capture. Other controls retain their authored values:
frame-zero automation and top-level modulation of non-reset controls affect the
subsequent normal audio pass. This preserves the existing instrument initialization
boundary for instruments without top-level reset edges. The
[reset example](../examples/internal-reset-modulation.maac) demonstrates this
shared-graph mapping.

Reset capture follows the same stable graph order: each target LFO initializes
before its output supplies downstream captures. Source LFO phases and filter
history start at their reset state, and previews do not advance them. Captured
phase is validated in `0..=1` and one is canonicalized to zero. Later control or
input changes do not recapture it. Each instrument instance captures independently;
renderer reset restores its captured pristine state. A public runtime reset with
new controls captures anew; a failed capture preserves the previous runtime state.

At note-off, all release captures use the current frame's **pre-release** state.
This follows MaaC/1 section 6's parameter-before-event ordering. The preview uses
current public controls and the note's gate-end pitch, timbre, and pressure. All
release values are collected and validated before any ADSR is released. Thus an
upstream ADSR with zero release still supplies its pre-release value to a
downstream release capture on that frame.

Previews evaluate only the dependencies needed by the captures. They do not use
the previous frame's cached output, advance processor state, update a pluck
coefficient cache, or copy a pluck ring. Their outputs are discarded. The normal
audio pass then evaluates the post-event graph and advances each live processor
once. In particular, a filter downstream of a newly released envelope must use
the post-release input for its audible sample and history, even when its
pre-release preview supplied a release capture.

Existing audio and modulation edges all participate in DAG validation. Self,
indirect, and mixed audio/modulation cycles remain errors; capture introduces no
feedback exception. Note-offs and finished-voice retirement still precede
same-frame note-ons. Each voice has independent captured values and DSP state.

## Resource accounting

Let `G` be the existing conservative voice graph cost: 16 per pluck node, 1 per
other node, plus the audio-connection and modulation-edge counts. Ordinary
active-frame work continues to use `G`, including event-rate edges.

For each note, a graph containing onset modulation is charged one extra
`G + node_count + modulation_count` for the onset preview and capture sweep.
A graph containing release modulation is charged the same amount for note-off.
These are full-graph upper bounds even when only a dependency subset is visited.
Each preview additionally charges each present pitch, timbre, and pressure
expression lookup using the existing `17 + ceil(log2(point_count))` rule. Gain
expression is outside the graph and is not evaluated by the preview. Charges
apply to zero-depth edges and silent notes, including immediate release.
The note-off charge is retained conservatively even when the event falls at or
beyond the render endpoint.

Any internal modulation of the designated amplitude envelope's release selects
the descriptor's maximum release duration, currently 1800 seconds, for the voice
work bound. This applies without a public release control and combines with the
existing top-level release-modulation policy. Each voice's bound is clipped at
the render endpoint. Other ADSRs' releases and onset parameters do not extend
voice allocation. This conservative policy can reject a dense arrangement even
when its actual modulation is small; it does not extend the authored render tail.
Phase targets use the same single onset-preview charge, including when combined
with ADSR onset targets; they do not extend release duration or voice storage.

For a shared graph with reset modulation, each instrument instance is charged
one `G_shared + node_count + modulation_count` reset preview. `G_shared` uses
the same graph-cost rule. This includes zero-depth edges, disconnected instances,
and instances with no notes. It adds no expression lookup, per-voice multiplier,
or release bound. Ordinary shared-frame work and existing reset snapshot storage
remain unchanged.

Preview scratch space is bounded by the existing 64-node graph limit and the
fixed native parameter counts. Previews allocate no extra voice history or pluck
rings. Existing initialization and storage charges remain unchanged. Graphs
without internal event-rate edges retain their previous execution path and work
accounting.

## Acceptance

- Source and retained programs agree on ADSR, voice phase, and shared-LFO reset
  admission, units, mono sources, stage restrictions, and cycles.
- Phase capture uses initial expression and public controls, supports both cycle
  endpoints, holds across later expression changes, and initializes downstream
  capture sources in graph order.
- Shared reset capture uses authored controls and silent input, preserves source
  state through previews, restores deterministic replay, and fails atomically.
- Analytical audio proves onset capture, the all-envelope pre-release snapshot,
  capture holding, expression endpoints, independent voices, and same-frame
  capacity reuse.
- Previewing oscillators, noise, filters, wavetables, and plucks does not advance
  their audible state twice. Valid captures remain valid when unused sums would
  be out of range between events.
- Exact resource allowances pass and one-below allowances fail before callbacks
  through compilation, retained validation, rendering, and output capture.
- Source-free CLI replay matches authored audio; failed captures preserve output
  destinations. Legacy and top-level modulation regressions remain green.
