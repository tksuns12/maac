# Native warp-rate validation

Status: implementation and all required automated checks passed; final independent
evidence review approved.

The [warp-rate contract](warp-rate.md) implements existing MaaC/1 §14.3 semantics.
This slice follows commit `5cd2f2745b61683beea6b448ed4bc406bd667f36`, which committed
native rate-mode clips. Evidence is retained locally in
`target/warp-rate-validation/`. Validation uses Rust/Cargo 1.95.0 on macOS ARM64.

## Implementation and focused evidence

Astra Low implemented source admission, private V6 artifacts, certified warp
preparation, compilation, DSP, workflow tests, and the example. Astra High
specified and integrated the contracts and maintained documentation. Independent
Sol Max review approved the source rules, shared envelope, and retained-plan
boundary, corrected numerical kernel, and integrated implementation. The final
verification-evidence audit approved the unchanged R3 revision and qualification.

Warp clips are native graph sources. Ordered musical anchors map directly to
source-frame coordinates through the complete tempo map, including future tempo
changes in the tail. Rate clips retain their V5 payload; mixed plans use V6.
Existing public closed plan, processor, and CLI result types are unchanged.
One timing context covers scheduling, validation, and renderer preparation.
Decoded PCM storage is shared by kits and both clip modes.

Focused checks cover exact q/bar placement, fractional starts, negative origins,
constant/step/increasing/decreasing tempo, piecewise anchors, source slices,
mono/stereo interpolation, overlapping linear/equal-power fades, empty sampled
intervals, natural-duration fade-out, tail tempo changes, reset, and nonfinite
failures. Hostile source and retained inputs cover forbidden fields, malformed
or oversized anchors, forged bounds, typed ports, metadata, shared point limits,
and exhausted numerical/execution budgets.

The preparation checks include a fractional-ramp and zero-tail regression that
exposed equivalent logarithmic times expressed using different warp subdivisions.
Preparation now uses one canonical prefix per actual tempo interval, preserving
exact clock-expression structure without changing the shared clock. The failing
regression and passing correction are retained. Forward and backward residual
checks reject lost endpoint precision, and prepared spans must cover the complete
certified frame interval before runtime lookup.

Independent review also supplied an exact public V6 artifact where a positive
backward gap disappeared when subtracted from the right source endpoint, while
the separately rounded forward approximation still appeared inside the slice.
The regression failed before correction. Preparation now requires the backward
subtraction itself to remain in the admitted half-open domain; the exact artifact
fails with `E_TIME_PRECISION`. The reviewer reran and approved the correction.
The resulting preparation suite has 16 passing tests.

The full normal run also caught a legacy diagnostic regression: scores without
warp anchors received warp-inclusive point-budget wording. The compiler now
preserves the original diagnostic for those scores and retains the expanded
message when warp anchors contribute. The failing regression and 33 passing
related compilation tests are recorded in `diagnostic-fix/`; independent review
approved the correction. Budget arithmetic is unchanged.

Six DSP tests exercise analytical waveforms and mixed graphs; eight production
and CLI tests exercise the source/retained boundary. Master, stem, and shared
effect deliveries match at 44.1, 48, and 96 kHz, including external compressor
sidechain context. The selected short ramp fixture yields 1,053, 1,146, and 2,291
delivery frames respectively, with 1,146 engine frames and manifest version 2.
Failed checks retain completed audio; failed forced exports preserve existing
destinations. Copied source and assets are removed before CLI retained replay.

## Example and compatibility

```sh
maac build examples/warp-rate.maac --project-root . -o warp-rate.wav
```

The example has two warp clips, one rate clip, two hits, and one note through
reverb and master gain. A clip starting at `31/4q` crosses score end at `8q`
and the later tempo change at `33/4q`. It uses the original core-kit PCM asset.
Source and retained Float32/PCM16 outputs match byte for byte: 195,375 mono
frames at 48 kHz, Float32 sample peak approximately 0.187.

The committed rate-mode executable is retained at
`target/audio-clips-validation/install/bin/maac`, SHA-256
`d16e1d091393cbd1711048c19ecce9fc1812216f2a5bf6895d2b6a1ae1ad1ff6`.
The existing rate example and production fixture compile to byte-identical JSON,
including production identity. The shared-envelope refactor also preserves the
rate example's Float32 and PCM16 WAV bytes and its measured timing-work threshold.

## Final gates

Formatting, Clippy with all targets and warnings denied, and diff whitespace
checks passed. The frozen Rust/Cargo manifest contains 130 files at
`target/warp-rate-validation/integration-inputs.json`.

The final revision passed a fresh isolated release build and installation.
The executable is
`target/warp-rate-validation/qualification-r3/install/bin/maac`, SHA-256
`67aa63358b0332d2bcbeea6d990ddba0c4da754a2a5164a4b6721e73d487f056`.
All three explicit release-mode metering audits passed with zero ignored tests.
All 20 archives passed size/hash/CRC verification, and 89 extracted WAV hashes
matched. All 130 input hashes stayed unchanged throughout these stages.
These selected fixtures do not establish full ITU/EBU compliance; the
[metering evidence](production-metering-evidence.md) defines their scope.

Earlier results remain separately preserved as superseded evidence. Final normal,
release, audit, and installed evidence is under `qualification-r3/`, using the
same 130-file manifest, SHA-256
`4c8eca11c6cf24ae89b0147f4bb8b79805b26b0f9697a88d37f352beac78432a`.
The full `cargo test --locked --offline --no-fail-fast` run passed 789 tests
across 88 targets, including the compile-fail documentation test, with zero
failures. Its three ignored metering audits passed in the separate release
invocation above: 792 tests passed across the two invocations. Formatting,
Clippy, and the full suite used the same unchanged frozen inputs. Exact commands,
environment, logs, and totals are in `qualification-r3/integration/summary.json`.

All 621 installed checks passed with zero failures or skips:

| Gate | Passed checks |
| --- | ---: |
| Standard CLI | 38 |
| Existing production workflows | 147 |
| Tempo ramps | 69 |
| Kits and earlier compatibility | 91 |
| Independent WAV inspection | 20 |
| Rate clips and compatibility | 119 |
| Retained replay from an empty directory | 29 |
| Native warp workflows and failed-check audio retention | 102 |
| Installed endpoint regression and destination preservation | 6 |

The final installed executable reproduces the precision rejection and preserves
both existing and absent destinations. The mixed example and named deliveries
replay without their source/assets, and older JSON/WAV/production identities
retain their bytes. The previous rate executable rejects V6 with `E_VERSION`
and creates no output. All 130 source hashes, the example, and six executable
hashes remained unchanged through qualification. Results and exact commands are
under `target/warp-rate-validation/qualification-r3/installed/`.

Python 3.14.4 and all seven pinned development dependencies were verified.
Specification smoke passed 48 notes and 22 arithmetic checks; three generated
fixtures matched tracked bytes. Production smoke passed 8 valid, 77 invalid,
one schema, and 60 arithmetic checks. Independent SRC verification passed for
41,474 coefficients and its certificate. All 17 scoped tooling inputs remained
unchanged; these checks are separate from Rust runtime validation.

All required automated gates passed; none was skipped. The three audits excluded
from the normal invocation were explicitly executed in release mode. No source,
test, or dependency changes followed qualification.

Automated waveform checks do not establish human listening acceptance or
cross-platform bit identity. Preserve-pitch warping, sample import, pattern audio,
and new modulation remain outside this slice. Numerical and resource exhaustion
fail explicitly; ordinary IEEE rounding after successful preparation remains
part of the reference engine.
