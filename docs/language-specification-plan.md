# MaaC language specification and conformance plan

Status: L1, L2, L3, L4, and L5 are addressed 2026-09-13. L1 authority and the
L5 metric and bound are **RESOLVED**. L4 is committed at `e948ce7`.
Baseline: A1–A4 are addressed at
`b19ae2884fbfad2dbf5efb1526a1eff8f5452d9d`. This plan records specification
priorities. The adopted authored-state decision and the scoped L1 semantic
vectors are complete. Runtime normalizer/editor conformance is tracked
separately and is not an L1 gate.

## Goal and boundary

Improve the completeness, precision, and interoperability of the [MaaC-1
specification](../MaaC-1-Specification.md). Implementation and profile gaps are
tracked in the [Core Audio conformance audit](core-audio-conformance-audit.md)
and [capability matrix](capabilities.md); the [playable foundation
implementation plan](implementation-plan.md) remains foundation context. Each
slice must keep normative language, examples, expected vectors, and
implementation status distinct.
Arbitrary programming facilities and implicit behavior are intentionally
excluded from this backlog.

The ordered priorities were L1 first, followed by the bounded L2 and L3
clarification slices, L4 interchange work, and L5 corpus consolidation.
Conformance vectors accompany every slice. The bounded L1–L5 work is addressed;
future obligations remain separate from these completed slices.

## L1 — identity and edit consistency

**Problem.** §20 normalized defaults and revision identity need a precise
relationship with §21 authored-presence, unset, `expect_absent`, and inverse
patch behavior.

**Scope.** Specify omitted `tail` versus authored `tail = 0s`, with canonical
authored graph **A** as revision authority and a separate non-mutating
normalized execution view **N(A)**. Preserve authored units, constructors,
field presence, records, IDs, references, and labels in **A**; define
execution-hash label exclusion by the display-metadata role and never
recursively strip execution parameters merely because a parameter is named
`label`. The [adopted identity/editing decision](l1-identity-editing-decision.md)
records the authority resolution.

