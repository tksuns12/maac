# Built-in basic instruments: delivery evidence

This report records local validation on 2026-09-08 for the working tree based on
`a49895dbfe86fce90c8161b986e68a4b85b08ba8`. It covers 24 synthesized instruments,
`std/basic/1.0.0` offline imports, catalog discovery, and 25 standalone examples.
The [instrument guide](basic-instruments.md) describes the musical controls and
usage. From this checkout, `cargo run -- instruments` lists the catalog without
changing a globally installed `maac`; acceptance used `target/install/bin/maac`.

Astra High packages implemented and validated the instrument families, seeded
noise, one-pole high-pass filtering, built-in resolution, catalog API/CLI,
examples, and behavioral tests. Astra Max integrated the packages. A separate
Astra High agent reviewed the core changes read-only and reported no blocking
findings; that review is distinct from the executed checks below. Work followed
the supplied AGENTS.md instructions; its linked orchestration, verification, and
GUI policy files were absent from the checkout.

## Source and executable identity

The frozen [library source](../stdlib/basic/1.0.0.maac) has SHA-256:

```text
cc7d6e307ed28810f75cdd5c8aa1ed24b08d7db94f78d88cc293d0ceb7ba397b
```

The [100-file source-input manifest](../target/basic-acceptance/final-gates/source-manifest-before.json)
covers Rust sources, library source/editorial metadata, examples, tests,
acceptance scripts, and specification-checker inputs. Its file SHA-256 is
`eefac6c55917dc44fae0e2a7617d7295af5eee117becf78c72597a42169671d8`.
All recorded source-input hashes remained unchanged through the quality gates
and installed acceptance. The evidence report itself is not an executable input.

The freshly installed executable had the same SHA-256 before and after every
new installed CLI invocation and at the end of the run:

```text
251698a666b34ab3fe86e214e2413f89cf36e25e8b3b62129b11bf50c26b3ba1
```

## Executed gates

Environment: macOS 26.6.2, build 25G83, arm64; Rust/Cargo 1.95.0; acceptance
Python 3.14.4. Disposable specification smoke used the existing Python 3.12.14
interpreter with Lark 1.2.2 and jsonschema 4.25.1.

| Command | Observed result |
| --- | --- |
| `cargo fmt --all -- --check` | Exit 0 |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Exit 0 |
| `cargo test --locked --offline` | Exit 0; 255 tests passed, 0 failed/ignored; 0 doctests |
| `cargo build --release --locked --offline` | Exit 0 |
| `git diff --check` | Exit 0 |
| `python3 scripts/basic_instrument_acceptance.py --python /private/tmp/scoreir-ci-venv.VTyCKr/bin/python` | Exit 0; all installed checks passed |

Exact arguments, exit codes, durations, and stdout/stderr are retained in the
[quality-gate logs](../target/basic-acceptance/final-gates/20260908T051551Z/results.json)
and [installed acceptance results](../target/basic-acceptance/runs/20260908T051823Z-502f54a1/results.json).
Changed Markdown files, including this report, also passed the disposable
[relative-link, fence, and trailing-whitespace check](../target/basic-acceptance/final-gates/docs-results.json).

The full suite includes six basic-instrument matrix tests and two example tests.
The matrix covers 777 instrument/pitch cases at velocities 0, 0.35, and 1
(1,342,656 stereo frames), plus 609,600 unique stereo frames across all 24
instruments rendered four ways with exact f64-bit comparisons. Other assertions
cover common controls, fixed drum tuning, release/capacity behavior, independent
instance state, and reset. The two example tests cover all 25 examples and PCM16
export. These are numerical and behavioral assertions, not listening judgments.

## Fresh installed acceptance and auditions

The new runner first executed `scripts/instrument_acceptance.py`, which completed
its fresh offline Cargo installation, legacy acceptance (38/38 checks), reusable
instrument acceptance (31/31 checks), and disposable specification smoke with
byte-identical generated reference files. The four historical legacy WAV hashes
also matched in their recorded Darwin arm64 environment.

The new run recorded 182 commands and 753 passing checks, including executable
stability checks. From a fresh temporary directory outside this checkout, it:

- Retrieved the 24-entry catalog and every detail response, checked the frozen
  source hash and four common controls, and validated every generated usage example.
- Rejected unknown instruments, unknown library versions, and mixed import forms
  with explicit nonzero JSON errors; failed compile/build attempts created no output.
- Built every checked-in example to float32 and PCM16, repeated float32 builds
  with identical bytes, compiled version 2 plans, removed the copied compositions,
  and rendered retained plans with identical float32 bytes.
- Verified embedded graph programs, source/dependency identities, nonzero u32
  noise seeds, and absence of sample assets.

The [50 audition WAVs](../target/basic-acceptance/auditions/20260908T051823Z-502f54a1/)
contain 24 individual instruments plus the full band, each in both formats.
Each format contains 4,848,000 stereo frames in total at 48 kHz. Every output was
finite, non-silent, and below full scale. Across the examples, float32 peaks were
0.072843477–0.260073125; normalized PCM16 peaks were 0.072845459–0.260070801.
The 9.8-second [full-band float32 audition](../target/basic-acceptance/auditions/20260908T051823Z-502f54a1/full_band.float32.wav)
had peak 0.260073125 (the targeted f64 measurement was approximately 0.260073123).

Logs, manifests, plans, and WAVs under `target/` are ignored, reproducible local
evidence rather than checked-in release contents. The
[acceptance runner](../scripts/basic_instrument_acceptance.py) namespaces each
fresh run so old artifacts cannot satisfy a new check; the
[latest results](../target/basic-acceptance/results.json) point to this run.

## Limits

Listening review was **not performed**. Finite samples, peak measurements,
deterministic bytes, and generated WAVs do not establish instrument-family
recognizability, perceptual quality, or absence of audible clicks. Linux/Windows
execution and cross-platform bit identity remain unverified. No hosted CI,
release publication, or global executable installation is claimed.
