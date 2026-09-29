# Materializing a placement

`materialize-instance` implements the explicit authoring operation described in
[MaaC/1 §11](../MaaC-1-Specification.md#11-instance-overrides-and-inserts).
It gives one placement private copies of its pattern occurrences so that an
editor can change a repeated note or its expression without changing the other
occurrences or the original shared definitions.

```text
maac materialize-instance INPUT PLACEMENT --pattern NEW_ID -o OUTPUT
```

`--project-root ROOT` supplies the containment root for imported source files.
`--force` permits replacement of an existing output. The selected new pattern ID
must be valid and unused. The operation affects the entire selected placement,
including all its repetitions and nested uses.

Use `--json` to retain the correspondence and inverse for later editing:

```sh
maac --json materialize-instance song.maac chorus --pattern chorus_variant \
  -o edited.maac > materialization.json
```

On success, `edit` contains the applied transaction result, including its inverse
and new revision. `materialization` contains the selected placement, new pattern,
complete `mappings`, and diagnostics. Each mapping records `old_event_address`,
`new_event_address`, `old_source_object_path`, and `new_source_object_path`.

## What is copied

The operation creates a wrapper pattern and supporting private patterns. Each
placement repetition and nested use repetition gets its own invocation copy.
The selected placement points to the wrapper; other placements and original
pattern definitions retain their references.

This is a structural transformation using existing `pattern` and `use` objects.
It retains authored pitch constructors, rational stretches, transpositions, cut
boundaries, physical offsets, and expression clocks. It does not reconstruct
source from compiled floating-point pitches or sample-frame positions.

Referenced curves and tunings are copied privately for each leaf occurrence.
Imported dependencies are resolved in their original library context and copied
into local declarations. Imported files and their hash pins remain unchanged.

Existing overrides keep their final-state replacements, with their event targets
redirected to the copied occurrences. A deleted occurrence still has an address
mapping and remains deleted. Inserts stay beneath the placement and still occur
once in placement-local coordinates; they are not repeated with the new pattern.

## Address mapping and identity

The authoring result includes the correspondence between original and new event
addresses and source-object paths. This includes deleted override targets and
inserts. The destination leaf path, together with its retained expression-child
IDs, identifies the objects an editor can change in a subsequent transaction.
Use these mappings rather than depending on the spelling of generated helper IDs.

The map is result metadata. Callers that need historical correspondence should
retain the structured result alongside the edit and inverse. No new source
extension or Protocol 2 operation tag is introduced.

Materialization changes structural event identity. Timing and musical values are
preserved, but identity-dependent randomness, equal-time ordering, execution
hashes, or numerical reduction order can change. The result reports
`W_IDENTITY_CHANGE`; ordinary human-readable output displays that warning too.
Event-class ordering and authored `order` values remain in force. Byte-identical
rendered audio is not promised.

## Library API

`FoundationEditContext` and `BundleEditContext` expose
`prepare_materialize_instance(&AuthoredDocument, placement_id, new_pattern_id)`.
The returned `MaterializeInstancePlan` contains a normal `Transaction`, the
selected IDs, complete `MaterializedEventMapping` entries, and diagnostics.

```rust
let prepared = context.prepare_materialize_instance(
    document.authored(), "chorus", "chorus_variant",
)?;
let applied = document.apply(&prepared.transaction, &context)?;
```

An editor can also append operations addressing the new leaf or its expression
children and construct one transaction against the same original revision. Both
the materialization and those follow-up changes then commit atomically.

## Atomic edits and limits

Preparation constructs an ordinary Protocol 2 transaction against the original
authored revision. Existing semantic validation, source-preserving projection,
inverse generation, and publication rules apply. Failed preparation or
application leaves the source and revision unchanged; failed validation does not
publish output, including replacement requested with `--force`.

Materialization is finite and subject to the existing document, transaction,
operation, nesting, and rational-number limits. Empty repeated patterns also
consume planning work and generated-object capacity. A placement that compiles
within the performance limits may still be too large to materialize as explicit
editable source. A successful operation returns the complete mapping rather
than a truncated correspondence.

Provenance is additionally limited to 100,000 event mappings and a total of
4 MiB of serialized mapping records. Exceeding a planning allowance reports
`E_RESOURCE_LIMIT` before publication.

The operation is part of Document editing. It does not render audio or require
a device or message receiver. It provides an independent copy; later changes to
the original shared patterns, curves, or tunings do not propagate into that copy.

## Verification

The focused regression gate contains 13 [core tests](../tests/materialize_instance.rs)
and 7 [CLI tests](../tests/materialize_cli.rs). It covers private repeated and
nested occurrences, all expression clocks, exact transposition and offsets,
cut/spill, overrides and inserts, notes/hits/messages, imported ownership,
source preservation, inverses, stale revisions, bounded refusals, and output
publication. Independent implementation and acceptance review found no remaining
correctness issue. This evidence is bounded to the materialization operation.

On 2026-09-28 the combined P3/P4 verification completed the full Rust suite:
**1,428 passed, 0 failed, 3 ignored**, including doctests. Formatting,
all-target Clippy with warnings denied, diff whitespace, and added documentation
file-link checks passed. The source remained unchanged throughout the final
verification gate.

The Rust commands were `cargo test --locked --offline`,
`cargo clippy --all-targets --locked --offline -- -D warnings`, and
`cargo fmt --all -- --check`. The run used two build jobs and two test threads,
with an internal temporary `CARGO_TARGET_DIR` and
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0` to reduce artifact I/O.
Optimization and debug-assertion settings retained their defaults. The three
ignored tests were not forced to run. The local logs, stage exit codes, and
source digest are retained under `target/materialize-validation/`.

This completes the previously interrupted P3 full-suite check as well as P4's
regression gate. Release-build, installed-acceptance, and remote-CI runs were
outside this verification.
