# Authored revisions and the Protocol 2 editing kernel

## Status and scope

This is an additive **P0 foundation slice**, not a completed Document-profile
implementation. `maac::editing` provides authored revision identity, strict
Protocol 2 decoding, and an atomic typed-tree transaction kernel. It introduces
no source-language syntax, performance-plan version, or DSP behavior. Existing
production/import identities and static conformance fixtures are unchanged.

A **trusted `EditContext` implementation is mandatory**. The bounded
`FoundationEditContext` now supplies the implemented source-only core semantics and
normalization contract, while unsupported library/extension contexts fail explicitly.
`SourceDocument` projects accepted transactions through parser spans without flattening
unrelated source text, and `maac patch` exposes that path through the CLI. The test-only
`FixtureContext` remains intentionally narrow. Full Document-profile disclaimers remain
in force.

## Public API

`AuthoredDocument::from_document` accepts the existing parsed `Document`.
`AuthoredDocument::from_json` accepts the tagged syntax-tree JSON. Both preserve
authored omissions, explicit empty records, unit tags, constructors, labels,
object/child identities and compact pattern structure. Rational scalars are
reduced with a positive denominator. Shape validation does not assert semantic
validity of a source graph or availability of its extensions.

`canonical_bytes()` uses the existing MaaC canonical JSON implementation.
`revision()` is `sha256:` plus SHA-256 of those bytes, without a prefix in the
preimage. The algorithm identifier `maac.revision.authored.sha256/1` is metadata.
Source comments and formatting are not part of the typed authored graph.

`Transaction::from_json` accepts the exact Protocol 2 envelope. It rejects
unknown or duplicate JSON keys, malformed Unicode, JSON floating-point values,
unknown tagged shapes, invalid paths, invalid revision strings, false/null
absence guards, and coexistence of `expect` and `expect_absent`. An unsupported
protocol is refused before inspecting its base revision or operation semantics.
Programmatically constructed transactions are rechecked at the apply boundary.

`apply_transaction` supports all five operations:

- `set` and `unset` traverse existing records only. They never synthesize an
  absent intermediate record or address a list by an unstable numeric index.
- `insert_object` and `delete_object` manipulate complete typed subtrees.
  Deletion severs immutable-base correspondence for every descendant.
- `rename_id` moves an identity and rewrites tagged source references. Existing
  descendants keep their base correspondence. The context must also rewrite
  occurrence addresses and declared structural mappings, or reject the rename.

No rename rewrites a later operation's target, expectation, or write payload.
Preconditions always refer to the original base through surviving identity
correspondence. Reinserting an identical object under a deleted ID does not
restore that correspondence. Authored absence is checked without defaults;
effective-value expectations use the context's fixed-base normalization.

The implementation validates the original source, works on a private candidate,
and validates the complete final candidate before committing. Temporary
semantic invalidity is allowed between operations. Structural and resource
limits still apply to those intermediates. A failure in any gate, including
inverse construction, leaves the caller's document and revision unchanged.

## The semantic boundary must not be stubbed

The context must provide all of the following, within its declared supported
capabilities and resource limits:

1. Complete source semantic validation, including references, types, required
   fields, temporal constraints, pattern cycles, occurrence overrides, writer
   uniqueness, graph causality and applicable dependencies.
2. Fixed-base object normalization with specified defaults and **labels retained**.
3. Fixed-base field normalization in the actual source type, meter, name and
   dependency context. Do not use audible equivalence or candidate context.
4. Structural reference rewriting for occurrence addresses and understood schemas.

The production execution-hash projection is not directly suitable for edit
preconditions: it removes source labels. Likewise, source-schema validation
alone does not cover all the later compiler checks. Returning raw JSON as a
normalization fallback, silently ignoring structural mappings, or calling a
renderer as the only validity gate would not satisfy this contract.

Context methods are trusted Rust host code, not executable document fields.
They must not mutate external state while evaluating a private candidate.
Unknown required capabilities must fail with `E_CAPABILITY`, not be guessed.
The editing kernel itself performs no file access, network access or rendering.

