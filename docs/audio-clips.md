# Native rate-mode audio clips

Implemented and validated. See the [validation report](audio-clips-validation.md).

This slice implements MaaC/1 §14.2 using existing source syntax. It adds arranged
audio to the same finite performance and explicit processing graph as notes and
hits. Sample preparation remains external tooling. Warp modes, WAV import,
audio placement inside patterns, automatic looping, implicit crossfades and new
modulation capabilities are outside this slice.

Try the [runnable example](../examples/audio-clips.maac):

```sh
maac build examples/audio-clips.maac --project-root . -o audio-clips.wav
```

It combines three clips, two hits and one note through an explicit mixer and
master gain. The clips demonstrate normal, reversed and half-speed sliced
playback with q, physical-time and bar placement, using the existing kit samples.

## Source contract

A top-level `audio` object requires `asset`, `at`, `source`, and `mode = rate`.
The asset must be verified mono/stereo core PCM as defined in the
[kit contract](core-kit.md#assets). `at` is an absolute q, `bar(b,u)`, or
seconds/ms position. Bar positions use the existing global meter resolution.
`source = [a frame, b frame]` is a nonempty half-open slice of integer source
frames, with `0 <= a < b <= asset.frames`. Zero-frame assets remain valid data
but cannot supply a nonempty clip.

`speed` is a positive dimensionless rational, default 1; `reverse` defaults to
false; `gain` is nonnegative, default 1. `fade_in` and `fade_out` are nonnegative
physical durations, default zero. `fade_shape` is `linear` or `equal_power`,
default linear. Rate changes affect pitch and duration together. Reverse changes
frame order inside the slice, preserving interleaved channel order.

An optional `track` references an existing track for grouping. That track needs
no event target unless an event placement uses it. Grouping creates no audio
connections or controls. The clip exposes only `out`, whose width equals the
asset's channel count. Its output may be the project master or connect to other
processors, including named delivery ports. Clips have no event input, implicit
parameter ports, or automation lanes for their source fields.

Unknown fields fail. The separate [warp-rate contract](warp-rate.md) implements
musical warping; preserve-pitch warping remains unsupported.
`warp` and `processor` fields are invalid for rate mode. Asset and output
references remain declaration-order independent. Source IDs are globally unique.

## Continuous timing and playback

Let the absolute physical start be `S`, determined by `T(at)` for a score anchor
or the authored physical position for a seconds anchor. Let `O=T(score.start)`,
`R` be the engine rate, and `D=(b-a)/(asset.rate*speed)`. The clip begins inside
the physical score interval, `O <= S < T(score.end)`.

At engine frame `n`, elapsed clip time is `t = O + n/R - S`. The signal is zero
unless `0 <= t < D`. In that interval, the source coordinate is
`u=t*asset.rate*speed`, using the specified slice/reverse mapping and linear
interpolation with zero outside the slice. The first audible sample retains
its fractional source position; playback does not reset phase at `ceil(S)`.

Active sample bounds are certified ceilings relative to the reset origin:
`start_frame=ceil(R*(S-O))` and
`end_frame=ceil(R*(min(S+D, render_end)-O))`. Their interval is half-open.
Positive-duration clips containing no output sample are valid and emit no
samples; their interval is never extended. A physically pre-score-end clip may
first contribute in the declared tail, since the continuous transport has
already begun. Clips stop at source end or render end, with no inferred fade.

Linear fades use `clamp(t/fade_in,0,1)` and
`clamp((D-t)/fade_out,0,1)`; a zero fade duration contributes 1.
Equal-power fades apply `sin(pi*x/2)` to each fade factor. Both fade factors
multiply even when they overlap. Overlapping clips and explicit connections
express crossfades. Gain and fade output must remain finite; no normalization
or clipping is inferred.

Boundary certification uses the existing exact/certified tempo clock. One
timing context and numerical budget spans each compilation or renderer
preparation, including clip bounds and prepared DSP values. Runtime evaluates
from a local physical origin and frame offset, with no accumulated phase drift
or per-frame numerical certification. Derived phase and fade ratios must be
formed without overflowing or underflowing intermediate time/rate conversions;
unresolvable numerical boundaries fail explicitly.
Prepared phase and fade segments use a bounded total advance evaluated from
the absolute frame offset, so a tiny per-frame increment cannot underflow
before multiplication by the elapsed frame count.
Exact zero and finite nonzero values, including subnormals, are valid prepared
values. If a mathematically nonzero prepared phase or fade value rounds to zero
or the wrong sign, preparation fails with `E_TIME_PRECISION`; later gain must
not amplify a silently lost value. Engine-unrepresentable gain and nonfinite
DSP arithmetic fail with `E_NONFINITE`. Ordinary floating-point rounding after
successful preparation remains part of the reference engine.

## Saved plans and compatibility

Rate clips require performance-plan version 5 or later through the opaque
`PlanArtifact` API. The compiler selects V5 unless another feature requires a
later version: warp clips select V6 and core modulation selects V7. Earlier
features retain their supported behavior and records. Existing public plan,
processor, event and CLI result types remain compatible.

The private V5 representation reuses embedded raw audio assets and the current
core/kit graph node representation, adding a genuine audio-transport processor
record. A clip remains its own graph source and is never a synthetic note or
hit. The record retains the exact start recipe, source interval, speed,
reverse, gain, fades, source identity, optional track grouping metadata and
certified active frame bounds. Clip node `params` is empty. V4 rejects audio
transport records; earlier readers reject V5 rather than reinterpret it.

Public artifact loading, encoding, rendering and export independently validate
the complete payload. Retained plans render and deliver without original
source or assets. Source grouping references are resolved at compilation;
retained grouping/source IDs are bounded metadata, not file retrieval or
implicit routing instructions. Existing note and hit counts remain accurate;
artifact inspection and CLI reporting expose clip count separately, omitting
the new result field when zero.

## Resource and delivery requirements

Clips count toward the existing graph node, object, identifier, metadata-string,
rational and plan-byte bounds. Raw sample limits and verification are unchanged.
Each decoded asset is shared by all kit voices and clips in one engine.
Silence, zero gain, unconnected sources and reverse playback do not exempt
resources from limits. For each clip with `C` channels, conservative execution
cost is `total_frames*(4+C) + active_frames*(32+8*C)`, using checked arithmetic.
Active frames are clipped at render end. These are accounting units, not a
runtime performance promise. Clip metadata and preparation traversals also
consume the existing structural and numerical budgets.

Production identity includes clip configuration, exact timing recipes and
sample resources. V5 uses the existing manifest/2 exact-duration recipe.
Named masters and stems retain the complete graph, effects and tails. Source
and retained deliveries must match at 44.1, 48 and 96 kHz. Existing transactional
WAV publication and failed-check audio retention behavior remain unchanged.

## Acceptance

Analytical tests cover sliced mono/stereo data, reverse channel order, unequal
rates and speeds, fractional starts, short intervals, both fade shapes,
overlapping fades/clips, physical and score anchors, ramps, negative reset
origins, score-end tails and reset. Hostile imports cover asset/source bounds,
identities, typed ports, unsupported modes, forged frames and exhausted budgets.

Installed source and retained workflows must match with source/assets removed,
including mixed clips, notes, hits, effects, selected ports and named deliveries.
Older plans and executable outputs are compared for compatibility. Final gates
include formatting, Clippy, the full suite, all three explicit metering audits,
release/install, specification smoke checks and independent Sol Max review.
Automated waveform checks do not assert human listening acceptance.
