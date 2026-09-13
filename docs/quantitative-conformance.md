# Bounded quantitative conformance

**Status: accepted 2026-09-13; the bounded contract and evidence slice is
addressed.** This document defines the portable logical contract for the auxiliary policy
`maac.core-audio.reference-f64/1`. It supplies a bounded numerical criterion
for the reference processors in [MaaC-1 §18](../MaaC-1-Specification.md#18-reference-processor-library).
It is evidence within the Core Audio profile, not a fifth §1.2 profile and not
a universal tolerance or a cross-platform bit-identity claim.

## Measurement contract

The observation is the finite sample produced by the claimed runtime at the
pre-encoding binary64 boundary. Decode each IEEE-754 binary64 observation as
its exact dyadic rational `actual`; do not compare an exported binary32 sample
in its place. Every channel and frame in the declared window participates in
the same bound. There is no alignment, latency correction, gain fitting,
transient removal, padding, resampling, or crop adjustment.

Each expected reference sample is an independently specified finite real value
or a reduced rational interval `[reference_lo, reference_hi]` enclosing that
value. The interval width is at most `1/10^80`. A reference oracle may be
rounded to binary64 for a separate diagnostic, but that rounding is not the
reference value used by this policy.

Using exact rational arithmetic, calculate:

```text
E_upper = max_samples(
    max(abs(actual - reference_lo), abs(actual - reference_hi))
)
```

For a point reference, the two endpoints are equal and `E_upper` is the
ordinary maximum absolute error. The accepted inclusive bound is:

```text
epsilon = 1/100000000000000
```

The numerical result is certified only when `E_upper <= epsilon`. If
`E_upper > epsilon`, the run fails conservative certification. With a
non-point interval, that result alone does not prove that the unknown true
error exceeds `epsilon`; the interval width and both endpoint comparisons
remain part of the evidence.

## Fixed reference conditions

The bounded suite has six fixtures at 48 kHz. Each renders the reset-relative,
half-open frame window `[0,8)`. Together they contain 64 channel samples:

1. a 6000 Hz Sine with a two-frame attack;
2. equal-power Pan at center;
3. equal-power Pan at `pan = 1/2`;
4. a 6000 Hz OnePole impulse response;
5. a one-frame Delay with half-gain feedback across four score frames and four
   tail frames; and
6. seeded Noise with seed 7.

Each fixture freezes its source, referenced assets, output port, channel order,
score and tail settings, reset state, expected reference values or intervals,
and provenance. The reference derivation is independent of the implementation
under test and records its source inputs, applicable mathematical and
processor contracts, author, toolchain, and environment. Every fixture also
checks retained-plan or repeat-render behavior where that boundary is part of
the fixture. The Noise references are exact rational known-answer values
computed from the specified counter and SHA-256 formula. Their sample
comparisons use the same accepted bound as every other case. Existing bitwise
Noise regression tests remain separate processor evidence.

The machine-readable [L5 reference corpus](../conformance/l5/README.md) carries
the current source, asset, interval, provenance, and window inventory. Its
stdlib [static verifier](../scripts/check_l5_reference.py) checks those bytes
and derivations without compiling or rendering production code. A verifier pass
therefore establishes corpus consistency only; runtime numerical evidence must
still identify its actual public render boundary and observations.

This inventory deliberately makes no claim about signed-zero or subnormal
behavior, pan endpoints, connection-order reduction, long-duration or broad
amplitude behavior, cross-platform equality, or full Core Audio coverage.
The absolute-error metric treats `+0` and `-0` equally. Asset and PCM
bit-preservation contracts remain separate exact obligations; they are outside
this numerical metric.

## Prerequisites and result reporting

Before calculating `E_upper`, the harness must establish the exact declared
shape and channel order, reset origin, half-open frame window, event and timing
coordinates, and finite status of every observed sample and reference. A
failed prerequisite is reported in the conformance result under the structural,
timing, shape, or finite-value category. When a public compiler or render
operation produces the failure, that operation retains its established MaaC
language diagnostic code; this conformance taxonomy does not add or reinterpret
source-language diagnostics. A required check that was skipped, unsupported,
or not run cannot pass by omission; those states are reported separately from
pass, fail, or conservative certification.

Static corpus validation may establish source, asset, interval, provenance,
and byte-digest consistency. Runtime evidence must separately identify the
public compile/render or retained-plan boundary, executable and configuration,
observed binary64 values, exact metric calculation, and any repeat-render
result. Static validation is not runtime fidelity evidence. L4's raw
binary32 PCM encoding and PCM/file hashes remain separate output evidence and
do not enter `E_upper` or change the render key.

The accepted policy does not add a source numeric rounding rule. It relies on
the real-valued mathematical semantics and scheduling rules already defined by
MaaC-1, including the reference processor algorithms. It also does not change
L1 authored **A** versus normalized **N(A)** identity, L2 timing coordinates,
L3 diagnostics, or L4 closed interchange fields.

## Versioning and scope

The policy identifier and the six-fixture conditions are a single versioned
contract. A change to the metric, interval interpretation, epsilon, reference
scope, window, or required fixture semantics requires a new suite or policy
version. It must not silently reinterpret results produced under
`maac.core-audio.reference-f64/1`.

This document records the accepted numerical choice and portable logical
conditions. The companion corpus and checker provide fixed source, asset,
reference, and provenance evidence; final runtime observations and review
status are recorded in the [integrated L5 record](../target/l5-quantitative-validation/integrated-final.json).

## Bounded evidence status

This bounded contract and evidence slice is addressed 2026-09-13. The public
compile/retained-plan/replay/f64-render gate passed 7 Rust tests across six
fixtures, 48 frame rows, and 64 channel samples; the maximum observed
`E_upper` was approximately `1.5935876903e-16`, and retained replays were
bit-identical. The static reference verifier passed 13 focused tests. The
conformance index passed 22 focused tests while binding seven native suite
manifests and 179 fixture pins.

The targeted L2/L3 cross-slice gate passed 13 tests (3 L2 and 10 L3). Legacy
L1 checker/tests passed 18 tests, L4 checker/tests passed 23 tests, and the
disposable syntax/semantics smoke run matched its three tracked JSON outputs
byte-for-byte. Sol ran the runtime/reference and L2/L3 gates; Luna ran the
index, legacy Python, and smoke gates; the L5 reference-test executor ran its
focused regression suite. Independent read-only semantic and code/harness
reviews passed, and root performed integration acceptance.

Production Rust, Cargo, grammar, and schema files remain unchanged. Full Rust
suite, release build, installed acceptance, remote CI, cross-platform runtime,
and listening checks were omitted. Runtime normalizer/editor behavior, generic
lock verification/discovery/rendering, descriptor wire schemas, and loss-report
schemas remain outside this bounded claim. It does not claim full-profile
execution, a universal tolerance, or cross-platform bit identity.
