# Native warp-rate audio clips

Implemented and [validated](warp-rate-validation.md). This contract implements MaaC/1 §14.3 using the
existing `audio` object syntax and the shared tempo map. It adds no general
computation or new authoring syntax. Asset conversion remains external tooling;
preserve-pitch stretching remains an explicit unsupported extension.

Try the [runnable example](../examples/warp-rate.maac):

```sh
maac build examples/warp-rate.maac --project-root . -o warp-rate.wav
```

It combines two warp clips, a rate clip, two hits, and one note through reverb
and master gain. Its tail clip crosses an explicit tempo change after score end.
The example reuses the original core-kit PCM asset.

## Source and musical meaning

An audio object with `mode = warp_rate` requires `asset`, musical `at`, `source`,
and `warp`. Placement accepts global q or the existing `bar(b,u)` notation.
`source = [a frame,b frame]` denotes the nonempty half-open asset interval `[a,b)`.
Assets, typed output routing, optional track grouping, gain, and physical fades
follow the [rate clip contract](audio-clips.md).

The warp list contains 2–4,096 pairs `(local_q, absolute_source_frame)`.
Local q and integer source frames both strictly increase. Its first pair is
`(0q,a frame)` and its final pair is `(Lq,b frame)`, with `L > 0`.
Physical-time warp anchors and fractional frame anchors are invalid.
Authored `speed`, `reverse`, and `processor` fields are forbidden, including
values that would otherwise be defaults. `warp_preserve` fails with
`E_CAPABILITY`; it never falls back to rate warping.

For reset origin `O=T(score.start)`, engine rate `R`, and output frame `n`,
the local score position is `q=T^-1(O+n/R)-at`. Linear interpolation of the
authored warp anchors gives the absolute asset coordinate `F(q)`. The existing
core interpolator reads the slice-relative coordinate `F(q)-a`, including its
zero-valued neighbor outside the slice. Asset rate does not scale this mapping:
the authored source frames and musical anchors determine instantaneous speed
and therefore pitch.

Natural transport start and finish are `S=T(at)` and `E=T(at+L)`.
The clip must start inside the physical score interval. It may continue into
the declared tail, following the actual tempo map including future tempo
changes after score end. Render end truncates playback. Physical fades use
natural duration `E-S`, even when render end cuts the clip short.

Active frames are the half-open interval
`[ceil(R*(S-O)), ceil(R*(min(E,render_end)-O)))`. Empty intervals are valid.
Fractional first-sample phase is preserved, and exact transport end is excluded.
Warp and tempo boundaries use the same certified clock as other musical data.

## Preparation and finite execution

Preparation merges warp anchors and tempo boundaries in score order. The number
of resulting segments is additive in the two input counts. Each active segment
uses a local time and source origin, bounded combined coefficients, and direct
evaluation from the output frame offset. Runtime performs no arbitrary-precision
arithmetic, numerical certification, or phase accumulation. Existing automation
behavior at score end is unchanged.
Cut times reuse a fixed prefix for each actual tempo interval. Warp-only cuts
integrate from that same tempo origin, keeping equivalent logarithmic clock
expressions consistent without repeatedly scanning the whole tempo map.

Exact score boundaries are deduplicated before frame conversion. Boundaries
that round to the same frame remain distinct and may produce empty segments.
A sample exactly on a boundary belongs to the following segment. Preparation
maps each exact `[x,y)` to
`[ceil(R*(T(x)-O)), ceil(R*(T(y)-O)))`, clipped at render end.
It certifies each retained boundary before omitting empty sampled intervals.
Preparation
certifies nonnegative first-sample elapsed time and strictly positive final-sample
distance from each natural segment end. It checks both forward source phase and
an independently formed backward residual. Positive prepared progress must not
disappear when added to the local source origin, and reconstructed endpoints
must remain inside the half-open source domain.
Subtracting the backward residual from the right source origin must also leave
a representable position strictly below that origin.
In absolute source coordinates this requires finite, ordered
`F(x) <= first <= last < F(y)`. Failure to retain a mathematically positive
prepared residual or advance is `E_TIME_PRECISION`.
An exactly grid-aligned segment start is the only zero first-progress exception.
A single-sample segment needs no total advance but retains its fractional phase.

