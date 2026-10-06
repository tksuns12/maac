# Bounded WAV media import

`maac import-wav` is the first bounded Phase 3 media slice. It creates an
ordinary, editable MaaC project from a WAV file or a source-frame crop. It does
not complete Phase 3 or define a project archive format.

## Command

```text
maac import-wav INPUT.wav --output-dir NEW_PROJECT
maac import-wav INPUT.wav --start-frame N --end-frame M --output-dir NEW_PROJECT
maac import-wav INPUT.wav --retain-original --output-dir NEW_PROJECT
maac verify-import NEW_PROJECT
maac import-wav INPUT.wav --disk-media [--profile default|song] --retain-original --output-dir NEW_PROJECT
maac verify-import NEW_PROJECT --disk-media [--profile default|song]
```

The optional crop is a nonempty half-open source-frame interval `[N, M)`;
`--start-frame` and `--end-frame` must be supplied together. Omitting both
imports the complete WAV. The output directory must not already exist. The
command stages all files and publishes the directory atomically without
replacing an existing destination.
`--retain-original` opts into retaining the exact snapshotted input bytes in
`original.wav`. `verify-import` accepts a project created with that option.
Imports without the option keep the original three-file layout and version 1
manifest.

## Supported input and conversion

The importer accepts mono or stereo RIFF/WAVE containing signed PCM16, PCM24,
PCM32, or IEEE float32 samples. It preserves the source sample rate, channel
order, and selected frame order.

- Integer samples convert to float32 by dividing by `2^(bits_per_sample - 1)`.
  No gain adjustment, clipping, dithering, or sample-rate conversion is done
  during import.
- Finite float32 sample bits are copied exactly, including signed zero and
  subnormal values. A selected NaN or infinity is rejected.
- Other channel counts, sample encodings, and empty or out-of-range crops fail
  explicitly.

The generated project uses the existing 48 kHz render profile. Its audio asset
retains the source rate; ordinary MaaC rate-mode clip playback handles the
source/render-rate difference. The imported PCM asset uses
`pcm_f32le_interleaved/1`.

## Snapshot, provenance, and bounds

Before decoding, the importer copies the selected input file into a temporary
disk snapshot while computing its SHA-256 identity. Decoding uses that snapshot,
so the recorded identity refers to the bytes actually imported. Snapshot
copying and hashing are bounded to 1 GiB and use a fixed-size buffer; a small
crop still requires reading the complete input once.

The output directory contains:

- `main.maac` — an ordinary composition referencing the imported PCM asset;
- `media.pcm` — only the selected crop, not the entire source WAV; and
- `import.json` — source filename, exact source hash, source encoding/rate/
  channels/frame count, selected interval, versioned decoder/conversion IDs,
  and output asset metadata/hash.

With `--retain-original`, the project also contains `original.wav`, copied
from the same bounded snapshot used for decoding. Its version 2 manifest
declares the fixed path, byte count, and hash. This can retain a WAV larger
than 4 MiB when the selected native PCM crop fits the existing asset limit.
The original remains subject to the 1 GiB input limit.

`verify-import` pins one project directory handle for the manifest, original,
PCM, and current MaaC source closure. It checks version and decoder identity,
re-decodes the declared crop, compares source and output metadata and hashes,
compares native PCM byte for byte, and compiles the current editable source and
local dependencies. The closure must still contain the verified `media.pcm`
asset. It rejects missing or changed members and symlinks escaping the project
root. Valid edits to MaaC sources do not invalidate the import record.
Verification needs no access to the original input path after the project is
moved. For absolute in-project symlinks, verification accepts targets under
the selected or canonical project root spelling; another alias for that root
may be rejected. Ordinary `check` and `compile` retain their existing symlink
behavior.

The importer enforces the existing 4 MiB per-asset limit and validates that
the generated standalone plan fits the normal plan serialization limit before
publishing. The provenance sidecar is not needed to check, compile, or build
the generated project.

`--disk-media` selects a separate file-backed import path for a decoded crop up
to 1 GiB of native PCM. The WAV input remains capped at 1 GiB. Conversion writes
to a private file with bounded memory, then validates the staged project using
the disk-media compiler before atomic publication. It keeps the same `main.maac`,
`media.pcm`, `import.json`, and optional `original.wav` layout and version 1/2
import records. Reopened projects use `check --disk-media` and
`build --disk-media`; `compile` still requires the ordinary inline limits.
The default execution-work profile remains in force. Select `--profile song`
on import and retained verification for longer projects, and use the same
profile on later `check` or `build` commands when their work exceeds the
default allowance. The profile changes work allowance only, not byte limits.
Use `patch --disk-media [--profile song]` for source-preserving Protocol 2
changes to the imported clip's source frames, placement, fades, or other
supported fields. These edits preserve the immutable import provenance and
use the current captured dependency closure; see the
[editing contract](editing-kernel.md#source-edits-with-disk-backed-media).
For retained imports, `verify-import --disk-media` re-decodes and compares the
original, PCM, and current project dependency closure using bounded private
snapshots. An ordinary `verify-import` remains limited to the inline profile.

## Acceptance boundary

After an embedded import, the project can be moved and reopened with ordinary
`check`, `compile`, and `build` commands without access to the original WAV.
Larger file-backed imports use `check --disk-media` and `build --disk-media`
with the appropriate execution profile. This is a bounded crop-import and
reopen result. The original WAV is retained only with
the opt-in flag; this does not define a dependency-complete editable archive.
The DSP artifact engine separately offers opt-in disk-backed sampling for
validated PCM that is still embedded in the artifact. This writes a private
temporary snapshot and uses a two-page cache bounded to 32 KiB per asset; the
artifact still retains its bytes. Existing per-asset, aggregate, and plan-size
limits are unchanged. It avoids an additional full decoded-PCM allocation but
does not support external-media or long-media streaming. The
[native archive](editable-archive.md) can preserve this verified original WAV
and its import record in a version 3 checkpoint. A version 6 archive can
retain several explicitly selected existing import directories used by one
composition. Archive patching records explicit entry-source and direct library
edits. Whole-output freezes and a single native effect output freeze support
explicit reuse after conservative input validation. Broader edit journaling,
processor-state packaging, broader selective invalidation, and multiple frozen
graph branches are not implemented.