## Inverses and impact

Every committed transaction returns a serializable Protocol 2 inverse based on
the final authored revision. The inverse stores complete prior values via
changed-root subtree replacement. It is not intended to be a minimal textual
patch. Changed/new roots are removed before original roots are inserted, so a
rename inverse does not transiently duplicate two large subtrees. No optional
guards are copied from intermediate states.

Applying the inverse restores the original canonical authored tree, including
omitted records/fields, labels, typed units and constructors. It does not
restore comments, formatting, or transient identity history outside a transaction.
Both forward and inverse must fit the same public transaction bounds before
commit. An otherwise valid large edit may therefore fail with `E_RESOURCE_LIMIT`
when a valid bounded inverse cannot be returned.

Impact reporting includes a bounded list of changed source paths, total count,
truncation flag, renamed identities, and—when the semantic context can prove the
source dependency closure—a bounded list of expanded event addresses. The
`FoundationEditContext` now distinguishes three render scopes: `none` for
non-executing label-only changes, `affected_events_and_dependents` for bounded
pattern/place changes, and `full` for global or otherwise unbounded execution
changes. Unknown impact remains conservative. This is still not a sample-accurate
dependency-region analysis. Renames return `W_IDENTITY_CHANGE` because IDs can
affect randomness, summation order and hashes.

## Bounds

Document and transaction JSON each have a 4 MiB allowance. Identifiers have the
existing 128-ASCII-byte limit and rational components the existing 4096-bit
limit. Additional kernel bounds are 1,024 operations, 200,000 JSON values,
64 logical typed/path levels, 96 physical JSON nesting levels, and a 256 MiB
conservative transaction byte-work allowance. Programmatic JSON values and
parsed-but-programmatically-modified source ASTs are preflighted before
recursive conversion/serialization. Callback implementations must independently
bound their normalization, dependency and structural-rewrite work.

At most 1,024 affected source paths and 1,024 affected expanded event addresses
are returned; separate total counts and truncation flags prevent either bounded
summary from being mistaken for a complete listing.

## Tests and validation

The Rust integration target is:

```sh
cargo test --locked --offline --test editing
```

It contains 39 tests covering the 18 fixed L1 authored snapshots and focused
transaction, precondition, identity, inverse, rollback and hostile-wire cases.
Only the authored revision test executes the frozen L1 snapshots directly.
The other tests exercise the kernel with the explicit test-only context.
They do **not** run or certify the complete L1 normalization/edit corpus.

On 16 September 2026, this kernel was compiled and tested on macOS arm64
with Rust 1.95.0 against repository commit
`aaa6b63609ab984f50a7872c0d2689efb08dd321`. The 39-test editing target,
full default Rust test suite (including doctests), formatting check, and
Clippy with warnings denied passed. The L1 static corpus, its 18 Python
checker tests, and the conformance evidence index also passed. The original
patch needed rustfmt changes; no behavioral correction was required by these
gates. Rust builds used `CARGO_BUILD_JOBS=2`; the full test run used
`RUST_TEST_THREADS=2`. Ignored tests were not forced to run.

These are local execution results, not a GitHub CI pass or full Document conformance.
Subsequent work connected the complete existing L1 normalization/position/Protocol-2
corpus to `FoundationEditContext`, added source-preserving projection, and added the
`maac patch` API/CLI path. Unsupported library and extension editing contexts remain
explicit capability boundaries.

## Remaining work before closing the first P0

Expand editing normalization beyond the source-only foundation to reusable libraries,
imports, native production extensions, and future external processor descriptors without
weakening the explicit capability boundary. Extend bounded impact beyond pattern/place
source dependencies to processor/asset/dependency-region analysis where the host can prove
it, and add broader source-format preservation cases for complex inserted/deleted subtrees.

Native message resolution is the other P0 from the implementation audit and is
not changed by this patch.
