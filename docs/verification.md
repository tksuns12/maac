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

## Independent native-effect freeze groups (2026-09-26)

Archive version 9 records 2–16 independent node freezes in one checkpoint.
Capture and reuse each execute the graph once for the group. Reuse requires
every member to remain eligible; the final WAV is encoded and hashed separately.
The archive keeps scalar freeze records and older checkpoint IDs unchanged.

| Check | Command | Result |
| --- | --- | --- |
| Focused core and history | `cargo test --lib node_freeze --quiet`; `cargo test --lib archive_history::tests --quiet` | Passed: 18 node tests and 16 archive history tests |
| Group CLI lifecycle | `cargo test --test archive_multi_node_freeze_cli --quiet` | Passed: 4 tests |
| Full Rust gate | `cargo test --all-targets --quiet` | Passed: 308 active library tests, 3 existing ignored, and all integration targets |
| Formatting, Clippy, whitespace | `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings`; `git diff --check` | Passed |
| Independent review | Read-only Astra review of the combined change | Two findings resolved: executable changes no longer invalidate archive integrity, and removed effects produce structured stale results |

The group CLI tests cover canonical selection order, relocation and source
deletion, replay, byte-identical edited-mix output, one stale member blocking
publication, removed-effect staleness, and duplicate-boundary rejection. Core
tests cover mixed scalar/group histories, append and patch promotion, private
stream corruption, foreign executable identity, and the 64-reference limit.
The group payload limit is 1 GiB total, checked during archive preflight.

## Grouped archive source edits (2026-09-27)

Archive version 10 journals one edit checkpoint for 1–16 direct local leaf
imports, with an optional entry source edit. Each import keeps its own before
and after revision, forward edit, and inverse edit; the entry is repinned once
after all import edits. Older edit records and checkpoint identities remain
valid. The CLI accepts repeated `--import ALIAS=PATCH.json`, optionally with
`--entry-patch PATCH.json`, and writes the new archive atomically. Supplied
patches are limited to 4 MiB in total, and generated repinning is limited to
16 KiB.

| Check | Command | Result |
| --- | --- | --- |
| Group CLI lifecycle | `cargo test --test archive_group_edit_cli --quiet` | Passed: 3 tests |
| Archive core | `cargo test --lib archive_ --quiet` | Passed: 27 focused tests; the final replay-budget regression passed in the full gate |
| Focused compatibility | `cargo test --test archive_group_edit_cli --test archive_import_edit_cli --test archive_multi_node_freeze_cli --quiet` | Passed |
| Full Rust gate | `cargo test --all-targets --quiet` | Passed: 312 active library tests, 3 existing ignored, and all integration targets |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Passed |
| Independent review | Read-only Astra review of the combined change | Replay accounting, inverse replay context, and CLI input bounds findings resolved |

The group CLI tests cover canonical alias ordering, original and current
archive extraction, relocation after source deletion, stale versus reusable
whole-output freezes, subsequent archive append, duplicate and no-clobber
rejection, tampered edit evidence, and a no-op group checkpoint. Core tests
cover v3 edit evidence, inverse replay against the original trusted bundle,
and a replay budget charged for each member closure pass.

## Shared and transitive archive source edits (2026-09-27)

Archive version 11 adds a source-path mode to `archive patch-group` for 1–16
reachable non-entry local sources. Edit evidence version 4 records independent
user transactions and generated pin transactions for every affected importer.
It replays the exact projected bytes, validates inverses in isolated contexts,
and keeps older checkpoint records unchanged. The source-path and alias modes
are mutually exclusive; the alias mode continues to produce version 10.

| Check | Command | Result |
| --- | --- | --- |
| Source graph CLI lifecycle | `cargo test --test archive_source_edit_cli --quiet` | Passed: 4 tests |
| Archive core | `cargo test --lib archive_ --locked --offline --quiet` | Passed: 29 tests after the final pin-hash cache change |
| Focused compatibility | `cargo test --test archive_group_edit_cli --test archive_import_edit_cli --test archive_source_edit_cli --locked --offline --quiet` | Passed: 3, 4, and 4 tests |
| Full Rust gate | `cargo test --all-targets --locked --offline --quiet` | Passed: 313 active library tests, 3 existing ignored, and all integration targets |
| Doctest | `cargo test --doc --locked --offline --quiet` | Passed: 1 test |
| Integration type check | `cargo check --all-targets --locked --offline` | Passed |
| Formatting, Clippy, whitespace | `cargo fmt --all -- --check`; `cargo clippy --all-targets --locked --offline -- -D warnings`; `git diff --check` | Passed |
| Independent review | Read-only Astra review | Replay preflight, duplicate-alias pin repair, duplicate hashing, and conservative render invalidation findings resolved; no remaining actionable findings |

