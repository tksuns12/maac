# MaaC language specification and conformance plan

**Status:** active language/specification roadmap; scope reset 2026-09-28.
This plan owns current priorities. The [broader production
plan](end-to-end-production-plan.md) is deferred; device integration and
full-application readiness do not determine language completeness.

**Historical baseline:** L1, L2, L3, L4, and L5 are addressed 2026-09-13 for their bounded
contracts and evidence. L1 authority and the L5 metric and bound are
**RESOLVED**. L4 is committed at `e948ce7`. Later bounded runtime slices add
reusable source modules, generic lock verification/construction, strict external
descriptor and dependency discovery, MIDI loss reporting, bounded built-in/core
generic rendering, an explicit native external ABI host, and a single-generator
external render bridge; mixed external/core graph execution remains deferred.
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

## Current priorities

**Reader and action:** P1–P23 are addressed within their
bounded scopes. P5 adds exact score-window queries over validated resolved
performances; P6 corrects execution normalization for existing expression and
message events. P7 retains proven original-source UTF-8 byte ranges in editing
and CLI patch diagnostics. P8 closes §19 region semantics; P9 proves normalizer agreement; P10 meets
the §23 diagnostic location contract. P11 executes `warp_preserve`; P12 adds
pitched sample instruments; P13 lets them use disk-backed assets; P14 adds velocity layers and
crossfades. P15 places audio clips inside patterns. P16 adds stereo samples; P17 lets samples play composition assets. P18 closes the D01, D02 and D05 evidence gaps with grammar-driven and inventory tests. P19 runs every role of the L1 metadata-role fragment through the execution projection (D10). P20 settles unknown extensions at the §26 boundary (D16). P21 extends the §23 location contract to the recognized extensions (D14); P22 extends it to the entry source's dependency declarations. P23 writes the rules settled in P12–P22 into the specification.
P2's [evidence map](document-performance-evidence.md) identified the final edit
graph validation discrepancy corrected by P3. P4 implements the specified
`materialize-instance` authoring operation. Contributors should retain implemented behavior and state separately
what is specified, implemented, and proven by conformance evidence. This
roadmap does not authorize unrelated grammar, runtime API, or profile changes.
P1's normative clarification and scoped evidence are recorded separately.

### Scope and ownership

| Track | In scope | Completion evidence |
| --- | --- | --- |
| Language and libraries — active | Musical meaning, units and clocks, finite reusable data, reference ownership, source identity, exact edits, and explicit capability requirements | Consistent normative contract, independent expected outcomes, compatibility review, and tests at the public language boundary |
| Portable interchange — active when required by a selected language slice | Pins, dependency/state descriptions, authored-versus-derived identity, losses, and refusal semantics | Round-trip and tamper/refusal vectors that identify exactly what is preserved |
| Reference runtime — supporting | Implementing and checking the selected declared subset under explicit limits | Scoped compiler/renderer evidence; no inference of full-profile conformance from processor coverage |
| AI production tooling — active | Tools that let an AI producer perceive and judge its own renders: measurement, findings, images, and workflow guidance. They read compiled plans and never change the language or plan formats | Known-answer fixtures, deterministic reports, and use on real compositions; [T1 `maac analyze`](analyze.md) addressed 2026-10-03 |
| Host/application tooling — deferred | Device discovery, microphone capture, monitoring, live transport, hardware latency calibration, GUI, and executable plugin hosting | Separate product/hardware contracts and acceptance, outside language completion |

An asset's source frame and declared origin are portable data. Measuring a
microphone's latency or routing its monitor signal is host work. Likewise,
describing processor state/dependencies is an interchange concern; loading an
executable plugin is a host concern. No language slice requires a microphone,
audio output device, or DAW session to establish its semantic results.

### Ordered work

