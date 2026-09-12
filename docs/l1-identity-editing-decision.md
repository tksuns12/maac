# L1 identity and editing decision

**Status: ADOPTED 2026-09-13.** Authored field presence is canonical for L1
revision and editing semantics. This decision is reflected in the normative
§20–§21 amendment; it does not claim that a runtime normalizer or editor has
implemented the full protocol.

## Decision boundary

L1 aligns §20 canonical data and hashes with §21 authored-presence and
transaction behavior. The decision fixes whether omitted defaults and authored
equivalents share revision identity, how `expect_absent` behaves, what an
inverse restores, and which bytes a patch precondition names. The label-role
clarification in §20.2 is independent: only an actual source
`Object.fields.label` display field is excluded from execution identity while
nested record labels remain data unless an understood explicit schema marks
them nonexecuting.

## Adopted decision: authored state is canonical

Define a canonical authored typed graph **A** for revision and editing. It keeps
the existing field maps and empty records, authored units, constructors,
reference paths, labels, and object/child identities. It reduces rational
values to canonical numerator/denominator form and excludes text comments and
whitespace from canonical bytes. Semantic normalization **N(A)** is a separate
execution view; it must never silently materialize defaults or lower `bar` calls
into **A**.

Under this decision, the revision is `SHA-256(canonical A)`. Execution is built
from `N(A)` with the scoped §20.2 display-label exclusions. Omitted `tail` and
explicit `tail = 0s` therefore remain distinct authored revisions while their
execution hashes agree when all other source and dependency data are equal.
Authored `20ms` and `1/50s`, or `bar` and its resolved `q` address in a fixed
meter context, keep their source representation for identity and editing while
defined normalization makes the corresponding typed execution values equal.
In a pitch context, defined normalization likewise makes `C4` and `key(60)`
equal; audible coincidence between distinct pitches does not collapse them.

Patch expectations compare values semantically in a fixed base context,
including type, meter, dependencies, and labels. Target paths address the
progressively edited candidate; existing objects keep their base correspondence
through renames and field/record replacement, while newly inserted objects have
no base correspondence and cannot carry base preconditions. Deleting and
reinserting an ID creates a new identity for that object and its descendants.
An inverse restores the canonical authored tree, including field absence, units,
and constructors; it uses the final revision as its base and does not promise
to restore original text formatting.

**Acceptance.** Vectors must cover omitted versus explicit defaults, unit and
constructor preservation, `expect_absent`, rename-then-edit paths, conflicts,
and inverse restoration. They must show separate revision identity alongside
any execution equivalence and preserve nested labels described above.

## Evidence boundary and status

The portable [L1 identity/edit corpus](../conformance/l1/manifest.json) contains
57 files, 4 normalization groups, 2 global-position vectors, 20 expected patch
cases (9 success and 11 failure), and 1 metadata projection. The semantic
vectors and this specification have independent planner clearance at manifest
hash `ff5cc38e80b86da7c0ed0e4d666369d91c3de32434e941ff59f99da78ddbd221` and
specification hash
`f59426ca3ad8223daca042a8fe7d173db497344ed50bc338d2bdc5b20d73aa41`.
The final checker evidence covers 25 complete typed documents structurally
validated against `syntax-tree.schema.json` and 57 dual-digest checks; 18
targeted regression tests pass, including the missing-required-file regression.
The 20 patch cases are reviewed static expected outcomes, not transactions
applied by a runtime editor. The ignored local [stage2 summary](../target/l1-identity-validation/stage2/summary.json)
records this evidence. Independent semantic/specification, compatibility, and
checker-final-delta review passed. This evidence covers static self-consistency
and expected vectors, not execution by a runtime patch editor or normalizer.

## Rejected alternative: normalized state is authoritative

Revision bytes and patch expectations using normalized state `N(A)` were
considered and rejected. That choice would make authored equivalents such as
omitted versus explicit defaults collapse earlier and could simplify canonical
interchange. It would also weaken source-preserving edit identity: authored-
presence operations would need corresponding normalized-state rules for
`unset`, `expect_absent`, inverse patches, units, and constructors. An editor
could lose the distinction needed to restore the authored tree, so the
alternative trades simpler revision equivalence for more complicated editing
semantics.

## Compatibility and status

Protocol 2 is independently versioned from `maac 1` and syntax-tree version 1.
It uses the descriptive revision context `maac.revision.authored.sha256/1` and
preserves retained-plan formats and all current Rust production/import hashes.
It does not silently reinterpret version 1: a version-2-only receiver rejects a
version-1 transaction with `E_CAPABILITY` before mutation, without guessing or
converting a normalized base hash. No new public schema is finalized here.

The authority choice is resolved by this adopted decision. Specification and
vector acceptance for this L1 slice are complete as of 2026-09-13, with
implementation status reported separately. Runtime/editor conformance remains
separately deferred and is not an L1 acceptance dependency for this
specification slice.
