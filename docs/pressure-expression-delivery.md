# Pressure expression delivery evidence

This report covers the [per-note pressure contract](pressure-expression.md) on
base commit `43e4b8bdb70dde70f46446634c5e2af87c034899`. Astra Low implemented and
validated the graph source, plan payload, source/runtime propagation, example,
and behavioral tests. No dependencies or frozen instrument libraries changed.
User composition edits were preserved.

## Behavior and compatibility

The voice-only `synth.pressure/1` source provides an independent `[0,1]` signal
to authored graph mappings. Pressure, timbre, gain, and pitch may coexist on
overlapping notes. Default zero, initial values, release holding, signed ordered
modulation, final parameter bounds, and processing under zero gain or velocity
are covered. Pressure and timbre have independent graph-presence capabilities;
core and frozen-library receivers do not gain implicit mappings.

The strict optional `pressure_expression` payload is omitted when absent.
Existing wire shapes and the public runtime `note_on` signature are preserved;
Rust note literals require the new optional field. Older readers reject the
new payload or processor. All four expressions share point and work limits.
Pressure and timbre reuse scalar interpolation and unit-interval validation.

Graph and source acceptance tests first failed at the unsupported-feature
boundary. The new plan tests initially failed to compile without the public
types and field. A playback test exposed a zero pressure output before runtime
wiring. Targeted checks passed after implementation. Two obsolete diagnostic
expectations now correctly report range/unit errors for recognized pressure.

The existing independent pluck, wavetable, and filter oracles retain their
timbre-only scenarios and additionally exercise pressure alongside timbre.
They cover state continuity, overlap, mute/resume, release, reset, and retained
plans. A deliberate fixture mutation aliasing pressure curves to timbre made
all three tests fail; restoring the distinct curves passed. This demonstrates
test sensitivity, not a pre-fix production failure.

## Final validation

Environment: macOS 26.6.2 arm64, Rust/Cargo 1.95.0. Builds were offline; audio
fixtures were local and synthetic. Commands, logs, snapshots, retained plans,
and WAV files are under ignored `target/pressure-validation/`.

| Check | Observed result |
| --- | --- |
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| `cargo test --locked --offline` | 568 passed, 0 failed, 3 ignored; 630.43 seconds |
| `cargo build --release --locked --offline` | Passed |
| Fresh `cargo install --path . --locked --offline --root target/pressure-validation/install` | Passed |
| Installed pressure probes | 32 CLI invocations produced the expected outcomes |
| Existing installed acceptance checks | 38/38 passed |

The release and installed executable hashes match. Installed probes ran
outside the checkout: the 91,200-frame stereo example and 288-frame mono
fixture each matched direct builds against two source-free retained renders
in Float32 and PCM16. Headers, finite/nonzero audio, capabilities, and all four
expressions were checked. Late invalid pressure mappings failed under zero
gain and independently zero velocity, preserving existing `--force` outputs.
Four additional saved-plan renders retained WAV artifacts with matching hashes.
The acceptance wrapper changed only install/artifact paths in the execution
of the existing acceptance logic.

The preserved pre-gate snapshot contains 204 inputs. Release and final inputs
match it except for one reviewed correction to a stale pressure sentence in
the timbre guide; all 203 other inputs, including executable inputs and user
composition files, match. Six frozen library files match the base revision.
This report and its guide link were added after validation.

Two validation-harness issues were corrected with evidence retained: a label
collision overwrote the first snapshot, leaving a genuine second pre-gate
capture that was preserved and used for comparison; and the WAV probe was
extended to recognize the renderer's extensible Float32 header. Only the
affected probe was rerun. The documentation-only snapshot difference was
checked explicitly before resuming release validation. No product code changed
in response to these harness issues.

## Independent review and limits

Independent Sol Max review: **Approved with residual risk.** The reviewer
independently inspected the stable feature diff against base
`43e4b8bdb70dde70f46446634c5e2af87c034899`, including public wire and
schema boundaries, compiler expansion, exact arithmetic, graph topology,
timing and state, resource accounting, compatibility tests, and final
validation artifacts. The stale timbre-guide claim and installed-probe wording
were corrected and rechecked; no blocking findings remain. Residual risk
includes the unverified paths below and the unavailable literal first snapshot
described above; the preserved second pre-gate capture predates every
verification command.

The three ignored metering checks are the full-duration EBU loudness tone audit,
the external official EBU corpus audit, and the private ITU corpus audit.
Human listening, cross-platform bit identity, actual older-reader execution,
and worst-case capacity/performance measurement remain unverified. These
checks do not establish acoustic realism or a wall-clock speed guarantee.
