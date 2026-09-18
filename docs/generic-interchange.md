# Generic interchange v1

**Status: normative field contract; this bounded L4 slice was addressed
locally on 2026-09-13 after static schema/corpus validation and independent
semantic and harness review.** This document defines the frozen field shapes
and semantic wire rules for the additive generic lock, configuration, and
render-input family. It preserves existing production and instrument
identities and does not change the source syntax/tree, retained-plan formats,
or DSP/rendering algorithms. A generic runtime lock verifier is implemented by `maac::generic_lock`: it validates
the closed v1 wire, ordering, digests, cross-pins, timing, evidence shape, and can
verify the lock against caller-supplied resolved execution/dependency/processor/engine
context and exact bytes. `maac::external` now adds a strict MaaC/1 §17
external-processor descriptor wire, verifies the complete owner-scoped locked
dependency closure, and exposes explicit host ABI/adapter/permission authorization.
`maac::generic_lock_normalization` now deterministically constructs canonical Config,
RenderInput, and Lock artifacts from caller-resolved semantic context and exact bytes.
`maac::generic_render` adds a bounded renderer for an already-resolved `Plan`: it first
verifies the lock inputs, requires the concrete host engine identity, accepts only the
understood built-in/core processor set and the block-independent null-schedule contract,
executes from reset through `render_frames`, then applies crop/channel order and emits raw
`pcm_f32le_interleaved/1` plus evidence. A separate `maac::external_host` boundary now supports explicitly registered executable ABI adapters, and `maac::external_native` implements the published Unix `maac.native-c-abi/1` / `maac.native-dylib-adapter/1` dynamic-library contract. The current generic renderer still rejects external nodes because the core resolved `Plan` does not yet carry them.

## Common wire rules

All keys shown in a shape are required. Null is allowed only where a table
explicitly says so. Unknown keys, duplicate JSON keys, malformed Unicode, and
unknown format or version values are rejected. The only accepted version value
is the literal JSON integer 1.

The following type abbreviations are used throughout:

| Type | Definition |
|---|---|
| D | Full-match string sha256: followed by exactly 64 lowercase hexadecimal characters |
| U | String matching the full regular expression 0\|[1-9][0-9]*; no plus sign, minus sign, leading zero, or whitespace |
| Path | Nonempty array of valid source-ID strings in the resolved namespace; each ID follows the existing bounds, including at most 128 ASCII bytes |
| typed record | A normalized MaaC §20 typed record, using its existing tags and canonical JSON rules |

Canonical JSON uses the exact §20.1/§20.2 rules: UTF-8, no BOM or final
newline, no insignificant whitespace, Unicode-code-point key ordering, and
mandatory lowercase \u00xx escaping for control characters rather than
optional short escapes. Reduced tagged rationals are used instead of JSON
floating-point numbers, and no extra hash prefix bytes are added. A transport
may pretty-print or reorder object keys, but digest input always uses these
exact canonical bytes. The field order in the tables describes the shape; it
does not override canonical object-key sorting.

## Top-level artifact shapes

### Configuration

Config has exactly these fields:

| Key | Type or value | Required meaning |
|---|---|---|
| format | "maac.generic-config" | Generic configuration discriminator |
| version | 1 | Generic family version |
| processor_type | Nonempty string | Processor type identifier |
| descriptor_hash | D or null | External descriptor pin; null for a core processor bound by its type and engine |
| config | Normalized §20 typed record | The node.config record only, with defaults from the understood processor type or descriptor expanded and nested labels retained |

The configuration digest is SHA-256 of the complete canonical Config object
bytes with no prefix. It includes the processor and descriptor context in the
envelope, including the understood type/schema context represented by
processor_type and descriptor_hash, and all normalized configuration data,
including defaults and nested labels. An omitted node.config normalizes to the
understood empty/default record. It excludes node identity, node.params, and
processor state.

For example, these are the exact canonical UTF-8 bytes for a core sine Config
whose normalized config contains voices = 64. The sequence has no BOM and no
final newline:

    {"config":{"fields":{"voices":{"d":"1","n":"64","t":"number"}},"t":"record"},"descriptor_hash":null,"format":"maac.generic-config","processor_type":"core.sine/1","version":1}

