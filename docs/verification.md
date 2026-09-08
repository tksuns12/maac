# MaaC release verification evidence

This report records automated evidence for the MaaC (`maac`) source-only release
preparation. MaaC/1 is the source language; MaaC source files use `.maac` and
require the canonical `maac 1;` document header. The parser rejects
noncanonical language headers.
The separate
[release-readiness review](release-readiness.md) records publication decisions,
audit limits, and remaining release work.

## Current naming cleanup

The current tree uses MaaC consistently in the specification, grammar, schema,
and documentation. The normative [MaaC-1 specification](../MaaC-1-Specification.md)
defines the language; its deterministic noise namespace is `maac-noise-1`, and
its package lock example is `maac.lock.json`.

The documentation and metadata scan passed: no case-insensitive deprecated
product-name text or filename remains in those paths.

A pinned Python 3.12.14 environment ran `check_spec.py` from a disposable copy.
The current run reported `surface_parse: pass`, `syntax_tree_schema: pass`, 48
expanded example notes, and 22 arithmetic assertions. All three generated files
matched the current tracked files byte-for-byte:

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `check-results.json` | 228 | `f58aa82db335ac4a16023b7ef2b2610751cf54452158c67e862627f969da3e54` |
| `conformance.json` | 3,621 | `94dc043060464dd6e4dbb1796009252bd343a06a4c9f5b5d096ea6c1cbeeddd4` |
| `example.syntax.json` | 21,600 | `358e59e4a9352e4d25b1e1e99b6a3fc799a861e322d5510e8833104fad455ca8` |

The current implementation checks also passed:

| Check | Command | Result |
| --- | --- | --- |
| Formatting | `cargo fmt --all -- --check` | Passed |
| Clippy | `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| Tests and doctests | `cargo test --locked --offline` | 102 unit/integration tests passed; 0 failed; 0 doctests |
| Release build | `cargo build --release --locked --offline` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | 38 of 38 checks passed |

The sections below are historical evidence from an earlier validation snapshot.
Their commit, manifest, test, artifact, and generated-fixture hashes remain
exactly as observed and do not assert results for the current tree.

## Historical validation identity and scope

The final disposable snapshot used the working tree based on commit
`870e311f37e69ca80525b5ba97fe707036063aef`.

The snapshot was copied with `rsync -a`, excluding `.git`, `target`,
`graphify-out`, and `.DS_Store`. The runner sorted all POSIX-relative files and
wrote one SHA-256 digest per file. The 53-file manifest excludes
`docs/verification.md` and `docs/release-readiness.md` so the evidence reports
do not hash themselves. Its digest is
`ffded29068ccc5084408ba975aa12a209d5fa2b97751af47dc9d948dd7280d91`.
The tracked working-tree diff digest captured with that snapshot was
`4db3265df8ae724dc623a519b1fda7aa24a418478f3c44db2c544b4ca70469af`;
untracked release files are represented by the content manifest.

The full generated manifest is retained locally at
`target/header-verification/snapshot-source-manifest.sha256`. The manifest
includes the MaaC package/library/binary rename, the `.maac` example extension
rename, the canonical `maac 1;` header and `maac` code-fence updates, and the renamed acceptance runner,
the release additions `.github/workflows/ci.yml`, `CHANGELOG.md`,
`CONTRIBUTING.md`, `LICENSE`, `NOTICE`, `SECURITY.md`, and
`requirements-dev.txt`, and the final documentation wording. The ignored
manifest and logs are local evidence, not release contents.

## Environment

The macOS runner observed macOS 26.6.2 (build 25G83), stable
`aarch64-apple-darwin`, Rust 1.95.0, and Cargo 1.95.0. The installed-CLI
runner used Python 3.14.4 and only the Python standard library. The pinned
smoke checker used Python 3.12.14 with the versions in `requirements-dev.txt`.

## Historical Rust gate

The release-validation agent ran these commands after the package, library,
binary, and test references were renamed to MaaC:

| Check | Command | Result |
| --- | --- | --- |
| Formatting | `cargo fmt --all -- --check` | Passed |
| Clippy | `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed (`maac v0.1.0`) |
| Tests and doctests | `cargo test --locked --offline` | 101 unit/integration tests passed; 0 failed; 0 doctests (`Doc-tests maac`) |
| Release build | `CARGO_NET_OFFLINE=true cargo build --release --locked` in the clean snapshot | Passed (`maac v0.1.0`) |

