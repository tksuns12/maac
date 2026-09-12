# MaaC language specification and conformance plan

Status: L1 addressed 2026-09-13; authority is **RESOLVED**. L2 and L3
addressed 2026-09-13; L4 is the next specification slice and L5 remains
planned.
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

The ordered priorities are L1 first, followed by the bounded L2 and L3
clarification slices, then L4 interchange work, and L5 corpus consolidation.
Conformance vectors accompany every slice; L5 consolidates and extends the
overall corpus.

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
broader L4 interchange and L5 quantitative-conformance work remains planned.

## L4 — portable interchange

**Problem.** Generic locks, render digests, and configuration digests need a
portable byte contract.

**Scope.** Specify versioned exact keys, types, serialization, and hash-input
bytes for generic locks, render digests, and configuration digests. Track
descriptors and loss reports as bounded follow-ups with explicit scope.

**Acceptance, dependencies, status.** Canonical JSON/byte examples, hashes,
version transitions, and rejection vectors must be independently reproducible.
L4 depends on L1 identity and L3 field contracts. Status is planned; no wire
version or hash policy is chosen here.

## L5 — quantitative and machine-readable conformance

**Problem.** Conformance needs measurable criteria without implying universal
tolerances or cross-platform bit identity.

**Scope.** Decide a numerical metric and acceptance bound for Core Audio while
preserving the distinction between numerical tolerance and portable identity.
Build machine-readable vectors for valid/invalid source, canonical bytes and
hashes, patch/conflict/inverse outcomes, event timing, and reference audio
fixtures. The existing conformance checker remains selected smoke evidence.

**Acceptance, dependencies, status.** Publish the metric, bound, provenance,
and expected outcomes with independent vectors and compatibility review. L5
depends on L1–L4 and consolidates or extends every slice's conformance corpus.
Status is planned; no tolerance, corpus, or cross-platform identity policy is
chosen here.

## Optional candidate

**F1 — reusable musical modules.** Patterns and curve exports are an
exploratory candidate outside L1–L5 completion. Scope, compatibility, and
acceptance need definition before promoting it.

## Completion rule

A slice moves from planned to addressed only after normative prose is accepted,
relevant schema or grammar changes are made only when needed, independent
expected vectors and appropriate validation exist, compatibility and versioning
are reviewed, and implementation status is reported accurately. Material
semantic choices must be recorded and resolved before implementation, including
revision authority, hash policy, numerical metric/bound, and wire version.