The matching [canonical Config artifact](../conformance/l4/canonical/minimal-config.json)
and [L4 SHA-256 ledger](../conformance/l4/SHA256SUMS) record the fixed digest.

### Render-input

RenderInput has exactly the six render-input fields plus its discriminator and
version:

| Key | Type or value | Required meaning |
|---|---|---|
| format | "maac.generic-render-input" | Generic render-input discriminator |
| version | 1 | Generic family version |
| execution_hash | D | §20 normalized execution identity |
| assets | Asset[] | Complete content-addressed asset inputs |
| dependencies | Dependency[] | Complete declared dependency closure |
| processors | Processor[] | Processor and configuration inputs |
| engine | Engine | Engine and numeric execution context |
| output | Output | Normalized render selection and raw output contract |

There is no stored render_key or evidence field in RenderInput. The render key
is SHA-256 of the exact canonical bytes of this complete envelope, with no
prefix bytes. It identifies the pre-render inputs and conditions. The
execution_hash remains the §20.2 N(A) hash under the
maac.execution.sha256/1 algorithm context; this family adds no algorithm key.

### Lock

Lock has exactly these fields:

| Key | Type or value | Required meaning |
|---|---|---|
| format | "maac.generic-lock" | Generic lock discriminator |
| version | 1 | Generic family version |
| execution_hash | D | §20 normalized execution identity |
| assets | Asset[] | Complete content-addressed asset inputs |
| dependencies | Dependency[] | Complete declared dependency closure |
| processors | Processor[] | Processor and configuration inputs |
| engine | Engine | Engine and numeric execution context |
| output | Output | Normalized render selection and raw output contract |
| render_key | D | SHA-256 of the exact canonical RenderInput envelope; identifies pre-render inputs |
| evidence | Evidence or null | Optional output evidence, never part of render_key |

An optional external whole-lock digest may bind the complete canonical lock,
including evidence, but no self-referential whole-lock field is stored in the
lock. PCM and file hashes in evidence are separate output evidence and do not
affect render_key.

## Nested field shapes

### Asset

Asset has exactly these fields:

| Key | Type or value | Required meaning |
|---|---|---|
| source | Path | Actual source asset identity in the resolved namespace |
| kind | "audio", "blob", "descriptor", or "module" | Asset role |
| sha256 | D | Exact asset-byte digest |
| bytes | U | Verified byte length |

Assets are unique by source identity and sorted by the unsigned UTF-8 bytes of
their canonical source-path identity. Equal hashes do not merge distinct
source IDs.

### Dependency

Dependency has exactly these fields:

| Key | Type or value | Required meaning |
|---|---|---|
| owner | Path or null | Owning source, processor, or package-level engine context |
| role | Nonempty array of nonempty strings | Exact role in the understood dependency registry |
| sha256 | D | Exact dependency-byte digest |
| bytes | U | Verified byte length |

The v1 role registry is closed:

| owner | Allowed role | Cardinality or meaning |
|---|---|---|
| null | ["engine", "implementation"] | Required exactly once; package-owned engine implementation |
| Processor node Path | ["processor", slot], where slot is implementation, descriptor, adapter, or state | Processor-owned pin |
| Source Path or null | ["imported-source", capability_id, source_id] | Imported source or package-owned import |
| Source Path or null | ["auxiliary", capability_id, slot_id] | Other contract-declared closure member |

capability_id, source_id, and slot_id are nonempty strings interpreted by an
understood published contract. An imported source_id identifies the qualified
dependency slot under that import capability, including declaration or alias
identity; it is not merely the target source path. Two slots that name the
same target retain two dependency records. Unknown or unqualified roles and
slots are rejected at the capability boundary. Dependencies are sorted by
the unsigned UTF-8 bytes of the canonical identity object
{owner:<owner>,role:<role>}; equal hashes do not merge distinct identities.
No untyped context map or fallback role is part of the v1 contract.
Every processor-owned role must refer to an existing processor and its pin
must agree with that processor's corresponding field. The complete closure
must be established by the understood contract, including transitive imports,
modules, adapters, numeric tables, and state. A static lock does not claim
that an implementation discovered every dependency. Resolution does not
guess paths, fetch from a network, or silently substitute a missing artifact.

### Processor

