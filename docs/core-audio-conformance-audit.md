# Core Audio conformance audit

Audit date: 2026-09-10. Implementation baseline:
`6844d3f6e21b40635282bba61bddbe47a697d832`.

## Conclusion

The implementation has a path for every reference processor identifier in
MaaC/1 §18 within its declared capability and resource limits. The audit
confirmed a Sine arithmetic mismatch; the A1 follow-up below records its fix.
Other findings remain open. Processor coverage does **not** establish
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
| Compact source normalization and revision identity (§20) | Partial implementation evidence | [syntax.rs](../src/syntax.rs) preserves tagged syntax; [production_identity.rs](../src/production_identity.rs) normalizes execution identity for supported compiled documents. This does not establish a general revision-hash/normalization interface for all core documents. |
| Atomic editing (§21) | Missing public interface | No implementation of the five-operation transaction protocol, base-revision preconditions, inverse patches, or validation-before-commit was found. [capabilities.md](capabilities.md) explicitly defers transactional editing. |
| Message performance (§§6.1, 8) | Explicitly deferred | Source message validation exists in [semantic.rs](../src/semantic.rs), but [compiler.rs](../src/compiler.rs) rejects message execution with a capability diagnostic. General Performance resolution is not established by the note/hit path. |
| Seeking with prehistory (§28 vector 17) | Full-implementation evidence absent | [dsp.rs](../src/dsp.rs) renders from reset through the full interval. No public seek/window API or seek-versus-full-render-crop test was found. Spool seeking during delivery is not DSP-state restoration. A future seek interface must retain the required prehistory; this audit does not invent a new seek wire format. |
| Full dependency locks (§22) | Separate Locked Render work | Production execution/render identity and pinned assets are not a complete generic `maac.lock.json` verifier. This is not a missing §18 processor. |
| External processors and adapters (§§17, 25) | Deferred/unadvertised interfaces | No general external ABI host or MIDI/notation/DAW adapter was found. Do not advertise fidelity or loss-report guarantees for nonexistent adapters. Unsupported extension execution is explicitly rejected. |
| Diagnostics/resources/package boundaries (§§23–24) | Existing bounded evidence; not exhaustively audited | [diagnostic.rs](../src/diagnostic.rs), [bundle.rs](../src/bundle.rs), [bundle_fs.rs](../src/bundle_fs.rs), and [bundle tests](../tests/bundle.rs) cover relevant validation and containment paths. Finding A3 records one diagnostic inconsistency. |

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

### A3 — caller channel-limit diagnostics need consistency

Several [plan.rs](../src/plan.rs) checks report `E_RANGE` when a channel count
exceeds the caller's `max_channels` limit. §23 specifies `E_RESOURCE_LIMIT`
for a declared host limit. Keep this distinct from invalid authored dimensions
and from unavailable processor capabilities. This finding is based on code
inspection; a diagnostic-matrix regression should accompany a future fix.

### A4 — rendered sum-order evidence can be stronger

The cancellation-sensitive sum test exercises the `sum_samples` helper.
Connection-ID sorting is separately implemented in the renderer. A complete
rendered graph with cancellation-sensitive values and reordered connection
declarations would check that integration boundary directly. No wrong sum
output was observed in this audit; this is an evidence gap.

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

## Recommended next implementation slice

Address **A2**, with omitted/empty/valid configuration cases across all required
reference-processor configs. A3 and A4 are separate, bounded follow-ups. The A1
fix does not change those validation or evidence findings.

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
