# Professional production language review

**Status:** non-normative research memo; dated 2026-09-13

**Current scope (2026-09-28):** active priorities belong to the
[language specification and conformance plan](language-specification-plan.md#current-priorities).
This memo preserves exploratory proposals and optional adoption experiments;
its historical ranking does not select the next task. Device and host readiness
are not language-completeness gates.

**Reader and action.** This memo is for maintainers deciding which language
proposal should receive the next design pass. The reader should be able to
select a narrowly testable proposal and adoption experiment. It does not create
L6, change MaaC-1, or approve an implementation scope.

## Historical assessment frame

This memo was prepared for an earlier workflow: compose and arrange in MaaC,
then finish in a DAW. A later full-application proposal also broadened that
scope. Both are optional product directions; neither governs the active
language backlog. The research remains useful for the optional handoff path:
reusable material, precise timing, expression, and an auditable interchange.
Standalone full production and film scoring remain separate conditions with
different needs. These observations guide research; they do not approve an
individual proposal.

The current specification already has a strong closed-performance foundation.
Patterns have finite nesting, repetition, stretching, transposition, cuts, and
source-addressed expansion in [finite patterns and composition](../MaaC-1-Specification.md#9-finite-patterns-and-composition)
and [tracks, placement, and event identity](../MaaC-1-Specification.md#10-tracks-placement-and-event-identity).
Curves and automation have declared clocks, anchors, interpolation, ordering,
and a single replacement lane in [instance overrides and inserts](../MaaC-1-Specification.md#11-instance-overrides-and-inserts)
and [curves and parameter automation](../MaaC-1-Specification.md#12-curves-and-parameter-automation).
Audio transports, explicit warp maps, routing, sidechains, latency, freeze
invalidation, and production deliveries are also specified, including the
shared execution and selection rules in [production delivery](production.md#7-shared-execution-timing-and-selection).
The core deliberately rejects implicit humanization, arbitrary computation,
guessed conversion, and hidden fallback behavior. A proposal should preserve
those properties rather than make a convenient editor action part of the
renderer by accident.

Several research questions sit around that foundation. Pinned pattern, curve,
and tuning exports now work, as does the bounded [musical source module
artifact](musical-module-artifact.md). The [reconciled library
contract](instruments.md#reusable-musical-declarations) now defines their
ownership and validation; [scoped evidence](musical-library-contract-validation.md)
records the completed P1 slice.
The specification defines `materialize-instance` for occurrence expression
changes, creating an independent pattern copy. The [P2 evidence map](document-performance-evidence.md#performance-obligations)
does not establish an integrated public operation/test for that contract.
Retaining a local bend while propagating later
shared pitch edits would require a new, explicitly designed ownership model.

Notes, audio, and automation are representable, and exact transactions already
provide atomic commit and inverse patches. High-level mixed-media move or
duplicate helpers remain tool proposals over those contracts. The take/comp
extensions now preserve alternate membership and synchronized lane selections;
they are not wholly missing language features.

Runtime normalization/editing, generic lock construction and verification,
strict descriptor/dependency inspection, and bounded MIDI 1.0 SMF export with
loss reports also exist, as do bounded built-in/core generic rendering and an
explicit native ABI host. Mixed external/core graph execution and broader
receiver adapters remain separately scoped. Disk-media paths support
bounded external native PCM and retained imports; they are not general-purpose
streaming. Reset-correct excerpts can render from reset and crop the result;
a faster seek/checkpoint interface is optional implementation work, not an
inferred language requirement.

## Historical research ranking and current status

This ranking preserves the original research order. It is not the active
backlog. Candidate scenarios are proposed tests unless the status below records
a later implementation; full product or listening acceptance is not implied.

| Rank | Candidate | Boundary | Existing foundation and missing contract | First observable acceptance scenario |
| ---: | --- | --- | --- | --- |
| 1 | Pinned pattern and curve library exports | Language/library semantics | Bounded pattern/curve/tuning exports, module artifacts, and the reconciled library contract are implemented and evidenced. | Two projects import one pinned motif and curve, place and transform them differently, preserve source addresses, and reject an altered pin. |
| 2 | Occurrence-local expression variants | Language semantics | Per-note pitch, gain, pressure, and timbre exist; complex occurrence expression currently requires materialization. | Change bend on one occurrence, change source pitch afterward, and observe local bend retention with intended shared pitch propagation. |
| 3 | Multi-lane arrangement operations | Editor/tool contract over existing transactions | Notes, audio, and automation are separately expressible; grouped selection, clock conversion, merge policy, and linked ownership are unspecified. | Duplicate an eight-bar mixed-media chorus across a tempo change with approved alignments, unchanged unrelated automation, and one inspectable inverse. |
| 4 | Faithful handoff: MIDI, aligned stems, descriptors, and loss reports | Interchange/profile and adapter contracts | Descriptor fields, latency, state, and fidelity refusal are specified; the initial MIDI 1.0 SMF format-0 adapter and versioned loss-report wire are now implemented, while aligned-stem handoff, descriptor receiver mappings, notation, and DAW-session profiles remain deferred. | A Type 1 MIDI file, aligned PCM WAV stems, original MaaC source/dependencies, and manifest account for overlaps, expression, tempo ramps, audio, and automation, rejecting any unapproved loss. |
| 5 | Deterministic groove and musical transformations | Tool lowering contract | Finite explicit data is the safe boundary; generators must not become hidden computation. | A tool lowers a groove or strum request to explicit events and curves with stable pins, addresses, and repeat output; changed inputs change the digest. |
| 6 | Versioned articulation and pedal-map adapters | Library/receiver adapter contract | Expression messages and receiver-specific controllers exist; notation marks and sampled playback are not universal semantics. | A pinned map selects two advertised techniques through a repeat and full render crop; an unsupported map fails or reports an approved loss. |
| 7 | Take, comp, and grouped-edit provenance | Editor/tool contract, conditional | Bounded `maac.takes/1` and `/2` now preserve alternates, explicit selections, synchronized lanes, and inverse/archive history. Broader editing and audition workflows remain optional tool work. | Comp two phase-related mic takes while preserving originals, synchronized cuts/warp coordinates, exact undo, and no implicit crossfade. |
| 8 | Quality warp engine profile | Audio transport/processor extension, conditional | `warp_rate` uses specified linear sample interpolation; `warp_preserve` already requires a pinned algorithm and descriptor. | Vocal and transient-drum cases retain anchors, duration, channels, and pitch under a pinned profile, with repeatability and a separate listening trial. |
| 9 | Timecode, channel roles, and delivery variants | Film-oriented domain profile, conditional | Score timing and named deliveries exist; a film-oriented frame and role profile is absent. | A rational frame-rate project identifies its timecode origin and channel roles, produces declared variants, and rejects ambiguous conversion. |

### Pinned reusable musical exports

The bounded implementation now provides this reuse without an editor or host.
The original design proposal below is historical context for the contract
reconciliation; it is not a request to implement exports again. An export can package a
pattern, curve, or both with a versioned identity, ownership, and import
namespace. Its dependency closure should include nested patterns, tunings, and
referenced curves. A pattern still binds its destination through existing
placement and track fields, and a curve is consumed through existing expression
or automation fields; the export should not add parameterized section or port
semantics. Callers must resolve references explicitly, and an import must not
replace the composition's tempo map. Placement supplies the transform, so one
motif can serve verse and chorus without changing its source.

The completed contract reconciliation covers export kinds, stable addresses,
dependency closure, caller bindings, and compatibility against existing
behavior and fixed vectors. Linked audio or multi-track sections remain
separate proposals and need demonstrated use before adding semantics.

### Local expressive variants

Materialization is correct for an independent copy, but costly when a shared
phrase needs one performance variation. An optional occurrence expression
contract could retain shared pitch and rhythm while overlaying one bend,
pressure, gain, or timbre curve. It must define child identity, precedence,
curve clocks, invalidation, and inverse edits; it must never mutate the shared
pattern silently.

The tradeoff is convenience against a harder source-address model. The table's
scenario tests the unresolved propagation question: a source pitch edit affects
the occurrence if pitch remains shared, while the local bend remains local.
That needs a decision before syntax.

### Multi-lane edits as transactions

Professional arranging often moves notes, recorded audio, and filter automation
together. Section 21 already supplies atomic commit, preconditions, conflict
reporting, and inverse patches. The missing high-level editor/tool contract
should identify the source interval, calculate score and physical-time effects
under existing tempo rules, copy shared curves explicitly, and lower to those
operations. Segment merges must report conflicts instead of using last-writer-
wins. This improves the workflow without making a persistent “linked chorus”
an execution concept.

Persistent clip or module-relative ownership belongs only if users need edits to
propagate between copies. It would need link identity, break-link behavior,
conflict handling, and source mapping. The language should not absorb that cost
merely because an editor offers a move command.

### Take and comp provenance

An audible comp can use existing immutable assets, source-frame slices, fades,
and overlaps, so each rendered interval has source identity. The bounded take extensions now retain
alternate assets, selection/edit history, and grouped-microphone membership.
Any broader proposal should continue to separate active audio from UI
highlighting, retain original take hashes, and use one synchronized edit
coordinate for grouped microphones. Ableton documents parallel takes, a main
audible lane, and provenance highlighting
([comping](https://www.ableton.com/en/live-manual/12/comping/)); those product
choices are evidence of a workflow need, not proposed MaaC semantics.

A persistent selection extension must be executable and pinned if it changes
audio; an inert sidecar may describe the editor view but not choose a take at
render time.

Automatic crossfades should not be inferred. Fade, phase alignment, or warp
adjustment belongs in explicit source state or a declared adapter. This is
valuable for full production; broader workflow acceptance remains separate
from the implemented take semantics.

### Articulation and pedal maps

Expression children, release velocity, and explicit messages provide building
blocks. An articulation map should be a versioned library or adapter contract
mapping advertised techniques to exact receiver actions, controller order,
initialization, state, and unsupported behavior. A notation mark alone remains
inert. Cubase's expression-map documentation describes maps as triggering
instrument articulations, illustrating receiver-specific behavior rather than a
universal MaaC sound meaning ([Steinberg Expression Maps](https://www.steinberg.help/r/cubase-pro/15.0/en/cubase_nuendo/topics/expression_maps/expression_maps_c.html)).

Sampled playback, legato, and pedal behavior need an identified instrument or
processor. A vague core “legato” keyword would hide receiver state and weaken
fidelity diagnostics. This matters for orchestral and film workflows, but should
follow a concrete adapter/library experiment.

### Faithful interchange, descriptors, and loss reports

DAWproject provides a useful reference for exchanging audio, notes,
automation, and plug-in state across beat and seconds timelines, but its
structure cannot establish universal receiver behavior
([DAWproject repository](https://github.com/bitwig/dawproject)). A profile should
name a receiver version and map each field to preserved, frozen, approximate,
or omitted output. The proposed common baseline is Standard MIDI File Type 1,
aligned PCM WAV stems, the original MaaC source and pinned dependencies, and an
explicit manifest/loss report. That combined package is a MaaC convention, not
an existing universal standard. MIDI and MPE should be independently declared
profiles, not an implied conversion for every target. The split keeps editable
note data separate from rendered audio while the source, dependencies, and
manifest remain authoritative. Ableton
documents MIDI-file import and WAV support ([Ableton MIDI](https://help.ableton.com/hc/en-us/articles/209068169-Understanding-MIDI-files),
[Ableton audio](https://help.ableton.com/hc/en-us/articles/211427589-Supported-Audio-File-Formats));
Logic documents Standard MIDI Files and WAV support ([Logic MIDI](https://support.apple.com/guide/logicpro/standard-midi-files-lgcpdf6a3851/mac),
[Logic audio](https://support.apple.com/en-euro/guide/logicpro/lgcp32add66e/mac)).
These support receiver trials, not identical import behavior. A receiver's
documented limitations also show why profile and version mapping matter
([Steinberg DAWproject exchange guidance](https://helpcenter.steinberg.de/hc/en-us/articles/31857767508242-DAWproject-Exchange-Cubasis-projects-with-Cubase-and-other-DAWs)).

Operationally, the baseline should declare one common audio origin, render
range and tail, explicit graph taps, and an optional reference mix. Selected
stems need not sum to the master when shared effects or nonlinear routing are
present, so tap semantics must be recorded ([production execution and
selection](production.md#7-shared-execution-timing-and-selection)). The
manifest is MaaC provenance, not something a DAW may be assumed to interpret;
the receiver profile must document and test tempo import, instrument setup, and
manual mapping. MIDI timing or tempo-ramp approximation, same-key overlaps,
microtonal or per-note expression, effects/routing, and unmapped automation
require source-addressed loss or refusal under [explicit loss reporting](../MaaC-1-Specification.md#25-interchange-and-explicit-loss-reporting).
Full MaaC semantics remain in the source package. “Type 1” here means Standard
MIDI File format 1, not MIDI 2.0.

The existing descriptor contract specifies stable parameter IDs, state, latency,
and determinism. The bounded external descriptor wire, dependency-discovery,
loss-report, and MIDI adapter slices now exist. A receiver mapping, executable
host runtime, aligned-stem handoff, and DAW/session adapters remain separate
later work; state hashes remain
distinct from seek checkpoints. CLAP likewise separates stable parameter
identity, per-note modulation, and latency/state change concerns ([CLAP parameter
extension](https://github.com/free-audio/clap/blob/main/include/clap/ext/params.h)).
The acceptance scenario must eventually include a real receiver; JSON checks
alone cannot prove musical interoperability.

### Deterministic transforms, quality warp, and domain profiles

Groove, harmony, and strum tools should first lower to finite notes, messages,
and curves with explicit addresses. A tool may be sophisticated, but its
output—not a hidden generator—must be what MaaC validates and renders. This
keeps reproducibility and loss reports tractable.

Quality warp is a separate engine profile. The current `warp_rate` transport
uses explicitly specified linear sample interpolation but makes no high-end
anti-aliasing claim; preserve-pitch warp already pins an algorithm, latency,
duration, and initialization. A profile proposal should therefore measure
anchors and repeatability before advertising quality. Listening is a separate
acceptance gate. Timecode, rational frame rates, channel roles, and delivery
variants should likewise be a domain profile for film scoring, rather than
changes to the core clock or silent conversions.

## Optional product adoption experiments

| Historical priority | Experiment or prerequisite | Boundary |
| ---: | --- | --- |
| 1 | Common MIDI/stems/manifest baseline tested in two named DAW versions | Relevant only if a DAW handoff workflow is selected; static schemas and hashes cannot establish receiver fidelity. |
| 2 | Pinned library export and source-address experiment | Implemented as a bounded language path; the next contract audit reuses its evidence. |
| 3 | Transactional editor/tool API, public patch/normalizer, and inspectable inverse history, if MaaC editing is in scope | Bounded public editing/normalization and inverse history already exist; broader workflow helpers remain optional. |
| 4 | DSP seek/checkpoint/window rendering plus streaming or external media handling, if standalone/full production is in scope | Current bounded full-bundle, offline 48 kHz mono/stereo operation and final-WAV excerpts do not cover long recordings, samplers, or practical previews. |
| 5 | Host/plugin and sampled-instrument adapters, if standalone/full production is in scope | A descriptor or opaque state alone does not provide a usable external host or sample-playback workflow. |
| 6 | Workload, listening, and producer evaluation | Professional readiness remains unproven without real projects, producer feedback, and separate audio-quality trials. |

The original two-consumer motif/curve experiment is now implemented in bounded
musical-library tests, including differing caller context, source mappings,
tuning, and altered-pin rejection. Contract reconciliation is complete for the
bounded P1 slice. The next work is the active language plan's evidence mapping
for Document and Performance obligations. It does not require a DAW experiment.

If a DAW handoff track is selected later, a separate experiment can export the
proposed MIDI/stems/source/manifest package to named receiver versions, inspect
preserved timing/expression/automation/audio, and record exact losses. No such
receiver trial, listening review, or producer acceptance is claimed here.
Occurrence-local expression sharing and broader mixed-media arrangement remain
optional proposals requiring their own semantics and compatibility decisions.
