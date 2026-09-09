# Instrument pitch delivery evidence

This report covers [per-note pitch on reusable instruments](instrument-pitch.md)
against base `523d6affcbf1a4735ae6974d7f24a59380fc7f48` plus the instrument-pitch
changes. Validation ran locally on macOS 26.6.2 arm64 with Rust/Cargo 1.95.0.
User composition edits were preserved; no dependencies or frozen library bytes
changed. The work has not been committed by this delivery step.

## Implementation and behavioral evidence

Astra Low implemented the receiver, per-voice runtime, and execution accounting,
and added the oscillator and state tests. A source acceptance test failed at the
expected capability boundary before implementation; the subsequent targeted
regression run passed 80 tests. Four additional state tests passed against
independent pluck and wavetable calculations, covering overlapping bends,
mute/resume, release holding, tuning automation, frequency modulation, and
score-clock stretch with fractional physical offsets.

The Astra Low CLI executor was unavailable on two attempts. Under the repository
orchestration fallback, Sol High added three CLI tests. The final target passed
all three after correcting duplicate identifiers in test fixtures. This was a
fixture correction, not a production fix or a preimplementation RED result.
Astra Low performed the final build, install, static, suite, and installed-CLI
validation.

## Final checks

| Check | Observed result |
| --- | --- |
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| `cargo test --locked --offline` | 507 passed, 0 failed, 3 ignored; 436.16 seconds |
| `cargo build --release --locked --offline` | Passed |
| Fresh `cargo install --path . --locked --offline --root target/instrument-pitch-validation/install` | Passed |
| Installed example and custom mono probes | Passed |
| Existing acceptance checks using the fresh binary | 38/38 passed |

Commands, logs, hashes, plans, and audio are retained under ignored
`target/instrument-pitch-validation/`. The acceptance wrapper preserves the
existing check logic while redirecting its artifact and installation paths to
that directory.

The installed binary ran outside the checkout. Direct builds and two retained
plan renders after source deletion produced identical WAV bytes in Float32 and
PCM16. Headers and samples were checked: 48 kHz, finite and nonzero audio,
91,200 stereo frames for the example and 288 mono frames for the custom fixture.
An invalid expressed base returned `E_RANGE`; a later effective node-frequency
failure returned `E_NONFINITE` even under zero gain. Failed builds and retained
renders preserved existing destinations with `--force`.

Production inputs stayed unchanged through validation, and all six frozen
basic/acoustic library files match the base revision. The initial input snapshot
preceded the CLI fixture correction and a documentation resource-row correction;
it is not a claim that every input was frozen throughout the suite. The final CLI
file has SHA-256
`24fc1cb97c6d3c7682bb29674e3079477e9b06d5b509225dc61e14b40dc35dab`
and passed separate executor and independent reviewer runs as well as the suite.

## Review and limits

Independent Sol Max review reran 36 runtime/compile/plan tests and the three CLI
tests successfully. Its verdict is **Approved with residual risk**, with no
blocking findings; the unverified paths are listed below.
No callable Grok review tool was available; no Grok review is claimed.

The three ignored tests are the full-duration EBU loudness-tone audit, the
external official EBU corpus audit, and the private ITU corpus audit. This
delivery does not establish listening acceptance, cross-platform bit identity,
or worst-case execution speed. Runtime checks remain necessary for effective
node frequencies after live controls and modulation.
