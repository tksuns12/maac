# Instrument delivery and evidence

The accepted user plan is implemented against [the instrument contract](instruments.md).
This is high-risk work because it adds a source/plan contract, untrusted local
dependencies, and stateful DSP. `gpt-5.6-sol` with `high` reasoning executed
bounded packages, and separate fresh Sol Max contexts independently reviewed
the integrated code. Astra execution of this implementation is not claimed.

Correction to the original model-selection rationale: the orchestrator wrongly
assumed Astra was unavailable and cited the global policy's section 11 fallback.
That prerequisite was not established. A subsequent explicit
`gpt-6-astra` / `high` sub-agent spawn succeeded and completed a read-only task;
the local model configuration and model catalog also include Astra. The Sol
execution choice came from the orchestrator's explicit overrides, not a
demonstrated runtime restriction. Future substantive execution must use the
configured Astra High role. The test and review evidence below still records
the models and checks actually used.
The user explicitly prohibited Luna execution.
The initially launched Luna agents were interrupted before any repository edits.

## Package ownership

| Package | Primary boundary | Observable acceptance |
| --- | --- | --- |
| Imports | `bundle.rs`, `bundle_fs.rs` | Explicit pinned local source/asset dependencies resolve or fail within published bounds |
| Basic synthesis | `synth.rs` | Oscillators and sample-instant ADSRs satisfy analytic fixtures |
| Graph contracts | `graph.rs` | Typed voice/shared graphs, controls, and all edges independently validate |
| Wavetables | `wavetable.rs` | Explicit mono cycles decode and render with wrapping, morphing, and harmonic banks |
| Library exports | `library.rs` | Every export validates, with typed presets and independent instance values |
| Version 2 plans | `plan.rs`, `instrument_plan.rs` | Self-contained plans validate without source files and reject malformed payloads |
| Source lowering | `compiler.rs`, `semantic.rs` | Bundle APIs retain exact scheduling and expose only instance controls and ports |
| Voice execution | `voice.rs`, `dsp.rs` | Independent voices sum stably before shared effects, with explicit lifecycle/overflow |
| FM evidence | Spectral fixtures | Sample-wise modulation, sidebands, through-zero behavior, and residual aliasing are characterized |
| CLI | `cli.rs` | Source commands resolve bundles, library checks work, and hashing is read-only |
| Starter sounds | Example library and composition | FM bell, wavetable pad, bass, reusable presets, and multiple instances play offline |
| Delivery validation | Tests, release/install, Python smoke | Automated acceptance and legacy PCM comparisons pass on the final tree |

Write ownership is exclusive. Interfaces are agreed before concurrent work;
enum/constructor/module integration is coordinated explicitly. Initial execution
used two Sol agents; independent packages were added after healthy API and test
checkpoints. Each owner returns exact commands and outcomes. A partial handoff
or tests blocked by another package do not satisfy the final verification gate.

## Baseline

Before behavioral changes, the complete existing suite passed with
`cargo test --locked --offline` (Sol execution). The existing DSP suite also
passed independently, 14 tests.

The orchestrator captured the baseline executable's outputs with
`target/debug/maac build <source> -o <artifact> --format <format> --force`.
All four commands exited 0. Files and a machine-readable hash record are under
the ignored `target/instrument-baseline/` directory.

| Source | Format | Frames | WAV SHA-256 |
| --- | --- | ---: | --- |
| `example.maac` | float32 | 816,000 | `0fe1e92b399697f30796af7586e8c1c3b1b82a298ebd336f92d1c57c44a471dd` |
| `example.maac` | pcm16 | 816,000 | `f8e3d0017bd54c68af7c09af62fcaf8cb79869d31269b5364b99860cf4268a51` |
| `evening-window.maac` | float32 | 1,968,000 | `0c053fd2e53e0658ec356e0336a00585522297f1f9922f0b654f870c3b886924` |
| `evening-window.maac` | pcm16 | 1,968,000 | `58d0d6cb70100aa1bfb06cd1738ef4fd7550028426a028efe1978b83f71637c8` |

## Final gate

All automated gates passed on 2026-09-08. Validation used the uncommitted
working tree based on `f5563959b48d165a713cf99b9b92515dacb0c1b5`, on macOS
26.6.2 build 25G83 / Darwin 25.6.0 arm64, with Rust and Cargo 1.95.0.

| Check | Command / result |
| --- | --- |
| Formatting | `cargo fmt --all -- --check` — exit 0 |
| Clippy | `cargo clippy --all-targets --locked --offline -- -D warnings` — exit 0 |
| Rust tests | `cargo test --locked --offline` — exit 0; 212 passed, 0 failed (21 unit, 191 integration, 0 doctests) |
| Release build | `cargo build --release --locked --offline` — exit 0 |
| Diff hygiene | `git diff --check` — exit 0 |
| Documentation | 12 changed documentation files passed relative-link, code-fence, and trailing-whitespace checks |
| Installed legacy CLI | Fresh offline `cargo install` through `scripts/acceptance.py` — 38/38 checks passed |
| Instrument delivery | `scripts/instrument_acceptance.py` — exit 0; 31/31 checks passed |
| Disposable Python smoke | Python 3.12.14, Lark 1.2.2, jsonschema 4.25.1; all three generated references match tracked bytes |

The exact final delivery command was:

```sh
python3 scripts/instrument_acceptance.py --python /private/tmp/scoreir-ci-venv.VTyCKr/bin/python
```

For a fresh checkout, pass a local interpreter with `requirements-dev.txt`
installed, such as `--python .venv/bin/python`. The runner preserves virtual
environment launchers and does not depend on the recorded temporary path.
Legacy WAV comparisons explicitly check the reference macOS environment.

