# Opt-in disk media for native PCM compositions

**Status:** Implemented Phase 3 slice. The bounded command and validation
contract is below; the current test evidence is in [verification](verification.md).

## Commands and scope

```text
maac check SOURCE --disk-media [--project-root ROOT] [--profile default|song]
maac build SOURCE --disk-media -o OUTPUT.wav [--project-root ROOT] [--profile default|song]
maac import-wav INPUT.wav --disk-media --output-dir NEW_PROJECT [--retain-original] [--profile default|song]
maac verify-import NEW_PROJECT --disk-media [--profile default|song]
```

`--disk-media` selects a process-local, file-backed render path for a MaaC
composition. The existing `asset kind = audio` declaration supplies the
package-relative path, SHA-256 hash, `pcm_f32le_interleaved/1` format, positive
sample rate, mono/stereo channels, and frame count. The score, clips, kits,
instruments, routing, automation, and native production graph retain their
ordinary authored meaning. Default `check` and `build` keep the embedded-media
profile and its existing limits.

For `import-wav`, the flag also permits a selected decoded WAV crop up to the
same 1 GiB PCM cap and stages its provenance record and optional original WAV.
`verify-import --disk-media` rechecks a retained import and its current source
closure after relocation. `--profile song` on import and verification increases
only the existing execution-work allowance, matching `check` and `build`.
The flag does not produce a standalone performance plan. `compile`, `render`,
and `deliver` retain their existing contracts and do not accept it. This path
does not change MaaC/1 syntax, the public `SourceBundle`, or saved plan versions
1–7.

## Validation and resource bounds

The command opens one pinned project root and resolves source and media
dependencies within it. Every declared media file must be a regular file.
Before rendering, the command reads the complete file through a fixed-size
buffer, checks its exact byte length (`frames × channels × 4`), SHA-256 hash,
and every float32 sample for finiteness, including samples outside selected
clip crops. It copies verified bytes to private temporary storage. Rendering
reads that snapshot through bounded frame-aligned cache pages; changes to the
original path afterward cannot alter the prepared audio.

The disk-media profile permits at most 1 GiB per PCM file, 1 GiB aggregate
PCM, and 64 media assets. The scan budget is separate from the ordinary plan
structural-work budget. Source text, non-audio assets, graph size, event count,
duration, channel count, execution work, and output rules keep their existing
limits. `--profile song` changes the existing execution-work allowance only;
it does not enlarge the disk-media byte budget.

Missing files, unsupported formats, incorrect hashes or lengths, nonfinite
samples, root escapes, resource exhaustion, and snapshot I/O failures abort
with an explicit diagnostic. `build` publishes a WAV atomically; a failed
operation preserves any existing destination, including with `--force`.

## Acceptance boundary

A composition can check and build from native PCM larger than the 4 MiB
embedded-asset limit without loading its full media into a PCM vector. A
relocated project with the same pinned files produces the same output in the
same environment. Compact compositions render the same samples through disk
and embedded paths. External media is required for this process-local path;
the project is not a dependency-complete archive, and no source-free retained
plan is produced. File-backed WAV import still snapshots the entire input and
is limited to RIFF/WAVE PCM16/24/32 or float32 mono/stereo. Recording, broader
history, freeze invalidation, and general editable archive packaging remain
separate Phase 3 work.
