# Core Audio conformance audit

Audit date: 2026-09-10. Implementation baseline:
`6844d3f6e21b40635282bba61bddbe47a697d832`.

## Conclusion

The implementation has a path for every reference processor identifier in
MaaC/1 §18 within its declared capability and resource limits. The audit
confirmed a Sine arithmetic mismatch and a required-configuration validation
gap; the A1 and A2 follow-ups below record their fixes. The A3 caller
channel-limit diagnostic discrepancy has been fixed, with targeted evidence
recorded below. The A4 rendered sum-order evidence gap is addressed by a
test-only regression and targeted validation recorded below. Broader profile
obligations remain open. Processor coverage does **not** establish
the full Core Audio profile. Under [§1.2](../MaaC-1-Specification.md#12-conformance-profiles),
Core Audio additionally inherits Document and Performance requirements and
includes the core asset/transport profile.

This audit compares normative requirements with implementation paths and
targeted test evidence. It is not an exhaustive proof of all valid documents,
an external processor certification, or a Locked Render claim. The existing
[capability declaration](capabilities.md) continues to disclaim all four full
profiles.

## Classification

- **Implemented with evidence:** inspected implementation and relevant tests
  support the behavior within the declared scope.
- **Missing interface or feature:** an obligation has no complete public path.
- **Validation discrepancy:** public validation and compilation disagree about
  a normative requirement.
- **Unverified:** relevant behavior or a full boundary has not been established
  by the evidence collected here.

Published host limits are considered separately from missing language features.
[§23](../MaaC-1-Specification.md#23-validation-and-diagnostics) permits bounded
resources. Mono/stereo and the 48 kHz engine limit alone do not demonstrate a
DSP mismatch; the distinction between resource and capability diagnostics still
matters. Unsupported extension execution must fail explicitly rather than
silently approximate it.

## Scope and evidence

The audit examines reference processors and causality (§§15–18), core audio
assets and transport (§14), and inherited document/performance obligations.
Read-only reviewers inspected the following implementation and test boundaries;
their inspection is distinct from the executor runs recorded later.

| Reference processor | Main implementation | Existing evidence inspected |
| --- | --- | --- |
| Sum (§18.1) | Connection sorting and sum processing in [dsp.rs](../src/dsp.rs) | [dsp tests](../tests/dsp.rs) |
| Gain (§18.2) | Gain processing in [dsp.rs](../src/dsp.rs) | [core_gain](../tests/core_gain.rs) |
| Fader (§18.2) | dB conversion and finite checks in [dsp.rs](../src/dsp.rs) | [core_fader_dsp](../tests/core_fader_dsp.rs) |
| Pan (§18.3) | Equal-power pan in [dsp.rs](../src/dsp.rs) | [dsp tests](../tests/dsp.rs), [modulation_dsp](../tests/modulation_dsp.rs) |
| Matrix (§18.4) | Prepared ordered rows in [dsp.rs](../src/dsp.rs) | [core_matrix_dsp](../tests/core_matrix_dsp.rs) |
| Delay (§18.5) | Read/evaluate/commit phases in [dsp.rs](../src/dsp.rs); causality in [plan.rs](../src/plan.rs) | [core_delay_dsp](../tests/core_delay_dsp.rs), [core_delay_plan](../tests/core_delay_plan.rs), internal scheduler-order test |
| OnePole (§18.6) | Filter state in [dsp.rs](../src/dsp.rs) | [dsp tests](../tests/dsp.rs), [modulation_dsp](../tests/modulation_dsp.rs) |
| Sine (§18.7) | Voice/envelope processing in [dsp.rs](../src/dsp.rs) | [pitch_expression_dsp](../tests/pitch_expression_dsp.rs), [gain_expression_dsp](../tests/gain_expression_dsp.rs), [event_rate_modulation_dsp](../tests/event_rate_modulation_dsp.rs); finding A1 below |
| Kit (§18.8) | [kit.rs](../src/kit.rs) | [kit_dsp](../tests/kit_dsp.rs) and internal kit tests |
| LFO (§18.9) | [core_control.rs](../src/core_control.rs), phase normalization in [semantic.rs](../src/semantic.rs) | [modulation_dsp](../tests/modulation_dsp.rs), [modulation_compile](../tests/modulation_compile.rs) |
| Constant (§18.10) | Control evaluation in [dsp.rs](../src/dsp.rs) | [modulation_dsp](../tests/modulation_dsp.rs) |
| Noise (§18.11) | Prepared SHA-256 prefix and counters in [dsp.rs](../src/dsp.rs) | [core_noise_dsp](../tests/core_noise_dsp.rs), including independent known-answer vectors |

Signed and multi-cycle LFO phases are accepted and reduced modulo one at the
source boundary. The retained `[0,1)` restriction represents canonical form,
not a missing source feature. Delay feedback and residual-cycle rejection are
implemented; positive latency is not used as a generic permission for cycles.

The §14 asset/transport review found implementation paths for the required raw
float32 format, exact hash/byte-length verification, finite sample validation,
package-relative resolution, rate playback, reverse/speed, fades, `warp_rate`,
and continuation through an explicit tail. Relevant code is
[audio_asset.rs](../src/audio_asset.rs), [bundle.rs](../src/bundle.rs),
[audio_clip.rs](../src/audio_clip.rs), and [warp_clip.rs](../src/warp_clip.rs).
Existing evidence includes [audio_compile](../tests/audio_compile.rs),
[audio_dsp](../tests/audio_dsp.rs), [kit_dsp](../tests/kit_dsp.rs), and the
[warp validation report](warp-rate-validation.md). This is bounded coverage,
not an exhaustive asset-security or numerical audit.

WAV/AIFF/FLAC importing and preserve-pitch warping are explicitly importer or
extension capabilities in §14. Their absence is not, by itself, a missing core
rate-transport implementation. Audio objects inside patterns are not a core
nesting location under §4. External ABI hosting (§17) is also not another
missing reference processor.

## Profile obligations beyond the processor catalog

| Area | Audit disposition | Evidence and limit |
| --- | --- | --- |
| Compact source normalization and revision identity (§20) | Bundle-aware implementation | [editing](../src/editing/mod.rs) exposes authored canonical revision identity; `BundleEditContext` re-resolves pinned imports, supports reusable library sources, and shares production descriptor defaults for label-retaining N(A). [production_identity.rs](../src/production_identity.rs) remains the execution-hash projection. Unknown external processor contracts remain explicit capability boundaries. |
| Atomic editing (§21) | Bundle-aware implementation | `maac::editing` implements all five Protocol 2 operations, immutable-base preconditions, identity correspondence through renames, atomic final validation, inverse patches, source-preserving projection, bounded impact reporting, source-selectable library editing, and candidate dependency re-resolution. `maac patch --project-root` exposes the same bundle-aware path with atomic publication. |
| Message performance (§§6.1, 8) | Performance transport implemented | Message leaves compile through pattern expansion into retained transport events with protocol/bytes preserved. `PlanArtifact::performance_dispatches` enforces exact adapter protocol advertisement and §6.1 same-frame class ordering; the built-in Core Audio renderer still rejects raw messages because no core raw-message receiver is defined. |
| Seeking with prehistory (§28 vector 17) | Full DSP-seek implementation evidence absent; reset-correct range export added | [dsp.rs](../src/dsp.rs) still renders from reset through the full interval. The [range contract](render-range.md) and [validation evidence](render-range-validation.md) cover final-WAV excerpts whose payload matches the corresponding full-render interval while the complete plan is still executed and validated. No public DSP seek/window/state-restoration API or seek-checkpoint contract exists; spool seeking during delivery is not DSP-state restoration. A future seek interface must retain the required prehistory; this audit does not invent a new seek wire format. |
| Full dependency locks (§22) | Generic lock generation, verification, and bounded core rendering implemented | `maac::generic_lock_normalization` constructs canonical generic v1 artifacts; `maac::generic_lock` independently validates them and separates pre-render input verification from evidence verification. `maac::generic_render` executes an already-resolved built-in/core `Plan` only after those input checks, requires its concrete engine identity, supports the proven block-independent null schedule, preserves reset prehistory, and emits exact raw binary32 evidence. `maac::external` verifies external closure, but executable external ABI hosting remains outside this renderer. |
| External processors and adapters (§§17, 25) | Descriptor/discovery boundary implemented; executable ABI deferred | `maac::external` strictly parses the §17 descriptor wire, checks descriptor identity/latency/determinism and state compatibility against the lock, and requires explicit host ABI/adapter/permission policy before use without loading module bytes. `maac::interchange` separately provides a bounded MIDI 1.0 SMF adapter with versioned loss reports and faithful-mode refusal. Native ABI invocation, notation/DAW adapters, and generic external rendering remain deferred. |
| Diagnostics/resources/package boundaries (§§23–24) | A3 implemented with targeted evidence; not exhaustively audited | [diagnostic.rs](../src/diagnostic.rs), [bundle.rs](../src/bundle.rs), [bundle_fs.rs](../src/bundle_fs.rs), and [bundle tests](../tests/bundle.rs) cover relevant validation and containment paths. The A3 follow-up records the bounded caller channel-limit classification and evidence. |

## Findings

### A1 — confirmed: finite Sine parameters can silently produce wrong audio

At the audited baseline, `Voice::attack_amplitude` divides elapsed frames
by `attack * rate` ([dsp.rs](../src/dsp.rs), line 2073). A finite attack of
`10^308` seconds overflows that denominator at 48 kHz. The computed envelope
becomes zero, and later multiplication by a finite level of `10^308` cannot
recover the intended sample.

The executor reproduced this through the existing debug CLI with a four-frame
12 kHz Sine note, velocity 1, attack `10^308` seconds, level `10^308`, and release
zero. The authored large number was the exact decimal `"1" + "0" * 308`.
Compilation/rendering succeeded with exit 0 and returned four zero float32
samples. At frame 1, §18.7's real-valued expression simplifies to `1/48000`,
approximately `2.0833333333333333e-5`; its float32 rounding is
`2.0833333110203966e-5`. The observed result was exactly zero, without an error.

Local probe artifacts are `target/core-audio-audit/extreme-sine.maac`,
`build.json`, `extreme-sine.wav`, and `inspection.json`. They are generated
evidence, not tracked conformance fixtures. The finding is an admitted audio
mismatch, not a claim that ordinary Sine rendering fails.

### A2 — confirmed: source schema validation misses two required configurations

`validate_source` bypasses `validate_processor_config` when the entire config
record is absent. Sum and OnePole lack the explicit missing-record guard that
the other required core configurations have. Their port descriptors then use
a mono fallback. An authored empty `config = {};` does reach the required-field
check. Compilation separately requires `channels` and rejects either form.

A target-only Rust driver linked against the current debug library reproduced
the public API discrepancy:

| Processor/config | `validate_source` | `compile` |
| --- | --- | --- |
| Sum, config omitted | Accepted; empty config and mono output descriptor | `E_RANGE` |
| Sum, empty config record | `E_RANGE` | `E_RANGE` |
| OnePole, config omitted | Accepted; empty config and mono output descriptor | `E_RANGE` |
| OnePole, empty config record | `E_RANGE` | `E_RANGE` |

The driver and observed output are in
`target/core-audio-audit/schema-probe.rs` and `schema-probe.out`. No production
code or tracked tests were changed to run the probe.

The affected boundary is the public source-schema validation promise in
[reference.md](reference.md), implemented in [semantic.rs](../src/semantic.rs)
around lines 2112–2156 and 2638–2650. This is separate from cardinality and
causality, which SourceGraph intentionally defers to lowering.

### A3 — implemented: caller channel-limit diagnostics are consistent

Several [plan.rs](../src/plan.rs) checks reported `E_RANGE` when a supported
channel count exceeded the caller's `max_channels` limit. §23 specifies
`E_RESOURCE_LIMIT` for a declared host limit. The shared validation now keeps
that resource classification distinct from invalid authored dimensions and
unavailable processor capabilities; the targeted matrix evidence is recorded
below.

### A4 — addressed: rendered sum-order evidence strengthened

No incorrect sum output was observed in the original audit. The
cancellation-sensitive sum test exercised the `sum_samples` helper, while
connection-ID sorting is implemented separately in the renderer. The new
test-only rendered-graph regression exercises that boundary directly. Its
baseline, deliberate isolated-fault proof, and focused target validation are
recorded in the A4 follow-up below. A4 does not change production renderer
behavior.

## A1 follow-up: Sine envelope arithmetic

The follow-up keeps the ordinary envelope calculation
`elapsed_frames / (duration_seconds * rate)` when the denominator is finite.
Only when that product overflows does it evaluate
`(elapsed_frames / rate) / duration_seconds`. This preserves ordinary binary64
evaluation while recovering the representable envelope for very large finite
durations. Attack and release share the same progress calculation; note-off
capture and voice-retirement checks continue to use those envelope methods.

No authored range, clipping, or implicit duration limit is added. Zero-length
attack/release behavior is unchanged. The fix chooses a finite-output path for
the reproduced case rather than a new render error.

The original acceptance criteria were:

1. Add a regression for the admitted finite attack/level case that fails on the
   audited baseline.
2. Produce the correct finite sample within an explicit numerical tolerance
   when representable, or fail explicitly with `E_NONFINITE` when the supported
   numerical evaluation cannot produce it. Do not report successful silence.
3. Preserve ordinary attack/release, zero-length envelope stages, note-off
   capture, reset/replay, and source-free retained rendering.
4. Verify CLI failure preserves an existing output if an explicit-error path
   is chosen. Do not impose an unrelated arbitrary authored envelope range.

The regressions are [sine_envelope_numeric](../tests/sine_envelope_numeric.rs)
and [sine_envelope_cli](../tests/sine_envelope_cli.rs). They check the reproduced
case at the engine and final WAV boundaries, together with ordinary envelope
arithmetic and retained replay.

## A2 follow-up: required processor configuration

Source validation now requires an explicit `config.channels` for Sum and
OnePole, as specified in §§18.1 and 18.6. An omitted config record reports
`E_RANGE` at `config.channels`, matching the required-channel guard already used
for Gain and Fader. Explicit empty records continue to fail the existing
required-field check. Kit retains its separate required-configuration path.

This corrects validation-only acceptance of invalid implicit mono nodes.
Compilation already rejected these declarations; valid processor behavior and
optional configuration defaults are unchanged.

The [required-configuration regressions](../tests/required_processor_config.rs)
exercise omitted, empty, and valid mono/stereo configuration through source
validation and compilation for Sum, OnePole, Gain, Fader, Matrix, Delay, and
Noise. They also retain positive controls for omitted Sine/Pan configuration.

## A3 follow-up: caller channel-limit diagnostics

The shared `PlanView::validate_channel_count` in [plan.rs](../src/plan.rs) now
uses one channel-count classification. Zero remains `E_RANGE`. A positive dimension
above the foundation's mono/stereo capability is `E_CAPABILITY`, including the
legacy cases that previously surfaced as `E_RANGE`. A supported mono or stereo
dimension above the bounded caller `max_channels` is `E_RESOURCE_LIMIT`.
Capability is checked before the caller resource limit for each dimension.

The existing diagnostic paths and dimension traversal order remain in place;
Matrix dimensions are checked in `inputs` then `outputs` order. The checks cover
output settings, audio clips, Kit, common OnePole/Gain/EQ/Compressor/Reverb/Sum,
Fader/Noise/Delay, Matrix input and output dimensions, Instrument, and
Compressor sidechain channels. Caller values such as 0, 1, 2, and 4 continue to
be bounded by the published `PlanLimits` ceiling. Accepted plans and audio,
published resource bounds, wire versions, and DSP behavior are unchanged.

The [channel-limit regression](../tests/channel_limit_diagnostics.rs) covers
valid-at-limit controls, caller limits 0/1/2/4, supported, invalid, and
unavailable widths, both Matrix directions, and propagation through source
compilation and retained-plan loading. The original RED reproduced the actual
classification failure; the recovery executor's expanded matrix then passed
the targeted GREEN gate.

## A4 follow-up: rendered sum-order evidence

The test-only regression in [dsp.rs](../tests/dsp.rs) renders a two-frame,
12 kHz Sine through three Matrix branches into Sum. The fan-in values use
`+2^54`, `-2^54`, and `+1`; the stereo branch supplies the opposite second
component. It covers mono and stereo widths, all six fan-in declaration
permutations, both node declaration orders, bitwise frame output, stereo
retained replay, and a reassigned-connection-ID control that still follows
connection-ID order while changing the required reduction from
`[+large, -large, small]` to `[+large, small, -large]`. Its expected sample
therefore changes from `1.0` (stereo `[1, -1]`) to zero.

The evidence separates the unchanged-renderer baseline from a deliberately
sort-disabled isolated copy; the latter is a regression proof rather than an
observed product fault. A4 adds no production renderer behavior. Counts,
statuses, hashes, and ownership are recorded in the Verification record.

## Recommended next specification slice

Continue with the [language specification and conformance plan](language-specification-plan.md),
beginning with L4 portable interchange. L1 identity and edit consistency, L2
timing coordinates, and the bounded L3 field and processor contracts are
addressed; the A1–A4 fixes and evidence remain recorded above.

## Verification record

The specification smoke checker passed in a disposable copy: surface parsing,
schema validation, 48 expanded notes, and 22 arithmetic assertions. Generated
`check-results.json`, `conformance.json`, and `example.syntax.json` matched the
tracked fixtures byte-for-byte. Python 3.12 used an isolated environment under
`target/core-audio-audit/` with the repository's pinned `requirements-dev.txt`.
The initial system-Python attempt lacked dependencies; an offline install also
found no cached packages. Installing the pinned dependencies in that isolated
environment succeeded, and the checker then exited zero. No system Python or
tracked checker fixtures were changed. As §28 states, this checker is selected
syntax/semantics evidence, not a conformance proof.

The current audit's focused Rust run completed **126 passing tests across 28
integration targets, with zero failures and zero ignored tests**. It covered:

- Gain and the Fader/Matrix/Delay/Noise plan, DSP, CLI, and available dedicated
  semantic targets;
- Kit compile, DSP, CLI, and delivery targets;
- core modulation compile, plan, DSP, CLI, and production targets;
- event-rate modulation DSP, CLI, and work-limit targets.

The run used `cargo test --locked --offline` with those explicit test targets.
Its log is `target/core-audio-audit/focused-tests.log`. These tests passed while
the separate A1 and A2 probes exposed previously uncovered behavior. Passing
regressions do not invalidate either finding.

The baseline had previously passed the optimized full suite with 960 passed,
zero failed, and three existing ignored audits. That full suite was **not**
rerun for the original documentation-only audit. That audit added no production
implementation or tracked regression test. The A1 follow-up is a separate
implementation change with its own Red → Green and regression verification.

For the A1 follow-up, the executor observed the engine regression fail before
the fix (two passed, one failed) and the CLI regression fail with four zero
samples. After the fix, the strengthened engine target passed all three tests
and the CLI target passed its one test. The CLI checks expected float32 samples
with an absolute tolerance of `1e-10` and requires identical retained WAV bytes.

Final executor validation passed formatting, whitespace checks, and Clippy
across all targets with warnings denied. The optimized full suite passed with
964 tests, zero failures, and three existing ignored EBU/ITU audits, including
one passing doctest in that total. It ran with `--locked --offline`, LTO disabled,
and 16 codegen units. The orchestrator independently checked the saved logs and
all four zero exit statuses in `target/sine-envelope-validation/`. The ignored
audits were not run; this fix does not establish full Core Audio conformance.

For A2, the executor reproduced missing-config source validation returning an
accepted graph before the fix. The focused required-configuration target then
passed all four tests after the fix. Final formatting, whitespace checks, and
all-target Clippy with warnings denied passed. The optimized full suite passed
968 tests, with zero failures and the same three existing ignored EBU/ITU
audits; the total includes one doctest. The run used `--locked --offline`, LTO
disabled, and 16 codegen units. The orchestrator independently checked the
aggregate results and four zero exit statuses saved under
`target/required-config-validation/`. No external corpus audit was run for A2.

For A3, the original `a3_executor` added the initial regression and observed the
actual baseline failure in `target/channel-limit-validation/red.log`: one test
failed with exit 101 because `E_RANGE` was returned where `E_RESOURCE_LIMIT` was
expected. The `a3_recovery_executor` owns the implementation, expanded
regression matrix, and targeted checks. Its
`target/channel-limit-validation/matrix-red-final.log` records 10 test groups
failing with exit 101; some groups stopped at their first failing variant, so
that log does not claim that every row was independently observed RED. Its
`target/channel-limit-validation/focused-green-final.log` records all 10 groups
passing with zero failures and zero ignored tests, exit 0, including the
remaining variants. Python specification checks, including byte-identical
generated fixtures, are recorded in
`target/channel-limit-validation/spec-checks/summary.json` and passed. The
original `a3_executor` owns the initial regression, Python checks, and this
documentation update. Independent `a3_review` code/test review completed with
no blocking findings, and the root completed its integration inspection of the
slice. The final optimized test command was
`CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo
test --release --locked --offline`; it exited 0 with 129 result groups, 978
passed tests (977 unit/integration tests and one doctest), zero failures, three
ignored tests, and no filtered or measured tests. The ignored tests are the
full-duration EBU prescribed tones, external EBU corpus, and private ITU corpus
audits; none was run. Final formatting, diff checks, and Clippy also passed
after test-only corrections. The independent reviewer approved the final
`core_gain` expectations for zero (`E_RANGE`) and 3/255 (`E_CAPABILITY`), as
well as the `ProcessorFactory` type alias. Build and fresh installed basic and
production acceptance also passed: the optimized release build exited 0, the
fresh offline basic installation passed 38/38 checks with `ok: true` in
`target/acceptance/results.json`, and the fresh offline production `song` run
passed 148/148 checks with `status: pass` in
`target/production-acceptance/run-6xrleokz/results.json`. Root independently
confirmed the 12 specification input hashes remained unchanged. No new human
listening test was run; these are local validations rather than hosted CI. The
three ignored audits remain skipped, and full Core Audio conformance remains
unclaimed.

For A4, the recovery executor owns the test-only rendered-graph regression and
the evidence under `target/a4-sum-order-validation/`. The unchanged renderer's
focused baseline passed one test with 14 filtered and exit 0. The deliberate
sort-disabled isolated copy compiled and failed the first mono permutation with
actual frame bits `[[0], [0]]` versus expected
`[[0], [4607182418800017408]]`; zero tests passed, one failed, 14 were
filtered, exit 101. The full `dsp` target passed 15 tests with zero failures,
ignored, or filtered tests, exit 0. Formatting, diff checks, and Clippy each
exited 0, and the restored-source comparison exited 0. Stable candidate and
source hashes are recorded in the evidence files; `tests/dsp.rs` is
`74ec54dc6d6ce5d2cfe68518eda95f2cc80b78daede9aaceac4162ed0a67e2db`, while
the production `src/dsp.rs`, `src/plan.rs`, and `Cargo.lock` remained unchanged.
A4 did not rerun the full suite, release build, installed basic or production
acceptance, Python specification checks, external corpus checks, or human
listening; no hosted-CI validation is claimed. Acceptance is complete for code
and evidence: independent `a3_review` code/test review found no blockers, and
root verified the final logs and candidate/source hashes.
