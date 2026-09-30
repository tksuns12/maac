# L4 portable interchange decision

**Status: Accepted material direction and bounded L4 slice — 2026-09-13.** The user approved the
additive generic version-1 direction, preservation of existing production and
instrument formats, the pre-render render-key boundary, and separate PCM/file
hash evidence. The [Generic interchange v1](generic-interchange.md) document
records the frozen field shapes and semantic wire rules. The original contract slice did
not claim runtime implementation. Subsequent bounded runtime slices now provide generic v1
lock generation/normalization and independent verification, plus strict external descriptor
discovery. A later bounded runtime slice adds generic rendering for already-resolved built-in/core plans under the concrete host engine identity and null block schedule. A subsequent host slice publishes and implements an explicit Unix native dynamic-library ABI boundary; a subsequent bounded bridge executes one verified output-only external generator with zero technical latency, static parameters, and an explicitly block-independent adapter; mixed external/core graph integration remains deferred.

## Purpose and fixed context

MaaC-1 §20 defines canonical authored data **A**, the separate normalized
execution view **N(A)**, revision and execution hashes, and a render key. §22
defines dependency-lock and reproducibility requirements while separating
source, performance, and audio equivalence. L1 fixed SHA-256 for the relevant
canonical hashes and fixed the canonical JSON encoding plus the authored
presence rules for **A** and **N(A)**. L4 must build on those rules rather than
introducing a second identity authority. Existing production and instrument
identity records retain their current identities and compatibility contracts.

## Accepted direction and wire contract

The accepted direction is an additive generic version-1 family with independent
discriminators for three related artifact roles:

- a **lock** artifact that records the dependency and execution conditions
  needed to reproduce a declared result;
- a **configuration** artifact that identifies the relevant configuration
  capture and its declared implementation context; and
- a **render-input** artifact that identifies the inputs presented before a
  render, including the applicable source/execution identity and other
  declared dependencies required by §22.

The generic family version is not a new language, syntax-tree, retained-plan,
production, or instrument version. The three roles must remain independently
recognizable when transported or rejected. A generic artifact must not be
accepted as an existing production or instrument identity merely because it
has similar data.

The [Generic interchange v1](generic-interchange.md) document records the
exact envelope fields, dependency-role registry, canonical byte boundaries,
ordering rules, and semantic verification boundary for this direction.

The accepted render-key boundary covers the pre-render inputs and conditions.
Expected PCM bytes and container/file bytes are output evidence of the
resulting render; their hashes are separate from the render key and do not
affect it. A matching render-input key therefore does not by itself claim
audio equivalence; §22's declared numerical bound or byte-identical PCM
evidence still governs that claim.

## Compatibility and scope boundary

The accepted direction is additive and preserves current
production/instrument identities, existing source and retained-plan formats,
and the L1 revision and execution-hash rules. A receiver that does not
understand the generic family must distinguish it from known identities and
reject or preserve it according to the existing extension and capability
rules; it must not silently reinterpret it as a different artifact role.
Version transitions and rejection follow the strict version, capability, and
migration contract in [Generic interchange v1](generic-interchange.md).

The field contract now provides the portable normative shape and byte
boundaries. The accepted local executor evidence covers 54 corpus files, 2
configurations, 4 render-inputs, 6 locks, 28 invalid artifacts, 3 portable
pairs, 3 timing vectors, 15 schema documents, 23 focused L4 tests, and 54
shasum/OpenSSL-verified file hashes. Its checker passed the fixed inventory,
structure, canonical bytes, digests, relationships, and rejection vectors;
the symlink and capability-ordering RED controls now fail as intended. The
semantic/specification review passed. The independent harness findings on
strict manifest canonical-flag typing and confinement/symlink checks before
reading fixed manifest/hash inputs were corrected, and four focused regression
tests then passed; both independent reviews are complete. Subsequent runtime slices added
strict external descriptor/discovery support, explicit interchange loss reporting, typed
generic v1 lock generation/normalization plus verification, bounded built-in/core generic rendering, and an explicit native ABI host boundary. General mixed external-node graph integration and the L5 numerical metric and bound remain follow-ups. This slice does
not add a processor, alter DSP behavior, or claim hosted-CI or cross-platform
audio conformance.

## Evidence boundary and next bounded slice

SHA-256 and the L1 **A**/**N(A)** canonicalization rules are fixed, as are the
accepted render-key boundary and the separation of output evidence. The
executor evidence is recorded in the local
[validation record](../target/l4-interchange-validation/executor-summary.json).
All evidence must continue to distinguish schema structure, semantic host
verification, and actual runtime or repeated-render claims. Generic v1 lock
construction and verification now exist, `maac::generic_render` supplies the bounded built-in/core execution bridge, and `maac::external_native` supplies a published explicit native ABI loader contract. This still does not constitute full Locked Render coverage: the external render bridge is limited to one output-only generator with explicitly block-independent execution, and no mixed DAG or sandbox/subprocess contract is claimed.
