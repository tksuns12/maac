# Professional production language review

**Status:** non-normative research memo; dated 2026-09-13

**Latest product direction (accepted):** MaaC should support end-to-end
production with MaaC and an open library/processor ecosystem. The
[end-to-end production plan](end-to-end-production-plan.md) carries that
roadmap. The accepted initial surface is CLI-first with playback and recording
tools; GUI work is deferred beyond the initial workflow. This memo's language
ranking remains exploratory; its MaaC to DAW handoff is optional within the
self-complete direction.

**Reader and action.** This memo is for maintainers deciding which language
proposal should receive the next design pass. The reader should be able to
select a narrowly testable proposal and adoption experiment. It does not create
L6, change MaaC-1, or approve an implementation scope.

## Historical assessment frame

This memo was prepared for an earlier workflow: compose and arrange in MaaC,
then finish in a DAW. That assessment is now superseded in product priority by
the self-complete direction, but remains useful for the optional handoff path:
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
shared execution and selection rules in [production delivery](production.md#6-shared-execution-timing-and-selection).
The core deliberately rejects implicit humanization, arbitrary computation,
guessed conversion, and hidden fallback behavior. A proposal should preserve
those properties rather than make a convenient editor action part of the
renderer by accident.

Several gaps sit around that foundation. The [local library
contract](instruments.md#documents-and-dependencies) exports instruments,
presets, and wavetables, but not reusable patterns or curves. Occurrence
expression changes can use `materialize-instance`, which creates a new pattern
for the selected placement, but no shared-pattern expression override keeps one
bend local while later pitch edits remain shared.
Notes, audio, and automation can be represented, and [exact editing
transactions](../MaaC-1-Specification.md#21-exact-editing-protocol) already
provide atomic commit and inverse patches. Missing is a high-level mixed-media
move/duplicate contract for clock conversion, curve copy-on-write, and segment
merging, plus persistent linked sections. Take editing remains an authoring
concern even though immutable assets and slices represent an audible comp.
[Generic interchange](generic-interchange.md#semantic-and-evidence-boundary) has
a contract and static corpus; a runtime lock verifier, normalizer, editor, or
general DAW/MIDI adapter does not.

These implementation facts are adoption prerequisites, not missing syntax. The
[capability matrix resource bounds](capabilities.md#resource-bounds) describe
bounded media and source limits; the runtime reads complete asset bundles,
renders offline 48 kHz mono/stereo, and supports reset-origin final-WAV
excerpts while still lacking general DSP range/seek/window rendering,
streaming, and a public patch API. Four minutes of stereo 32-bit-float audio is
about 92 MB before other assets. Raising a limit alone would not establish
streaming, retained-artifact safety, or useful previews.

## Prioritized improvements and their boundaries

The ranking measures compose/arrange leverage, contract clarity, and first-test
size. “Acceptance” is a future observable test, not a current runtime claim.

| Rank | Candidate | Boundary | Existing foundation and missing contract | First observable acceptance scenario |
| ---: | --- | --- | --- | --- |
| 1 | Pinned pattern and curve library exports | Language/library semantics | Finite patterns and curves work; exports need kinds, ownership, dependency closure, and caller bindings. | Two projects import one pinned motif and curve, place and transform them differently, preserve source addresses, and reject an altered pin. |
| 2 | Occurrence-local expression variants | Language semantics | Per-note pitch, gain, pressure, and timbre exist; complex occurrence expression currently requires materialization. | Change bend on one occurrence, change source pitch afterward, and observe local bend retention with intended shared pitch propagation. |
| 3 | Multi-lane arrangement operations | Editor/tool contract over existing transactions | Notes, audio, and automation are separately expressible; grouped selection, clock conversion, merge policy, and linked ownership are unspecified. | Duplicate an eight-bar mixed-media chorus across a tempo change with approved alignments, unchanged unrelated automation, and one inspectable inverse. |
| 4 | Faithful handoff: MIDI, aligned stems, descriptors, and loss reports | Interchange/profile and adapter contracts | Descriptor fields, latency, state, and fidelity refusal are specified; the initial MIDI 1.0 SMF format-0 adapter and versioned loss-report wire are now implemented, while aligned-stem handoff, descriptor receiver mappings, notation, and DAW-session profiles remain deferred. | A Type 1 MIDI file, aligned PCM WAV stems, original MaaC source/dependencies, and manifest account for overlaps, expression, tempo ramps, audio, and automation, rejecting any unapproved loss. |
| 5 | Deterministic groove and musical transformations | Tool lowering contract | Finite explicit data is the safe boundary; generators must not become hidden computation. | A tool lowers a groove or strum request to explicit events and curves with stable pins, addresses, and repeat output; changed inputs change the digest. |
| 6 | Versioned articulation and pedal-map adapters | Library/receiver adapter contract | Expression messages and receiver-specific controllers exist; notation marks and sampled playback are not universal semantics. | A pinned map selects two advertised techniques through a repeat and full render crop; an unsupported map fails or reports an approved loss. |
| 7 | Take, comp, and grouped-edit provenance | Editor/tool contract, conditional | Assets and slices identify audible source intervals; alternate-take preservation, audition/selection history, and grouped-mic provenance are unspecified. | Comp two phase-related mic takes while preserving originals, synchronized cuts/warp coordinates, exact undo, and no implicit crossfade. |
| 8 | Quality warp engine profile | Audio transport/processor extension, conditional | `warp_rate` uses specified linear sample interpolation; `warp_preserve` already requires a pinned algorithm and descriptor. | Vocal and transient-drum cases retain anchors, duration, channels, and pitch under a pinned profile, with repeatability and a separate listening trial. |
| 9 | Timecode, channel roles, and delivery variants | Film-oriented domain profile, conditional | Score timing and named deliveries exist; a film-oriented frame and role profile is absent. | A rational frame-rate project identifies its timecode origin and channel roles, produces declared variants, and rejects ambiguous conversion. |

### Pinned reusable musical exports

This is the best first language investigation because it compounds existing
semantics without requiring an editor or host. An export could package a
pattern, curve, or both with a versioned identity, ownership, and import
namespace. Its dependency closure should include nested patterns, tunings, and
referenced curves. A pattern still binds its destination through existing
placement and track fields, and a curve is consumed through existing expression
or automation fields; the export should not add parameterized section or port
semantics. Callers must resolve references explicitly, and an import must not
replace the composition's tempo map. Placement supplies the transform, so one
motif can serve verse and chorus without changing its source.

Questions are export granularity, stable addresses, dependency closure, caller
bindings, and compatible language/profile versions. The tradeoff is modest
authoring complexity for inspectable, hashable reuse. Start with pattern and
curve exports; linked audio or multi-track sections can follow only if real use
requires them.

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
and overlaps, so each rendered interval has source identity. Missing are
alternate-take preservation, audition/edit history, active selection, and
grouped-microphone membership. A proposal should separate active audio from UI
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
valuable for full production, but follows reusable exports because it needs an
editor model and recorded workloads.

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
selection](production.md#6-shared-execution-timing-and-selection)). The
manifest is MaaC provenance, not something a DAW may be assumed to interpret;
the receiver profile must document and test tempo import, instrument setup, and
manual mapping. MIDI timing or tempo-ramp approximation, same-key overlaps,
microtonal or per-note expression, effects/routing, and unmapped automation
require source-addressed loss or refusal under [explicit loss reporting](../MaaC-1-Specification.md#25-interchange-and-explicit-loss-reporting).
Full MaaC semantics remain in the source package. “Type 1” here means Standard
MIDI File format 1, not MIDI 2.0.

The existing descriptor contract already specifies stable parameter IDs, state,
latency, and determinism. The first handoff proposal can finish the loss-report
schema and MIDI/audio adapter before external processor descriptor wire schemas
or host runtime exist. Those descriptor and host pieces are a separate later
slice; state hashes remain
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

## Adoption prerequisites and experiments

| Priority | Prerequisite | Why it gates adoption |
| ---: | --- | --- |
| 1 | Common MIDI/stems/manifest baseline tested in two named DAW versions | The accepted workflow ends in a DAW; static schemas and hashes cannot establish a faithful handoff. |
| 2 | Pinned library export and source-address experiment | Tests the highest-ranked language candidate with existing finite pattern and curve semantics. |
| 3 | Transactional editor/tool API, public patch/normalizer, and inspectable inverse history, if MaaC editing is in scope | Makes arrangement and local-variant proposals usable without hidden mutations; it is not required to compose and arrange through an external editor workflow. |
| 4 | DSP seek/checkpoint/window rendering plus streaming or external media handling, if standalone/full production is in scope | Current bounded full-bundle, offline 48 kHz mono/stereo operation and final-WAV excerpts do not cover long recordings, samplers, or practical previews. |
| 5 | Host/plugin and sampled-instrument adapters, if standalone/full production is in scope | A descriptor or opaque state alone does not provide a usable external host or sample-playback workflow. |
| 6 | Workload, listening, and producer evaluation | Professional readiness remains unproven without real projects, producer feedback, and separate audio-quality trials. |

For the next proposal, investigate pinned pattern-plus-curve exports. The
semantic fixture should be small: one pinned motif and one curve shared by two
projects, placed at different musical locations, with changed tempo, explicit
tuning and existing placement/automation bindings, source-address inspection,
and an altered-pin rejection. This validates a language contract and is
separate from the first real adoption experiment. That experiment should have
someone compose and revise a representative short arrangement, export the
proposed common baseline (Type 1 MIDI, aligned PCM WAV stems, original MaaC
source and pinned dependencies, and an explicit manifest/loss report), and open
it in Ableton Live and Logic Pro at named versions. They should inspect timings,
expression, automation, and audio,
record exact source-addressed losses, and finish the work in a DAW. A richer
DAWproject profile is optional for receivers whose support is verified. No such
experiment is claimed here. A later fixture can test one occurrence-local
expression edit. If real use instead shows that users mainly need section
moves, implement transaction and tool rules before introducing linked section
semantics.

Open questions remain research questions: export granularity and versioning,
occurrence precedence, grouped-take representation, receiver/profile scope,
streaming boundaries, and film timecode policy. They do not block this review,
but each must be resolved before a proposal becomes normative. No runtime,
DAW round trip, listening trial, or producer acceptance was performed for this
memo.