The CLI tests cover a shared diamond with duplicate aliases, an overlapping
selected ancestor and descendant, exact pin propagation after relocation,
request-order identity, duplicate selection and output no-clobber rejection,
topology-edit rejection, no-op journaling, version 11 continuation, and
promotion from a version 10 history without changing prior checkpoints.
The full Rust gate compiled before a final target-hash memoization change;
the focused core and CLI tests, type check, and strict Clippy covered the final
source afterward. Memoization avoids repeated hashing for several aliases to
one imported source and does not change edit records or transaction order.

## Opt-in native processor context in archives (2026-09-27)

Archive version 12 adds `archive create --processor-context`. A bounded
`maac-processors.json` record captures compiled native processors, initial
parameters, reset origin, declared latency, and resolved local instrument
bindings. Verification derives it again from the archived source; appends and
journaled edits retain or regenerate it. Older archive identities remain
unchanged.

| Check | Command | Result |
| --- | --- | --- |
| Context CLI lifecycle | `cargo test --test archive_processor_context_cli --locked --offline --quiet` | Passed: 1 test |
| Processor-context core | Four focused library tests | Passed |
| Full Rust gate | `cargo test --all-targets --locked --offline --quiet` | Passed: 319 active library tests, 3 existing ignored, and all integration targets; run began before the final size-guard change |
| Post-fix type check and lint | `cargo check --all-targets --locked --offline`; `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| Doctest | `cargo test --doc --locked --offline --quiet` | Passed: 1 test |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Independent review | Read-only Astra review | A resource-bound issue was fixed; no remaining actionable findings |

The CLI test covers relocation, verify and unpack, approved gain edit reuse of
a native node freeze, inherited context on append, and regenerated context
after a Protocol 2 entry edit. Core tests cover canonical decode, resolved
defaults and latency, freeze projection, local instrument bindings, and the
other native processor variants. A large shared-instrument fixture exposed
graph duplication before the 4 MiB limit; capture now counts each node's
serialized size before retaining it. After the fix, the focused core and CLI
tests passed, and that fixture returned `E_RESOURCE_LIMIT` in about two
seconds. The full gate's pre-fix timing is stated above so it is not mistaken
for a second full run after the guard changed.

## Phase 1 inventory and partial group freeze reuse (2026-09-27)

The [lifecycle inventory](end-to-end-lifecycle-inventory.md) maps the current
bounded implementation against media, recording, latency, crops, takes,
automation, routing, delivery, archive, and clean reopen gates. Its 54 local
links and heading fragments resolved in a documentation-only check.

`archive freeze-render --reuse-current-nodes` can explicitly reuse the still
eligible members of an independent native-effect freeze group while executing
stale members from the current source. The default group render remains
all-or-nothing. The feature changes no archive format or older freeze
identity.

| Check | Command | Result |
| --- | --- | --- |
| Node-freeze core | `cargo test --lib node_freeze --locked --offline --quiet` | Passed: 20 tests |
| Focused archive CLI | `cargo test --test archive_multi_node_freeze_cli --test archive_node_freeze_cli --test archive_processor_context_cli --locked --offline --quiet` | Passed: 4, 5, and 1 tests |
| Full Rust gate | `cargo test --all-targets --locked --offline --quiet` | Passed: 321 active library tests, 3 existing ignored, and all integration targets |
| Type check and strict Clippy | `cargo check --all-targets --locked --offline`; `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| Doctest | `cargo test --doc --locked --offline --quiet` | Passed: 1 test |
| Release build | `cargo build --release --locked --offline` | Passed |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Independent review | Read-only Astra review | Diagnostic issue fixed; no remaining actionable findings |

The tests cover one eligible member after relocation with a v12 context,
two eligible members in a three-effect mono/stereo group, exact final PCM
against an ordinary render, all-current opt-in output, zero eligible members,
removed boundaries, a corrupted unused cache, and the singleton option
rejection. All failure cases preserve the absent destination. A review-found
diagnostic issue was fixed so private-spool write failures retain
`E_RENDER_STATE` rather than being reported as stale. A controlled
`RLIMIT_FSIZE=140000` reproduction confirmed this for normal and opt-in
rendering; neither published a WAV.