| Priority | Work | Evidence for the gap | Acceptance boundary |
| --- | --- | --- | --- |
| P1 — addressed 2026-09-28 | Reconcile the existing musical-library contract | The older declaration inventory excluded implemented pattern/curve/tuning exports. The [amended contract](instruments.md#reusable-musical-declarations) now defines ownership, caller context, validation, and identity consistently with the existing module path | [52 scoped tests and semantic review](musical-library-contract-validation.md); no new runtime/export feature or wire format |
| P2 — addressed 2026-09-28 | Map Document and Performance obligations to public-boundary evidence | The [requirement/evidence map](document-performance-evidence.md) separates bounded runtime evidence, static vectors, unsupported behavior, unverified obligations and demonstrated failures | 29 requirement groups, 250 passing scoped tests, independent semantic review, and a reproduced final edit-validation discrepancy; no complete-profile claim |
| P3 — addressed 2026-09-28 | Reject same-sample graph cycles before an edit commits or publishes source | Shared Document validation now rejects the P2 self-cycle with `E_ALGEBRAIC_LOOP` before foundation/bundle edits commit or CLI patch publishes | [Nine new public regressions and compatibility evidence](document-performance-evidence.md#p3-correction): atomic refusal, output preservation, valid explicit-delay feedback, modulation cycles, final-state repair, and preserved Document-only editing |
| P4 — addressed 2026-09-28 | Materialize a selected placement as independent editable source | Foundation/bundle preparation APIs and CLI `materialize-instance` now implement §11's explicit copy and source-address mapping | [20 focused tests, independent review, and the full 1,428-test gate](materialize-instance.md#verification): private repeated/nested occurrences, exact transforms and reference ownership, complete mapping, atomic source edits/inverses, and CLI publication |
| P5 — addressed 2026-09-28 | Query expanded events over an exact score window | §10 promises full event intervals and source addresses for intersecting notes, but [P04](document-performance-evidence.md#performance-obligations) had no public narrow-window query evidence | [Score-window contract and scoped verification](windowed-event-query.md#verification): public artifact query and CLI, fixed interval/address vectors, full validation, and bounded resource behavior; no lazy-expansion claim |
| P6 — addressed 2026-09-29 | Normalize existing expression children and message events for execution identity | Valid expression and message compositions compiled outside production but failed at execution normalization when production identity was attached; the [D09 map](document-performance-evidence.md#document-obligations) left complete normalization unverified | [Bounded correction and verification](execution-event-normalization.md#verification): identity vectors, production compilation, artifact/P5 query compatibility, atomic edit regression, unchanged unsupported receiver behavior, and the complete Cargo-built Rust test inventory |
| P7 — addressed 2026-09-29 | Preserve available original-source locations in editing and CLI patch errors | [D14](document-performance-evidence.md#document-obligations) left span preservation through `EditError` unverified | [Original-source diagnostic contract and verification](editing-diagnostics.md): parser spans, authored value/record/child locations, surviving rename identity, conservative unknown provenance, atomic refusal, CLI code/path/location, and public API compatibility |
| P8 — addressed 2026-09-30 | Close §19 region semantics | [D06](document-performance-evidence.md#document-obligations) left reversed/outside spans, overlap/nesting and nonacoustic behavior unverified; `validate_source` skipped range checks on `bar(b,u)` endpoints | Closed §19 rules in the specification, `bar` lowering through the project meter in `validate_source`, and [nine public region vector groups](../tests/regions.rs): accepted boundary/overlap/nesting, refusals in `q` and `bar` form, top-level-only placement, bit-identical events and audio, execution-hash span/label roles, and atomic span edits |
| P9 — addressed 2026-09-30 | Prove editing and execution normalization agree | [D09](document-performance-evidence.md#document-obligations) left complete core normalization unverified; the only agreement vector was a single sine node | [Differential vectors](../tests/normalization_agreement.rs) over every core kind and compile-profile processor, omitted versus explicit defaults and equivalent spellings; unreachable tuning defaults removed. LFO/constant/modulate and unknown extensions stay outside this claim |
| P10 — addressed 2026-09-30 | Meet the §23 location contract for every source code | [D14](document-performance-evidence.md#document-obligations) left comprehensive diagnostics unverified | Normative §23 path rules, a [21-code public catalog](../tests/diagnostic_catalog.rs), and location corrections in the parser, semantic validator and compiler; codes unchanged |
| P11 — addressed 2026-09-30 | Execute §14.3 `warp_preserve` with a core reference stretch | [P07](document-performance-evidence.md#performance-obligations) listed `warp_preserve` as unsupported; §14.3 allowed only module-asset stretchers | Normative `core.stretch.ola/1` (Hann overlap-add, 20 ms hop, grains at the original rate), optional plan `stretch` field, and [six public vectors](../tests/warp_preserve.rs); module-asset and unknown stretchers stay `E_CAPABILITY` with no rate-warp fallback |
| P12 — addressed 2026-09-30 | Add pitched sample instruments | [P07](document-performance-evidence.md#performance-obligations) listed pitched sample instruments as unsupported; `core.kit/1` is hit-only | Local-library `sample` declarations and the voice-graph [`synth.sample/1`](instruments.md#samples) processor: key zones, rate-converted pitched playback with continuous bends, forward sustain loops, shared embedded-sample budget, provenance and archive binding, and [seven public vectors](../tests/sample_instrument.rs) |
| P13 — addressed 2026-09-30 | Let samples use disk-backed audio assets | P12 samples were embedded plan values limited to 262,144 frames in total | `sample` declarations with a core PCM `format` lower to ordinary plan audio assets (inline or `--disk-media`), with reserved asset IDs, version 4+ plan routing, plan cross-validation, and [asset](../tests/sample_instrument.rs) and [disk-media CLI](../tests/sample_disk_media_cli.rs) vectors. Plan-error diagnostics now keep every §23 code (such as `E_ASSET` and `E_HASH`) instead of reporting them as `E_RANGE` |
| P15 — addressed 2026-10-01 | Place audio clips inside patterns | [P07](document-performance-evidence.md#performance-obligations) listed audio placements inside patterns as unsupported; §4 allowed no nested `audio` | Normative [§9.1 audio leaves](../MaaC-1-Specification.md#91-audio-leaves): pattern and insert `audio` leaves with local `at`, stretch-scaled warp anchors, no transposition or cut, an explicit placement `out` port, occurrence overrides limited to timing, gain and fades, and [public vectors](../tests/pattern_audio.rs); see the [contract](pattern-audio.md). Each placement lowers to one `clips` plan node, so occurrences no longer consume the 256-node plan limit |
| P23 — addressed 2026-10-03 | Write settled rules into the specification | Rules decided in P12–P22 were implemented and evidenced but absent from the normative text: identifier-boundary tokens (P18), the library extension's sample forms (P12–P17), extension namespace codes (P20), and diagnostic locations for compiled-graph extension checks and other sources (P21, P22). Writing §23 also showed that P22 had left dependency failures in imported sources unlocated while syntax and validation failures there were located in that source | §2 states the token boundary; §1.3 lists mono/stereo WAV, PCM and asset-backed samples with velocity layers and crossfades; §23 reads locations in the source where a failure is found, names a non-entry source at the start of the message, and reports dependency failures at the declaration's `hash` or `path`; §26 gives `E_CAPABILITY` and `E_REFERENCE` at the `namespace` field. By the owner's decision, dependency failures in imported sources are now located in that source too ([test](../tests/dependency_diagnostics.rs)) |
| P22 — addressed 2026-10-02 | Report dependency-declaration failures at the entry source | P21 left import, sample, wavetable and asset pin failures (path, hash, missing source or bytes) without a source location; they named the declaration only in message text | Discovery records each entry declaration's `path` and `hash` fields; in-memory resolution, import-graph validation and filesystem loading report failures there (hash failures at `hash`, all others at `path`), and an import cycle closed by the entry at that import. P22 left failures in other sources unlocated; P23 locates them in their own source instead, consistent with syntax and validation failures there. A [catalog](../tests/dependency_diagnostics.rs) of 18 in-memory and 5 filesystem cases, plus checks that failures in other sources stay unlocated; every located case failed before the change |
| P21 — addressed 2026-10-02 | Report recognized-extension diagnostics at their source | [D14](document-performance-evidence.md#document-obligations)'s catalog covered core sources only; production and takes diagnostics used internal paths such as `production.rate`, `project.requires` and `takes`, which name no authored object and could not say which delivery, target, group or region failed | Shared located reads (`src/located.rs`) give every production and takes source check the authored object, field path and span; compiled-graph target checks and the engine-rate check report at the target `output` and `project.rate`; bundle discovery reports schema-reference failures at the extension's `schema`. A [67-case catalog](../tests/extension_diagnostics.rs), all of which failed before the change, fixes each location. Codes are unchanged |
| P20 — addressed 2026-10-02 | Settle unknown extensions at the §26 boundary | [D16](document-performance-evidence.md#document-obligations) was **U** for unknown-extension normalization; the bundle check reported an unknown extension at a production path, a namespace missing from `project.requires` was not reported as such, and `execution_identity` hashed unknown extension data | The owner [chose refusal](unknown-extensions-proposal.md) over schema validation or opaque editing. `maac::extensions` holds the supported identifiers and §26 checks shared by validation, bundle preparation and edit contexts; unsupported namespaces are `E_CAPABILITY` and unrequired ones `E_REFERENCE`, at the `namespace` field; execution identity refuses unsupported namespaces; [tests](../tests/unknown_extensions.rs) cover preservation and every refusal |
| P19 — addressed 2026-10-02 | Run the L1 metadata-role fragment through the execution projection | [D10](document-performance-evidence.md#document-obligations) left the full metadata-role fragment as static **S** evidence: only the Python corpus checker read it, and its `fixture_*` kinds cannot be normalized | [Metadata-role tests](../tests/metadata_roles.rs) place each fragment role on a real object of a validated production bundle and require `execution_identity` to match the fragment's keep/remove verdict. The work also fixed a §4 gap: every library-extension kind and `import` refused the optional `label`; they now accept a string label without changing the plan, and refuse other values with `E_UNIT` |
| P18 — addressed 2026-10-02 | Close the lexical, structure and value-type evidence gaps | The [evidence map](document-performance-evidence.md#document-obligations) left exhaustive lexical boundaries (D01), the full accepted/rejected inventory and nesting combinations (D02), and every dimensional field and constructor location (D05) unverified | A [grammar-driven lexical corpus](../tests/lexical_corpus.rs) checked against Lark as an independent oracle, a spec-derived [structure inventory](../tests/structure_inventory.rs) and [value-type inventory](../tests/value_types.rs); fixes for the gaps they found: `maac` and pitch tokens end at identifier boundaries in `grammar.lark`; `use.at` is required; `project` and core `node` children are refused; lowercase pitches, nonintegral `key()` and nonpositive `ratio()` are refused; wrong-unit warp anchors are `E_UNIT`; and list items, override replacements, node parameters and config values report §23 field paths |
| P17 — addressed 2026-10-01 | Let samples play composition assets | A recording used both as a clip or kit sound and as a pitched sample had to be declared and carried twice, once under a reserved `__sample_N` ID | A `sample { asset = &a; root = ...; }` form taking rate, channels, frames and provenance from a same-document audio asset, one shared plan audio asset, no own file pin, local-reference and field refusals, and [public vectors](../tests/sample_instrument.rs) |
| P16 — addressed 2026-10-01 | Add stereo samples | P12–P14 samples and `synth.sample/1` nodes were mono only, so stereo recordings had to be collapsed or split into two hard-panned instruments | Stereo WAV and core PCM `sample` declarations (optional `channels` on PCM), an optional `synth.sample/1` `config.channels` that every zone sample must match (`E_PORT_TYPE`), per-channel playback sharing each zone's position and gain, a value-counted embedded budget, unchanged mono plan bytes, default-invariant execution identity, and [public vectors](../tests/sample_instrument.rs) |
| P14 — addressed 2026-09-30 | Add velocity layers and zone crossfades | P12 zones were hard, non-overlapping key ranges with no velocity selection | Optional zone `velocity`, `key_fade`, and `velocity_fade` pairs and node `fade_shape` (linear or equal power); overlapping zones sum at their fixed note-on gains; defaults expand identically in editing and execution views; [public vectors](../tests/sample_instrument.rs) |

P2/P3 are not a blanket promise to implement every profile, renderer algorithm,
or external receiver. Unsupported capabilities must remain explicit. The
existing L1–L5 corpus is retained; its completed status is not reset and no L6
conformance level is introduced by this roadmap.

### P1 record: musical-library contract reconciliation

**Addressed 2026-09-28.** This slice documented and verified the reusable musical
data that already worked. Semantic review preceded the normative amendment;
independent final review and [scoped verification](musical-library-contract-validation.md)
cover its acceptance. The handoff requirements below are retained to bound the
result. Implementation behavior is evidence to inspect, not automatic authority
over the specification.

The contract must settle:

1. Permitted library declarations and exported kinds, including patterns,
   curves, and tunings; validation of all exports, including unused ones.
2. Resolution of nested references in their declaring library, qualified import
   aliases, dependency closure, and rejection of accidental caller-name capture.
3. Composition-owned tempo, meter, tracks, placement, and automation bindings;
   importing reusable material must not replace those choices.
4. Alias-qualified source provenance versus expanded occurrence addresses,
   authored revision identity, and derived execution identity.
5. Exact pin/refusal behavior and module export/check/unpack compatibility.

Acceptance uses existing `check_bundle`, compilation, and `ModuleArtifact`
public boundaries. Fixed fixtures must cover two consumers of one pinned motif
and curve under different caller contexts, nested/aliased dependencies, explicit
tuning, source mapping and occurrence identity, an altered pin, an invalid
unused export, and module round-trip identity. Existing musical-library and
module-artifact tests are starting evidence; add only missing semantic vectors
after comparing their coverage with the reconciled contract.

Do not add import syntax, a registry, direct artifact imports, pattern
parameters, tempo exports, linked sections, or device behavior in P1. Success
is a consistent language/library contract with scoped evidence, not a new
sampler, editor, host, or full-profile declaration.

### Credited work and optional proposals

Pinned pattern/curve/tuning reuse and the bounded F1 module artifact already
exist. Exact transactional editing/normalization, generic lock construction
and verification, descriptor/dependency inspection, MIDI loss reporting, and
the take/comp extension also have bounded implementations. Their limits remain
in the capability matrix; do not reintroduce them as wholly missing features.

Occurrence-local expression variants that preserve shared source edits are an
optional language-design candidate. The [materialization operation](materialize-instance.md)
creates independent copies through public editing APIs and the CLI; P4's tests
extend [P2's evidence map](document-performance-evidence.md#performance-obligations)
with execution evidence for that specified operation.
A different sharing model needs a separate
ownership, precedence, identity, and compatibility decision; it is not a defect
merely because the current contract lacks it. Multi-lane move/duplicate helpers
belong first to tools over existing transactions. Pitched samplers and the
core preserve-pitch stretch were later addressed as P11–P14. Additional receiver
adapters, module-asset stretchers, and executable hosting each need their own
scoped proposal; none is the automatic next task.

The [production-language research memo](professional-production-language-review.md)
is background, not a competing ordered backlog. Its historical adoption and
listening experiments do not gate this plan.

### Evidence for this roadmap reset

During the earlier roadmap reset on 2026-09-28, inspection identified the library declaration contradiction
above. The existing `musical_library`, `module_artifact`, `module_cli`, and
`editing_bundle` test targets were run with Cargo's locked/offline settings:
27 tests passed. Added documentation links and anchors were checked. This
supported that documentation-only inventory correction; it did not complete P1
or amend normative semantics. P1's subsequent amendment and 52-test gate are
recorded above. Neither step establishes a full conformance profile, and neither
changes runtime code or device behavior.

## Completed specification slices

The L1–L5 sections below preserve the decisions and evidence of their original
slices. Historical statements about omitted runtime work describe that slice's
acceptance; consult current priorities and the capability matrix for later work.

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
Subsequent work added the `maac::generic_lock` runtime verifier, the
`maac::external` strict external-descriptor/dependency-discovery boundary, and the
typed generator (originally two parallel modules, consolidated on 2026-09-30 into
`maac::generic_lock_normalization`), which deterministically constructs canonical v1 Config, RenderInput, and
Lock artifacts from already-resolved typed context and exact bytes.
`maac::generic_render` now renders already-resolved built-in/core `Plan` contexts after pre-render verification, with a concrete host identity and null block schedule. `maac::external_host` and `maac::external_native` subsequently add an explicit executable native ABI boundary; a bounded single output-only external-generator render bridge is now implemented; general mixed external-node graph integration remains deferred. These later runtime slices do not turn the historical L4 schema/corpus
slice into a full Locked Render conformance claim. The historical L4 slice itself
did not claim full Rust/build or remote-CI gates. Status
is **addressed 2026-09-13** for this bounded contract/schema/corpus/checker
slice. Strict external descriptor/discovery support and bounded interchange loss reporting
were added in later L4 runtime slices; bounded built-in/core generic rendering and an explicit native ABI host boundary were added subsequently, while mixed external/core DAG execution remains a separate follow-up. See the
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
implemented for the bounded language contexts. Generic lock verification and
construction, strict external descriptor/dependency discovery, and the versioned
`maac.interchange-loss-report` contract with its bounded MIDI 1.0 SMF adapter now
exist, as do bounded built-in/core generic rendering, an explicit native ABI host,
and a single-generator external render bridge; mixed external/core graph execution,
notation, and DAW-session adapters remain separate work. L5 is addressed for this bounded contract, corpus, checker, index, and runtime-evidence slice; it makes
no full-profile, universal-tolerance, or cross-platform bit-identity claim.

**Acceptance, dependencies, status.** L5 follows L4 and consolidates or
extends every slice's conformance corpus. The accepted metric, fixed corpus,
public boundary, static checks, runtime observations, and independent reviews
are complete for this bounded slice. Status is addressed 2026-09-13; the
omitted work above remains outside its acceptance boundary.

## Optional candidate

**F1 — reusable musical modules.** The bounded source-module artifact slice is
implemented and validated, including closure identity, exact source/assets, and
check/export/unpack behavior. A broader package registry, binary module format,
and direct artifact-import feature remain outside scope. See the [module artifact
contract](musical-module-artifact.md).

## Professional-production research

The non-normative [professional production language review](professional-production-language-review.md)
records possible language improvements and adoption prerequisites for
professional music production. It does not create L6 or alter L1–L5 status;
the bounded F1 artifact slice is implemented, while broader F1/package work
remains outside the accepted L1–L5 scope.

## Deferred broader-product direction

The non-normative [end-to-end production plan](end-to-end-production-plan.md)
preserves the earlier composition-through-reopen application proposal and its
existing tooling evidence. Further capture, monitoring, backing-track transport,
hardware alignment, and host integration are deferred. They do not change
L1–L5 status or block the language priorities above.

## Completion rule

A slice moves from planned to addressed only after normative prose is accepted,
relevant schema or grammar changes are made only when needed, independent
expected vectors and appropriate validation exist, compatibility and versioning
are reviewed, and implementation status is reported accurately. Material
semantic choices must be recorded and resolved before implementation, including
revision authority, hash policy, numerical metric/bound, and wire version.