The 101 tests comprise 16 library, 13 adversarial-plan, 3 CLI, 11 compiler,
14 DSP, 6 export, 10 foundation, 10 music, 11 plan-validation, and 7
semantic tests. The library tests include canonical `maac 1;` acceptance and
intentional rejection of a noncanonical language header. The final snapshot's README build supplied release-build
evidence after the extension rename; no second standalone release build was
run.

## Historical installed MaaC CLI acceptance

The renamed `python3 scripts/acceptance.py` performed an offline installation
and ran `target/install/bin/maac`. All 38 of 38 checks passed, covering both
original compositions, the tutorial source fence, check/compile/render/build,
repeat renders, PCM16 export, the library music API, overwrite protection, and
failure-safe destination behavior.

The fresh canonical-header source hashes are:

```text
example.maac
3e336b05bc1099112c1107c8aa506e2ab2e585012e12ce3415d15383565fe2ae

evening-window.maac
9a72ae3503f2bb3410963db71964240ae89c2dd5ce24a8a19d7ca8445b8a745b
```

The fresh measurements were:

| Artifact | Notes | Stereo frames | Duration | Float32 peak | Nonzero samples |
| --- | ---: | ---: | ---: | ---: | ---: |
| `example.wav` | 48 | 816,000 | 17 s | 0.25385034 | 1,544,396 |
| `evening-window.wav` | 182 | 1,968,000 | 41 s | 0.37564763 | 3,864,958 |
| `tutorial.wav` | 3 | 144,000 | 3 s | 0.16918710 | 199,678 |

All float32 samples were finite and non-silent. Float32 render, source build,
and repeat render bytes matched for both original compositions. PCM16 builds
were valid, and the WAV output hashes remained unchanged after the header
rename. Observed output SHA-256 values were:

```text
example.wav               0fe1e92b399697f30796af7586e8c1c3b1b82a298ebd336f92d1c57c44a471dd
evening-window.wav        0c053fd2e53e0658ec356e0336a00585522297f1f9922f0b654f870c3b886924
example.pcm16.wav         f8e3d0017bd54c68af7c09af62fcaf8cb79869d31269b5364b99860cf4268a51
evening-window.pcm16.wav  58d0d6cb70100aa1bfb06cd1738ef4fd7550028426a028efe1978b83f71637c8
```

## Historical pinned Python smoke checker

The pinned Python 3.12.14 environment passed `pip check` and contained
`lark 1.2.2`, `jsonschema 4.25.1`, `attrs 25.4.0`,
`jsonschema-specifications 2025.9.1`, `referencing 0.36.2`, `rpds-py 0.27.1`,
and `typing-extensions 4.15.0`.

The checker ran from a disposable directory in the final source snapshot. Each
generated fixture matched its snapshot copy byte-for-byte:

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `check-results.json` | 228 | `f58aa82db335ac4a16023b7ef2b2610751cf54452158c67e862627f969da3e54` |
| `conformance.json` | 3,619 | `af3651fc5c10ccb86cafcfa3a44b1090d53d78e45424f394ddfec8329b05175b` |
| `example.syntax.json` | 21,600 | `358e59e4a9352e4d25b1e1e99b6a3fc799a861e322d5510e8833104fad455ca8` |

The checker covers syntax plus selected semantics only. It is not a complete
semantic validator, renderer, or proof of specification conformance.

## Historical README quick-start snapshot

From a clean final source snapshot, the README build and install commands were
run with a snapshot-local Cargo install root. The install directory was
prepended to `PATH`, and `command -v maac` resolved to that installed binary.
`maac check example.maac`, `maac compile example.maac -o
example.performance.json`, and `maac render example.performance.json -o
example.wav` all exited successfully, reporting 48 notes and 816,000 frames.
The rendered WAV was 6,528,068 bytes with SHA-256
`0fe1e92b399697f30796af7586e8c1c3b1b82a298ebd336f92d1c57c44a471dd`.

## Listening review

Pending. No human listening review is claimed. Creating WAV files, measuring
samples, and starting playback do not establish that someone listened to the
compositions.

## Skipped or pending evidence

This preparation does not claim a hosted GitHub CI result, a cross-platform
run, a human listening review, a release tag, or publication to a repository
whose destination has not been selected. Publication decisions and audit
limitations are tracked in [release-readiness.md](release-readiness.md).