**Scoped L1 deliverable (addressed 2026-09-13).** The
adopted authored-state decision defines canonical authored **A** versus the
normalized execution view **N(A)**, and protocol 2 records the independent edit
wire version. The portable [L1 identity/edit corpus](../conformance/l1/manifest.json)
contains 57 files, 4 normalization groups, 2 global-position vectors, 20
expected patch cases (9 success and 11 failure), and 1 metadata projection.
The [MaaC-1 semantic amendment](../MaaC-1-Specification.md#202-semantic-normalization)
and semantic vectors have independent planner clearance at manifest hash
`ff5cc38e80b86da7c0ed0e4d666369d91c3de32434e941ff59f99da78ddbd221` and
specification hash
`f59426ca3ad8223daca042a8fe7d173db497344ed50bc338d2bdc5b20d73aa41`.
The final checker evidence covers 25 complete typed documents structurally
validated against `syntax-tree.schema.json` and 57 dual-digest checks. Its
targeted regression suite reports 18 passing tests, including the
missing-required-file RED/GREEN regression. The 20 patch cases are reviewed
static expected outcomes, not transactions applied by a runtime editor.
Evidence is recorded in the ignored local
[stage2 summary](../target/l1-identity-validation/stage2/summary.json). It
establishes static corpus self-consistency and expected-vector behavior, not
execution by a runtime patch editor or normalizer. The docs/CI work is owned by
the luna executor, checker/tests/execution by the sol executor, and independent
semantic and checker readers provide review; root owns integration. The user
accepted authored field presence. Independent semantic/specification,
compatibility, and checker-final-delta review passed. Rust/build and remote-CI
gates are skipped for this documentation/evidence slice; runtime
normalizer/editor conformance remains separately deferred.

**Acceptance, dependencies, status.** Normative examples and independent
vectors must cover defaults, authored presence, failed expectations, inverse
patches, and revision/hash inputs. Depend on MaaC-1 §§20–21 and
[implementation decisions](implementation-decisions.md). The [adopted
identity/editing decision](l1-identity-editing-decision.md) records the
resolved authority. Status is addressed 2026-09-13; final semantic, vector,
checker, and compatibility review passed, with implementation status reported
separately. Runtime normalizer/editor conformance is not an acceptance
dependency for this specification slice.

## L2 — timing coordinates

**Problem.** Automation and audio timing need interoperable coordinate origins
and truncation order.

**Scope.** Specify explicit automation anchors with relative points; distinguish
audio seconds as absolute physical time under `T(0)=0` versus the render-reset
origin `T(score.start)`; and define
project-end effective truncation after offsets in relation to pattern-cut
order. Use [implementation decisions](implementation-decisions.md),
[performance-plan](performance-plan.md), and [audio clips](audio-clips.md) as
existing evidence and compatibility context, not as independent authority over
the normative language.

**Acceptance, dependencies, status.** Paired valid/invalid examples and timing
vectors must expose each origin and ordering rule, including offsets and cuts.
L2 is addressed 2026-09-13 as a bounded normative clarification with no DSP or
playback-feature expansion. The [L2 timing corpus](../conformance/l2/README.md)
contains 16 fixed source cases and one PCM asset. The [timing regression](../tests/l2_timing.rs)
exercises the existing public `SourceBundle`, `compile_bundle_artifact`, and
`render_artifact` APIs: three tests pass across all 16 cases (9 accepted and 7
rejected), and successful JSON-retained artifacts replay with the same sample
bits. Expected arithmetic is independent literal data; the test-local `1e-12`
observation bound applies only to its ratio/PCM checks and is not a universal L5
tolerance. The L2 specification and fixed outcomes have independent semantic
and harness clearance at specification hash
`03fc433593cea4b451e36f818762a087445d3f606d989ef41096c8431bce8b5b`, manifest
hash `45f44982c2302a2df71319ca56f5983e8c5d644c2963fd4ca62d11b5a953b154`, and
the [local evidence summary](../target/l2-timing-validation/summary.json).
Nine selected existing regression tests, formatting, targeted Clippy, and the
asset digest gate passed. Luna owns the specification/docs, Sol owns the
corpus/harness/validation, independent readers cleared semantics and harness
behavior, and root owns integration. Production source, Cargo, grammar, and
schema are unchanged; no language or plan version was added. The full Rust
suite, release build, installed acceptance, and remote CI were skipped. L3
follows L2; L2 follows the existing tempo, offset, and cut rules and remains
bounded to this timing corpus.

## L3 — complete field and processor contracts

**Problem.** Several field contracts are underspecified across schema,
descriptors, and processor semantics.

**Scope.** State requiredness, type, default, and range for tuning
`reference_index` and `reference_frequency`; distinguish raw pan input range
from effective output range; and define processor-descriptor input cardinality
and `zero_default` behavior.

**Scoped L3 deliverable (addressed 2026-09-13).** The normative contract now
covers raw versus effective `core.pan/1` values, core input cardinality and
`zero_default`, event empty-stream policy, and tuning requiredness, units,
diagnostics, and the user-accepted `reference_index` domain of any
dimensionless mathematical integer subject only to declared host limits. The
[bounded corpus](../conformance/l3/README.md) has 41 source cases: pan 8 (6
accepted, 2 rejected), inputs 17 (2 accepted, 15 rejected), and tuning 16 (6
accepted, 10 rejected). Ten L3 tests exercise public source compilation,
retained plan inspection, and render
replay; the tuning target also pins resolved `pitch_hz` bits and diagnostic
object/field paths. Two implementation regressions were demonstrated with
actual RED → GREEN evidence: the old in-steps reference-index restriction and
the missing tuning object/field context on signed-64 overflow. Forty-one
selected existing compiler, foundation, music, and semantic regression tests,
formatting, and targeted Clippy passed. The [L3 tuning evidence](../target/l3-tuning-validation/)
contains the focused logs and exits. The combined pan/input/tuning gate passed
10/10 in the [final integration log](../target/l3-contract-validation/final/combined-l3-tests.log);
the corresponding integrated summary is maintained at
`target/l3-contract-validation/integrated-final.json`.

Luna owns the specification/docs and tuning source, corpus, and harness;
Sol owns the pan/input corpus and harness. Independent semantic and harness
readers cleared the bounded changes, and root owns integration. The full Rust
suite, release build, installed acceptance, and remote CI were skipped. Full
profile obligations and runtime normalizer/editor behavior tracked by L1 remain
outside this slice. No language or plan version, schema/grammar, dependency, or
wire-format change was made.

**Acceptance, dependencies, status.** L3 follows L2 and depends on the
processor inventory and its existing compatibility evidence. Status is
addressed 2026-09-13 for this bounded field and processor-contract slice; the
addressed L5 quantitative-conformance section records its separate numerical
evidence.

## L4 — portable interchange

**Problem.** The requirements in MaaC-1 §§20 and 22 need a portable generic
lock, configuration, and render-input family that is independently
distinguishable from existing production and instrument identities.

**Scope.** Define an independently discriminated additive generic version-1
family for locks, configuration captures, and render-input identity while
preserving the current production and instrument contracts. The accepted
material direction (2026-09-13) is that the render key covers pre-render
inputs; expected PCM and file hashes are separate output evidence and do not
affect the render key. SHA-256 and the L1 canonical authored **A** /
normalized execution **N(A)** rules are fixed. Descriptors, loss reports, and
quantitative tolerances remain separate slices.

**Acceptance, dependencies, status.** Canonical JSON/byte examples, hashes,
version transitions, and rejection vectors must be independently reproducible
through portable contract text, schemas, and a checker corpus. L4 depends on
the addressed L1–L3 contracts and the existing §20/§22 requirements. The
[generic interchange v1 field contract](generic-interchange.md) records the
exact artifact shapes and semantic wire rules. The accepted local executor evidence has
passed its fixed schema/corpus checks: 54 corpus files (2 configurations, 4
render-inputs, 6 locks, 28 invalid artifacts, 3 portable pairs, and 3 timing
vectors), 15 schema documents, 23 focused L4 tests, and 54 file hashes checked
by both shasum and OpenSSL. The checker also passed the L1 regression checks
(20 corpus cases and 18 focused tests); the symlink and capability-ordering
controls provide the meaningful RED/GREEN evidence, while the initial missing-
checker failures were setup-only. The reviewed [generic schema](../interchange.schema.json)
and [L4 manifest](../conformance/l4/manifest.json) are recorded in the local
[L4 validation summary](../target/l4-interchange-validation/executor-summary.json),
SHA-256 `afc4ad795f4ec63498ade5469520d0e7e3d9af35a34d4e33b5933fa5ec1a1a81`.
The independent harness findings on strict boolean typing for manifest
canonical flags and confinement and symlink checks before reading the fixed
manifest or `SHA256SUMS` were corrected by Sol; four focused regression tests
then passed. Independent semantic/specification and harness reviews both
passed for this bounded slice.

Ownership is split between the luna executor for specification/docs and the
Sol recovery executor for schema, corpus, checker, and tests; independent
semantic/specification and harness review passed, and root owns integration.
The original L4 slice left production Rust and runtime verification unchanged.
Subsequent work added the `maac::generic_lock` runtime verifier for the frozen v1
contract; lock generation/normalization, dependency discovery, and generic rendering
remain deferred. The historical L4 slice itself did not claim full Rust/build or
remote-CI gates. Status
is **addressed 2026-09-13** for this bounded contract/schema/corpus/checker
slice. Descriptor wire schema/ABI and loss-report schemas are separate L4
follow-ups. See the
[L4 portable-interchange decision](l4-portable-interchange-decision.md) for
the accepted direction and scope boundary.

## L5 — quantitative and machine-readable conformance

**Problem.** Conformance needs measurable criteria without implying universal
tolerances or cross-platform bit identity.

**Scope.** Apply the accepted numerical metric and acceptance bound for the
bounded Core Audio suite while preserving the distinction between numerical
tolerance and portable identity. Build machine-readable vectors for
valid/invalid source, canonical bytes and hashes, patch/conflict/inverse
outcomes, event timing, and reference audio fixtures. The existing conformance
checker remains selected smoke evidence. The final [conformance evidence
index](../conformance/index.json) binds seven native suite manifests and 179
fixture pins. Its checker passed 22 focused tests and intentionally does not
execute the runtime recipes recorded in the index.

**Bounded L5 deliverable (addressed 2026-09-13).** The user accepted the
`maac.core-audio.reference-f64/1` policy: one bounded 48 kHz suite, six
`[0,8)` reset-relative fixtures, 64 channel samples, exact dyadic binary64
observations, reduced-rational reference intervals, and inclusive
`E_upper <= 1/10^14`. The [quantitative-conformance decision](l5-quantitative-conformance-decision.md)
records that choice, and the [accepted quantitative contract](quantitative-conformance.md)
defines its portable logical conditions.

The final public compile/retained-plan/replay/f64-render gate passed 7 Rust
tests across 6 cases, 48 frame rows, and 64 channel samples. The maximum
observed `E_upper` was approximately `1.5935876903e-16`; all retained replays
were bit-identical. The ignored [runtime summary](../target/l5-quantitative-validation/runtime/summary.json)
records the actual logs and the observation-file SHA-256
`c4f58b7e0a6ac261fde8214fe694c1aefc29a043e9a510b5ab155a01483d930f`.
The static L5 verifier passed its 13-test regression suite. The targeted L2/L3
cross-slice gate passed 13 tests (3 L2 and 10 L3). Legacy L1 checker/tests
passed 18 tests, L4 checker/tests passed 23 tests, and the disposable
syntax/semantics smoke run matched `check-results.json`, `conformance.json`,
and `example.syntax.json` byte-for-byte.

The root-owned [integrated L5 record](../target/l5-quantitative-validation/integrated-final.json)
binds the final summary hashes, 22 reviewed/candidate file hashes, seven
manifest pins, and all 179 fixture pins. Luna owns the specification, index,
and CI documentation; Sol owns the reference corpus, static verifier, and
runtime harness; the L5 reference-test executor owns the verifier tests. Sol
ran the runtime/reference and L2/L3 gates; Luna ran the index, legacy Python,
and smoke gates; the reference-test executor ran its focused regression suite.
Independent read-only semantic and code/harness reviews passed, and root
performed integration acceptance.

The historical L5 slice left production Rust, Cargo, grammar, and schema files
unchanged and omitted full Rust, release-build, installed, remote-CI,
cross-platform, and listening gates. Runtime normalizer/editor behavior was later
implemented for the bounded language contexts, and generic lock verification now
exists through `maac::generic_lock`; generic lock discovery/rendering, descriptor wire
schemas, and loss-report schemas remain separate deferred work. L5 is addressed for this
bounded contract, corpus, checker, index, and runtime-evidence slice; it makes
no full-profile, universal-tolerance, or cross-platform bit-identity claim.

**Acceptance, dependencies, status.** L5 follows L4 and consolidates or
extends every slice's conformance corpus. The accepted metric, fixed corpus,
public boundary, static checks, runtime observations, and independent reviews
are complete for this bounded slice. Status is addressed 2026-09-13; the
omitted work above remains outside its acceptance boundary.

## Optional candidate

**F1 — reusable musical modules.** Patterns and curve exports are an
exploratory candidate outside L1–L5 completion. Scope, compatibility, and
acceptance need definition before promoting it.

## Professional-production research

The non-normative [professional production language review](professional-production-language-review.md)
records possible language improvements and adoption prerequisites for
professional music production. It does not create L6, alter L1–L5 status, or
promote F1 to an accepted implementation scope.

## End-to-end production direction

The latest accepted product direction is for MaaC and an open ecosystem to
support complete production from composition through archive and reopen. The
non-normative [end-to-end production plan](end-to-end-production-plan.md)
records the proposed capability boundaries and dependency-ranked roadmap. It
does not change L1–L5 status or approve new language, runtime, processor, or
editor scope. The accepted initial surface is CLI-first with playback and
recording tools; visual GUI work is deferred beyond the initial workflow.

## Completion rule

A slice moves from planned to addressed only after normative prose is accepted,
relevant schema or grammar changes are made only when needed, independent
expected vectors and appropriate validation exist, compatibility and versioning
are reviewed, and implementation status is reported accurately. Material
semantic choices must be recorded and resolved before implementation, including
revision authority, hash policy, numerical metric/bound, and wire version.
