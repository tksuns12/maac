# Pitch expression delivery evidence

Astra Low exercised the integrated change on Darwin arm64 with rustc 1.95.0
and cargo 1.95.0, based on `477eb90974987b8f94f9c413891733b56582c372`
plus the uncommitted pitch-expression diff. Execution and independent review
are recorded separately below.

## Observed checks

All final commands below exited 0, from the repository root:

| Command | Result |
| --- | --- |
| `cargo test --locked --offline --test pitch_expression_cli` | 2 passed; 24.53 s |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| `cargo test --locked --offline` | 452 passed, 0 failed, 3 ignored; 228.75 s |
| `cargo build --release --locked --offline` | Passed; 51.50 s |
| `cargo install --path . --locked --offline --root target/pitch-expression-validation/install` | Fresh isolated install succeeded using the release build |
| `python3 target/pitch-expression-validation/installed-proof.py` | 8 installed CLI commands succeeded |
| `python3 scripts/acceptance.py` | 38 checks passed across 24 commands, including its own offline install |

The new CLI tests exercise the checked-in example through check, compile,
direct build, and retained render after deleting source. Both float32 and
PCM16 output bytes match; headers and finite/nonzero audio are checked.
An invalid pitch curve also fails without replacing existing output even
with `--force`. These integration tests were added after compiler/DSP
implementation; no pre-implementation red result is claimed for them.

The initial format check found task-touched formatting, which was corrected.
Clippy exposed a missing `pitch_expression: None` in an export unit-test
literal and a default-field reassignment in a new compiler test. Both were
corrected within delegated scope. A temporary missing `mut` in that lint
correction was also corrected before the successful gates. No behavioral
implementation repair was performed by this validation package.

## Installed boundary

The isolated installed binary ran from a temporary directory outside the
checkout with only a copied `examples/pitch-expression.maac`. After check,
compile, and direct builds, the source was deleted. Retained render and a
second retained render were byte-identical to the direct build in each
format. All outputs contain 110,400 mono frames at 48 kHz (2.3 seconds).

| Format | Bytes | Peak | SHA-256 |
| --- | --- | --- | --- |
| float32 | 441,668 | 0.16499804 | `b95a519283e3cb3f6bc61d195f64ddc8969c9492269ce1efc3c58058526e1b42` |
| PCM16 | 220,844 | 5,406 integer units | `29bb45e8902af486a343cb02173cc063d928f96458bf5861dfaad8426a50b9bc` |

Release build/render command wall times were 0.447–0.475 seconds for this
2.3-second example. This is a local smoke measurement, with the acceptance
runner active concurrently; it is not a broad performance benchmark.

Exact installed command arguments, exits, wall times, and audio metadata are
in `target/pitch-expression-validation/installed-proof.json`; gate records
and command logs are alongside it. Installed output WAVs and the retained
plan remain there. Existing acceptance details are in
`target/acceptance/results.json`. These generated artifacts are ignored by
Git; the external temporary work directory was removed.

## Limits

The suite's three ignored tests were not run: full-duration EBU loudness
tones, the external official EBU corpus, and the private ITU corpus. They
exercise production metering outside this pitch-expression slice. No broad
performance benchmark, perceptual listening approval, or external production
mutation is claimed. Existing user edits under
`compositions/soldier-of-fortune` were preserved.

## Independent review

Sol Max approved the integrated change with residual risk and no blocking
findings. It inspected the specification, implementation diff, tests, and
installed evidence, and independently reran:

`cargo test --locked --offline --test pitch_expression_plan --test pitch_expression_compile --test pitch_expression_dsp --test pitch_expression_cli`

All 27 tests passed. Remaining gaps are worst-case performance/memory stress,
human listening, a direct test against an older executable, and combined
seconds/score-clock CLI scenarios beyond the separate compiler/DSP coverage.
Older-reader rejection is supported by the previous strict unknown-field
deserializer; downstream Rust note literals require the documented new field.

The reviewer's additional Grok MCP check was unavailable because no matching
tool/server was exposed. No Grok review is claimed; the independent Sol Max
review and local verification above were completed.
