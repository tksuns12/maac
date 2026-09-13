# L5 quantitative conformance decision

**Status: ACCEPTED 2026-09-13; the bounded contract and evidence slice is
addressed.** This record captures the accepted bounded Core Audio
reference-audio metric and bound. It does not change the MaaC source contract,
L1–L4 identities, diagnostics, timing rules, or any processor algorithm.

## Authority and boundary

The decision follows the Core Audio profile in [MaaC-1 §1.2](../MaaC-1-Specification.md#12-conformance-profiles),
exact sample scheduling in [§6](../MaaC-1-Specification.md#6-time-offsets-and-scheduling),
reference processor semantics in [§18](../MaaC-1-Specification.md#18-reference-processor-library),
and the separate audio-equivalence choices in [§22](../MaaC-1-Specification.md#22-dependency-locks-execution-and-reproducibility).
The addressed L1–L4 slices remain the source of identity, timing, field,
canonical-byte, and interchange rules. The accepted contract is recorded in
[the quantitative conformance contract](quantitative-conformance.md) and is
normative for this bounded evidence suite. It does not select a universal
tolerance or cross-platform bit-identity policy. The bounded numerical suite is
evidence within the Core Audio profile, not a fifth full conformance profile
under §1.2.

## Accepted bounded profile

Use one versioned **48 kHz reference-audio suite v1** for L5's initial bounded
evidence. Each fixture identifies its valid source or retained-plan input,
dependencies, reset state, output port, channel order, and exact render window.
The measured window is an explicit reset-relative half-open frame interval
`[frame_start, frame_end)`. No implicit alignment, latency correction, gain
fit, transient removal, padding, resampling, or crop adjustment is permitted.

For every channel and frame in that window, compare the runtime's finite
pre-encoding binary64 sample with an independently specified finite real
reference sample. A reference is represented by an exact real value when
available, or by a certified rational interval that encloses that value; it is
not required to be rounded to binary64. The metric is the maximum
absolute sample error. Decode each finite binary64 observation exactly as a
dyadic rational `actual`, and use exact rational arithmetic. For a certified
reference interval `[reference_lo, reference_hi]`, compute

`E_upper = max_samples(max(abs(actual - reference_lo), abs(actual - reference_hi)))`.

For an exact point reference, the interval endpoints are equal and this reduces
to the maximum absolute sample error. A run receives conservative numerical
certification only when `E_upper <= epsilon`. When `E_upper > epsilon`, the run
is not certified; for a non-point interval, that result alone does not prove
that the unknown true error exceeds epsilon. Record every interval's width.
Rounding an oracle to binary64 is a separate approximation and must not replace
the independently specified real-valued reference processing.

Shape, channel order, frame bounds, reset origin, event/timing coordinates, and
finite-value checks are performed before the numerical metric. A mismatch in
any of those prerequisites is a structural or timing failure, not a numerical
error to be hidden by the bound. This decision defines no additional source
rounding rule; reference values use the existing mathematical and processor
contracts, with their derivation and implementation provenance recorded.

The accepted bound is `epsilon = 1/10^14`, inclusive, under policy identifier
`maac.core-audio.reference-f64/1`. The completed local calibration observed
`E_upper` approximately `1.5935876903e-16`, with reference interval width at
most `1e-80`; debug and release builds produced the same local sample bits.
Its [calibration summary](../target/l5-quantitative-validation/calibration/summary.json),
SHA-256 `40f2c14088f6be43e134a5e5d25bda2067df3df2fdb674ea83134de1c72cdb27`,
is local rationale and records the distribution, derivation, and
reproducibility limits; it is not proof of broader conformance. The roughly
63-fold separation between that observation and the accepted bound is an
engineering margin from this calibration, not a proof. No relative floor or
per-processor exception applies, and no new source rounding policy is
introduced.

## Independent reference and evidence roles

Reference values must come from an independently derived path or literal
oracle whose derivation, source inputs, algorithm references, author, and
toolchain/environment provenance are recorded. The reference path must not
silently reuse the implementation under test. Each expected vector records
literal samples or a pinned reference artifact, exact window metadata, channel
shape, finite status, and the provenance needed for independent review.

Static corpus checks can validate fixture inventory, metadata, canonical bytes,
and expected reference vectors. Runtime checks must separately record the
actual executable, render boundary, observed binary64 samples, metric result,
and retained-plan or repeat-render result when exercised. A static pass is not
a runtime fidelity claim. L4's raw PCM encoding and PCM/file hash evidence
remain separate output evidence; decoding or comparing encoded binary32 is not
the accepted L5 metric.

L1 authored revision/edit outcomes, L2 exact timing and reset coordinates, L3
field/cardinality/capability diagnostics, and L4 canonical identities remain
exact obligations. A numerical mismatch cannot be relabeled as `E_CONFLICT`,
`E_RANGE`, `E_CAPABILITY`, or another established diagnostic, and the L5
profile does not alter the L4 render key or evidence fields.

## Alternatives kept separate

A mixed absolute/relative metric could protect low-level signals while scaling
large signals, but it introduces a relative floor and scale policy that needs
its own authority decision. It remains an alternative, not a hidden part of
this decision. Comparing decoded binary32 output would measure export
representation and could hide binary64 DSP differences; it also overlaps the
separate L4 encoding/evidence contract. Existing §22 byte-identical PCM claims
under a locked environment remain available as a distinct claim.

## Accepted bounded suite

The accepted suite has six fixtures at 48 kHz. Each fixture measures the
reset-relative half-open window `[0, 8)`, and the six windows together contain
64 channel samples. The exact inventory is:

1. a 6000 Hz Sine with a two-frame attack;
2. equal-power Pan at center;
3. equal-power Pan at `pan = 1/2`;
4. a 6000 Hz OnePole impulse response;
5. a one-frame Delay with half-gain feedback across four score frames and four
   tail frames; and
6. seeded Noise with seed 7.

Mono and stereo are used where the processor contract permits them. Every case
also checks retained-artifact replay. Each expected sample carries an
independently derived real value or certified rational interval and provenance.
The suite is bounded evidence for Core Audio numerical behavior; it does not
claim signed-zero, subnormal, pan-endpoint, connection-order, full-profile, or
cross-platform coverage.

The [machine-readable reference corpus](../conformance/l5/README.md) records
the current source, asset, interval, provenance, and window inventory. Its
static verifier is evidence of corpus consistency; it does not compile or
render the production implementation.

## Completion checklist and status

The accepted decision and bounded L5 contract/evidence slice are addressed
2026-09-13. The final public compile/retained-plan/replay/f64-render gate
passed 7 Rust tests across the six fixtures, 48 frame rows, and 64 channel
samples. The maximum observed `E_upper` was approximately
`1.5935876903e-16`, and all retained replays were bit-identical. The static
reference verifier passed its 13-test regression suite. The evidence index
passed 22 focused tests while binding seven native suite manifests and 179
fixture pins.

The targeted L2/L3 cross-slice gate passed 13 tests (3 L2 and 10 L3). Legacy
L1 checker/tests passed 18 tests, L4 checker/tests passed 23 tests, and the
disposable syntax/semantics smoke run matched its three tracked JSON outputs
byte-for-byte. The root-owned [integrated L5 record](../target/l5-quantitative-validation/integrated-final.json)
holds the final summary and reviewed-file bindings. Luna owns the normative
specification and index/CI documentation; Sol owns the reference corpus,
static verifier, and runtime harness; the L5 reference-test executor owns the
verifier tests. Sol ran the runtime/reference and L2/L3 gates; Luna ran the
index, legacy Python, and smoke gates; the reference-test executor ran its
focused regression suite. Independent read-only semantic and code/harness
reviews passed, and root performed integration acceptance.

Production Rust, Cargo, grammar, and schema files remain unchanged. Full Rust
suite, release build, installed acceptance, remote CI, cross-platform runtime,
and listening checks were omitted. Runtime normalizer/editor behavior, generic
lock verification/discovery/rendering, descriptor wire schemas, and loss-report
schemas remain separate deferred work. The bounded result does not claim a
full profile, universal tolerance, or cross-platform bit identity. Any change
to the metric, reference interpretation, epsilon, scope, window, or required
fixtures requires a new suite or policy version.
