# Native composition archives and checkpoint history

**Status:** Phase 3 bounded native archive slices. Version 1 captures one
composition; version 2 preserves an explicit linear sequence of complete
composition checkpoints; version 3 also retains verified original WAV import
records when present. Freeze records remain future work.

## Commands

```text
maac archive create SOURCE --output-dir NEW [--project-root ROOT] [--profile default|song]
    [--previous ARCHIVE] [--expect-previous-hash SHA256]
maac archive verify ARCHIVE [--expect-hash SHA256]
maac archive unpack ARCHIVE --output-dir NEW [--expect-hash SHA256] [--revision SHA256]
```

`create` accepts a project directory or a composition source file. It resolves
and compiles the currently supported native composition closure, then
publishes a new directory containing exact authored source text and hash-pinned
local dependencies. Audio assets use the bounded disk-media snapshot path, so
native PCM can exceed the embedded asset limit.

Each checkpoint contains a `maac-archive.json` manifest identifying the entry,
execution profile, members, byte lengths, and SHA-256 hashes. Ordinary
composition checkpoints use version 1 snapshots inside a version 2 history.
If a selected project-root `main.maac` has a valid retained WAV import, its
checkpoint uses `maac.archive-snapshot/1` to declare the exact `import.json`
and `original.wav` bytes alongside the composition closure. A history
containing such a checkpoint uses a version 3 root manifest. Version 3 may
also contain unchanged version 1 checkpoints. Existing version 1 and 2
archives keep their original identities and remain readable.

`--previous` verifies a prior archive, copies its complete checkpoints into a
new independent archive, and appends the current project. It declares a linear
parent relationship; MaaC does not infer that the worktree was edited from the
prior head. If the current snapshot is identical to that head, creation copies
the history without adding a checkpoint. A later return to an older snapshot
after an intervening change creates a new checkpoint because its parent
differs. Each checkpoint retains its own source and media closure, including
dependencies removed from later versions. Full copies can use substantial
disk space.

`verify` checks the root manifest and every historical checkpoint, then
resolves and compiles each archived project. For retained imports it also
rechecks the original WAV, declared crop, native PCM, and exact import record.
It rejects a missing, altered, or undeclared dependency even in a checkpoint
other than the head. `unpack` repeats full-history verification, then
publishes the selected checkpoint's verified source, asset, and retained
import members to a new directory. `--revision` names the
checkpoint ID; omitting it selects the head. This ID covers the complete
snapshot and parent, and is distinct from a Protocol 2 authored-document
revision. The source bytes, including comments, omissions, labels, and
formatting, remain unchanged. The unpacked project uses ordinary `check` and
`build`; large PCM requires `--disk-media`.

Versions 2 and 3 store `checkpoints/<checkpoint-hex>/` directories. The root
manifest records the ordered IDs, parent IDs, snapshot manifest hashes, and
head ID. A checkpoint ID is the
SHA-256 hash of canonical checkpoint identity bytes containing format,
version, parent ID, and snapshot hash. The CLI `digest` is the SHA-256 hash of
the root manifest, so a pinned digest covers the entire history. Inspect
`maac-archive.json` for checkpoint IDs to use with `--revision`.

Creation and unpacking stage files beside their destination and publish the
directory atomically without replacing an existing path. Source dependencies
are opened beneath a pinned project root, and media bytes are copied from
private verified snapshots. Output directories cannot be created inside an
input archive. Histories permit at most 32 checkpoints, with aggregate caps of
64 MiB source text, 64 MiB ordinary assets, and 4 GiB combined native PCM and
retained original WAV bytes across all stored copies. Each original WAV is
limited to 1 GiB, each import record to 16 KiB, and each checkpoint retains
its source, asset, execution, and native PCM limits. Unsupported source
features fail during compilation.

## Scope of the claim

Each checkpoint preserves its authored composition and the local dependency
closure recognized by the engine: local imports, native PCM, wavetables, and
the recognized native production descriptor. Built-in imports retain their
exact identities and require the matching built-in registry when reopened.
The manifest hash is an external pin for archive identity; a manifest alone is
not a signature or proof of audio equivalence across environments.

History begins with its first captured checkpoint or with a supplied
predecessor. It does not reconstruct edits before that point or retain inverse
transactions, arbitrary external processor modules or state, rendered plans,
freeze caches, or delivery outputs. Version 3 retains one original WAV and
import record for a root `main.maac` created with `import-wav
--retain-original`. It does not invent an original for a legacy import that
has only a version 1 `import.json`; that record remains outside the archived
composition closure. Multiple original inputs, nested import packaging,
automatic edit journaling, freeze invalidation, and a complete producer
archive remain Phase 3 work.