## File-backed WAV import and retained archive reopen (2026-09-27)

`import-wav --disk-media` can decode a selected native PCM crop beyond the
ordinary 4 MiB asset limit into bounded private storage, preserving the version
1/2 provenance record. `verify-import --disk-media` rechecks retained original
and PCM bytes and the current pinned project closure. The existing `song`
execution-work profile is available on both commands. Root and selected nested
retained imports can be captured, relocated, verified, unpacked, and reopened
through the native archive without materializing the full decoded PCM.

| Check | Command | Result |
| --- | --- | --- |
| Focused media CLI | `cargo test --test media_import_cli --locked --offline --quiet` | Passed: 9 tests |
| Focused archive CLI | `cargo test --test archive_retained_cli --test archive_multi_import_cli --locked --offline --quiet` | Passed: 6 and 7 tests |
| Full Rust gate | `cargo test --all-targets --locked --offline --quiet` | Passed: 324 active library tests, 3 existing ignored, and every integration target |
| Type check | `cargo check --all-targets --locked --offline` | Passed |
| Strict Clippy | `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| Doctest | `cargo test --doc --locked --offline --quiet` | Passed: 1 test |
| Release build | `cargo build --release --locked --offline` | Passed |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Independent review | Read-only Astra review | Work-profile and pinned-root coverage gaps fixed; no remaining findings |

The focused tests cover four WAV encodings with exact disk/embedded conversion
parity, a 4,240,000-byte decoded stereo crop, the default profile rejection and song
profile acceptance of a 210-second stereo import, relocation without the input
path, changed PCM and original detection, and root and nested retained archive
reopen. A deterministic pinned-root replacement test covers disk verification.
The review also confirmed private snapshot write failure publishes no project,
and escaping original-WAV symlinks and removed current media dependencies fail.
The input snapshot and decoded PCM each remain capped at 1 GiB; this is a
bounded disk path, not general streaming or recording.

## Direct source edits with disk-backed media (2026-09-28)

`patch --disk-media [--profile song]` connects retained long-media imports to
source-preserving Protocol 2 edits using captured local source bytes and
private native PCM snapshots. The public patch command/API, complete inverse
and impact result, and atomic output contract remain compatible.

| Check | Command | Result |
| --- | --- | --- |
| Snapshot and existing CLI unit tests | `cargo test --lib cli:: --locked --offline --quiet` | Passed: 4 tests |
| New disk-media editing workflow | `cargo test --test disk_media_editing_cli --locked --offline --quiet` | Passed: 3 tests |
| Existing editing regressions | `cargo test --test editing_cli --test editing_bundle --test editing_source --locked --offline --quiet` | Passed: 5, 8, and 6 tests |
| All-targets strict Clippy | `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| Release build | `cargo build --release --locked --offline` | Passed |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Independent review | Read-only Astra max review | Local-dependency documentation corrected; no implementation findings |

The workflow imports and relocates retained stereo media above 4 MiB, edits
source frames, placement, and fades atomically, checks exact final samples,
verifies unchanged import provenance, and replays the same edit through archive
journaling and relocated reopen. The returned inverse restores the original
authored revision. Failure tests cover stale revisions, invalid crops, an
existing but uncaptured local media path, corrupted or missing PCM, and
destination preservation with `--force`. A 210-second active clip built from
a compact media fixture demonstrates default work rejection and song-profile
acceptance. The pinned-root test replaces the project directory after loading
and confirms editing uses the captured source and PCM. Its initial assertion
was corrected to compare the authored tail quantity rather than source spacing.

This slice used focused editing/CLI execution gates and all-targets static
checking; it does not claim a new full-suite run. Recording, take membership,
and comp selection remain outside this implementation.

## Bounded take membership and comp selection (2026-09-28)

`maac.takes/1` adds alternate native mono/stereo asset membership, a shared
physical origin, and disjoint selected frame regions. Validation requires each
selection to agree exactly with an existing native rate-mode audio clip. The
source capability uses existing Protocol 2 transactions, saved-plan versions,
and archive formats. See the [contract](takes-and-comping.md) and
[runnable synthetic example](../examples/take-comp.maac).

