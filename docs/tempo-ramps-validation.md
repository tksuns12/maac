# Tempo ramp validation

Validation was performed on 2026-09-09 on macOS 26.6.2 ARM64 with Rust/Cargo
1.95.0. The working tree is based on `e447239f18cb9d24f075d14a8253fe6648cf613c`;
the tempo changes are uncommitted. A frozen manifest of 109 Rust source, test,
and Cargo files identifies the tested code under
`target/tempo-validation/integration-inputs.json`.

The [tempo contract](tempo-ramps.md) defines the behavior. The new
[example](../examples/tempo-ramps.maac) has eight notes, acceleration,
deceleration, score-clock automation, and 208,158 output frames including tail.

## Checks

| Boundary | Observed result |
| --- | --- |
| Formatting and Clippy | `cargo fmt --all -- --check` and `cargo clippy --locked --offline --all-targets -- -D warnings` passed |
| Full Rust suite | `cargo test --locked --offline`: 627 passed, 0 failed; the 3 opt-in audits passed separately below |
| Opt-in metering audits | Release-mode `production_analysis -- --ignored --nocapture`: 3 passed, 0 failed, 0 ignored |
| Release and installation | Locked, offline release build and fresh isolated installation passed |
| Standard installed CLI | 38 checks passed |
| Installed ramp workflows | 69 checks passed across 16 commands |
| Installed production workflows | 148 checks passed |
| Previous executable comparison | Two step-plan JSON files and six Float32/PCM16 WAV files matched byte for byte |
| Specification smoke | Syntax/schema checks, 48 expanded notes, and 22 arithmetic assertions passed in a disposable copy |
| Production smoke | 8 valid documents, 77 invalid documents, schema-reference rejection, and 60 arithmetic cases passed |
| Resampler coefficients | Independent coefficient verification passed; tables and algorithm were unchanged |

The installed ramp probe removes copied source files before retained renders,
and removes both source and the descriptor schema before retained deliveries.
Float32/PCM16 builds match their retained renders. Master/stem deliveries at
44.1, 48, and 96 kHz match retained WAV bytes and render keys. The independent
duration fixture is `2*ln(2) + 1/100` seconds, with ceilings of 61,577, 67,023,
and 134,045 frames respectively. Existing destinations remain protected.
Production acceptance also verifies selection, shared graph context, and
completed audio retention when requested loudness checks fail.

The comparison executable was preserved from an earlier local installation;
its exact source revision is unverified. Its SHA-256 is
`5588eb36226ab8668f6fc89a26f7794fc2dbb31d5b2b48eb203e5de8cfde5990`.
The fresh executable SHA-256 is
`d36dba2a3a6123ce60b98e20064e03f5e4569a6cc91a73f63e6e05e11e92e191`.
The older executable rejects version 3 with `E_SYNTAX` for the absent legacy
`on_seconds` field and creates no WAV; it does not reinterpret the plan.

Commands, exits, snapshots, and artifacts are retained locally under
`target/tempo-validation/`; the final full-suite, static, release, and install
logs use the `integration-complete-` prefix. The installed probe's first two attempts exposed
fixture issues: a missing `sha256:` prefix and an overly specific expectation
of `E_VERSION` from the older executable. Those attempts were preserved, the
probe was corrected to the documented contracts, and all 69 checks passed.
No product-code changes were made for those fixture corrections.

After quota became available, Astra Low explicitly ran all three opt-in audits
with `cargo test --locked --offline --release --lib production_analysis --
--ignored --nocapture`. The existing EBU and ITU corpus paths were supplied
through their documented environment variables. Archive hashes matched the
recorded provenance, and extracted WAV bytes matched the archives. All five
prescribed loudness cases, seven applicable EBU loudness cases, 27 EBU true-peak
measurements across the three rate selections, and 19 ITU loudness files passed.
All 109 frozen source hashes matched before and after execution. Commands,
exit status, corpus verification, and per-case reports are retained under
`target/tempo-validation/remaining-metering/`.

## Review and execution

Astra Low implemented and tested the initial timing, shared validation, plan,
compiler, and converter work. After its usage limit prevented further execution,
the orchestration policy's section 11 fallback was used: Sol executors and the
main agent completed implementation, integration, and validation. The final
main-suite and installed checks above are fallback execution evidence, not a
claim of Astra Low execution. Astra Low performed the subsequent opt-in metering
audits after quota recovery.

Independent Sol Max timing review approved the corrected numerical and budget
implementation and reran 74 targeted/legacy tests. Independent Sol Max
integration review found no remaining technical blockers in the final snapshot,
reran 125 targeted integration tests, and verified all 109 frozen file hashes.
Review corrections cover numerical-budget resets
across nested compilation, renderer preparation, and export; interior
score-clock knots incorrectly treated as seconds; production aggregate
preflight occurring after numerical work; and preparation state removing
`DspEngine`'s public `Sync` guarantee. Failing regressions were observed before
the corresponding fixes. Tests now verify shared compilation/preparation
budgets, empty WAV sinks after preparation failure, ramp-derived interior knot
frames, production aggregate rejection before timing work, and `Send + Sync`
compatibility. Preparation-only timing state is dropped before returning the
renderer. Production attachment retains combined plan/production resource
bounds without restarting numerical timing.

## Limits of this evidence

All three opt-in metering audits were rerun successfully on this snapshot; no
automated test remains skipped. The analyzer and SRC algorithms are unchanged.
Metering profile scope and historical evidence are recorded
[separately](production-metering-evidence.md). Numerical certification remains
bounded to 1024 bits and can explicitly return `E_TIME_PRECISION` for unresolved
cases, including difficult logarithmic identities. Testing and review do not
establish an exhaustive arbitrary-precision oracle or cross-platform byte identity.
Human listening acceptance awaits the user's assessment of the rendered
tempo-ramp example; the audio was supplied for that assessment. Automated
measurement is not a human listening result.

An existing limitation remains: production execution-identity normalization
rejects nested per-note `expression` objects. The legacy compiler reproduces
that rejection as well. Ramp tests separately exercise instrument expressions
and production delivery; this unrelated limitation was left unchanged.
