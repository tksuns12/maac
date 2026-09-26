# MaaC release verification evidence

The [instrument delivery evidence](instrument-delivery.md) records verification
for the reusable sound-library milestone. The naming and release-preparation
sections below are dated snapshots. Follow-up sections distinguish fresh Rust
checks from older release-preparation evidence.

This report records automated evidence for the MaaC (`maac`) source-only release
preparation. MaaC/1 is the source language; MaaC source files use `.maac` and
require the canonical `maac 1;` document header. The parser rejects
noncanonical language headers.
The separate
[release-readiness review](release-readiness.md) records publication decisions,
audit limits, and remaining release work.

## Release-preparation snapshot (2026-09-21)

The following checks were run against the working tree available on September
21, 2026, including the bounded generic-lock construction slice. They are local
evidence, not hosted-CI or human-listening evidence.

| Check | Command | Result |
| --- | --- | --- |
| Formatting | `cargo fmt --all -- --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Tests and doctests | `cargo test` | Passed: 243 library tests, all integration targets, and 1 doctest; 3 existing tests ignored |
| Release build | `cargo build --release` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | 38 of 38 checks passed |
| Production coefficient tables | `python3 scripts/production_src_coefficients.py --verify` | Passed |
| Python production checker | `python3 check_production.py` | Unavailable in this environment: `lark` and `jsonschema` are not installed |

The Python production checker has historical passing evidence in the integration
check table in [production-delivery evidence](production-delivery.md); its
dependency absence here is an environment gap, not a product failure. The
working tree recorded at that snapshot contained uncommitted implementation and
documentation changes.

## Follow-up Rust verification before RIFF review fixes (2026-09-23)

These Rust checks were rerun after the initial bounded WAV import slice. They
predate the later RIFF review fixes and do not refresh the September 21 release
build, installed-CLI acceptance, or Python-checker evidence above.

| Check | Command | Result |
| --- | --- | --- |
| Formatting | `cargo fmt --all -- --check` | Passed |
| Tests and doctests | `cargo test` | Passed: 243 library tests, all integration targets, and 1 doctest; 3 existing tests ignored |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |

## Post-RIFF-review verification (2026-09-23)

The baseline committed at `2e08985` includes the single-pass RIFF importer and
duplicate-chunk regressions. The full `cargo test` run was interrupted after
its completed targets passed; this section claims the focused gate only.

| Check | Command | Result |
| --- | --- | --- |
| Formatting | `cargo fmt --all -- --check` | Passed |
| Focused Rust tests | `cargo test --lib --test media_import --test media_import_cli --test generic_lock_generation --test audio_storage` | Passed: 248 library tests, 3 existing ignored; 12 media-import, 1 import-CLI, 9 generic-lock-generation, and 1 audio-storage test |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Release build | `cargo build --release` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | 38 of 38 checks passed |
| Production coefficient tables | `python3 scripts/production_src_coefficients.py --verify` | Passed |
| Python production checker | `python3 check_production.py` | Unavailable: `lark` and `jsonschema` are not installed |

## Retained-original import verification (2026-09-23)

These checks cover the opt-in `--retain-original` package, `verify-import`, and
the pinned project-root resolver. The complete Rust integration suite was not
rerun for this slice.

| Check | Command | Result |
| --- | --- | --- |
| Library tests | `cargo test --lib` | Passed: 253 tests; 3 existing ignored |
| Focused integration | `cargo test --test bundle --test cli_project_entrypoint --test media_import --test media_import_cli` | Passed: 17 bundle, 8 project-entrypoint, 12 importer, and 6 import-CLI tests |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Release build | `cargo build --release` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | 38 of 38 checks passed |
| Release CLI smoke | Retained import, remove input, relocate project, `verify-import`, `check`, and `build` | Passed |

An independent review reproduced a root-replacement false-success case before
the pinned-root fix. After the fix, both root-replacement probes failed with
`E_REFERENCE`; the deterministic regression and FIFO/symlink compatibility
tests passed. The verifier's absolute-symlink alias limit is documented in
[media import](media-import.md).

## Opt-in native PCM disk media (2026-09-23)

This gate covers source `check` and `build --disk-media` with hash-pinned native
PCM snapshots beyond the embedded 4 MiB asset limit. It does not claim a new
standalone plan format or full Phase 3 completion.

| Check | Command | Result |
| --- | --- | --- |
| Focused Rust tests | `cargo test --lib --test disk_media_cli --test audio_storage --test bundle` | Passed: 257 library tests, 3 existing ignored; 3 disk-media CLI, 1 audio-storage, and 17 bundle tests |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Release build | `cargo build --release` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | Passed: 38 of 38 checks |
| Release CLI smoke | Build `examples/audio-clips.maac` as PCM16 through embedded and `--disk-media` paths, compare WAV bytes | Passed |

The disk-media CLI tests cover a 20,000,004-byte native PCM asset, relocation,
hash rejection with `--force` preserving an existing WAV, and byte-identical
Float32/PCM16 builds for compact clips, kits, notes, and routing. Core tests
cover private snapshot isolation, duplicate path pins, nonfinite samples, and
bounded page reads. Independent review found no actionable issue and also
probed PCM16 overload and unsupported-command behavior. The full `cargo test`
run was stopped after its library tests and integration targets through
`core_delay_plan` passed; later targets are not claimed here.

## Native composition archive version 1 (2026-09-23)

This gate covers `archive create`, `verify`, and `unpack` for one current native
composition and its pinned source/media closure. The first archive version has
no edit history, freeze record, or original WAV sidecar. The complete Rust
integration suite was not rerun for this slice.

| Check | Command | Result |
| --- | --- | --- |
| Focused Rust tests | `cargo test --lib --test archive_cli --test disk_media_cli --test module_cli --test media_import_cli --test bundle` | Passed: 263 library tests, 3 existing ignored; 3 archive CLI, 3 disk-media CLI, 3 module CLI, 6 media-import CLI, and 17 bundle tests |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Release build | `cargo build --release` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | Passed: 38 of 38 checks |
| Release CLI smoke | Archive and unpack `examples/audio-clips.maac`, then compare original and reopened Float32 WAV bytes | Passed |

The archive tests cover native PCM above 4 MiB, relocation after deleting the
original project, exact authored source bytes, native production descriptor
and built-in imports, tampering, extra members, symlinks, and existing output
preservation. A deterministic regression proves manifest, tree, and closure
verification stay on one pinned root when the selected path changes. The
post-fix independent review found no material issue in this bounded slice.

## Immutable native archive checkpoints (2026-09-23)

Version 2 adds an explicit linear history of complete version 1 snapshots.
The fixed version 1 fixture was produced by the prior release binary and
retains digest `sha256:15d422b07aeb803b3f488c42a9b07d307f0cfdb78f09bd1173c4face9bb9842a`.
No complete Rust integration-suite pass is claimed for this slice.

| Check | Command | Result |
| --- | --- | --- |
| Focused Rust tests | `cargo test --lib --test archive_cli --test disk_media_cli --test module_cli --test media_import_cli --test bundle` | Passed: 268 library tests, 3 existing ignored; 5 archive CLI, 3 disk-media CLI, 3 module CLI, 6 media-import CLI, and 17 bundle tests |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Release build | `cargo build --release` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | Passed: 38 of 38 checks |
| Release CLI smoke | Upgrade fixed v1 fixture with `examples/audio-clips.maac`, select both checkpoints, compare current reopened and original Float32 WAV bytes | Passed |

The history tests cover v1 verification and upgrade without changing its
snapshot manifest; a two-checkpoint archive whose later source removes a 5 MiB
PCM dependency; independent reopen of both revisions after deleting the
worktree and predecessor; no-op append; non-head tampering; wrong predecessor
hash; unknown revision; and output paths inside the input archive, including a
symlink alias. Core tests cover A→B→A parent identity, pinned child handles,
and the shared tree-entry limit. Independent review found and prompted a fix
for a create/verify tree-entry budget mismatch; the revised code enforces the
same limit before publication and during verification.

## Retained-original WAV archive checkpoints (2026-09-24)

Version 3 stores the exact retained `import.json` and original WAV in eligible
checkpoints. A fixed version 2 fixture made with the prior release binary keeps
digest `sha256:4d6247bdd8dee4587fdabe1f580c2d637ae56f187381918ee8a4415c46fa9f40`;
the fixed version 1 digest above also remains unchanged. No complete Rust
integration-suite pass is claimed for this slice.

| Check | Command | Result |
| --- | --- | --- |
| Focused Rust tests | `cargo test --lib --test archive_cli --test archive_retained_cli --test archive_v2_compat_cli --test media_import_cli --test disk_media_cli --test module_cli --test bundle`; affected tests rerun after the verifier diagnostic fix | Passed: 273 library tests, 3 existing ignored; 5 archive CLI, 6 retained archive CLI, 1 fixed v2 compatibility, 17 bundle, 3 disk-media CLI, 6 media-import CLI, and 3 module CLI tests |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Release build | `cargo build --release` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | Passed: 38 of 38 checks |
| Release CLI smoke | Retain WAV import, archive, delete input and project, relocate, verify pinned digest, unpack, verify import, and compare all four project files and rendered WAV bytes | Passed |
| Independent review | Read-only Astra review of uncommitted changes | No actionable findings |

The retained tests cover an original WAV larger than 4 MiB with a small crop,
exact record bytes, no-op and changed-record checkpoints, two distinct
historical originals, tampering outside the crop in a non-head checkpoint,
v2-to-v3 promotion without changing the old checkpoint ID, malformed or
orphan sidecars, nested-root guidance, and combined PCM/original budget checks.
The preflight checks metadata and aggregate bytes across every checkpoint
before loading original WAVs.

## Inactive whole-output archive freezes (2026-09-24)

Version 4 can retain an opt-in full-output float32 WAV without replacing the
editable graph. The fixed version 3 fixture made with the prior release binary
keeps digest
`sha256:e20f7ca361cd3877b943e61c4f51aac0e1287543627357f4987527bdb56c269a`.
Versions 1 and 2 keep the digests recorded above. The full Rust integration
suite was stopped after its 276 passing library tests (3 ignored) and
completed integration targets preceding `module_artifact` had passed; no
full-suite pass is claimed for this slice.

| Check | Command | Result |
| --- | --- | --- |
| Archive integration | `cargo test --test archive_cli --test archive_freeze_cli --test archive_retained_cli --test archive_v2_compat_cli --test archive_v3_compat_cli` | Passed: 5 archive, 3 freeze, 6 retained, and 1 each fixed v2/v3 compatibility tests |
| Post-review affected tests | `cargo test --lib freeze`; `cargo test --test archive_freeze_cli --test archive_v3_compat_cli` | Passed: 3 selected library, 3 freeze CLI, and 1 fixed v3 compatibility test |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed after the buffer fix |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | Passed: 38 of 38 checks after the buffer fix |
| Independent review | Read-only Astra review of the uncommitted changes | Two findings resolved: explicitly staged the ignored historical WAV fixture and buffered freeze WAV sample reads |

The freeze tests cover relocation, replay and stale detection after an exact
source edit, an unfrozen successor preserving its frozen predecessor,
non-head tampering, and disk-media output above the inline asset limit. Core
tests cover canonical manifest validation, independent decoded PCM hashing,
and version 4 history promotion. Historical verification checks integrity;
`freeze-check` additionally compares the current source and executable
identity, and optional replay compares rendered output evidence.

## Explicit archive edit transactions (2026-09-24)

Version 5 records one Protocol 2 transaction and its inverse between complete
native checkpoints. The fixed version 4 freeze fixture made with the prior
installed binary keeps digest
`sha256:c836edfa73e44c0f4f44a16313e2761ce6def74971d42baf56a46e511bb08155`;
fixed versions 1–3 retain the digests above. No full Rust integration-suite
pass is claimed for this slice.

| Check | Command | Result |
| --- | --- | --- |
| Library tests | `cargo test --lib` | Passed: 280, with 3 existing ignored |
| Focused integration | `cargo test --test archive_edit_cli --test archive_v4_compat_cli --test archive_v3_compat_cli --test archive_v2_compat_cli --test archive_cli --test archive_retained_cli --test archive_freeze_cli --test editing_cli --test disk_media_cli` | Passed: 33 tests across the nine targets |
| Release integration | `cargo test --release --test archive_edit_cli --test archive_v4_compat_cli` | Passed: 8 archive-edit and 1 fixed-v4 tests |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Installed CLI acceptance | `python3 scripts/acceptance.py` | Passed: 38 of 38 checks |
| Independent review | Read-only Astra review of the uncommitted changes | Two findings resolved: authored-tree inverse replay at the 4 MiB source limit and preservation of semantic diagnostic codes |

The edit tests cover source-preserving forward application and authored-tree
inverse replay, offline relocation, a successful authored no-op, a stale base
revision, a changed dependency closure, retained-original import bytes,
native PCM above 4 MiB under the song profile, historical edit tampering,
and a label-only edit that stales a historical freeze. A source exactly at
4 MiB and an invalid import pin have regressions for the review fixes. The
archive preflights edit metadata, payload sizes, aggregate transaction work,
and closure-copy work before replay.

## Multiple retained WAV imports (2026-09-25)

Version 6 records up to 16 explicitly selected retained WAV import directories
per checkpoint. The initial full Rust gate passed before the freeze-check review
fix. The affected tests and release integration were rerun after that fix.

| Check | Command | Result |
| --- | --- | --- |
| Full Rust gate before review fix | `cargo test --all-targets` | Passed: 281 active library tests, 3 existing ignored, and all integration targets |
| Post-review library | `cargo test --lib` | Passed: 281 active, 3 existing ignored |
| Post-review archive integration | `cargo test --test archive_multi_import_cli`; `cargo test --test archive_freeze_cli` | Passed: 5 multi-import and 3 freeze tests |
| Post-review release integration | `cargo test --release --test archive_multi_import_cli` | Passed: 5 tests |
| Formatting, Clippy, whitespace | `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings`; `git diff --check` | Passed |
| Independent review | Read-only Astra review of the staged change | One finding resolved: a removed selected import now reports a stale freeze when the current composition remains valid |

The multi-import tests cover two nested retained imports, archive relocation,
exact original and record restoration, identical WAV output, journaled entry
edits, selected-import freeze replay and staleness, explicit root selection,
mixed checkpoint history, non-head original tampering, invalid selections,
and failure without publishing an output directory. Fixed v1–v4 archive
compatibility tests passed in the full gate.

## Direct library archive edits (2026-09-25)

Version 7 journals a Protocol 2 edit to one directly imported local leaf
library and the generated entry-source `import.hash` update as one checkpoint.
Verification replays both forward transactions and both authored-tree inverses.
The existing archive versions and their fixed compatibility fixtures remain
readable.

| Check | Command | Result |
| --- | --- | --- |
| Full Rust gate before review fixes | `cargo test --all-targets --quiet` | Passed: 284 active library tests, 3 existing ignored, and all integration targets |
| Post-review library | `cargo test --lib --quiet` | Passed: 285 active, 3 existing ignored |
| Post-review archive integration | `cargo test --test archive_import_edit_cli --test archive_edit_cli --test archive_multi_import_cli --test archive_freeze_cli` | Passed: 3 import-edit, 8 ordinary edit, 5 multi-import, and 3 freeze tests |
| Post-review release integration | `cargo test --release --test archive_import_edit_cli` | Passed: 3 import-edit tests |
| Fixed archive compatibility | `cargo test --test archive_v2_compat_cli --test archive_v3_compat_cli --test archive_v4_compat_cli` | Passed: all three fixed fixtures |
| Formatting, Clippy, whitespace | `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings`; `git diff --check` | Passed |
| Independent review | Read-only Astra review of the uncommitted change | Two findings resolved: a new import edge to an already retained source is rejected before publication, and the CLI reports full render invalidation for a changed imported source |

The import-edit tests cover exact-byte pinning across Unicode comments, offline
relocation and reopening of both versions, changed render output, an authored
no-op, stale-revision and invalid-alias failures without publication, and
non-head history integrity. A core regression rejects an edit that adds an
import edge to an existing closure member while keeping the archive's original
digest and version unchanged.

## Explicit whole-output freeze reuse (2026-09-25)

`archive freeze-render` uses an eligible checkpoint's verified private float32
WAV and publishes it atomically to a new path. It does not change archive wire
formats or run the DSP renderer during reuse.

| Check | Command | Result |
| --- | --- | --- |
| Full Rust gate before review fix | `cargo test --all-targets --quiet` | Passed: 287 active library tests, 3 existing ignored, and all integration targets |
| Focused archive integration | `cargo test --test archive_freeze_cli --test archive_multi_import_cli`; `cargo test --test archive_import_edit_cli` | Passed: 3 freeze, 5 multi-import, and 4 import-edit tests |
| Post-review collision regression | `cargo test --test archive_freeze_cli` | Passed: concurrent writers leave one complete WAV and return `E_OUTPUT_EXISTS` for the other |
| Release integration | `cargo test --release --test archive_freeze_cli` | Passed: 3 freeze lifecycle tests |
| Formatting, Clippy, whitespace | `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings`; `git diff --check` | Passed |
| Independent review | Read-only Astra review of the uncommitted change | One finding resolved: publication-time destination collisions now use the same `E_OUTPUT_EXISTS` code as pre-existing destinations |

The integration tests cover relocation and byte-identical reuse, stale input,
removed retained-import provenance, selected historical freezes in both v4 and
v7 histories, unfrozen heads, corrupted freeze bytes, an existing destination,
and a destination inside the archive. Core tests cover private snapshot use
after archive-path tampering, no-clobber publication, and concurrent copies
from one verified snapshot.

## Single native-effect node freeze (2026-09-25)

Archive version 8 can retain one internal native effect output as exact
interleaved binary64 samples. An eligible source can render its normal graph
with that effect output substituted before downstream processing. The final WAV
is encoded and hashed separately from the cached node stream.

| Check | Command | Result |
| --- | --- | --- |
| Focused node and archive tests | `cargo test --lib node_freeze`; `cargo test --test archive_node_freeze_cli --test archive_freeze_cli --test archive_import_edit_cli --test archive_multi_import_cli --test archive_v2_compat_cli --test archive_v3_compat_cli --test archive_v4_compat_cli` | Passed: 4 node core tests; 2 node CLI, 3 whole-output freeze, 4 import-edit, 5 multi-import, and all fixed v2–v4 compatibility tests |
| Full Rust gate before final I/O buffering | `cargo test --all-targets --quiet` | Passed: 292 active library tests, 3 existing ignored, and all integration targets |
| Final-code regression | `cargo test --lib node_freeze`; `cargo test --test archive_node_freeze_cli --test archive_freeze_cli` | Passed: 4 node core, 2 node CLI, and 3 whole-output freeze tests after buffered cache I/O and final digest assertions |
| Release lifecycle | `cargo test --release --test archive_node_freeze_cli` | Passed: 2 node freeze CLI tests |
| Formatting, Clippy, whitespace | `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings`; `git diff --check` | Passed |
| Independent review | Read-only Astra review of the uncommitted change | One performance finding resolved: node cache capture and reuse now buffer per-sample file I/O |

The node CLI tests cover a mono reverb cache feeding a stereo mix, relocation,
exact replay, byte-identical final WAV, conservative staleness, payload
tampering, unsupported nodes, and failure without archive publication. Core
tests cover mixed whole-output and node freezes in a v8 history, exact
binary64 capture and replacement, downstream execution, and rejection of
invalid boundaries and nonfinite replacement samples.

## Bounded downstream node-freeze reuse (2026-09-25)

New `maac.node-freeze/2` manifests pin the source closure except for numeric
gain and pan value tokens on eligible downstream project nodes. Legacy v1 node
manifests keep their exact-source rule. Reuse renders the captured current
source, so an accepted mix edit changes the final WAV without changing the
cached binary64 effect output.

| Check | Command | Result |
| --- | --- | --- |
| Focused core and history | `cargo test --lib node_freeze`; `cargo test --lib archive_history::tests::v8_retains_mixed_freezes_and_replays_node_payload` | Passed: 8 node core tests and mixed-history regression |
| Node CLI lifecycle | `cargo test --test archive_node_freeze_cli` | Passed: 4 tests, including current-graph gain/pan reuse and rejection of frozen, upstream, and unrelated edits |
| Full Rust gate before review reporting fix | `cargo test --all-targets --quiet` | Passed: 296 active library tests, 3 existing ignored, and all integration targets |
| Post-review regression | `cargo test --test archive_node_freeze_cli --test archive_freeze_cli` | Passed: 4 node and 3 whole-output tests; the accepted edit reports distinct current-source and frozen-source digests |
| Corrected release lifecycle | `cargo test --release --test archive_node_freeze_cli` | Passed: all 4 node CLI tests after the provenance fix |
| Formatting, Clippy, whitespace | `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings`; `git diff --check` | Passed after the review fix |
| Independent review | Read-only Astra review of the uncommitted change | One provenance finding fixed: `freeze-render` now reports the original cache-producing source digest separately from the edited source digest |

The core tests also verify recomputed reuse identity, legacy v1 eligibility,
candidate snapshot activation, and reverse-path exclusion through delay edges.
The first release run began before the provenance fix and failed the new digest
assertion; the corrected release result appears in the table.

## Independent-branch node-freeze reuse (2026-09-26)

New `maac.node-freeze/3` manifests permit numeric gain and pan value edits on
project nodes without an audio or modulation path to the frozen effect. This
includes sibling mix branches. Version 1 remains exact-source; version 2 keeps
its downstream-only rule. The archive root remains version 8.

| Check | Command | Result |
| --- | --- | --- |
| Focused node and archive regression | `cargo test --locked --offline --lib node_freeze`; `cargo test --test archive_node_freeze_cli --test archive_freeze_cli` | Passed: 12 node-related unit tests, 5 node CLI lifecycle tests, and 3 whole-output freeze tests |
| Full Rust gate before sidechain regression | `cargo test --all-targets --quiet` | Passed: 299 active library tests, 3 existing ignored, and all integration targets |
| Sidechain exclusion regression | `cargo test --lib compressor_sidechain_gain_cannot_change_under_v3_freeze` | Passed: a gain feeding the frozen compressor's external sidechain cannot change under v3 reuse |
| Release lifecycle | `cargo test --release --test archive_node_freeze_cli` | Passed: all 5 node CLI tests |
| Formatting, Clippy, whitespace | `cargo fmt --all -- --check`; `cargo clippy --locked --offline --all-targets -- -D warnings`; `git diff --check` | Passed after the sidechain regression was added |
| Independent review | Read-only Astra review of the uncommitted change | No correctness, security, or regression findings; added an explicit compressor-sidechain test from the coverage review |

The new CLI test changes gain and pan on a sibling branch, then confirms that
replay matches the cache and the reused WAV matches an ordinary build of the
edited mix. Unit tests pin v1/v2 behavior, exclude delayed feedback and
compressor sidechain changes, and reject forged v2/v3 reuse identities.

## Historical naming cleanup

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

The release-preparation snapshot checks also passed:

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
