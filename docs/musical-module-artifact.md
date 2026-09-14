# Musical source module artifact

This document describes the bounded F1 source-artifact slice. It does not claim
completion of a broader package registry, binary module format, or direct
artifact-import feature.

## Contract

A module artifact packages one existing MaaC library entry and its complete
reachable source and asset closure. Its format discriminator is
`maac.module-source` and its integer version is `1`.

The canonical JSON object contains exactly these fields:

- `format`
- `version`
- `entry`, the normalized logical entry-source path
- `sources`, sorted source records containing `path`, raw `sha256:` identity,
  a `builtin` marker, and exact UTF-8 `text`
- `assets`, sorted asset records containing `path`, raw `sha256:` identity, and
  lowercase hexadecimal `bytes`

Exports and dependency identities are derived from parsed source declarations.
They are deliberately not copied into the wire format. The artifact has no
self-hash field; `ModuleArtifact::digest` hashes its canonical JSON bytes.

Construction and decoding require the entry to be a library with at least one
direct `pattern`, `curve`, or `tuning` export. Validation reconstructs the local
`SourceBundle`, resolves its pins and built-ins, compares the exact reachable
closure, and runs the existing library semantic/compiler validation. Unknown or
duplicate JSON fields, malformed identities, altered bytes, non-normalized
paths, missing or extra closure members, cycles, and existing bundle resource
limit violations are rejected.

Built-in source identities and bytes are retained in the artifact and checked
against the embedded registry. They are not emitted by unpack because the
authored `builtin = ...` declarations reproduce them through the existing
resolver.

## Public API

`ModuleArtifact` is opaque and is re-exported from the crate root. Its validated
entry points are:

- `from_source_bundle`
- `from_json` and `from_json_with_limits`
- `to_json` and `to_json_with_limits`
- `validate`
- `digest`
- `exports`
- `to_source_bundle`

`ModuleArtifactLimits` can lower the JSON allowance. The implementation always
caps it at `MAX_MODULE_ARTIFACT_JSON_BYTES` (72 MiB); existing per-file,
aggregate source, aggregate asset, syntax-object, import-depth, and expanded
musical-catalog limits remain authoritative.

## CLI

```text
maac module export LIBRARY -o MODULE [--project-root ROOT] [--force]
maac module check MODULE [--expect-hash SHA256]
maac module unpack MODULE --output-dir NEW_DIR [--expect-hash SHA256]
```

Module reads use their own bounded reader rather than changing the existing
4 MiB plan/source command reader. Export publishes the completed JSON through
the existing atomic file writer. Unpack validates the complete artifact and
optional expected digest before staging output, rejects an existing destination,
and publishes the staged directory without overwriting user data.

After unpack, compositions continue to use ordinary `{ path, hash }` imports.
The caller still owns tempo, meter, routing, tracks, placement, and automation.
There is no direct module-artifact import syntax in version 1, and the Plan and
SourceMapping wire formats are unchanged.
