# MaaC end-to-end production plan

**Status:** accepted product direction; non-normative capability roadmap; dated
2026-09-13

## Capability

MaaC should become a self-complete production environment: a person can
compose, record or import, edit, arrange, design sounds, mix, master, deliver,
archive, and reopen a project using MaaC and an open library and processor
ecosystem. The project source, its render-affecting state, and its pinned
dependencies remain authoritative throughout that lifecycle. A DAW handoff is
an optional interoperability path. A paid or proprietary DAW is not a required
part of the product's completion path.

This direction is accepted at the product level. The CLI-first surface with
playback and recording tools is accepted for the initial workflow. The
architecture, milestones, wire formats, and processor ABI remain proposals
requiring contracts; device and host integration choices remain open. The current
[capability matrix](capabilities.md), [design principles](design-principles.md),
and [Core Audio audit](core-audio-conformance-audit.md) describe a bounded
foundation; source descriptions and retained plans do not yet establish a
dependency-complete editable archive or full professional production.

## End-to-end outcome

The finish line is one reopenable project whose complete authored state a fresh,
open-baseline environment can understand and render. Each stage needs a durable
artifact and explicit error boundary.

| Stage | Capability that must exist | Durable result |
| --- | --- | --- |
| Compose | Notes, patterns, timing, tuning, expression, and arrangement remain source-addressed and editable. | MaaC source graph with authored musical intent. |
| Record/import | Recorded performances and external audio enter through identified, hashable assets with format, rate, channel, frame, and provenance metadata. | Original media plus an explicit imported or recorded source object. |
| Edit | Source edits, takes, comp selections, fades, crops, and grouped changes have atomic commit, conflict handling, and inverse history. | Editable history that never silently changes the active audible choice. |
| Arrange | Notes, audio, curves, automation, and routing can be moved or duplicated with explicit score/seconds behavior. | A mixed-media arrangement with inspectable source addresses and timing. |
| Sound design | Open instruments, sampled instruments, articulation maps, and versioned native processors expose stable controls, state, and dependencies. | Reproducible processor graph and library identity. |
| Mix | Routing, sends, sidechains, automation, latency, alignment, freeze boundaries, and channel roles remain explicit. | A renderable mix graph and invalidation-aware intermediate assets. |
| Master | Metering, true-peak policy, sample-rate conversion, encoding, dither, and release limits are selected explicitly. | Master and delivery policies with measured final artifacts. |
| Deliver | Masters, stems, reference mixes, and loss reports describe what was preserved or approximated. | Published audio plus a manifest of outputs and decisions. |
| Archive | Source, dependencies, media, processor descriptors/state, history, plans, and manifests form one portable package. | A dependency-complete editable archive. |
| Reopen | A clean provisioned environment verifies every pin and reconstructs the editable project before rendering. | Successful reopen, or an actionable missing-dependency error. |

An archive must preserve all render-affecting authored state: source forms and
addresses, library and asset pins, processor IDs and descriptors, non-parameter
state, latency and alignment decisions, automation, routing, delivery choices,
and the dependencies needed to interpret them. A missing asset, library,
descriptor, or processor must fail explicitly. The loader must never guess a
replacement, fetch an unpinned version, or turn an unavailable node into silent
audio. UI history may be retained for editing, but it cannot secretly choose an
audible take.

## Constraints and shared interfaces

The authored graph and its normalized execution view must remain distinct. A
portable archive carries the authored source and dependency closure; a retained
plan is derived. It enables fresh-environment reopen, but cannot promise
byte-identical audio across machines unless a separate render
profile identifies the exact implementation, numeric environment, processor
state, and output evidence. The existing [generic interchange contract](generic-interchange.md)
already separates render inputs from output evidence; the end-to-end package
must preserve that distinction.

Provisioning may obtain pinned packages, libraries, and processor modules, but
compile, edit, preview, and render should work offline afterward. Network access
must not become an implicit dependency of a render. Package policy must preserve
licenses, notices, source identity, and version pins for open content and code.
The exact license allowlist, notice layout, and whether a library may bundle
third-party sample content remain open decisions.

