# Native composition archives and checkpoint history

**Status:** Phase 3 bounded native archive slices. Version 1 captures one
composition; version 2 preserves an explicit linear sequence of complete
composition checkpoints; version 3 also retains verified original WAV import
records when present; version 4 can retain an opt-in full-output freeze;
version 5 can record an explicit Protocol 2 edit and its inverse; version 6
can retain several selected WAV import records in one composition; version 7
can record a local library edit with its exact parent import pin update;
version 8 can freeze one internal native effect output as exact binary64 samples;
version 9 can retain a group of independent native effect outputs; version 10
can journal a bounded group of direct local source edits in one checkpoint;
version 11 can journal edits to shared or transitively imported local sources.

## Commands

```text
maac archive create SOURCE --output-dir NEW [--project-root ROOT] [--profile default|song]
    [--previous ARCHIVE] [--expect-previous-hash SHA256]
    [--freeze-output | --freeze-node NODE ...]
    [--retain-import RELATIVE_DIR]...
maac archive patch ARCHIVE PATCH.json --output-dir NEW [--expect-hash SHA256]
maac archive patch-import ARCHIVE PATCH.json --import ALIAS --output-dir NEW
    [--expect-hash SHA256]
maac archive patch-group ARCHIVE --import ALIAS=PATCH.json [--import ALIAS=PATCH.json]...
    [--entry-patch PATCH.json] --output-dir NEW [--expect-hash SHA256]
maac archive patch-group ARCHIVE --source SOURCE=PATCH.json [--source SOURCE=PATCH.json]...
    [--entry-patch PATCH.json] --output-dir NEW [--expect-hash SHA256]
maac archive verify ARCHIVE [--expect-hash SHA256]
maac archive unpack ARCHIVE --output-dir NEW [--expect-hash SHA256] [--revision SHA256]
maac archive freeze-check ARCHIVE --source SOURCE [--project-root ROOT]
    [--revision SHA256] [--expect-hash SHA256] [--replay]
maac archive freeze-render ARCHIVE --source SOURCE -o NEW.wav
    [--project-root ROOT] [--revision SHA256] [--expect-hash SHA256]
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

Use repeatable `--retain-import` for a composition that uses native PCM from
several existing `import-wav --retain-original` directories. Each selected
directory is relative to the project root; `.` explicitly selects the root.
It must contain a version 2 `import.json`, `original.wav`, and a `media.pcm`
already in the compiled composition closure. The archive checks the exact
original WAV and declared crop against that PCM before capturing it. Up to 16
directories can be selected per checkpoint. A selected directory's generated
`main.maac` is retained only if the composition imports it as a source.
These checkpoints use `maac.archive-snapshot/2` with sorted import records
bound to their PCM members, and their histories use root version 6. Omitting
the flag preserves the existing version 1 snapshot and root import behavior.
When extending an archive with `--previous`, select the new checkpoint's
imports explicitly; earlier checkpoints keep their own exact records.

`--freeze-output` renders the complete native project output once as an
immutable float32 WAV and stores a strict freeze record. A history containing
a freeze uses a version 4 root manifest. The original source graph remains
the active path; the frozen output is a derived asset and does not replace
nodes or change ordinary `build` results. Older checkpoints and their IDs are
preserved when such a history is extended.

`archive patch` verifies the input archive, applies one Protocol 2 transaction
to its head composition source, and publishes a new history with at least a
version 5 root manifest. It records the canonical forward transaction and generated
inverse, the authored revisions before and after, and a new checkpoint even
when the transaction makes no authored change. Both checkpoints remain reopenable.
The input archive stays unchanged. This first journaled-edit path keeps the
same dependency closure, retained-import sidecars, and execution profile;
edits to imported files or changes to dependency membership are outside its
scope.

`archive patch-import` applies one Protocol 2 transaction to a directly
imported local leaf library. It generates the matching `import.hash` edit in
the entry source from the exact changed library bytes, then validates and
publishes both files in one checkpoint. This first grouped path requires one
incoming import edge and a library with no imports of its own. Shared,
transitive, and built-in sources are outside its scope. A version 7 history
records both forward transactions and both authored-tree inverses; verification
replays the two-file transition against the complete before and after
snapshots. The parent import pin hashes exact source bytes, including comments
and formatting. The generated pin transaction has one operation and is capped
at 16 KiB. Both prior and new checkpoints remain reopenable.

`archive patch-group` journals 1–16 distinct direct local leaf-library edits
and an optional entry-source edit as one version 10 checkpoint. Each supplied
Protocol 2 transaction addresses the original head and must be valid on its
own. MaaC applies all library edits, generates one entry transaction with the
exact new byte hash for each selected import, and checks the complete combined
composition before publication. Request order does not change archive identity.
The record retains each forward transaction and authored-tree inverse, plus
the generated pin transaction and its inverse. Verification replays all of
them and rejects changed import topology, unrelated source or media changes,
and altered journal bytes. Shared, transitive, and built-in imports remain
outside this bounded `--import` form; edits that require an invalid intermediate
source meaning are also outside it. An authored no-op still creates a
journaled checkpoint. The supplied patch files together are limited to 4 MiB;
the generated pin transaction is limited to 16 KiB. Older edit records and
checkpoint IDs stay unchanged.

The `--source` form selects 1–16 distinct existing, reachable, non-entry local
source paths. It writes a version 11 checkpoint with version 4 edit evidence.
Selected sources may be shared, imported transitively, or contain their own
imports. Each supplied transaction uses that source's original revision.
After the user edits, MaaC generates exact-byte hash updates in dependency
order for every affected incoming import edge, including multiple aliases to
one source. A selected importer's generated pin update follows its own user
edit. The journal records both stages and their inverses; verification derives
the expected propagation and checks the complete resulting closure. Request
order does not affect the archive identity. User edits must preserve import
declarations and dependency membership. Built-in sources and topology changes
remain unsupported. The `--source` and `--import` forms are mutually exclusive;
existing `--import` records retain their version 10 identities.

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
For frozen checkpoints it checks the recorded boundary, source association,
and complete WAV identity without replaying the renderer. For journaled edits,
it replays the forward transaction against the parent authored revision and
the inverse against the child authored revision. It rejects a
missing, altered, or undeclared dependency even in a checkpoint other than
the head. `unpack` repeats full-history verification, then
publishes the selected checkpoint's verified source, asset, and retained
import members to a new directory; it leaves the derived freeze in the
archive. `--revision` names the
checkpoint ID; omitting it selects the head. This ID covers the complete
snapshot and parent, and is distinct from a Protocol 2 authored-document
revision. The source bytes, including comments, omissions, labels, and
formatting, remain unchanged. The unpacked project uses ordinary `check` and
`build`; large PCM requires `--disk-media`.

Versions 2 through 10 store `checkpoints/<checkpoint-hex>/` directories. The root
manifest records the ordered IDs, parent IDs, snapshot manifest hashes, and
head ID. Frozen records also identify their freeze manifest; version 4 keeps
their files under `freezes/<freeze-hash>/`. Journaled records identify an edit
manifest under `edits/<edit-hash>/`, beside the exact forward and inverse JSON.
A checkpoint ID hashes its canonical parent and snapshot identity, plus the
freeze or edit identity when present. The CLI
`digest` hashes the root manifest and covers the entire history. Inspect
`maac-archive.json` for checkpoint IDs to use with `--revision`.

`freeze-check` verifies the archive, then captures and compiles a current
project with the frozen checkpoint's execution profile. Its result reports
integrity, whether the recorded inputs are current or stale, and whether a
requested replay matched the stored freeze payload. Stale input fails with
`E_FREEZE_STALE`. Any exact source or dependency change conservatively stales
this first whole-output freeze, even if the resulting sound would be the same.
A change to the MaaC executable bytes also stales it, including a different
build of the same source.
`--replay` renders only a matching candidate and compares output evidence;
eligibility alone is not a claim of identical audio on another engine.

For a whole-output freeze, `freeze-render` verifies the archive and the selected
checkpoint,
then compares the current source, retained import provenance, execution profile,
render key, and executable identity with its recorded inputs. If current, it
atomically publishes the exact stored float32 WAV to a new file without running
the DSP renderer. It reports `reused: true`, the selected checkpoint, source
and output digests, and frame count. A stale source fails with
`E_FREEZE_STALE`; an unfrozen checkpoint, changed archive bytes, or an existing
destination also fails without publishing output. The source graph remains
authoritative. This is full-output reuse only: it does not replace an internal
graph branch, convert formats, or infer eligibility from similar sound.

`--freeze-node NODE` selects one project-level `fx.eq/1`, `fx.compressor/1`, or
`fx.reverb/1` node whose `out` feeds another audio node before the project
output. Version 8 stores every reset-origin frame of that internal output as
finite interleaved IEEE-754 binary64 samples. `freeze-check` compares the
source closure, retained import provenance, execution profile, engine identity,
and selected boundary; `--replay` compares the internal binary64 payload.
For a current source, `freeze-render` replaces that node's output at its normal
graph position and runs the remaining graph through the ordinary final WAV
encoder. Its `output_digest` identifies the published WAV; the node cache has a
separate digest. New node freezes permit only numeric parameter-value edits on
project-level `core.gain/1` and `core.pan/1` nodes with no audio or modulation
dependency path to the frozen effect. This includes downstream and independent
mix branches. All other source bytes and dependencies stay pinned, including
the frozen effect, its ancestors and feedback paths, graph connections, musical
events, timing, automation, imported media, and retained provenance. Eligible
reuse renders the current verified source with the cached node output. Older
node freezes retain their rules: version 1 pins the exact source, while version
2 permits only eligible downstream gain/pan edits. Each checkpoint may carry one
freeze, and `--freeze-node` cannot be combined with `--freeze-output`.
Existing whole-output freezes keep their original format and behavior.

Repeating `--freeze-node` selects 2–16 independent native effect outputs for a
version 9 checkpoint. The selected effects must have no audio, sidechain,
modulation, or delayed-feedback path between them. Capture, replay, and reuse
process the group in one graph execution. `freeze-check` reports each member's
boundary, cache identity, and eligibility; all members must be current before
`freeze-render` publishes the final WAV. A stale member prevents all reuse.
Selection order does not change the archive identity. Singleton checkpoints
retain their original format, identifiers, and CLI result fields.

Creation, patching, and unpacking stage files beside their destination and publish the
directory atomically without replacing an existing path. Source dependencies
are opened beneath a pinned project root, and media bytes are copied from
private verified snapshots. Output directories cannot be created inside an
input archive. Histories permit at most 32 checkpoints, with aggregate caps of
64 MiB source text, 64 MiB ordinary assets, and 4 GiB combined native PCM,
retained original WAV, and frozen payload bytes across all stored copies. Each
original WAV or frozen payload is limited to 1 GiB; a grouped node freeze is
limited to 1 GiB across its members. Each import record is limited to 16 KiB, and
each checkpoint retains its source, asset, execution, and native PCM limits.
Edit manifests through version 3 are limited to 16 KiB; version 4 graph edit
manifests are limited to 256 KiB. Each forward or inverse transaction
is limited to 4 MiB and 1,024 operations. A history permits at most 64 MiB
of edit files, 2 GiB of preflighted transaction work, and 8 GiB of replay
closure-copy work. The verifier checks these bounds before replaying edits.
Unsupported source features fail during compilation.

## Scope of the claim

Each checkpoint preserves its authored composition and the local dependency
closure recognized by the engine: local imports, native PCM, wavetables, and
the recognized native production descriptor. Built-in imports retain their
exact identities and require the matching built-in registry when reopened.
The manifest hash is an external pin for archive identity; a manifest alone is
not a signature or proof of audio equivalence across environments.

History begins with its first captured checkpoint or with a supplied
predecessor. It does not reconstruct edits before that point or infer
transactions for ordinary `archive create` checkpoints. It does not retain
arbitrary external processor modules or state, rendered plans, or delivery
outputs. Version 3 retains one original WAV and
import record for a root `main.maac` created with `import-wav
--retain-original`. It does not invent an original for a legacy import that
has only a version 1 `import.json`; that record remains outside the archived
composition closure. Version 6 retains multiple selected existing imports,
including nested directories whose native PCM is in the composition closure.
Automatic edit journaling, arbitrary source-graph transactions, branching and merging,
broader selective invalidation, overlapping frozen graph branches, and a complete producer
archive remain Phase 3 work. Whole-output and native-node freezes require
explicit reuse after conservative input validation; they do not provide
automatic render-cache reuse or a claim that another engine produces identical
samples.