| Check | Command | Result |
| --- | --- | --- |
| Core validation | `cargo test --lib takes:: --locked --offline --quiet` | Passed: 10 tests |
| Library and adjacent integration gate | `cargo test --lib --test bundle --test editing_cli --test editing_source --test archive_cli --test archive_edit_cli --test archive_processor_context_cli --locked --offline --quiet` | Passed: 335 active library tests, 3 existing ignored; 17 bundle, 5 editing CLI, 6 source editing, 5 archive CLI, 8 archive edit, and 1 processor-context tests |
| Take workflow | `cargo test --test takes_cli --locked --offline --quiet` | Passed: 6 tests, including the post-review regression |
| Existing production and bundle editing | `cargo test --test audio_production --test production_data --test editing_bundle --locked --offline --quiet` | Passed: 4, 13, and 8 tests |
| Post-review editing gate | Foundation library tests and `editing_bundle` | Passed: 10 and 8 tests |
| All-targets strict Clippy | `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed after the review fix |
| Independent review | Read-only Astra max review and reproduction after the fix | No unresolved findings |
| Release build | `cargo build --release --locked --offline` | Passed after the review fix |
| Runnable example | Check and build `examples/take-comp.maac --project-root .` through embedded and disk-media paths, including the release binary | Passed: identical 48,000-frame WAV bytes |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |

The take workflow checks exact selected samples, atomic switching and inverse
restoration, preserved comments, asset/clip renames, and production delivery
coexistence in retained plans. It journals a selection in an archive with
processor context, deletes the original project, relocates and unpacks the
archive, then restores the original revision through the inverse. Both
alternate assets and the pinned schema survive. A nested source entry resolves
the descriptor from the package root; missing/corrupt inactive media and
corrupt descriptor bytes fail explicitly.

Invalid cases cover partial selections, unknown takes, overlapping regions,
duplicate managed clips, mixed formats, unsupported clip playback, crop
mismatches, unavailable frames, checked-add overflow, and aggregate limits.
Independent Astra review found that ordinary patch validation could accept
`requires=["maac.takes/1"]` without its required extension. Candidate validation
and normalization now enter artifact preparation for that declaration. The
regression proves incomplete addition/removal preserves an existing destination
and source, while complete metadata/capability removal preserves audio.

This is a library and focused-integration gate, not a new full all-targets
test-suite run. Device capture, synchronized microphone-file groups, playback,
automatic crossfades, and representative listening/producer acceptance remain
open.

## Synchronized microphone-file take lanes (2026-09-28)

`maac.takes/2` adds named microphone-file lanes with individual source-frame
origins and one comp selection across every lane. It reuses explicit audio
clips, Protocol 2 transactions, and existing archive and saved-plan formats.
V1 and v2 share aggregate metadata limits and clip ownership while retaining
separate pinned schemas. See the [grouped-take contract](grouped-takes.md) and
[synthetic example](../examples/grouped-takes.maac).

| Check | Command | Result |
| --- | --- | --- |
| Take core validation | `cargo test --lib takes:: --locked --offline --quiet` | Passed: 15 tests, including all 10 v1 tests |
| Library and adjacent integration gate | `cargo test --lib --test takes_cli --test bundle --test editing_bundle --test editing_cli --test editing_source --test production_data --test audio_production --test archive_cli --test archive_processor_context_cli --locked --offline --quiet` | Passed: 340 active library tests, 3 existing ignored; 6 v1 take CLI, 17 bundle, 8 bundle editing, 5 editing CLI, 6 source editing, 13 production data, 4 audio production, 5 archive CLI, and 1 processor-context tests |
| Grouped take workflow | `cargo test --test grouped_takes_cli --locked --offline -- --nocapture` | Passed: 7 tests, including relocated inverse restoration |
| V1 schema compatibility | Golden SHA-256 assertion in grouped CLI tests; diff against previous commit | Passed: unchanged `11cd8e70cf09aff986f821f88f46ad97056cfb5514a4344eeaa38a6d2e432960` |
| All-targets strict Clippy | `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed after the final acceptance-test changes |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Independent review | Read-only Astra max core/schema and acceptance review | No remaining material findings |
| Release build | `cargo build --release --locked --offline` | Passed |
| Release examples | Check both take examples with `--disk-media`; build `examples/grouped-takes.maac --project-root .` through embedded and disk paths | Passed: identical 48,000-frame stereo WAV bytes |

The deterministic workflow uses two takes with four distinct files, a mono
close lane and a stereo room lane, and four different file-frame origins.
Expected stereo samples are checked exactly before selection, after switching
all lanes together, and after inverse restoration. Ordinary and disk-media
patch paths preserve source comments; incomplete switches preserve both source
and an existing output. Requirement-only additions and partial metadata removals
fail atomically. Complete capability removal remains valid.

