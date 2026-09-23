# Native composition archive, version 1

**Status:** Phase 3 baseline archive slice. It captures one current native
composition and its pinned source and asset closure. History and freeze records
are future versions of this archive, not implicit data in version 1.

## Commands

```text
maac archive create SOURCE --output-dir NEW [--project-root ROOT] [--profile default|song]
maac archive verify ARCHIVE [--expect-hash SHA256]
maac archive unpack ARCHIVE --output-dir NEW [--expect-hash SHA256]
```

`create` accepts a project directory or a composition source file. It resolves
and compiles the complete currently supported native composition closure,
then publishes a new directory containing exact authored source text and
hash-pinned local dependencies. Audio assets use the bounded disk-media
snapshot path, so native PCM can exceed the embedded asset limit. The archive
contains a versioned `maac-archive.json` manifest that identifies the entry,
execution profile, members, byte lengths, and SHA-256 hashes. The command
reports the manifest hash; callers can pin that hash for later verification.

`verify` checks the manifest and every declared member, then resolves and
compiles the archived project. It rejects a missing, altered, or undeclared
dependency. `unpack` repeats verification and publishes only the verified
source and asset members to a new directory. The source bytes, including
comments, omissions, labels, and formatting, remain unchanged. The unpacked
project uses ordinary `check` and `build`; large PCM requires `--disk-media`.

Creation and unpacking stage files beside their destination and publish the
directory atomically without replacing an existing path. Source dependencies
are opened beneath a pinned project root, and media bytes are copied from
private verified snapshots. The member count, source/asset limits, execution
limits, and native PCM 1 GiB file/aggregate limits remain bounded. Unsupported
source features fail during compilation.

## Scope of the claim

Version 1 preserves the current authored composition and the local dependency
closure recognized by the engine: local imports, native PCM, wavetables, and
the recognized native production descriptor. Built-in imports retain their
exact identities and require the matching built-in registry when reopened.
The manifest hash is an external pin for archive identity; a manifest alone is
not a signature or proof of audio equivalence across environments.

The archive has one checkpoint. It does not retain earlier edits, inverse
transactions, original WAV input bytes, arbitrary external processor modules
or state, rendered plans, freeze caches, or delivery outputs. An imported
project's `import.json` and `original.wav` sidecars are outside this first
closure; use `verify-import` on the original import project when those records
matter. Project history, freeze invalidation, and a complete producer archive
remain Phase 3 work.
