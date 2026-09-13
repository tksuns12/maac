# L5 Core Audio reference corpus

This directory freezes the six-case, 48 kHz reference corpus for numerical
policy `maac.core-audio.reference-f64/1`. Each case covers reset-relative frames
`[0, 8)`. The policy compares finite pre-encoding binary64 samples, decoded as
exact dyadic rationals, with independently derived real-valued reference
intervals. Its adopted bound is maximum absolute sample error no greater than
the exact rational `1/100000000000000`.

For reference interval `[lo, hi]` and observed sample `actual`, the conservative
upper error is:

`E_upper = max_samples(max(abs(actual - lo), abs(actual - hi)))`.

A runtime is numerically certified for this suite only when its shape, timing,
finite-value, output-port, and channel-order checks pass and
`E_upper <= 1/100000000000000`. If the upper bound exceeds epsilon, the run is
not certified; for a non-point interval this alone does not prove that the
unknown true error exceeds epsilon.

The fixed inventory is:

1. a 6000 Hz Sine with a two-frame attack;
2. equal-power Pan at center;
3. equal-power Pan at `pan = 1/2`;
4. a 6000 Hz OnePole response to the pinned 32-byte impulse asset;
5. a one-frame Delay with half-gain feedback over four score and four tail
   frames; and
6. Noise with seed 7.

The [manifest](manifest.json) pins every source, asset, and reference byte hash,
as well as reset window, score/tail split, output port, channel order, and
independently calculated note-gate or audio-transport frames. The OnePole source
contains the literal impulse SHA-256; it has no authoring placeholder. The
[sample intervals](references/sample-intervals.json) contain only reduced
rational endpoints. Observed samples and sample bit patterns are deliberately
absent from the expected oracle.

Run the read-only fixed-corpus verification from the repository root:

```sh
python3 scripts/check_l5_reference.py --corpus conformance/l5 --verify
```

Run the separate public compiler and renderer numerical gate with:

```sh
cargo test --locked --offline --test l5_core_audio
```

That Rust integration test compiles every fixed source through
`SourceBundle` and `compile_bundle_artifact`, replays the retained
`PlanArtifact` JSON, renders both artifacts through the public binary64
callback, and requires bit-identical replay. It checks the pinned reset,
timing, output, frame, and channel metadata before comparing all 64 samples to
the independent rational intervals with the conservative bound above.

The standard-library verifier regenerates all intervals without production DSP
or `libm`. Square roots use integer-square inequalities on a `10^-90` grid. Pi
uses Machin's formula with exact alternating-series remainder brackets through
terms 120/121 and outward rounding to 100 decimal places. The OnePole
coefficient uses exact alternating exponential-series brackets at both pi
endpoints and outward rounding to 80 decimal places. Noise uses the specified
SHA-256 byte preimage and exact dyadic conversion. The largest permitted
reference interval width is `1/10^80`.

The static verifier establishes fixed corpus inventory, bytes, hashes, shapes,
and reference derivation; it does not compile sources or render audio. The Rust
gate establishes this bounded runtime result on the executing build and
platform. Neither gate proves a full Core Audio profile or a cross-platform
bound. The local calibration that informed the accepted epsilon remains
selection provenance; its observed binary64 samples are not inputs to these
references.
