# Gain expression delivery evidence

This report covers the `core.sine/1` per-note gain slice described in
[gain expression](gain-expression.md), including coexistence with pitch,
source compilation, retained plans, rendering, and local CLI installation.
It does not claim a full production metering audit or human listening approval.

The tested checkout is base `47aa8e6ae308bfb145c2dea819bc8c1e117ce081`
plus the uncommitted gain-expression diff. Unrelated user composition edits
were preserved. Environment: macOS 26.6.2 (25G83), Rust 1.95.0
(`59807616e`), Cargo 1.95.0 (`f2d3ce0bd`). No dependencies or network access
were added for this verification.

## Evidence

Astra Low executed the checks. Detailed local logs, command arguments,
exit statuses, elapsed times, retained plans, and WAV artifacts are under
ignored `target/gain-expression-validation/`.

- `cargo test --locked --offline --test gain_expression_cli`: 2 passed,
  0 failed, 0 ignored (42.75 seconds). Tests were added after implementation;
  no CLI RED run is claimed.
- The CLI tests exercise the checked-in example and a seconds-to-score
  variant, together covering all three gain clocks with concurrent pitch.
  After direct Float32 and PCM16 builds, source deletion precedes retained
  plan rendering. Both encodings are byte-identical, mono, 48 kHz,
  81,600 frames, finite and nonzero.
- Negative gain, an exponential zero endpoint, and positive gain causing
  PCM16 overload fail with `--force` while preserving an existing output.

The implementation agents reported these behavioral RED runs before their
changes: `cargo test --test gain_expression_plan` failed
`gain_payload_roundtrips` with `E_UNKNOWN_FIELD gain_expression`;
`cargo test --test gain_expression_compile` failed `normalized_gain_compiles`
with the expected unsupported-capability error; and
`cargo test --test gain_expression_dsp` failed the manual-plan analytic case
with `RenderState("per-note gain expression playback is not implemented")`.
Their respective relevant baselines passed 36, 27, and 21 tests. These are
agent handoff observations, separate from this delivery executor's logs.

## Final checks and delivery

| Command | Observed result |
| --- | --- |
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| `cargo test --locked --offline` | 485 passed, 0 failed, 3 ignored; 279.50 s |
| `cargo build --release --locked --offline` | Passed; 52.01 s |
| `cargo install --path . --locked --offline --root target/gain-expression-validation/install` | Passed; fresh installation root |
| `python3 target/gain-expression-validation/installed_probe.py` | Passed; 16 installed CLI commands |
| `python3 scripts/acceptance.py` | Passed; 38/38 legacy acceptance checks |

The three expected production-metering ignores are
`ebu_3341_prescribed_loudness_tones_1_through_5` (full-duration release run),
`ebu_official_corpus_audit` (external official corpus), and
`itu_official_loudness_corpus_audit` (private official corpus). They remain
outside this gain feature's validation; no metering-audit completion is claimed.

The installed CLI ran from an isolated `/private/tmp` directory outside the
checkout. Both the checked-in example and score-clock variant passed check,
compile, direct build, source deletion, retained render, and repeated render.
All corresponding WAV bytes and SHA-256 hashes matched for Float32 and PCM16.
Headers matched the 81,600-frame, mono, 48 kHz contract; all samples were finite
and each render contained nonzero audio. Retained plans and WAVs remain in the
ignored evidence directory.
The temporary execution directory was removed. `installed-probe.json` records
arguments, exit statuses, timings, headers, and hashes; `revision.json` records
source/test/build-input hashes. `legacy-acceptance-results.json` preserves the
existing acceptance script's result; its detailed artifacts remain under
`target/acceptance/`.

The release example direct builds took approximately 0.45 seconds (Float32)
and 0.44 seconds (PCM16) for 1.7 seconds of audio on this local machine.
These are small-fixture elapsed measurements, not a stress or throughput claim.

## Independent review

Sol Max approved the current gain-expression source and test revision with
residual risk and no blocking findings. It verified the recorded source/test
SHA-256 hashes against the current files and independently reran 28 gain
plan/compiler/DSP/CLI tests, 4 internal math tests, and 25 existing pitch
regression tests. All 57 passed.

The review confirmed the combined point cap is checked before expanded curve
payloads are cloned. Tighter object/work limits are applied when the resulting
plan is validated; maximum-capacity memory and runtime stress remains unmeasured.
Human listening and an actual older-binary rejection test also remain unverified.
The existing three production-metering ignores are identified above.

The additional Grok MCP check was unavailable because no corresponding tool
or server was exposed. No Grok review is claimed; independent Sol Max review,
test reruns, and installed-artifact verification were completed.