Long recordings expose a capability boundary. The current implementation uses
bounded full-bundle media and an offline 48 kHz mono/stereo profile. Four minutes
of stereo float audio is roughly 92 MB before takes and metadata. A professional
path needs streaming or external-media handling, disk-backed assets, safe
crop/checkpoint behavior, and resource accounting that does not pretend a larger
limit is streaming. Recording must define rate, channels, origin, latency,
dropout recovery, and whether a preview is verified or an approximation.

## Responsibility boundaries

| Boundary | Owns | Must not silently own |
| --- | --- | --- |
| Language and libraries | Musical meaning, source addresses, reusable content, explicit transforms, and dependency pins. | Hidden generators, implicit tempo replacement, or receiver-specific sound behavior. |
| Engine and runtime | Timing, graph execution, state, latency, crops, resource limits, and render evidence. | Guessed substitutes, undocumented compensation, or cross-machine exactness. |
| Tools and editor | Recording/import, audition, source-addressed edits, comp history, arrangement transactions, and delivery commands. | Unpinned render state or UI choices that alter audio invisibly. |
| Open processors | Versioned native instruments/effects, descriptors, controls, state, permissions, and deterministic profiles. | A dynamic host ABI or proprietary plugin assumption before that contract is accepted. |

The common interfaces are:

- A source package holds original MaaC documents, pins, imported media, library
  provenance, processor descriptors/state, and editable history. Its closure is
  explicit and inspectable.
- A compiler and editor expose source-addressed validation, atomic edits,
  inverse patches, dependency invalidation, and clear diagnostics. They must
  preserve authored state rather than only a normalized render view.
- An engine accepts a verified graph, executes declared processor and timing
  contracts, and reports latency, crop, tail, resource, and nonfinite failures.
- An open processor contract provides stable versioned identity, ports,
  parameters, event and expression behavior, state, latency, determinism, and
  permissions. Versioned native processors should become useful before a
  dynamic host/plugin ABI is selected.
- Delivery and interchange exporters emit audio, manifests, and exact
  source-addressed losses. The optional DAW baseline can use Standard MIDI File
  Type 1, aligned PCM WAV stems, original source/pinned dependencies, and a
  manifest; this package is a proposed MaaC convention, not a universal session
  format. A richer DAWproject profile is optional when receiver support is
  verified.

## Dependency-ranked roadmap

The sequence is a dependency order; parallel work remains possible.
Source/archive completeness and processor contracts are shared long-term
foundations, while the excerpt can proceed independently. Dynamic hosting should
not precede native open sounds or symbolic note handoff.

### 1. Reference end-to-end inventory

First map the current source, retained-plan, library, audio, production, and
interchange boundaries to the stages above. Mark each behavior as supported,
partially supported, importer-only, statically specified, or missing. This
inventory must include sample and recording inputs, latency, crop, disk assets,
multiple takes, automation, routing, delivery, archive, and reopen. The [release
readiness](release-readiness.md) and [capability matrix](capabilities.md) are
starting evidence, not a full-production claim.

### 2. Reset-correct offline excerpts and previews

The first bounded implementation slice is the [reset-correct WAV range
export](render-range.md). Its implementation and focused evidence are recorded
in [reset-correct WAV range validation](render-range-validation.md):

```text
maac render PLAN -o WAV --start-frame N --end-frame M
```

`N` and `M` are unsigned reset-origin frames with
`0 <= N <= M <= plan.output.total_frames`; an empty `[N,N)` range is valid.
The renderer validates and charges the complete plan, processes reset-state
through the declared end (including conversion), and encodes only `[N,M)`.
Existing encodings/default results remain; an explicit range reports `M-N`, and
selected PCM must equal the same-format full-render slice in the same
environment. Six targeted tests and an isolated release-built CLI raw-byte
comparison are complete and independently reviewed; the recorded full Rust
gate also passed. Excerpt acceptance is complete for this bounded final-WAV
contract. No speed-up, realtime, arbitrary seek, build, or delivery claim is
made.

