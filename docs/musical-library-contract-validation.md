# Musical-library contract reconciliation

This record covers the bounded P1 language-contract reconciliation. The
normative rules are in [library declarations and dependencies](instruments.md#documents-and-dependencies)
and the musical-definition sections that follow them. The
[MaaC-1 extension summary](../MaaC-1-Specification.md#13-implemented-local-sound-library-extension)
incorporates that contract. This record distinguishes those rules from
implementation evidence.

## Inconsistency and resolution

The earlier library declaration list excluded patterns, curves, and tunings,
although pinned musical exports and the F1 module artifact already supported
them. The amended inventory includes these definitions alongside instruments,
presets, and wavetables. Imports bind dependencies; they do not re-export an
imported namespace. Musical definitions retain their declaring-library
references, while consuming compositions supply tempo, meter, placement,
tracks, and global automation bindings. Note-expression attachments authored
inside a library retain their library-owned curve references.

This reconciles the documented local-library extension with established
behavior. It does not introduce a new core object, source header, grammar rule,
syntax-tree schema, performance-plan version, or module wire version. The
sound-only library contract and frozen built-in source bytes remain supported.
An implementation that lacks musical-library support must report the required
capability explicitly rather than ignore imported musical definitions.

## Fixed semantic evidence

The fixtures use literal source documents and explicit expected outcomes at
public bundle, compiler, editing, and module APIs. They do not derive expected
musical timing or namespace behavior by calling the resolver under test a
second time. Round-trip comparisons separately check preservation of an
already validated artifact.

| Guarantee | Existing fixed case or public-boundary check |
| --- | --- |
| Two consumers share musical data while using different tempo, placement, and automation anchors | `musical_library::two_projects_reuse_one_pinned_musical_library_without_capturing_context` checks event coordinates, source paths, tuning/pitch expression, and automation |
| Altered dependency bytes are refused | `musical_library::altered_pin_is_rejected_before_musical_reuse` expects `E_HASH` |
| Two import aliases preserve distinct source provenance | `musical_library::aliases_isolate_the_same_library_and_keep_authored_mapping_paths` |
| Library-only validation checks unused exports; a library is not a renderable composition | `musical_library::library_only_check_validates_every_export_but_compile_remains_a_conflict` and `library_only_check_runs_compiler_level_pitch_validation` |
| Missing library-local references cannot capture caller declarations or aliases | `musical_library::library_local_references_cannot_capture_entry_declarations_or_aliases` |
| A caller cannot traverse dependency aliases in an authored reference | `musical_library::caller_cannot_address_a_transitive_import_through_its_alias` expects `E_REFERENCE` for `&outer.inner.leaf_note` |
| Composition globals are not library declarations | `musical_library::libraries_reject_composition_owned_global_declarations` expects `E_UNKNOWN_KIND` for tempo/meter/track/place/global automation and `E_CONFLICT` for a combined library/project root |
| An unused invalid export in a transitive dependency is checked | The library-only validation case expects `E_RANGE` for an unused `ratio(0, 440Hz)` note in a root → middle → leaf dependency chain |
| Source spans exposed for editing belong to the authored entry | `musical_library::public_entry_document_never_exposes_spans_owned_by_imported_sources` |
| Module restoration preserves nested patterns, curves, tunings, closure, and compiled meaning for two callers | `module_artifact::restored_sources_compile_two_callers_without_capturing_context_or_changing_plan_json` |
| Authored entry/library edits retain fixed-context validation, atomic repinning, and refusal | `editing_bundle` fixed import-context, repin, bad-pin, and standalone-library cases |
| Raw-byte pins and authored revisions are distinct identities | `editing_bundle::library_comment_edits_change_import_pin_without_changing_authored_revision`; the repin case separately asserts that changing an entry import changes its authored revision |
| A valid library need not qualify as a musical module | `module_artifact::composition_roots_and_explicit_json_byte_limits_are_rejected` first validates empty and import-only libraries, then expects `E_CONFLICT` from module construction; dependency exports do not count as direct entry exports |
| The artifact lists only direct entry musical definitions | `module_artifact::canonical_json_roundtrip_digest_and_exports_are_derived` fixes the direct curve/pattern/tuning inventory, excluding the dependency's definitions |
| Export/check/unpack preserves exact source bytes after the original closure is removed | `module_cli::module_export_check_and_unpack_are_atomic_and_restore_exact_sources` |

The module's independent wire, tamper, resource, and publication checks remain
documented in the [F1 artifact evidence](testing/musical-module-artifact.tdd.md).
Those packaging guarantees do not replace musical name-resolution semantics.

## Executed verification — 2026-09-28

After the final test edit:

| Gate | Result |
| --- | --- |
| `cargo test --test library --test musical_library --test module_artifact --test module_cli --test editing_bundle --test editing_foundation --locked --offline --quiet` | 52 passed: 11 library, 11 musical-library, 7 module-artifact, 3 module-CLI, 9 bundle-editing, and 11 foundation-editing tests |
| Clippy for those six test targets, with `--locked --offline -- -D warnings` | Passed |
| `cargo fmt --all -- --check`; `git diff --check` | Passed |
| `python3 scripts/check_conformance_index.py --repo-root . --index conformance/index.json` | Passed: seven suites and 179 fixture bindings; the checker does not execute runtime recipes |

Three new test functions and extensions to existing cases close the identified
coverage gaps. Production Rust, grammar/schema files, dependency bytes, and
artifact wire formats are unchanged. The gates are scoped to this contract;
this is not a repository-wide runtime, release-build, or full-profile claim.
Semantic planning and independent final review cleared the amendment with no
open findings. Added local documentation links and anchors also passed.

## Acceptance boundary

The existing L1–L5 conformance index remains a separate static
inventory; P1 does not add a new conformance level or claim full Document,
Performance, Core Audio, or Locked Render support.

Direct artifact imports, export parameters, tempo-map exports, linked sections,
and changes to shared-occurrence expression are outside this reconciliation.
No device, audio hardware, DAW receiver, or executable plugin host is needed
for these semantic checks.