Processor has exactly these fields:

| Key | Type or value | Required meaning |
|---|---|---|
| node | Path | Processor node identity |
| processor_type | Nonempty string | Processor type identifier |
| implementation_hash | D or null | Per-node implementation pin; null for core behavior bound by its understood type contract and engine |
| descriptor_hash | D or null | Per-node descriptor pin; null for core behavior bound by its understood type contract and engine |
| adapter_id | Nonempty string or null | Textual identity of an understood published adapter contract; null when no adapter applies |
| state_hash | D or null | Exact source-declared initial state pin; null declares reset state, not unknown state |
| config | Config | Complete normalized configuration envelope |
| config_digest | D | SHA-256 of config canonical bytes |
| latency_frames | U | Declared fixed technical latency |
| determinism | "declared_deterministic" or "nondeterministic" | Determinism declaration |

External processors require non-null implementation_hash, descriptor_hash, and
adapter_id values. Core processors have all three values null unless their own
definition explicitly permits a per-node slot; core behavior is bound by the
understood processor contract and engine, never by a processor_type string
prefix heuristic. There is no corresponding processor role for a null field.
Processor hashes must match their corresponding dependency role records and
applicable source-asset pins. A non-null adapter_id requires the unique
processor/adapter role record to pin an artifact that implements the named
adapter contract; no redundant adapter hash field exists. The outer
processor_type and descriptor_hash context must agree with the nested Config
envelope.

A non-null state_hash is source-declared initial state applied at reset frame
zero under the understood contract's §17 load order: restore complete state,
then apply the configuration ABI, then apply explicit parameter overrides.
The pinned descriptor defines state serialization. It is not a checkpoint,
prehistory shortcut, or crop-restart state. Core state is prohibited unless
the core definition explicitly allows it under §15.

latency_frames and determinism must equal the understood type or descriptor
contract for the pinned configuration and state. A processor entry cannot
silently alter either value.

Processors are sorted by the unsigned UTF-8 bytes of their canonical node Path
identity.

### Engine

Engine has exactly these fields:

| Key | Type or value | Required meaning |
|---|---|---|
| implementation_id | Nonempty string | Engine implementation identity |
| build_id | Nonempty string | Engine build identity |
| platform_id | Nonempty string | Platform identity |
| architecture_id | Nonempty string | Architecture identity |
| numerical_mode_id | Nonempty string | Numeric execution mode |
| sample_rate | Positive U | Must equal the normalized project sample rate |
| block_schedule | Positive U[] or null | Positive block lengths spanning reset through render_frames; null only when block-independent |

An understood published engine or adapter contract is required to interpret
these identities and schedules. The engine implementation and build IDs must
match the required engine implementation artifact contract. Non-null
block_schedule lengths must sum exactly to render_frames. A pinned adapter
determines all relevant subordinate schedules; unknown schedule requirements
are rejected. Null requires a proven block-independent contract.

### Output

Output has exactly these fields:

| Key | Type or value | Required meaning |
|---|---|---|
| port | Non-null typed port reference | Project output port |
| score | [reduced q, reduced q] | Project's normalized score interval, with start < end |
| tail | Nonnegative reduced s | Project's normalized tail |
| render_frames | U | Exact reset-relative frame count |
| crop | [U, U] | Reset-relative half-open frame crop |
| channel_order | U[] | Exact output-channel permutation using indices 0 through C-1 |
| encoding_id | "pcm_f32le_interleaved/1" | Initial generic raw output encoding |
| clipping_id | "none" | No clipping |
| dither_id | "none" | No dither |

score and tail are exactly the project's normalized output selection; they are
not independently reinterpreted by this family. The output port reference must
resolve to an actual non-null typed port. Frame zero resets at
O = T(score.start), and render_frames is
ceil(sample_rate * (T(score.end) + tail - O)), with the existing exact
time-coordinate rules and no rational/decimal assumption for a tempo-ramp
origin. crop is reset-relative, half-open, and satisfies
0 <= crop[0] <= crop[1] <= render_frames.

