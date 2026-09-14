# Musical module artifact TDD evidence

## Source and journeys

No standalone plan file was supplied. The acceptance contract was provided by
the F1 source-artifact task.

- A library author can retain one validated musical library and its exact pinned
  closure as deterministic canonical JSON.
- A caller can restore that closure after the original files disappear and use
  the existing source import contract without capturing caller timing context.
- A CLI user can export, check, and safely unpack a module without destructive
  overwrite.
- Malformed, altered, incomplete, excessive, or non-library artifacts fail
  before use or publication.

## RED and GREEN checkpoints

RED command:

```text
cargo test --test module_artifact --test module_cli
```

Before production code existed, compilation failed with unresolved imports for
`maac::ModuleArtifact` and `maac::ModuleArtifactLimits`. The RED checkpoint is
commit `469578d` (`test: add red musical module artifact coverage`).

Initial GREEN command:

```text
cargo test --test module_artifact --test module_cli -- --nocapture
```

Result: artifact API 5 passed; CLI 2 passed. The GREEN checkpoint is commit
`9a6e2a7` (`feat: add source module artifact export and unpack`).

After boundary cases were added, the same target passed with artifact API 7
tests and CLI 2 tests.

Independent unpack review added a second RED checkpoint in commit `74fcc73`
(`test: add red module unpack collision coverage`). The focused unit test did
not compile because the required no-replace publication primitive was absent.
After implementing host-filesystem member preflight and atomic no-replace
directory publication, both focused regressions passed. The correction GREEN
checkpoint is `fix: make module unpack collision safe`.

## Test specification

| Guarantee | Evidence | Type | Result |
|---|---|---|---|
| Canonical JSON roundtrips and its digest is deterministic | `canonical_json_roundtrip_digest_and_exports_are_derived` | Integration | PASS |
| Export/dependency manifests are not duplicated in the wire | `canonical_json_roundtrip_digest_and_exports_are_derived` | Integration | PASS |
| Nested pattern, curve, and lexical tuning survive restoration | `restored_sources_compile_two_callers_without_capturing_context_or_changing_plan_json` | Integration | PASS |
| Caller tempo and placement vary without changing occurrence/source mappings | `restored_sources_compile_two_callers_without_capturing_context_or_changing_plan_json` | Integration | PASS |
| Built-in bytes/identity and raw assets are retained exactly | `builtin_and_asset_closure_roundtrip_exactly_without_unpacking_builtin_files` | Integration | PASS |
| Altered, missing, extra, malformed, unknown, duplicate, path, format, and version inputs fail | `strict_wire_rejects_tampering_missing_extra_and_malformed_members`; `missing_extra_and_altered_assets_are_rejected` | Integration | PASS |
| Cycle, depth, syntax-object, file-byte, and explicit JSON limits remain enforced | `module_decode_reuses_cycle_depth_object_and_source_byte_limits`; `composition_roots_and_explicit_json_byte_limits_are_rejected` | Integration | PASS |
| Export/check/unpack preserve output on expected failures | `module_export_check_and_unpack_are_atomic_and_restore_exact_sources` | CLI integration | PASS |
| Module check accepts a valid artifact larger than the ordinary 4 MiB reader | `module_reader_is_separate_from_the_four_megabyte_plan_reader` | CLI integration | PASS |
| Distinct logical members cannot overwrite each other when the host filesystem aliases their paths | `unpack_rejects_distinct_logical_members_that_collide_on_the_host_filesystem` | CLI integration | PASS |
| A destination appearing at publication time is preserved | `staged_directory_publication_never_replaces_a_concurrent_destination` | Unit | PASS |

## Coverage and known gaps

The tests exercise the new public API, strict wire decoder, existing bundle and
musical validation boundaries, filesystem loader, atomic file publication, and
staged directory unpack. There is no direct artifact import in this slice; the
restored source closure is intentionally consumed through existing imports.

Final repository-wide format, lint, test, and release-build results are recorded
in the task handoff after they are run.