The reusable example has 480,000 stereo frames at 48 kHz. Direct source build,
repeat build, and rendering a retained version 2 plan after deleting its copied
sources and WAV asset produce exactly the same float32 WAV. Both export formats
are finite and nonsilent, with peak below 1. A malformed v2 plan fails without
publishing output. All four legacy WAV hashes above match the freshly installed
CLI's results exactly.

| Artifact | SHA-256 |
| --- | --- |
| Reusable source / repeat / retained-plan float32 WAV | `c969d17d0bea119d3fbe016a0e3d341b96705b663b916544a09959da3818c280` |
| Reusable PCM16 WAV | `691a8cc1bbbe9ab870de959b918e4065da608f68f7e7999599759bd22c599281` |
| Installed CLI | `9158733a7dde7f7d1ebc9f8e6001b4ead840a41f738903721e2e1d4d54dc0854` |
| Reviewed acceptance runner | `cb0fee41f670f8e71ef3a681083c2f880ac93c57e6cbb2d1bd9709e017a0068f` |

The Cargo/Rust production fingerprint is
`98537dbcde14a05892f8b92feea2d951555442b733e2c6ab6ba872afd12c7468`.
The 52-file repository-visible Cargo/source/test/script/example manifest is
`24b12b8b936033a2c66b534f71a2537d9e7d9ddb510e38f0679c0de57ae2340f`;
it includes the raw wavetable and excludes generated bytecode and documentation.
The installed run's separate 63-input before/after manifest remained
`708c5b3645cfc4a91a4eb94b8a34d323321b2c8ca8f24822ad7231499110b0d3`,
with no changed input. Only the Python interpreter helper changed after the
Rust gates; that reviewed correction passed its own regressions and final
installed acceptance, and Cargo/Rust inputs remained unchanged.

Reproducible commands, logs, and manifests are retained locally under
`target/instrument-gates/`; installed artifacts and the final machine summary
are under `target/instrument-acceptance/`. These ignored directories are local
evidence, not required source-distribution contents.

## Independent review and limits

Separate Sol Max reviews approved the source/plan contracts, DSP behavior,
filesystem capability correction, and delivery runner. Review findings were
corrected and rechecked against the final file fingerprints. The DSP approval's
installed roundtrip condition is satisfied by the exact WAV comparison above.
Grok tools were unavailable; no Grok review is claimed.

Execution was verified on macOS. Linux/Windows runtime behavior and
cross-platform bit identity remain unverified. Review identified nonblocking
coverage gaps for numerical cancellation cases that distinguish reduction
orders, less common PCM widths, combined filtered-bank/frame morphs, and a
source shared-suffix DAG depth fixture. The corresponding ordering, decoding,
interpolation, and longest-path code was inspected; existing targeted tests
and independent plan-depth tests passed.

## Integration regressions corrected

Targeted development, installed acceptance, and independent review found and
corrected the following issues. The first full 200-test run passed before the
installed roundtrip and later review findings were covered. Targeted regressions
cover these behaviors; installed and independent reproductions supplied
additional failure evidence:

| Finding | Correction and evidence |
| --- | --- |
| Public envelope automation rejected note-on/note-off controls | Source validation now permits these rates; a six-frame analytic render proves captured attack/release values and exact loaded-plan agreement |
| Empty optional library metadata differed between source and plan validation | Both accept empty creator/license and enforce the same 4,096-byte metadata boundary |
| Bundle path keys could exceed the declared bounded-input profile | Source and asset keys, authored references, and normalized paths enforce the 4,096-byte limit |
| Standalone dependency depth depended on traversal order | Memoized longest paths reject a 33-edge reversed chain and a reused deep suffix while accepting the 32-edge boundary |
| Shared LFO reset controls were rejected entirely | Static defaults/presets/instance values are accepted; source and independently supplied plan automation reject reset-rate targets |
| Saved plans changed two fractional pitches by one binary64 rounding step | `serde_json/float_roundtrip` preserves pitch bits and rendered samples through v1/v2 JSON serialization and loading |
| A pathname could change after the filesystem containment check | Pinned directory capabilities and same-handle reads reject ancestor/final symlink replacements; already-open handles retain their original file, and Unix FIFOs fail without blocking |
| Old acceptance artifacts could hide a producer that failed to write | The new runner removes exactly 17 known legacy destinations before starting the installed checks and refuses directory collisions |
| Resolving a Python executable symlink lost its virtual environment | Launcher paths now remain lexical; absolute, relative, and PATH-based venv launchers preserve `sys.prefix`, and the final disposable smoke run passes |

The JSON correction changes numeric parsing to the intended nearest binary64
value for both plan versions; it does not change processor algorithms. Before
the fix, the installed starter render and reloaded plan differed at ten float32
sample positions. That failed acceptance evidence is retained locally under
`target/instrument-acceptance-roundtrip-red/`.

The contained filesystem reader uses `cap-std 4.0.3`, plus Unix `libc` flags;
both are locked in `Cargo.lock`. The addition resolved 31 new package entries,
including platform-specific Windows/Linux support. The cap and I/O family use
Apache-2.0 with LLVM exception / Apache-2.0 / MIT alternatives; `winx` uses
Apache-2.0 with LLVM exception. Dependency metadata retains each crate's license.
The initial download populated the Cargo cache; subsequent verification uses
`--locked --offline`. Source checking, compilation, and rendering perform no
network resolution.

FM sidebands, through-zero behavior, and measured residual aliasing are recorded
in [synthesis fidelity](synthesis-fidelity.md). The strong-FM fixture measures a
folded 20 kHz component of `0.059892865450` (about −24.45 dB); harmonic-limited
carrier banks do not eliminate FM sidebands. No oversampling is claimed.

Listening review is separate and remains pending. Automated signal measurements
and successful WAV generation do not establish a human listening review.
