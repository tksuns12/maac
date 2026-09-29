# Document and Performance evidence map

**Scope:** bounded P2 audit of the repository's declared language subset.
This is a contributor's map from normative obligations to public interfaces,
fixed cases, and remaining limits. It does not certify an entire conformance
profile. The [active language plan](language-specification-plan.md#current-priorities)
uses this map to select a concrete follow-up.

## How to read the map

[MaaC-1 §1.2](../MaaC-1-Specification.md#12-conformance-profiles) defines
Document as parsing, preservation, validation, normalization, and editing;
Performance additionally resolves musical events and controls. Performance
requires no audio engine. Core Audio execution and environment-qualified
Locked Render claims add separate obligations.

Each row names a bounded obligation, its public boundary, and the evidence
that supports that exact claim. A test name is an executable fixture, not a
proof for every valid document. The final execution record identifies which
tests were freshly run for this audit. Source inspection and static fixture
validation are recorded separately from execution.

| Status | Meaning |
| --- | --- |
| **E — evidenced within limits** | The inspected implementation and named executable cases support the stated subset. The row's limits and the execution record qualify the claim. |
| **S — specified-only** | A contract or static expected vector exists, without a corresponding public runtime path established by this audit. |
| **U — unsupported** | The current declared boundary excludes the behavior; an explicit refusal is distinct from silently changing its meaning. |
| **V — unverified** | The audit does not establish the behavior. This is an evidence gap, not proof that the feature is missing or incorrect. |
| **D — demonstrated discrepancy** | A public-boundary reproduction contradicts a stated obligation. This is a known failure, recorded separately from unsupported or unverified behavior. |

This is a requirement-group audit, not an exhaustive enumeration of every
normative sentence or a coverage percentage. §§2–16 and §§19–26 contribute
language, validation, identity, or interchange obligations. §18's audio
algorithms are outside the primary profiles mapped here; selected tests may
use a renderer as an observation boundary without making DSP a prerequisite
for Performance.

## Document obligations

The repository does not claim the complete Document profile. These supported
boundaries are useful individually; **V** rows identify incomplete proof of
broader obligations, even when implementation checks exist. Test names in a
row are from its linked target unless another target is named explicitly.

| ID / clause | Bounded obligation | Public boundary and fixed evidence | Expected result | Status and limit |
| --- | --- | --- | --- | --- |
| D01 — §2 | Exact grammar, source/spans, malformed-string refusal, stable version diagnostics | `parse`; [syntax unit tests](../src/syntax_tests.rs): `parses_example_and_preserves_source_spans`, `accepts_exact_values_and_rejects_bad_grammar`, `rejects_invalid_json_strings_and_unpaired_surrogates`, `reports_unsupported_version_as_a_maac_diagnostic` | `0.10` is exact `1/10`; source is retained; malformed strings fail `E_SYNTAX` and unsupported versions fail explicitly | **E** for named cases; exhaustive lexical boundary coverage **V**. |
| D02 — §§2, 4 | Reject unknown/duplicate source structure, including unused declarations | `parse`, `semantic::validate_source`, `apply_transaction`; [semantic tests](../tests/semantic.rs): `rejects_unknown_kinds_and_fields_with_stable_codes`, `validates_unused_declarations_and_does_not_discard_bad_objects`, `validates_graph_nesting_without_expanding_performance`; [editing tests](../tests/editing.rs): `duplicate_ids_and_field_child_collisions_fail_without_mutation` | Unknown fields/kinds and invalid nesting fail; duplicate edits do not mutate A | **E** for tested cases; full accepted/rejected inventory and all nesting combinations **V**. |
| D03 — §2.1 | Entry selection is deterministic and explicit filenames remain valid | CLI; [entrypoint tests](../tests/cli_project_entrypoint.rs): `omitted_input_uses_main_for_check_compile_and_build_without_changing_bytes`, `directory_inputs_select_their_main_and_keep_relative_outputs_in_cwd`, `no_main_does_not_search_neighbors_ancestors_or_descendants`, `explicit_files_and_library_checks_keep_their_existing_contract` | Omitted/directory input selects `main.maac`; absence never triggers neighboring-file search | **E** for the stated CLI selection boundary. |
| D04 — §§3.1, 20.2 | Preserve authored distinctions while canonicalizing exact rationals | `AuthoredDocument`; [editing tests](../tests/editing.rs): `authored_distinctions_are_not_normalized_away`, `scalar_reduction_and_negative_zero_are_canonicalized`, `invalid_denominators_are_rejected_before_any_mutation` | Omission differs from explicit zero; `20ms` differs from `1/50s` in A; invalid denominators fail atomically | **E** for canonical authored values; execution equivalence is a separate view. |
| D05 — §§3.1–3.3 | Enforce dimensional types, lower constructors separately, retain reference ownership | `semantic::validate_source`, editor normalization and bundle resolution; [semantic tests](../tests/semantic.rs): `validates_units_references_curves_and_single_automation_writer`; [required-config tests](../tests/required_processor_config.rs): `omitted_required_config_reports_range_at_source_and_compile_boundaries`; [foundation editing tests](../tests/editing_foundation.rs): `foundation_context_matches_all_l1_position_resolution_vectors`, `foundation_context_rewrites_occurrence_addresses_on_source_id_rename`; [P1 evidence](musical-library-contract-validation.md) | Bad units/references fail; two fixed bar-position vectors lower correctly; renames and library-owned references retain their declared ownership | **E** for named contexts; every constructor location and every dimensional field **V**. |
| D06 — §19 | Regions are valid non-rendering spans and survive editing | `semantic::validate_source`, `SourceDocument::apply`; [source editing tests](../tests/editing_source.rs): `insert_and_delete_object_keep_existing_text_byte_stable` | Inserting `[0q,1q]` preserves unrelated source bytes; inspected internal validator checks span order/containment | **V** for complete region semantics: dedicated public vectors for reversed/outside spans, overlap/nesting and nonacoustic behavior were not established. The insertion case alone does not prove them. |
| D07 — §20.1 | Typed values have closed tagged forms and dictionaries own IDs | `Document::to_syntax_json_value`, `AuthoredDocument::from_json`, `Transaction::from_json`; [syntax unit tests](../src/syntax_tests.rs): `emits_the_typed_syntax_tree_shape`; [editing tests](../tests/editing.rs): `duplicate_json_keys_are_rejected_at_every_depth`, `wire_rejects_empty_paths_unknown_tags_and_floats`, `payloads_with_object_ids_or_unknown_tag_fields_are_rejected` | Example tree matches fixed JSON; malformed tagged values, duplicate keys and extra fields fail | **E** for runtime cases. The [syntax schema](../syntax-tree.schema.json) separately supplies **S** shape evidence, not semantic validation. |
| D08 — §20.2 | Canonical A and revision have exact bytes and exclude formatting | `AuthoredDocument::canonical_bytes` / `revision`; [editing tests](../tests/editing.rs): `all_l1_authored_snapshots_have_the_recorded_canonical_bytes_and_revisions`, `canonical_source_hash_excludes_comments_order_and_whitespace`, `canonical_output_uses_lowercase_control_escapes_and_no_final_newline` | Eighteen fixed authored snapshots match canonical bytes/digests; no final newline; lowercase control escapes | **E** for the fixed snapshots and formatting cases. |
| D09 — §20.2 | N(A) is non-mutating and distinct from authored identity | `FoundationEditContext::normalize_document`; [foundation editing tests](../tests/editing_foundation.rs): `foundation_context_matches_all_l1_normalization_vectors`, `foundation_context_matches_all_l1_position_resolution_vectors`, `editing_and_execution_normalizers_share_core_semantics`; [P6 event identity](execution-event-normalization.md#verification) | Ten frozen normalization members and two position vectors match independent canonical expected bytes without rewriting A; expression children and messages retain executable identity with specified defaults | **E** for the original corpus and bounded P6 correction; complete core/extension normalization remains **V**. |
| D10 — §20.2 | Execution projection removes display labels by role, preserving payload labels | `execution_identity` requires already-validated source/plan context; [identity unit tests](../src/production_identity.rs): `labels_comments_and_declaration_order_do_not_change_execution_hash`, `complete_extensions_and_unknown_payload_labels_are_never_discarded` | Display-only edits retain execution hash; unknown payload labels are retained | **E** for inspected runtime cases; the full L1 metadata-role fragment remains **S** expected-vector evidence. Hashing alone does not validate unknown extension semantics. |
| D11 — §21 | Strict Protocol 2, original-base expectations, reversible atomic operations | `apply_transaction`, `FoundationEditContext`; [foundation editing tests](../tests/editing_foundation.rs): `foundation_context_executes_all_l1_protocol2_patch_cases`; [editing tests](../tests/editing.rs): `unsupported_protocol_precedes_revision_and_operation_checks` | All 20 frozen cases execute: success trees/revisions/inverses match fixtures; failure preserves A; protocol 1 fails before revision checks | **E** for this corpus. Other editing-kernel tests using `FixtureContext` prove kernel behavior, not general semantic validation. |
| D12 — §§16, 21 | Final semantic validation rejects graph cycles before edit commit/publication | `SourceDocument::apply`, foundation/bundle contexts, CLI patch; [P3 regression evidence](#p3-correction) | A gain self-cycle must fail `E_ALGEBRAIC_LOOP`, with unchanged source/revision and no output publication | **E** for the P3 cases: atomic refusal, direct validation/normalization, delayed feedback, modulation cycles, final-state repair, and CLI output preservation. The historical P2 discrepancy is retained below. |
| D13 — §21 | Source projection preserves unrelated text and reports bounded edit impact | `SourceDocument::apply`; all six [source editing tests](../tests/editing_source.rs); [foundation editing tests](../tests/editing_foundation.rs): `label_only_edits_do_not_invalidate_execution_or_render`, `pattern_leaf_edits_report_bounded_occurrence_addresses`, `global_execution_changes_keep_full_invalidation` | Untouched text is byte-stable; invalid edit rolls back; label/local/global changes report their defined invalidation scopes | **E** for tested shapes/scopes; exact dependency-region analysis is not provided. |
| D14 — §23 | Diagnostics identify stable code/path and available span | `compile`, `SourceDocument::parse/apply`, `CliError::from_edit`, CLI patch; [compiler tests](../tests/compiler.rs): `subsample_failure_keeps_source_path_and_span`; [required-config tests](../tests/required_processor_config.rs): `empty_required_configs_keep_their_field_specific_diagnostics`; [P7 public regressions](../tests/editing_diagnostics.rs) | Fixed compiler failures retain source context; P7 retains original UTF-8 parser/value/record/child ranges and CLI locations, with atomic refusal and conservative unknown provenance | **E** for named compiler cases and the bounded [P7 editing-location contract](editing-diagnostics.md); comprehensive diagnostics remain **V**. Reconstructed foundation/bundle offsets are never claimed as original-source locations. |
| D15 — §§23–24 | Explicit resource bounds and offline dependency integrity | Parser/editor/bundle loaders; [syntax unit tests](../src/syntax_tests.rs): `enforces_source_and_nesting_limits`, `limits_rational_component_bits`; [editing tests](../tests/editing.rs): `oversized_input_rationals_and_nesting_fail_explicitly`, `inverse_operation_budget_is_checked_before_the_forward_commit`; [bundle tests](../tests/bundle.rs): `preflights_counts_and_aggregate_bytes_across_unreachable_inputs`, `filesystem_loader_checks_import_pins_before_parsing_dependencies`, `filesystem_loader_rejects_source_and_asset_symlink_escapes`; [module tests](../tests/module_artifact.rs): `strict_wire_rejects_tampering_missing_extra_and_malformed_members` | Bounds, wrong pins, malformed closure and path escapes fail explicitly; inverse-budget failure prevents forward commit | **E** for selected boundaries, each with its own limits. No native execution or arbitrary compressed-archive claim. |
| D16 — §26 | Unknown required semantics are preserved for inspection but not silently normalized; version domains are independent | `AuthoredDocument`, editor contexts, production descriptor checks; [foundation editing tests](../tests/editing_foundation.rs): `foundation_context_refuses_capabilities_without_edit_normalization_contracts`; [production-data tests](../tests/production_data.rs): `invalid_source_data_is_rejected_before_objects_are_removed`, `descriptor_requires_recognized_exact_bytes_and_matching_pin`; D01/D11 version cases | Unsupported normalization fails explicitly; exact recognized descriptor bytes and pins are required; source 1 and protocol 2 are distinct | **E** for tested supported contexts/refusals; **U** for unknown-extension semantic normalization. Raw preservation/hashing does not establish a general extension validator. |

The L1 index retains its historical `static_corpus` classification. The later
`editing_foundation` harness also executes its 20 transactions, ten normalization
members and two position vectors through a real runtime context. These are
different evidence boundaries; neither fact should erase the other.

## Performance obligations

Single-source paths use `parse` then `compile` or `compile_versioned`;
`compile_bundle_artifact` resolves and compiles a `SourceBundle`.
`SourceBundle::resolve` separately exposes dependency resolution. `Plan` and `PlanArtifact`
provide retained-plan validation; `PlanArtifact::performance_dispatches`
resolves event dispatches against declared adapters. None of these claims
requires a live audio engine.

| ID / clause | Bounded obligation | Public boundary and fixed evidence | Expected result | Status and limit |
| --- | --- | --- | --- | --- |
| P01 — §§5–6.1 | Exact origins, tempo/meter conversion, physical offsets, frame ceilings and event-class order | Compile/Plan/dispatch; [foundation tests](../tests/foundation.rs): `thirds_and_step_tempo_changes_schedule_exact_frames`, `negative_pickup_and_meter_bar_position_share_the_exact_origin`; [tempo tests](../tests/tempo_compile.rs): `ramps_are_additive_and_certified`, `shifted_origins_offsets_and_end_clamp`, `unresolved_source_frame_boundary_reports_precision`; [compiler tests](../tests/compiler.rs): `score_end_truncation_happens_after_release_offset`; [message tests](../tests/message_performance.rs): `adapter_protocol_is_exact_and_same_frame_class_order_is_normative` | Certified frames or explicit precision failure; offsets precede end clamping; shared-frame messages precede note-offs, then note-ons | **E** for tested step/ramp/boundary cases. This event-class evidence is distinct from sample-time parameter evaluation during Core Audio execution. |
| P02 — §7 | Resolve explicit pitch constructors and tuning without an implicit MIDI range | Compiler and musical resolution; [music tests](../tests/music.rs): `spelled_key_pitch_preserves_accidentals_and_crosses_octaves`, `tuning_uses_floor_division_for_negative_degree_indexes`, `absolute_ratio_and_nyquist_rules_reject_nonfinite_or_aliased_values`; [L3 tuning tests](../tests/l3_tuning.rs): `tuning_corpus_matches_literal_pitch_and_diagnostic_contract` | Defined frequencies or prescribed range/precision/capability errors; negative degrees use floor division | **E** for these constructors and fixed [L3 expected values](../conformance/l3/tuning/expected.json), not all numerical boundaries. |
| P03 — §§8–8.1 | Preserve message protocols/bytes/timing; resolve note expression against receiver capabilities | Compile, retained Plan and dispatch; [message tests](../tests/message_performance.rs): `message_protocol_bytes_timing_and_roundtrip_are_preserved`, `nested_messages_inserts_and_overrides_expand_by_address`, `message_bytes_are_strict_and_builtin_renderer_rejects_missing_adapter`; [pitch Plan tests](../tests/pitch_expression_plan.rs): `all_clocks_validate_effective_domain_and_gate_endpoint`; [gain Plan tests](../tests/gain_expression_plan.rs): `all_clocks_and_interpolations_support_zero_and_unbounded_finite_gain`; [timbre](../tests/timbre_expression_compile.rs) and [pressure](../tests/pressure_expression_compile.rs) compile targets | Exact payloads, frames and curve fields survive; unsupported expression receivers fail explicitly; missing/wrong message adapters fail at dispatch or rendering, while compilation can retain their exact protocols | **E** for tested clocks, receiver opt-ins and adapter contracts. No live MIDI/device transport is claimed. |
| P04 — §§9–10 | Expand finite nested uses/placements with transforms, cuts and structural addresses | Compile; [compiler tests](../tests/compiler.rs): `nested_cut_is_applied_before_physical_offsets_and_mapping_is_source_stable`, `oversized_repetition_is_rejected_even_when_the_pattern_is_empty`, `nested_empty_repetitions_are_rejected_by_preflight_without_expanding`; [foundation tests](../tests/foundation.rs): `nested_stretch_cut_keeps_physical_offsets_unscaled`, `unused_bad_maps_fields_and_cycles_are_still_diagnosed`; [P5 score-window tests](../tests/windowed_event_query.rs) and [CLI tests](../tests/windowed_event_query_cli.rs) | Deterministic finite events/addresses or explicit cycle/resource failures; physical offsets are not stretched; half-open score-window queries return complete final gates, payloads, frames, and source mapping | **E** for full expansion and bounded [P5 score-window queries](windowed-event-query.md). P5 filters a fully compiled performance and does not establish lazy expansion or reduced global limits. |
| P05 — §11 | Apply occurrence override/delete/insert in final placement coordinates | Compile; [compiler tests](../tests/compiler.rs): `occurrence_pitch_replacement_is_final_and_does_not_reapply_transposition`; [foundation tests](../tests/foundation.rs): `occurrence_edits_are_isolated_and_use_final_placement_coordinates`; [message tests](../tests/message_performance.rs): `nested_messages_inserts_and_overrides_expand_by_address` | Only the addressed occurrence changes; replacement pitch is final; inserted events retain identity | **E** for the tested source compilation cases. Materialization is a separate editing boundary, recorded below. |
| P06 — §§12–13 | Resolve automation/control modulation and validate clocks, anchors, units, targets and dependencies | Compile/retained Plan; [foundation tests](../tests/foundation.rs): `seconds_automation_anchor_at_global_bar_is_lowered_exactly`; [tempo tests](../tests/tempo_compile.rs): `anchors_keep_their_source_clock`; [plan-validation tests](../tests/plan_validation.rs): `rejects_duplicate_automation_writers_and_invalid_curve_shapes`; [modulation tests](../tests/modulation_compile.rs): `native_controls_lower_to_v7_with_exact_defaults`, `event_rate_core_targets_lower_with_zero_and_nonzero_amounts`, `invalid_declarations_and_references_fail_even_unused` | Exact lowered records or explicit semantic errors, including conflicting writers and unused invalid declarations | **E** for the declared control subset; no DSP-output claim follows from lowered records. |
| P07 — §14 | Verify pinned assets and lower kit/audio/warp transport metadata | Bundle artifact compile; [kit tests](../tests/kit_compile.rs): `nested_hits_edits_and_source_free_assets`, `empty_and_unused_assets_are_preserved_and_verified`; [audio tests](../tests/audio_compile.rs): `defaults_are_native_sources_with_exact_assets_and_source_mapping`, `invalid_modes_intervals_references_and_budgets_fail`; [warp tests](../tests/warp_compile.rs): `native_warp_defaults_and_exact_recipe`, `warp_rejections_and_shared_expanded_point_budget` | Asset bytes and transport records survive compilation; malformed modes/resources/budgets fail | **E** for supported lowering. Playback is separate Core Audio evidence. **U** for pitched sample instruments, `warp_preserve`, and audio placements inside patterns; hit-based sample kits do not imply pitched instruments. |
| P08 — §§15–16 | Validate port contracts and combined same-sample graph dependencies with explicit causal breaks | Plan validation used by compilation/artifact loading and instrument graph validation; [gain tests](../tests/core_gain.rs): `invalid_automation_and_single_input_cycles_are_rejected`; [delay tests](../tests/core_delay_plan.rs): `delay_input_breaks_only_delay_edges_in_retained_causality`; [graph tests](../tests/graph.rs): `validates_every_node_and_rejects_mixed_audio_modulation_cycles`, `topology_uses_all_edge_types_and_node_ids_for_stable_ties` | Bad ports/cycles fail `E_PORT_TYPE` / `E_ALGEBRAIC_LOOP`; the tested delay breaks only its causal dependency; ordering is deterministic | **E** at these Plan/instrument-graph boundaries. D12 records the subsequent P3 evidence for final graph validation at edit commit. |
| P09 — §11 | Explicitly materialize a placement as independent patterns, preserve source-address mapping and redirect that placement | Foundation/bundle `prepare_materialize_instance`, `SourceDocument::apply`, and CLI `materialize-instance`; [core tests](../tests/materialize_instance.rs) and [CLI tests](../tests/materialize_cli.rs) | Every repeated/nested occurrence is independently editable; overrides, inserts, exact transforms and library-owned references retain their meaning; complete mapping and inverse are returned | **E** for the bounded [P4 materialization contract](materialize-instance.md). Supporting private patterns use existing syntax; mapping is result metadata. Structural identity changes are explicit and byte-identical rendering is not promised. |

## Related interchange and execution boundaries

These rows prevent existing portable contracts from being confused with a
complete host or renderer. Their supported APIs can be used independently of
device integration.

| ID / clause | Bounded obligation | Public boundary and fixed evidence | Expected result | Status and limit |
| --- | --- | --- | --- | --- |
| X01 — §§17, 24 | Strict processor descriptors, owner-scoped dependency identity, and explicit authorization | `ExternalProcessorDescriptor::from_json`, `discover_external_processors`, `ExternalHostCapabilities::authorize`; [external tests](../tests/external.rs): `descriptor_parser_is_strict_and_validates_connection_contracts`, `discovery_verifies_full_owned_closure_and_state_contract`, `host_authorization_is_explicit_for_abi_adapter_permissions_and_determinism` | Unknown fields and malformed descriptors fail; missing/tampered closure and incompatible state fail; unapproved permissions or ABI fail | **E** for descriptor/discovery/authorization APIs. **U** for executable external module hosting in this runtime; these tests never instantiate a plugin. |
| X02 — §§20, 22 | Canonical lock construction binds resolved inputs and keeps output evidence out of the render key | `GenericLockBuilder::build`; [generation tests](../tests/generic_lock_generation.rs): `minimal_generation_matches_all_canonical_artifacts`, `transitive_imported_auxiliary_and_same_byte_slots_are_retained`, `evidence_is_output_only_and_does_not_change_render_key` | Fixed canonical bytes match L4 fixtures; distinct dependency slots remain distinct even for identical bytes; adding PCM/file evidence leaves the render key unchanged | **E** for caller-resolved typed context and supplied exact bytes. This is not a general source-to-generic-render pipeline. |
| X03 — §22 | A lock verifies actual dependencies and separately supplied output evidence | `GenericLock::from_json` / `verify`; [lock tests](../tests/generic_lock.rs): `runtime_verifier_binds_execution_and_actual_dependency_bytes`, `runtime_verifier_binds_exact_schedule_crop_and_channel_order`, `runtime_verifier_checks_pcm_and_file_evidence_bytes` | Changed dependency bytes, execution input, schedule/crop/order, or output evidence are refused at their own boundary | **E** for the bounded v1 lock family. Generic rendering, empirical external determinism, and cross-platform PCM identity are not established. |
| X04 — §25 | Adapter loss is explicit and faithful mode refuses unapproved reductions | `export_midi1_smf`, `LossReport`; [MIDI tests](../tests/interchange_midi.rs): `faithful_mode_refuses_unapproved_loss_and_accepts_explicit_approvals`, `tempo_ramp_and_per_note_expression_have_required_loss_records`, `midi1_channel_messages_are_preserved_and_other_protocols_are_reported` | Unapproved loss yields `E_CAPABILITY`; losses identify source/property/limitation/decision; supported channel messages survive and unsupported protocols are reported | **E** for the declared MIDI 1.0 SMF format-0 adapter. **U** for notation and DAW-session adapters; receiver round trips and live MIDI transport are not claimed. |

## Selected next correction: final edit graph validation

**P3 was selected from a demonstrated semantic discrepancy**, not a request to add a new
processor or host capability. §§16 and 21 require an acyclic same-sample graph
and complete semantic validation before an atomic transaction commits.
The public `EditContext::validate_document` contract expressly includes
causality constraints.

The audit starts with a composition that successfully compiles:

```maac
maac 1;
project p { score=[0q,1q]; rate=48000Hz; tempo=&t; meter=&m; output=&g:out; }
tempo t { points=[(0q,120bpm,step)]; }
meter m { points=[(0q,4,4)]; }
node s { type="core.sine/1"; }
node g { type="core.gain/1"; config={channels=1;}; }
connect feed { from=&s:out; to=&g:in; }
```

Parse it with `SourceDocument::parse` and construct this Protocol 2 transaction,
replacing `BASE_REVISION` with that document's `revision()`:

```json
{
  "version": 2,
  "base_revision": "BASE_REVISION",
  "operations": [{
    "op": "set", "object": ["feed"], "field": ["from"],
    "value": {"t": "ref", "path": ["g"], "port": "out"}
  }]
}
```

The fixed source's authored revision is
`sha256:17e067666223a2aecef7189ea18bb482da12dae8b4fffbeb4f34fe4cfbbd48ec`.

This changes the gain's input to its own output without a delay.

| Public boundary | Expected | Observed in the P2 probe |
| --- | --- | --- |
| `SourceDocument::apply` with `FoundationEditContext` | Reject `E_ALGEBRAIC_LOOP`; source and revision unchanged | Commits; revision changes |
| Same transaction with `BundleEditContext` over the entry source | Same atomic refusal | Commits; revision changes |
| `cli::execute_artifact(Command::Patch)` with the source and transaction files | Reject before output publication | Returns success and publishes edited source |
| `compile` and `check_bundle_artifact` on the edited source | Reject the same-sample cycle | Both reject `E_ALGEBRAIC_LOOP` |
| `FoundationEditContext::validate_document` and `normalize_document` on the cyclic authored document | Refuse invalid graph semantics | Both accept |

The probe was executed against P2's unchanged runtime. It remains historical
diagnostic evidence; the subsequent correction and regression evidence are
recorded [below](#p3-correction).

### P3 acceptance

Add independent regression vectors at the public editing boundaries, then
correct the shared final-validation path. Establish all of the following:

1. A valid starting document compiles, and the cycle-producing transaction
   fails with `E_ALGEBRAIC_LOOP` before changing authored state or source text.
2. Foundation and bundle editing agree; CLI patch failure publishes no new
   output and preserves an existing output when applicable.
3. Final-state validation still permits a multi-operation transaction whose
   temporary cycle is repaired before commit. It must not validate each
   intermediate operation as if it were a finished document.
4. Valid feedback through `core.delay/1` with a delay of at least one frame
   remains accepted; graph validation must follow processor causality and
   include modulation edges.
5. Direct context validation and normalization also refuse the invalid final
   graph, rather than relying only on the transaction wrapper.
6. Existing Document-only behavior and unsupported-capability refusals remain
   explicit. Validation must not require audio rendering or device access.
   Preserve `document_editing_validates_messages_without_claiming_performance_execution`
   in [foundation editing tests](../tests/editing_foundation.rs): normalization
   accepts a valid Document-only message context that compilation cannot execute.

### P3 correction

The shared Document source-validation path now checks same-sample dependencies
using the existing plan cycle detector, without compiling a performance or
rendering audio. It includes disconnected nodes and modulation edges, even when
the modulation amount is zero. Only audio dependencies into a validated
positive-frame `core.delay/1` are removed. The transaction kernel still validates
the complete final candidate, allowing a temporary cycle repaired by a later
operation.

The nine public regressions in [editing_causality](../tests/editing_causality.rs)
cover the original source and fixed revision above; foundation and bundle atomic
refusal; direct validation and foundation normalization; final-state repair;
one- and two-frame delay feedback; zero-frame refusal; unrelated and disconnected
audio cycles; self- and multi-node modulation cycles; and CLI preservation of
absent and existing output, including `--force`. Before the runtime correction,
seven tests failed and the two valid final-state/delay tests passed.

On 2026-09-28 the focused Rust gate passed all 46 tests across
`editing_causality` (9), `editing_foundation` (11), `editing_bundle` (9),
`editing_cli` (5), `core_delay_plan` (4), and `modulation_compile` (8).
This includes the existing Document-only message normalization and explicit
unsupported-capability cases. Independent code and acceptance review found no
correctness issue. `cargo clippy --all-targets --locked --offline -- -D warnings`,
formatting, diff whitespace, and changed documentation file-link checks passed.

A broader Rust run completed 426 tests without a failure (3 ignored), including
the 399-test library gate, before it was stopped because integration-binary
launches were unusually slow. This was before the equivalent Clippy-requested
boolean simplification in the delay check; the focused gate was rerun afterward.
The subsequent [combined P3/P4 verification](materialize-instance.md#verification)
completed the full Rust suite with 1,428 passed, 0 failed, and 3 ignored,
including doctests; formatting and all-target Clippy also passed on the final
unchanged source. Release build, installed acceptance, and remote CI were not
run for this correction.

No language syntax, public API, wire format, or capability has been added.
Document-only message semantics and unsupported-capability refusals retain their
existing contract. This correction does not establish full Document conformance.

### P6 correction

Before P6, the valid `examples/pitch-expression.maac` compiled but execution
identity failed on its `expression` children. Adding a valid expression or
message to the native production example caused production compilation to
fail while attaching identity. The execution normalizer's recognized-kind
set omitted those already-supported event kinds.

The [bounded P6 correction](execution-event-normalization.md) recognizes
expression and message objects in the compact normalized source graph. Message
onset offset and order receive their specified defaults in N(A); the authored
graph retains explicit-versus-omitted fields. Identity and public editing
regressions cover payload sensitivity, production compilation, P5 query
compatibility, and atomic edit behavior. This does not authorize a raw-message
renderer or claim complete core/extension normalization.

The P6 scoped run passed 52 integration tests across the affected public
boundaries and 13 existing identity unit tests. The complete Cargo-built Rust
test inventory passed 1,444 tests across 176 harnesses, with 3 ignored, plus
one doctest. The older `tempo_compile` regression was updated to assert the
newly valid production expression path. Formatting and all-target Clippy
passed. See the [P6 verification record](execution-event-normalization.md#verification)
for the external-volume test-launch workaround.

## Execution record

The P2 audit uses the unchanged runtime at `7e6f9f9`. On 2026-09-28 the following
targets were freshly executed with Cargo's `--locked --offline` settings:

| Target / filter | Passed | Evidence boundary |
| --- | ---: | --- |
| `editing_foundation`, `editing_source` | 11 + 6 | Real edit context, frozen L1 cases and source projection; executed by the Document auditor |
| `editing`, `semantic`, `production_data`, `required_processor_config`, `cli_project_entrypoint` | 39 + 10 + 13 + 4 + 8 | Kernel/wire, semantic refusals, extension data and CLI entry selection |
| `--lib syntax_tests` | 13 | Grammar, typed-tree and parser limits |
| `--lib production_identity::tests` | 13 | Canonical identity and execution projection |
| `external`, `generic_lock`, `generic_lock_generation`, `interchange_midi` | 3 + 10 + 9 + 9 | Descriptor inspection, lock construction/verification and SMF losses |
| `foundation`, `compiler`, `tempo_compile`, `l3_tuning`, `message_performance` | 10 + 11 + 10 + 4 + 4 | Expansion, timing, tuning and exact event dispatch |
| `pitch_expression_compile`, `gain_expression_compile`, `modulation_compile` | 9 + 11 + 8 | Expression and modulation lowering/refusal |
| `kit_compile`, `audio_compile`, `warp_compile` | 7 + 6 + 5 | Pinned assets and transport metadata |
| `core_delay_plan`, `graph` | 4 + 13 | Explicit delay causality and combined graph edges |

These targets and filters total **250 passing tests**, with no failed tests.
Only targets or filters in this ledger count as fresh runtime verification for
P2. Other cited test bodies were inspected; linked P1 evidence retains its own
historical execution scope. No complete Rust suite or complete profile was
validated. The cycle probe was also executed and independently reproduced;
its **failure of the contract** is reported above, separately from passing tests.

The existing conformance-index checker passed **7 suites / 179 fixture
bindings**, checking static paths, schemas and digests only. It did not execute
the recorded runtime recipes. Added local links/anchors, named test references,
and whitespace checks passed. Independent semantic review accepted the map and
P3's correction boundary. These checks complete the bounded P2 audit; they do
not fix the demonstrated runtime defect.

P5 subsequently adds [score-window query evidence](windowed-event-query.md#verification)
to P04. Its scoped execution is recorded separately from the historical
250-test P2 audit: 39 passing API, CLI, execution-profile, compiler,
foundation, and message tests; targeted Clippy and formatting pass. The
complete Rust suite is not a P5 evidence claim.

## What this map does not require

- Device enumeration, microphone capture, monitoring, hardware latency
  measurement, live MIDI, GUI, or plugin loading. §24 places live devices in
  separately permissioned host sessions.
- Optimized seek or checkpoint restoration. §5 permits an exact crop of a
  render from reset; a missing speed-up is not itself a semantic discrepancy.
- New syntax to replace existing finite patterns, musical libraries, exact
  transactions, or take/comp data.
- A full-profile claim inferred from L1–L5 completion, processor counts,
  static digest checks, or passing a finite test suite.

The [conformance index](../conformance/README.md) continues to bind the existing
seven L1–L5 suites. Its checker validates fixture paths, shapes, and digests;
it never runs the recorded runtime recipes. This map supplements that index
without changing its schema, suite identities, or historical evidence.