### 3. Usable media, import, streaming, and editable packaging

Next, make long recordings and imported material practical. Add disk-backed or
streaming asset access, explicit import/decoder identities, recording metadata,
dropout/error recovery, and resource-safe crops. Define an editable package
that includes original media, pinned dependencies, processor context, source
history, and a normalizer or editor that can reopen it without losing authored
presence. A freeze must record its boundary, input/dependency hash, state,
latency, interval, and tail; an upstream edit invalidates it. Source archive
portability and exact same-environment PCM replay are separate acceptance
claims.

### 4. Capture, playback, revision, comping, and arrangement

With package and excerpt foundations in place, support capture and practical
editing: alternate takes, synchronized grouped microphones, comp selection,
source-addressed fades and warps, multi-lane moves, revision conflicts, and
inverse operations. Build on the existing exact editing protocol rather than
inventing a second mutation model. Persistent linked sections should be added
only if real workflows require propagation after duplication. Recording and
playback must remain explicit about device latency, monitoring versus render
paths, reset origin, and crop prehistory.

### 5. Open samplers, processors, libraries, and plugin contracts

Expand the open ecosystem with pinned sampled instruments, articulation and
pedal maps, sound libraries, and versioned native processors for the common
production path. Finish descriptor wire schemas, loss reports, state handling,
declared-latency validation, and permissions. Dynamic latency notifications, if
needed by a host, belong to future host design. Then evaluate an open SDK and
dynamic host contract. The host choice, ABI, sandbox, realtime guarantees, and
external plugin standard remain open; opaque plugin state alone is not a usable
host. Unsupported required processors must fail in faithful native production;
approved loss reports apply only to explicit conversions or approximations, and
no core node may be silently omitted.

### 6. Mix, master, release workload, and clean reopen

Complete the production path with quality transport profiles, channel roles,
alignment, mix and master automation, delivery variants, metering, and archive
publication. Test a real multi-minute production containing synthesizers, a
sampled instrument, recorded audio, multiple takes, automation, routing, mix,
master, and more than one delivery. Edit an upstream dependency, verify the
right renders invalidate, then reopen the final archive in a freshly provisioned
environment using only the open baseline. Confirm that source-addressed losses,
PCM/file evidence, and same-environment claims are each reported at their own
boundary.

## Acceptance and evaluation

Structural checks can prove pins, closure, schemas, authored presence, and
diagnostic paths. Runtime checks can prove reset-correct excerpts, resource
limits, replay, latency alignment, and retained artifacts. Neither proves that
a workflow is comfortable for a producer or that a warp, sampler, compressor,
or master sounds professional. The finish line requires representative
workloads, human listening, and producer acceptance; those evaluations have not
been performed. A DAW import trial is similarly separate from source/package
validation and must name the receiver and version, inspect preserved fields, and
record exact losses.

## Non-goals for this roadmap

This plan does not make a proprietary DAW a dependency, promise every existing
plugin, define a universal session-file format, or silently translate every
MIDI, MPE, automation, articulation, or effect feature. It does not turn a
GUI sidecar into render state, claim network discovery during offline work, or
replace explicit processor contracts with arbitrary code. It does not approve
changes to MaaC/1 grammar, schemas, or runtime APIs beyond separately accepted
slices.

## Open decisions and handoff

The remaining product decisions include package licensing/notices,
recording/device scope, media/checkpoints, processor state/ABI, sandbox and
permissions, and optional DAW/MIDI/DAWproject profiles. The
initial surface is accepted as CLI-first with playback and recording tools;
visual GUI work is deferred beyond that workflow. These are not blockers to the
accepted direction, but each must be resolved before its implementation slice.

The immediate handoff after excerpt acceptance is the practical media/import and
playback/capture contract work. In parallel, maintain the end-to-end inventory
and define the dependency-complete archive contract. Do not describe the
excerpt as completing end-to-end production; completion requires the full
workflow and the workload, reopen, listening, and producer gates above.