The archive test creates a checkpoint, journals the selection, deletes the
original project and base archive, relocates and verifies the resulting archive,
unpacks all four media files and the descriptor, and checks selected audio.
Applying the inverse to the reopened project restores its original revision
and samples. Corrupt inactive media fails with `E_HASH`. A project containing
v1, v2, and production extensions compiles to a retained plan and renders its
expected audio after deleting the source project.

Core failure cases cover missing or extra lanes, duplicate assets or clips,
channel-layout changes across takes, mixed sample rates, unsupported playback,
incorrect selected ranges and placement, unavailable frames, offset overflow,
lane-count bounds, aggregate bounds, mismatched descriptors, and duplicate or
missing capability extensions. Clip ownership is enforced across both versions.
Independent Astra review found no core/schema issues; acceptance review prompted
explicit inverse application after relocated reopen. The v2 schema received
structural/local-reference inspection; a formal Python Draft 2020-12 check was
not run because `jsonschema` is unavailable in the local Python environment.

This is a library and focused-integration gate, not a full all-targets test-suite
run. The fixtures prove authored coordinates and sample selection, not measured
capture latency, acoustic phase, listening quality, device recording, or playback.

## Rendered macOS playback (2026-09-28)

`maac play` supervises the existing source-build or retained-plan renderer,
validates its private Float32 WAV, then runs macOS `/usr/bin/afplay` on the
system-default output. See the [playback contract](playback.md) for supported
flags, signal handling, diagnostics, cleanup, and limits.

| Check | Command | Result |
| --- | --- | --- |
| Library and adjacent CLI gate | `cargo test --lib --test playback_cli --test cli --test cli_bundle --test cli_execution_profiles --test disk_media_cli --test grouped_takes_cli --locked --offline --quiet` | Passed: 354 active library tests, 3 existing ignored; 4 playback, 3 CLI, 7 bundle CLI, 3 execution-profile, 3 disk-media CLI, and 7 grouped-take CLI tests |
| Playback lifecycle | Fourteen library tests included above | Passed: isolated subprocess supervision, signals, diagnostics, finalized WAV validation, cleanup, and restored signal handlers |
| All-targets strict Clippy | `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed after final code changes |
| Formatting and whitespace | `cargo fmt --all -- --check`; `git diff --check` | Passed |
| Independent review | Read-only Astra max supervisor and CLI review | No remaining findings after the fixes below |
| Real renderer handoff | Private test helper invokes the real CLI renderer and compares the staged WAV with an ordinary build | Passed: exact Float32 WAV bytes for 48,000 stereo grouped-take frames; disk-media/song renderer; staging empty |
| Release build | `cargo build --release --locked --offline` | Passed |
| Release backend smoke | Quiet 50 ms source, disk-media/song source, and retained plan through the fixed system player | Passed outside the execution sandbox: each completed with 2,400 mono frames at 48 kHz, one JSON result, empty stderr, and empty staging |

The lifecycle tests cover renderer and backend startup/failure, malformed or
incomplete WAV output, exact player input bytes, bounded backend stderr, large
UTF-8 renderer diagnostics, option forwarding, SIGINT/SIGTERM during playback,
child termination/reaping, and temporary-file removal. Public CLI tests also
interrupt a renderer blocked on input and check usage conflicts and source/plan
diagnostic identity. Deliberate permission loss checks cleanup failure after
success, backend failure, and interruption. The primary failure and signal exit
status survive; the diagnostic identifies the remaining directory for recovery.

Independent review identified three issues that were fixed and covered by
regressions: parsing a large valid renderer error before truncating its message,
forwarding dash-prefixed project roots as one option value, and reporting cleanup
failure without replacing the primary error. Backend message truncation also
now includes an explicit marker. A fresh public-binary permission-loss check
returned SIGTERM exit 143 with `E_INTERRUPTED`, the surviving path, one JSON
object, and empty stderr.

An initial debug smoke inside the execution sandbox reported `E_PLAYBACK` for
`AudioQueueStart failed (-66680)`; the same short fixture succeeded outside the
sandbox. Real-player success proves successful backend process completion,
not that a listener heard it, measured latency, or the physical device rate.

This is a library and focused-integration gate, not a full all-targets test-suite
run or a cross-platform execution result. Device capture, input monitoring,
low-latency transport, and representative producer/listening acceptance remain
open.

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
