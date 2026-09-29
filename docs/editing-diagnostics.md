# Original-source locations for editing errors

P7 preserves available source locations through source editing and CLI patch
errors, as required by specification §23. It does not change Protocol 2,
authored revision identity, execution identity, or transaction commit policy.

## Location contract

`EditError::span` is an optional half-open UTF-8 byte range into the source
passed to `SourceDocument::parse`, or into the document's source at the start
of a failed `SourceDocument::apply`. Parser errors preserve the original parser
range. Path-based editing errors identify the original authored field **value**,
including nested record leaves; an object-only error identifies the original
object range. These are locations of the original material being edited, not
of the invalid replacement supplied in a transaction.

The source boundary uses the kernel's surviving immutable-base identities.
Renamed objects and descendants retain their original locations. Deleting and
reinserting an object with the same ID does not restore its source identity.
New objects, unauthored/defaulted fields, missing paths, and diagnostics without
a proven local correspondence have no span. A missing leaf does not fall back
to its parent record or object. Every returned range is checked against the
original string's UTF-8 boundaries.

Foundation validation reconstructs an AST and bundle validation may synthesize
source text or diagnose another file. Their offsets are never copied into an
editing error. Private diagnostic provenance distinguishes current candidate
paths, fixed-base normalization paths, and unknown source ownership. Bundle
resolution, descriptors and artifact checks conservatively retain no mapped
location; entry-source semantic paths can be mapped, while semantic diagnostics
for another selected bundle source remain unlocated when ownership is uncertain.
Host contexts report paths in the authored input of the corresponding method.
For foreign or uncertain ownership, they must call
`EditError::without_source_mapping()`; subsequent `at` calls retain that policy.
Refinement failures have no defined source correspondence and remain unlocated.
Projection failures in privately edited text likewise do not expose candidate
offsets as original-source ranges.

`CliError::from_edit` forwards the optional span while preserving the code and
joined object/field path. Existing human output prints `(start..end)` and JSON
output exposes `span.start` and `span.end`. Failure occurs before source or
revision assignment and before output publication, including `patch --force`.

## Public API compatibility

The existing constructors, `at`, error codes, paths and messages remain
available. `EditError::new` initializes `span` to `None`; serialization omits
the field when no location is available, preserving the previous spanless
JSON shape. Consumers enforcing exact error-JSON keys must accept the optional
`span` field when a location is present. Equality compares the public diagnostic fields, including span,
and ignores private provenance. `EditError` display formatting is unchanged;
CLI display includes the location through its existing span formatter.

Adding a public field and private provenance to the public struct is a Rust
source compatibility change for external struct literals and exhaustive
destructuring. Callers should use
`EditError::new(code, message).at(&object_path, &field_path)` and destructure
with `..`. The provenance is private and absent from serialized diagnostics;
no new transaction field or hash input is introduced.

## Verification

The [public source/CLI regressions](../tests/editing_diagnostics.rs) cover original
parser errors after Unicode text, existing scalar and nested record/child values,
rename correspondence, absent fields, delete/reinsert identity, direct typed
foundation/bundle errors, library resolution, foreign/local ID collisions, and
CLI human/JSON errors with preserved input and existing output under `--force`.
The [host-context regressions](../tests/editing_diagnostic_context.rs) cover
replacement of synthetic offsets, unknown ownership and refinement failures,
rename callback timing, fixed-base delete expectations, failure before a later
rename, and parent/child rename chains. Spanless serialization and public
equality remain independent of internal provenance. The initial parser/tail
regressions failed before implementation because `EditError` lacked `span`.

Adding the shared optional span and private provenance makes `EditError` 128
bytes on the verification platform. The editing module and one existing test
fixture helper explicitly allow only `clippy::result_large_err`, with comments
explaining preservation of the by-value `EditResult` and `Option<Span>` APIs.
No crate-wide warning policy is relaxed. Downstream functions returning the
public by-value `EditResult` may likewise need a scoped `result_large_err`
allowance under warnings-denied Clippy.

The focused editing/CLI gate passed **97 tests** across eight integration test
executables, including all **18 new P7 regressions**. `cargo fmt --all -- --check`,
`git diff --check`, and `cargo clippy --locked --offline --all-targets -- -D warnings`
passed. The doctest gate passed its one test. Independent planning/compatibility
review and implementation/test review were completed; findings concerning
callback offset leakage, external host opt-out, and collision-fixture provenance
were corrected and verified. Pre-existing diff blocks outside P7-owned files
were checked against the saved pre-P7 diff and remained unchanged.

The repository-wide gate passed **1,462 Rust tests**, with **3 ignored** and no
failures, across **178 Cargo-built test executables**. Cargo established the
complete inventory with `cargo test --locked --offline --no-run --message-format=json`;
all executables ran from local temporary storage with the repository as their
working directory. As in P6, this avoids slow external-volume executable launches;
CLI subprocesses still used Cargo's built CLI. The final diagnostic targets were
rebuilt after the collision-fixture correction and passed both their focused run
and the complete inventory run. Only lint allowances/comments changed after the
full inventory build; they do not change runtime behavior. The separate
`cargo test --locked --offline --doc` gate passed **one doctest**. This evidence
closes D14's bounded source-editing/CLI location gap without asserting complete
profile-wide diagnostic coverage.
