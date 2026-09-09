# Native audio clip validation

Status: implementation and automated validation complete.

The [clip contract](audio-clips.md) implements rate-mode MaaC/1 §14.2 through
existing source syntax. This working-tree slice is based on commit
`bb44e81cfc79cb697740f46a949194f3549e3e79`. Evidence is retained locally under
`target/audio-clips-validation/`.
Validation ran on 2026-09-10 with Rust/Cargo 1.95.0 on macOS 26.6.2 ARM64.

## Implementation and focused checks

Astra Low implemented and exercised shared sample storage, clip timing, source
validation, V5 artifacts, compilation, DSP, production delivery, CLI reporting
and the example. Astra High specified the contracts, integrated the work and
maintained documentation. Sol Max independently reviewed the implementation.
Final independent approval covered the complete slice and its verification
evidence; all 124 frozen input hashes matched with zero discrepancies.

Clips are genuine graph sources, with shared immutable decoded samples and
explicit routing. Their true physical start determines interpolation and fades;
runtime uses absolute frame offsets without accumulated phase. V5 retains exact
transport recipes and assets. Existing public closed plan/processor/result types
remain unchanged, and clip counts are separate from event counts.

Focused checks cover mono/stereo slices, reverse channel order, unequal rates,
speed, fractional placement, both overlapping fade shapes, empty sampled
intervals, tail admission, negative origins, ramps, mixed notes/hits/clips,
selected outputs, reset, shared storage and nonfinite failures. Hostile imports
exercise strict structure, forged timing, invalid references/ports/metadata,
caller limits and checked execution accounting.

Independent review found a numerical endpoint issue: a mathematically active
final sample or positive fade factor could round to the boundary and disappear.
Failing regressions were observed before correction. Preparation now validates
exact first/last values against runtime reconstruction and fails explicitly when
it cannot preserve the admitted domain. The corrected timing suite has 12
passing tests; its related tempo/DSP regression suite has 27.

Production/CLI checks pass for both WAV encodings, accurate counts, source-free
replay, default identity normalization, changed transport/resource identities,
atomic failure behavior and completed audio retained after failed checks.
Master, clip stem and shared-effect delivery match retained replay at 44.1, 48
and 96 kHz, including an external compressor sidechain. The short ramp fixture
uses duration `ln(2)/50 + 1/100` seconds and produces 1,053, 1,146 and 2,291
frames respectively through manifest version 2.

Formatting, Clippy with all targets and warnings denied, and diff checks pass.
The private V5 processor stores its clip in a box; before/after example plan JSON
is byte-identical. No lint suppression or new dependency was introduced.

## Runnable example

`maac build examples/audio-clips.maac --project-root . -o audio-clips.wav`
combines three clips, two hits and one note using the existing original kick
sample. Source and retained Float32 WAVs match byte for byte: mono, 48 kHz,
204,000 frames (4.25 seconds), measured sample peak approximately 0.168.
Generated WAVs and inspection evidence remain in the ignored `target` tree.

## Final integration gates

The frozen 124-file Rust/Cargo input manifest is
`target/audio-clips-validation/integration-inputs.json`. The isolated release
build and fresh installation passed. The installed executable's SHA-256 is
`d16e1d091393cbd1711048c19ecce9fc1812216f2a5bf6895d2b6a1ae1ad1ff6`.
All three explicit release-mode metering audits passed with zero ignored tests.
The 20 retained corpus archives and all 89 WAV hashes were verified first.
Analyzer and resampler algorithms are unchanged; these selected fixtures do not
establish full ITU/EBU compliance. The [metering evidence](production-metering-evidence.md)
defines the applicable scope.

Specification/tooling checks passed with Python 3.14.4 and all seven pinned
development dependencies. The disposable syntax/schema smoke expanded 48 notes
and passed 22 arithmetic assertions; its three generated fixtures matched
tracked files byte for byte. Production smoke passed 8 valid, 77 invalid, one
schema-reference and 60 arithmetic cases. Independent sample-rate-conversion
coefficient verification passed. These checks cover their selected schema and
arithmetic boundaries separately from Rust runtime validation.

The full `cargo test --locked --offline` run exited successfully: 741 tests
passed across 84 targets, including the compile-fail doctest, with zero failures.
The three opt-in audits ignored by that invocation all passed in the separate
explicit release run, giving 744 passing tests overall. All 124 Rust/Cargo
input hashes matched after the complete run. No required automated check was
skipped.

## Installed workflows and compatibility

All 513 installed checks passed without harness or product failures:

| Gate | Passed checks |
| --- | ---: |
| Standard CLI | 38 |
| Existing production workflows | 147 |
| Tempo ramps | 69 |
| Kits and earlier compatibility workflows | 91 |
| Independent WAV inspection | 20 |
| Native clips and nearest-baseline compatibility | 119 |
| Retained replay from an empty working directory | 29 |

Source and retained Float32/PCM16 WAVs match after the copied source and assets
are removed. The example retains three clips, two hits and one note; measured
PCM16 peak is approximately 0.168. Mixed master/stem/shared-effect deliveries
match at all three rates, including external sidechain context, render keys and
certified duration. Failure checks preserve existing destinations and retain
completed audio when requested analysis limits fail.

The validated version 1–4 fixture JSON and WAV bytes match the prior executable
from `bb44e81`, whose
SHA-256 is `ee97d5dbb7299360d7b9a94128d799059a31a307e5e1559b273b3d7fc58e748a`.
Its 117-file input manifest was independently verified against Git. V1 fixtures
were derived from verified resource-empty V2 outputs because CLI V1 emission is
unavailable. The older executable rejects V5 with `E_VERSION` and creates no
output. All 124 current frozen inputs and the four executables used by the
compatibility harness retained their hashes throughout installed validation.

Automated waveform checks do not establish human listening acceptance or
cross-platform bit identity. Warp modes, sample importing, pattern-contained
audio and new modulation capabilities remain outside this slice. Numerical
precision and resource exhaustion fail explicitly; ordinary IEEE floating-point
rounding after successful preparation remains part of the reference engine.
