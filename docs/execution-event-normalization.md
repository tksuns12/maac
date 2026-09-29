# Execution identity for expression and message events

P6 closes an execution-normalization gap for event kinds already accepted by
MaaC/1. It does not add source syntax or a new performance-plan version.

## Baseline failure

`examples/pitch-expression.maac` compiles to a resolved performance with two
notes, but `production_identity::execution_identity` rejects its `expression`
children with `normalization is not defined for object kind expression`.
Likewise, adding a valid expression or message to the native production example
made artifact compilation fail while attaching execution identity. These are
valid feature combinations that must remain available to production projects.

## Contract

The authored typed graph **A** keeps each declared field, original unit,
object/child ID, and label. The execution view **N(A)** remains compact:
patterns, notes, expression children, and messages are normalized in place,
without replacing them with expanded occurrences. It retains executable
expression kinds and curve references, and exact message protocols and bytes.

Omitted message `onset_offset` and `order` receive their specified defaults in
N(A). Explicit equivalent values produce the same execution identity but
remain distinct authored states. Physical units normalize to canonical
execution units. Actual object display labels remain in editing normalization
and are omitted only from the execution identity projection; protocol strings,
bytes, expression kinds, curves, and other payload values remain hash inputs.

This correction does not imply that the built-in renderer can execute raw
messages. Its receiver and adapter capability checks remain separate from
compilation and identity.

## Verification

The [event identity tests](../tests/execution_identity_events.rs) cover execution
identity for the existing pitch, gain, timbre, and pressure expression examples;
production compilation with a note carrying two expressions and a message;
identity equality for omitted and explicit message defaults; identity
sensitivity to changed bytes, protocol, curve values, and curve references;
and complete P5 query payloads. The [editing regression](../tests/execution_identity_editing.rs)
uses an `fx.reverb/1` node to select artifact normalization, checks its
plan-derived defaults, then applies a revision-bound label edit without
changing execution identity. An invalid candidate is rejected atomically.

The scoped gate passed 52 integration tests across event identity, editing,
production data, messages, materialization, and P5 queries, plus all 13
existing `production_identity` unit tests. The repository-wide gate passed
1,444 Rust tests across 176 Cargo-built test executables, with 3 ignored, and
one doctest. Cargo built the complete inventory with `--no-run`; to avoid slow
launches from the external volume, 175 executables ran from local temporary
storage. The updated `tempo_compile` target ran through Cargo and passed all
10 tests. Its prior assertion expected production identity to reject a valid
expression; it now verifies the successful bundle and tempo-sensitive hash.
The ordinary `cargo test --locked --offline --quiet` command was stopped after
its library gate because external-volume test launches were unusually slow.
Rust formatting, diff whitespace, and all-target Clippy with warnings denied
passed. Independent review found no runtime semantic or hash-compatibility
issue; its editing-fixture coverage finding was corrected and re-reviewed.
This bounded correction does not prove complete normalization for every core
or extension field.
