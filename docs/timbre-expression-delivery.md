# Timbre expression delivery evidence

This report covers the [per-note timbre contract](timbre-expression.md) on top of
base commit `75ac6b99bc1cf47ee329e4d6388728a3e377dcf4`. Astra Low implemented the
graph source, strict plan payload, source/runtime propagation, examples, and
behavioral tests, and performed the final validation. No dependencies or frozen
instrument libraries changed. User composition edits were preserved.

## Behavior and compatibility

The new voice-only `synth.timbre/1` source supplies each note's independent
`[0,1]` curve to existing graph modulation. Explicit mappings combine with
controls and other modulation before target range checks. Instruments opt in by
declaring the source; existing frozen libraries and `core.sine/1` reject timbre.
Default zero, initial values, gate-end holding, and simultaneous pitch/gain/timbre
are covered. The public runtime note-on signature remains unchanged; Rust note
literals require the new optional field, while absent fields stay omitted in JSON.

Graph and source acceptance tests failed at their expected unsupported-feature
boundaries before implementation. The plan payload's initial test could not
compile before its types and field existed. Later focused checks passed. Two
obsolete tests were updated to expect range/unit errors for newly recognized
timbre rather than an unsupported-kind error.

Independent numerical tests cover filter cutoff with controls and LFOs,
signed pluck damping with retained string history, and two-frame wavetable
morphing with retained phase. They exercise overlap, pitch/gain coexistence,
mute/resume, initial values, release holding, retained plans, and reset replay.
Plan tests cover strict wire data, exact bounds and knots, malformed rationals,
tiny positive exponential endpoints, receiver opt-in, and resource accounting.

## Final validation

Environment: macOS 26.6.2 arm64, Rust/Cargo 1.95.0. All checks used local synthetic
audio and offline builds. Exact commands, logs, hashes, plans, and audio are
retained under ignored `target/timbre-validation/`.

| Check | Observed result |
| --- | --- |
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| `cargo test --locked --offline` | 537 passed, 0 failed, 3 ignored; 560.76 seconds |
| `cargo build --release --locked --offline` | Passed |
| Fresh `cargo install --path . --locked --offline --root target/timbre-validation/install` | Passed |
| Installed CLI probes | 26 invocations passed |
| Existing installed acceptance checks | 38/38 passed |

All 187 snapshotted inputs matched before and after validation, and all six
frozen basic/acoustic library files matched the base revision. The release and
installed executables had the same SHA-256. This report and its guide link were
added afterward as non-executable delivery documentation.

Installed probes ran outside the checkout. Both the 91,200-frame stereo example
and a 288-frame mono fixture passed direct builds and two source-free retained
renders in Float32 and PCM16, with byte-identical output for each format.
Headers, finite/nonzero samples, and retained expressions were checked.
Invalid values returned `E_RANGE`; unsupported receivers returned `E_CAPABILITY`.
A late cutoff failure under zero gain returned `E_NONFINITE`. Failed builds and
source-free renders preserved existing destinations with `--force`.

The acceptance wrapper used the existing check logic with its install and
artifact paths redirected to this isolated validation directory. The original
acceptance script was not edited.

## Independent review and unverified paths

Sol Max independently reviewed the implementation and evidence and reran 26
timbre tests plus 21 gain regressions successfully. Verdict: **approved**. No
blocking findings were identified in the frozen implementation, tests,
documentation, or validation evidence. A fresh reviewer thread could not be
created because of the agent thread limit, so an existing independent Sol Max
reviewer handled this feature. No callable Grok review tool was available;
no Grok result is claimed.

The three ignored production-analysis tests are the full-duration EBU loudness
tone audit, the external official EBU corpus audit, and the private ITU corpus
audit. Human listening, cross-platform bit identity, actual older-binary
execution, and worst-case capacity/performance measurement remain unverified.
These checks do not establish acoustic realism or a wall-clock speed guarantee.