If the selected output has C channels, channel_order is exactly a permutation of
the indices 0 through C-1 and has length C; it is not restricted to a
natural-only channel order. The initial generic output is raw interleaved
little-endian binary32:
no resampling, clipping, or dither, and no container settings. Conversion
rounds to IEEE-754 binary32 with round-to-nearest, ties-to-even; it preserves
signed zero and representable subnormals, follows binary32 rounding for
underflow, and rejects nonfinite values or conversion overflow.

### Evidence

Evidence is either null or an object with exactly these fields:

| Key | Type or value | Required meaning |
|---|---|---|
| pcm_sha256 | D | Digest of the cropped interleaved PCM bytes |
| pcm_bytes | U | Exact PCM byte length: crop frames × channels × 4 |
| file | {sha256:D,bytes:U} or null | Optional separate container/file evidence |

When the raw profile supplies file, its bytes equal the PCM bytes. A future
container profile requires its own explicit version and does not inherit this
claim. Evidence is an assertion to verify, not proof of execution; no
numerical tolerance is introduced by this artifact.

## Ordering and compatibility

Assets, dependencies, and processors are unique by their canonical identity
and use the ordering rules above: source Path for assets, the explicit
owner/role identity for dependencies, and node Path for processors. Equal
hashes do not collapse distinct IDs or dependency slots. Meaningful order is
retained for channel, block-schedule, and typed-value lists.

The generic family is independently discriminated from existing production
and instrument formats. Unknown versions, capabilities, roles, or required
contracts are rejected; receivers do not guess, retag, or silently migrate a
generic artifact into a production identity.

## Semantic and evidence boundary

The envelope defines the field set and its schema establishes structure. The
§20 canonical byte codec and the independent checker establish canonical
bytes; schema validation alone does not reject every duplicate-key, lexical,
or canonical-order violation. The host remains responsible for semantic
verification: resolving the source
graph, computing the §20.2 N(A) execution hash under
maac.execution.sha256/1, validating normalized configuration and defaults,
checking processor type/descriptor context, verifying state and latency
contracts, resolving the output port and score/tail, and establishing the
complete dependency closure. A static corpus can verify structure, exact
preimages, digests, and rejection vectors; it does not discover dependencies,
normalize a runtime lock, or prove an actual render.

The source and dependency identities, configuration digest, engine and
processor context, block schedule, and output selection must agree
semantically before a host accepts a lock or render key. Any required
processor, engine, adapter, descriptor, state, imported-source, module, or
numeric-table contract that is unavailable or unknown is rejected explicitly.
No diagnostic API is added by this format; source-operation failures use the
established language diagnostic codes, while schema, semantic, and runtime
evidence remain separately reported.

An artifact may be transported with pretty-printing or a different object-key
order, but a digest is computed only from the exact canonical JSON bytes.
Canonicalization never accepts a BOM, surrogate, duplicate key, or untagged
JSON floating-point value in place of a typed or decimal field. A migration
must reconstruct the target envelope and recompute every affected digest; it
must not retag or copy hashes from another artifact role.

## Evidence status

The accepted local executor evidence records 54 corpus files, 15 schema documents, 23
focused L4 tests, and 54 file hashes checked by both shasum and OpenSSL. Its
standard-library checker and focused tests passed, including symlink and
capability-ordering controls after the corresponding RED cases. These are
static schema, byte, digest, and rejection results. The independent harness
findings on strict manifest canonical-flag typing and confinement and symlink
checks before reading fixed manifest/hash inputs were corrected; four focused
regression tests then passed. Independent semantic and harness reviews are
complete for this bounded contract. The Rust `maac::generic_lock` verifier now adds
runtime lock-envelope and caller-resolved-context verification, including exact
block schedule, crop/channel order, closure bytes, and optional PCM/file evidence.
The Rust `maac::external` boundary now parses the strict §17 descriptor wire,
discovers and verifies complete processor-owned locked dependency closures, and
requires explicit host capability authorization without executing module bytes.
The Rust `maac::generic_lock_normalization` boundary now generates canonical v1 Config,
RenderInput, render-key, and Lock artifacts from typed resolved context, independently
round-trips them through the validator/verifier, and keeps optional output evidence out
of the render key. Bounded built-in/core rendering is provided by `maac::generic_render`; explicit native ABI invocation is provided separately by `maac::external_host` and `maac::external_native`. Wiring those executable external instances into a resolved generic render graph remains separate work.
