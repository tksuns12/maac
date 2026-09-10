# Core modulation validation

Status: implementation and required automated qualification passed. The
orchestrator accepted the frozen revision with the residual risks below.

The [core modulation contract](core-modulation.md) implements existing MaaC/1
control syntax. The implementation follows commit
`b5cd00198a30cfbe958acd19e38a96a7425b1a1e`, which committed warp-rate clips.
Local evidence is retained in `target/core-modulation-validation/`.

## Implementation and review

Astra Low implemented source admission, private V7 artifacts, numerical control
preparation, DSP, and workflow tests under the earlier orchestration policy.
After the global policy changed, Luna Max took ownership of final mechanical
cleanup and qualification. The orchestrator specified the contract, integrated
the work, and maintained documentation. Independent Sol Max reviews cover the
wire format, numerical kernel, admission integration, source rules, and DSP.
The latest policy assigns final evidence verification and acceptance to the
orchestrator; completed independent reviews remain supporting evidence.

V7 retains embedded resources and earlier musical features while adding typed
scalar control ports and additive modulation edges. Public closed types and
V1–V6 execution paths remain unchanged. All LFOs share the plan's timing budget;
aggregate span work is checked before allocation. Runtime evaluates automation,
the control dependency graph, and sorted modulation contributions before the
target's final range policy and existing event processing.

Focused tests cover all four waves, both clocks, phase normalization, ramp
boundaries, score-tail holding, seconds-tail continuation, control chains,
cycles, typed target admission, nonfinite arithmetic, resource limits, and lost
endpoint precision. Public retained-plan regressions reproduce precision and
diagnostic-path failures before their correction. Independent review approved
the numerical kernel after additional ramp interpolation stress checks.

Source/DSP review independently reran 68 source and identity tests, 10 modulation
DSP tests, and 46 legacy DSP tests. Seven V2–V6 source artifacts matched the
previous installed executable's JSON bytes. Equivalent explicit and omitted
control defaults retain equal execution identities while authored source hashes
remain distinct.

Workflow review found that the original example replay test inherited the
repository working directory after deleting its copied inputs. The corrected
test and live harness render from separate empty working directories using
absolute plan paths. Both encodings passed, and independent review reran the
corrected test. The earlier workflow evidence remains historical.

## Frozen revision and normal checks

The frozen manifest contains 137 Rust/Cargo inputs and separately records the
example. Its SHA-256 is
`2eed8a0d7728e7e921e8efe9db4be980c76438f7ccae831cc0ac042ebf59786e`.
Formatting, all-target Clippy with warnings denied, and diff checks passed.

`cargo test --locked --offline --no-fail-fast` passed 841 tests across 93
targets, including one compile-fail documentation test, with zero failures.
The direct subprocess exit was zero. All frozen inputs and the example matched
before and after the run. Exact environment, command, counts, and hashes are in
`full-suite/summary.json`; the complete log is `full-suite/normal.log`.
Three metering audits ignored by this normal invocation passed in the separate
explicit release-mode run: 3 passed, 0 failed, 0 ignored, direct exit zero.
The two invocations therefore passed 844 tests in total. All 20 corpus archives
passed fresh size/hash/ZIP CRC checks, and all 89 extracted WAV files matched
their expected sizes and hashes. Exact audit commands, reports, and corpus
evidence are under `release-evidence/`. These selected fixtures do not establish
full ITU/EBU compliance; see the [metering scope](production-metering-evidence.md).

The isolated release build and installation passed with direct exit zero.
The installed executable is `target/core-modulation-validation/install/bin/maac`,
5,204,944 bytes, SHA-256
`297265ecc65c6394d13e85e37b41d7dbec25d4056782d39826fc7efee71dc69e`.
Frozen inputs matched before and after each stage. The previous warp-rate
executable retained SHA-256
`67aa63358b0332d2bcbeea6d990ddba0c4da754a2a5164a4b6721e73d487f056`.

The orchestrator verified that all 17 independently scoped specification,
schema, fixture, conversion, and SRC-tooling inputs match the earlier successful
smoke evidence. Those unchanged checks were not rerun. Retained results cover
48 expanded notes, 22 arithmetic assertions, three byte-identical generated
fixtures, 8 valid and 77 invalid production documents, one schema-reference
check, 60 production arithmetic cases, and 41,474 SRC coefficients with their
certificate. `tooling-evidence/retained-scope.json` records this hash comparison
and the exact retained evidence. These checks are separate from fresh Rust
runtime and installed-executable qualification.