For a linear-tempo segment of score length `d`, starting BPM `b`, and slope `s`,
the inverse fraction is `expm1(s*t/60)/(s*d/b)`, with the analytic constant-tempo
limit when `s=0`. Implementation forms combined local-time coefficients before
floating-point conversion, uses stable near-flat and large-change expressions,
and stores a direct total advance. This avoids dividing separately approximated
tiny logarithms or subtracting rounded source endpoints. Per-frame evaluation
uses explicit endpoint limits and does not accumulate increments.

One timing context and numerical budget spans scheduling, validation, and
preparation. Exact zeros and representable nonzero values are admitted. Lost
nonzero prepared values, unresolved certified boundaries, and inadequate phase
precision fail with `E_TIME_PRECISION`; exhausted work or size limits fail with
`E_RESOURCE_LIMIT`. Nonfinite gain or DSP results fail with `E_NONFINITE`.
Ordinary floating-point rounding after successful preparation remains part of
the reference engine. No silent approximation mode is introduced.

Automation points, per-note expression points, and warp anchors together must
not exceed the existing `max_automation_points` limit (65,536 by default).
Clips and their metadata retain existing graph, object, string, rational,
asset-byte, and plan-byte limits. Zero gain, disconnection, and empty active
intervals do not bypass metadata validation. Decoded asset buffers are shared
with rate clips and kit voices. Execution and preparation are conservatively
charged to the existing resource budgets.
For `C` channels, the execution charge is
`total_frames*(4+C) + active_frames*(96+8*C+ceil(log2(M)))`, where
`M=warp_anchor_count+tempo_point_count` bounds the merged segment count.
All accounting uses checked arithmetic; these units are not a performance claim.
Preparation also charges `32*M` structural work units for each warp clip,
including clips without audible output.

## Saved plans and compatibility

Warp-rate clips require private performance-plan version 6 or later through
`PlanArtifact`; adding core modulation selects V7. V6 includes a distinct `warp_rate` graph processor
retaining musical placement, exact warp anchors, source bounds, physical fades,
source identity, optional grouping, and certified active frame bounds. Its
parameter map is empty. Rate clips in V6 retain their V5 record unchanged.

Sources without warp-rate clips retain their previous plan version and bytes.
Existing public closed plan, processor, and CLI result types remain compatible.
Artifact clip counts include both rate and warp-rate clips. Notes and hits are
counted separately; clips are never synthesized into events.

Every public load, encode, render, and delivery boundary validates the complete
payload, including anchor ordering, assets, ports, frame bounds, and budgets.
Retained plans embed their PCM dependencies and work without source files.
Production identity retains mode-specific timing recipes; forbidden rate fields
are not synthesized into warp recipes. Named stems and masters retain the full
graph, effects, and tails, using the existing manifest/2 duration contract.

## Acceptance and verification

Analytical coordinate and waveform checks cover constant, stepped, increasing,
and decreasing tempo; piecewise anchors; fractional starts; exact boundaries;
negative reset origins; very short intervals; tail tempo changes; fades;
mono/stereo slices; and reset. Hostile source and retained inputs cover missing,
unordered, duplicate, oversized, fractional, mismatched, and forged anchors,
wrong ports, forbidden fields, and depleted structural/numerical budgets.

Installed source and retained workflows must agree at 44.1, 48, and 96 kHz,
including mixed notes, hits, rate clips, warp clips, effects, selected ports,
and named deliveries with original inputs removed. Older plan JSON and rendered
PCM are compared with the committed rate-mode executable. Final verification
includes formatting, Clippy, the full normal test suite, all three explicit
metering audits, release/install, Python/specification/SRC checks, and independent
Sol Max review. Automated checks do not assert human listening acceptance.
