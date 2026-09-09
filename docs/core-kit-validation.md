# Native kit validation

The [kit contract](core-kit.md) defines this slice. Validation was performed
on 2026-09-09–10 on the working tree based on
`400254cf3b02561ad406be52706af11baf1f92f5`. The kit changes were uncommitted
when validation ran.
The final Rust/Cargo snapshot contains 117 files, recorded locally in
`target/core-kit-validation/integration-inputs.json`.
The environment is macOS 26.6.2 ARM64 with Rust/Cargo 1.95.0.

## Implementation and focused evidence

Astra Low implemented and exercised the asset, event, source profile, artifact,
DSP, production, CLI and example boundaries. The public `Plan`, `PlanV3`,
`VersionedPlan`, `Processor`, `CommandResult` and existing entry points retain
their contracts. Opaque artifact APIs carry versions 1–4. Source syntax and
existing instrument-library versions are unchanged.

Focused tests cover exact asset bytes and package containment; strict V4 import;
hit expansion, overrides and certified step/ramp timing; analytical mono/stereo
sample interpolation; natural tails, voice expiry and overflow; level automation;
mixed instruments; reset and standalone replay. Production tests compare master,
stem and shared-effect outputs at 44.1, 48 and 96 kHz, including exact duration,
identity defaults and changes, overwrite protection, and retained audio after
failed checks. CLI tests exercise both Float32 and PCM16, actual note/hit counts,
legacy result compatibility and empty generic sinks after preparation failure.

Independent review identified and Astra Low corrected three import/API issues:
generic artifact serialization could bypass validation; raw asset IDs needed
identifier and global-uniqueness checks; unused hit release velocity needed
canonical positive zero. Failing regressions were observed before correction.
Raw PCM signed-zero bits remain preserved. Style cleanup leaves runtime layout
unchanged; the private event enum retains bounded inline note-expression state.

Formatting, Clippy with warnings denied, and diff checks passed. The isolated
release build and fresh installation also passed. The installed executable's
SHA-256 is `ee97d5dbb7299360d7b9a94128d799059a31a307e5e1559b273b3d7fc58e748a`.
All three explicit release-mode metering audits passed with zero ignored tests;
the retained corpus archive and 89 WAV hashes were verified first. Analyzer and
resampler algorithms are unchanged. Applicable fixture scope remains documented
in the [metering report](production-metering-evidence.md); this is not a full
ITU/EBU compliance claim.

The complete `cargo test --locked --offline` run exited successfully with
687 passed, zero failed and three opt-in audits ignored by that invocation.
Those three audits all passed in the separate explicit release run, for
690 passing tests overall. The normal run includes the artifact API's
compile-fail doctest. All 117 Rust/Cargo hashes and 89 corpus WAV hashes remained
unchanged after the release gate. No required automated check was skipped.

Specification/tooling checks also passed using the seven pinned development
dependencies on Python 3.14.4 (CI uses Python 3.12). The disposable syntax/schema
smoke expanded 48 notes and passed 22 arithmetic assertions; its three generated
fixtures matched tracked files byte for byte. Production smoke passed 8 valid,
77 invalid, one schema-reference and 60 arithmetic cases. Independent SRC
coefficient verification passed. These smoke checks establish their selected
schema/arithmetic boundaries separately from Rust runtime acceptance.

## Installed workflows and compatibility

All five installed-workflow gates passed, with 365 recorded checks and six
additional expected diagnostic-code assertions:

| Gate | Passed checks |
| --- | ---: |
| Standard installed CLI | 38 |
| Existing production workflows | 147 |
| Tempo-ramp workflows | 69 |
| Kit, delivery and legacy compatibility workflows | 91 |
| Independent WAV rate/channel/frame checks | 20 |

The production runner reused the exact fresh installation rather than installing
again. All 117 frozen inputs remained unchanged. Sample-kit builds and retained
renders match in Float32 and PCM16 after source and asset removal. Kit master,
stem and shared-effect deliveries match source-free replay at all three delivery
rates, including render keys and certified frame ceilings. Failures preserve
existing destinations; failed requested checks retain completed audio.

The previous executable has SHA-256
`d36dba2a3a6123ce60b98e20064e03f5e4569a6cc91a73f63e6e05e11e92e191`.
Its 109-file source manifest matches Git revision `400254c`. Existing step
version 2 and ramp version 3 plan JSON and Float32/PCM16 WAV bytes match the new
executable. A version 1 fixture, derived from an older core-only retained plan
after verifying its program and wavetable lists were empty, parses and renders
identically in both executables. The older executable rejects version 4 with
`E_VERSION` and creates no output.

Evidence is retained under `target/core-kit-validation/installed/`. An initial
harness assertion was too strict about the version 1 fixture's provenance-only
resource envelope; the harness was corrected and its complete rerun passed.
No product code changed for that correction.

## Independent review

Independent Sol Max review covered the asset/kernel contract, strict artifact
validation, source/event/DSP integration, and final CLI/export/production
boundary. Review corrections include validated artifact encoding, identifier
integrity, canonical unused hit fields, and consistent version 4 documentation.
No remaining technical blocker was found. The reviewer independently reran
24 compiler, DSP, expansion and semantic-profile tests, checked the corrected
import paths, and verified the final 117-file snapshot and installed binary.

Final commands, exit statuses, environment, input manifests and corpus checks
are retained under `target/core-kit-validation/integration/`; focused package,
static, installed and smoke evidence remains in the sibling directories.
All substantive implementation and primary execution/validation in this slice
was performed by Astra Low. Astra High specified and integrated the work and
documentation; Sol Max performed the independent test reruns and review
described above.

## Example

The [original drum example](../examples/core-kit.maac) has 24 hits, linear tempo
ramps and kit level automation. Its three raw PCM assets total 35,520 bytes;
the retained plan is 136,625 bytes. The optional external generator reproduced
the exact pinned asset hashes in the validation environment.

The example renders 208,158 mono frames at 48 kHz, including a quarter-second
tail. Float32 peak amplitude is approximately 0.44646. Both Float32 and PCM16
source builds matched retained renders byte for byte after disposable copies
of the source and sample files were removed. Output was finite and non-silent.
The local demonstration is `target/core-kit-validation/demo/core-kit.wav`;
commands, hashes and waveform checks are recorded alongside it.

## Evidence limits

Numerical timing certification remains bounded and may explicitly reject an
unresolved boundary. Checks on one executable and environment do not prove
cross-platform bit identity or human listening acceptance. The original sample
generator uses platform math functions; committed PCM bytes and declared hashes
are the playback inputs.

WAV import, arranged audio transports, sample-kit library exports, choke groups
and voice stealing are outside this slice. The pre-existing production identity
limitation for nested per-note expression objects remains unchanged.