## Installed compatibility

The first six installed gates passed 484 behavioral checks with no failures or
skips: standard CLI 38, production workflows 147, tempo ramps 69, kits 91,
independent WAV inspection 20, and native rate clips 119. Each gate verified the
frozen inputs, example, and current/historical executable hashes before and
after execution. `installed/runner-result.json` records the combined result.

The first native harness invocation omitted its binary-hash argument and failed
before testing. Its failed evidence is preserved; the corrected invocation
passed. This was an evidence-adapter failure, not a product regression.
The existing installed supplements passed another 137 checks: empty-directory
replay 29, warp workflows 99, failed-check audio retention 3, and endpoint
rejection/destination preservation 6. All four stage exits were zero, with no
input, fixture, or binary drift. `installed-supplements/supplement-result.json`
records these results. The combined existing compatibility total is 621 checks,
with no failures or skips.

The installed modulation package passed another 302 behavioral checks, with
four separate snapshot checks. The example replays from empty directories in
Float32 and PCM16. Named master, stem, and shared-effect deliveries match
source/retained audio, manifests, and execution identities at 44.1, 48, and
96 kHz, producing 1,053, 1,146, and 2,291 frames respectively. These scenarios
include notes, kit hits, rate/warp clips, both LFO clocks, an automated constant,
and external compressor sidechain context. Copied source, PCM, and schema files
are removed before retained delivery from an empty directory.

Malformed controls, control-as-output, invalid delivery event ports,
nonfinite products, and the retained precision witness fail as expected.
Failed forced renders preserve existing and absent destinations; a failed
named-delivery check retains complete audio. The previous installed executable
rejects V7 with `E_VERSION` without creating output. All frozen inputs and
executable hashes remained unchanged. Exact commands and results are in
`installed-modulation/installed-modulation-result.json`.

Final acceptance identified a mislabeled harness case: the invalid delivery
event port above had been described as event-rate modulation, and its derived
exit field was stale. The preserved direct command correctly records exit 1
and `E_PORT_TYPE`; `installed-modulation/event-rate-supplement/correction.json`
supersedes the misleading label and field. A separate installed check now
targets `core.sine/1.attack` from a constant, verifies exact `E_CAPABILITY`, and
checks both absent and existing destination preservation. Its eight behavioral
assertions and four snapshot assertions passed with no source or binary drift.
The original successful gates were not rerun for this correction.

The complete installed total is 931 behavioral checks: 621 compatibility,
302 modulation workflow, and eight event-rate rejection assertions. These are
CLI/harness assertions, separate from the 844 Rust tests and metering audits.

## Final acceptance

The orchestrator inspected the stable artifact, validation, and DSP changes;
checked the completed independent reviews and their corrected findings;
reparsed the full-suite log; verified the frozen inputs and executable hashes;
and inspected the release, corpus, audit, replay, and installed-delivery
evidence, including the corrected event-rate coverage and metadata addendum.
No blocking findings remain. Final evidence records and document hashes are
retained in `final-acceptance.json`.

No dependencies were added. Previous installed binaries and evidence remain
available for compatibility comparisons. Existing public closed types retain
their contracts; older readers explicitly reject V7. Qualification used the
working tree following the warp-rate commit named above.

## Example

```sh
maac build examples/core-modulation.maac --project-root . -o core-modulation.wav
```

The example produces 109,688 stereo frames at 48 kHz. Isolated source and
retained replay match byte for byte in both encodings. Float32 WAV SHA-256 is
`936253f18f28240e0f3f866a7203db5b80e3561e738acb450c8c4efd2bb899ab`;
PCM16 WAV SHA-256 is
`ad35a10c0e53bf7f1038bb03e0f8fea52384ea19f4fc98d0560a56b14e0b3bd7`.
Sample peaks are approximately 0.148195 and 0.148193 respectively.

## Scope and residual risk

This slice supports continuous sample-rate targets. Event-rate modulation,
feedback, implicit smoothing, and oversampling are outside its contract.
Floating-point waveform evaluation does not claim cross-platform bit identity.

Review noted two nonblocking diagnostic limitations: combined range errors use
the generic `modulations.target` path, and rejection of a reset-rate modulation
target uses wording about automation. Target coverage is representative rather
than exhaustive for every effect and public instrument parameter. Human
listening has not been performed; numerical checks do not establish listening
approval or full ITU/EBU compliance.
