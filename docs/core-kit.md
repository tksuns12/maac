# Native hits and sample kits: implementation contract

Status: implemented and validated. Existing Rust APIs remain compatible;
an additive, extensible plan-artifact API carries the new saved format.
See the [validation report](core-kit-validation.md) for evidence and limits.

This slice implements existing MaaC/1 sections 6, 8–11, 14.1, and 18.8. It adds
no source syntax. Shared event timing belongs in the musical core; `core.kit/1`
defines playback; sample preparation belongs in external tools. Reusable sample
packaging is a later library concern under the [design principles](design-principles.md).

## Assets

Core audio assets use `pcm_f32le_interleaved/1`: headerless, little-endian,
interleaved IEEE-754 binary32 samples. Metadata specifies a positive integer Hz
rate, channel count, nonnegative frame count, package-relative path, and exact
SHA-256 identity. The engine supports mono and stereo assets in this slice.
An asset may have a rate different from the 48 kHz engine rate.

The opened bytes must match both the hash and checked `frames * channels * 4`
length. Samples must be finite. Signed zero, subnormals, amplitudes outside
[-1, 1], and zero-frame assets remain valid; loading never clips or normalizes.
Allocation and decoding follow metadata and resource preflight.

Use the existing contained offline bundle loader. Core paths resolve from the
package root, including when the entry source is nested. Absolute paths,
package escapes, missing assets, mismatched hashes, malformed data, and resource
exhaustion fail explicitly. Existing 4 MiB per-file, 16 MiB aggregate asset,
64-asset, and 4 MiB serialized-plan bounds remain in force.

Saved plans retain exact source bytes and metadata and validate them again.
Playback must work after source files and external assets are removed. The plan
payload must not reinterpret samples as wavetables or synthesis graphs.

## Hits and arrangement

A native `hit` has `at`, string `key`, velocity (default 1, range [0, 1]),
physical onset offset (default zero), and integer order (default zero). It has
no pitch, duration, release time, note-off, or per-note expression.

Hits participate in finite patterns, nested uses, placements, repetition,
stable occurrence addresses, inserts, deletion, and final-state overrides.
Musical stretch transforms the onset; physical offsets remain unchanged.
Transposition has no effect on unpitched keys. Cut boundaries do not invent a
gate or shorten a sample tail. Overrides may replace `at`, `key`, `velocity`,
`onset_offset`, `order`, and `label`; they cannot introduce
note fields, change event kind, or change the destination. `set.at` is the final
musical offset from the placement's global origin; `set.onset_offset` remains
an independent physical offset. Inserts use placement-local final coordinates
and are not automatically repeated.

Use the shared exact/certified tempo clock for step maps and ramps. The effective
onset must lie in the score interval. Its frame is the certified ceiling relative
to the reset origin and must be strictly below the certified score-end frame.
A physically pre-end hit that rounds to the score-end frame fails with
`E_INTERVAL`: it is neither dropped, moved earlier, nor issued during the tail.
Hits and note-ons share their
specified scheduling class and sort by order and stable address after note-offs.

## Kit playback

`core.kit/1` has required mono/stereo `channels`, positive bounded `voices`
(default 64), and a nonempty `samples` list mapping unique string keys to audio
asset references. Every mapped asset must match the configured channel count.
Its `events` input accepts matching hits; its `out` port has the configured
audio width. Numeric sample-rate `level` defaults to 1 and must be nonnegative.
Unknown keys, incompatible event kinds, and voice overflow are errors.

A hit starts sample coordinate zero at its scheduled frame and plays once at
the asset's original physical rate. At engine frame `n`, a voice started at
`N` reads source coordinate `(n - N) * asset_rate / engine_rate`. Compute its
integer index and fractional remainder without accumulated phase drift. Apply
the specified linear interpolation, with zero outside the source buffer,
including beyond the final sample. Multiply by hit velocity and current level.
Sum voices in event-address order and reject nonfinite output.

At the first engine frame whose source coordinate is at least the asset's frame
count, the voice expires and is reclaimed before same-frame hits are allocated.
A zero-frame asset validates its key but allocates no voice slot and cannot
overflow the voice capacity. A
zero-velocity hit on a nonempty sample still consumes a voice for that lifetime.
Started samples continue through score end into the declared tail and stop at
source end or overall render end. No fade, choke group, looping, automatic
voice stealing, or gain adjustment is inferred. Engine reset reproduces output.

Sample storage, configured voice capacity, and interpolation work are bounded
before rendering. Work accounting includes the natural sample tail, clipped at
render end, and applies to silent hits and unconnected processors as appropriate.
The existing event/voice-capacity allowance includes hits. Structural work
includes raw asset bytes; sample records and mappings participate in aggregate
object and string allowances. Conservative kit execution work charges
`total_frames * (4 + channels)` per node and
`active_frames * (4 + 8 * channels)` per hit, where `active_frames` is the
ceiling of the natural sample lifetime in engine frames, clipped at render end.
These are accounting units, not a wall-clock performance guarantee.

## Saved plans and Rust compatibility

Kit-enabled projects use performance-plan version 4. Existing sources without
kit assets or hits retain their existing version and bytes. Versions 1–3 and
their existing public Rust types and entry points retain their contracts.
Kit outputs participate in the existing explicit audio graph, production
effects, and named deliveries. Production identity and saved-plan replay must
account for the exact sample resources as well as the hit occurrences.

Version 4 retains version 3's exact timing recipes and onset-only hit records
with absent off coordinates/frames. The shared event record's unused release
offset and release velocity are canonically zero for hits. It adds exact `audio_assets` payloads and
nodes that explicitly distinguish existing core processors from kit processors.
Existing core processor definitions are nested unchanged under the new node
representation. Kits carry channel count, voice capacity, and key-to-asset
references. Raw asset bytes serialize as byte arrays, preserving their identity.
Unknown or duplicate fields, invalid references, incompatible metadata, and
tampered timing or sample payloads are rejected on import.

An opaque `PlanArtifact` wrapper dispatches supported saved-plan versions
without exposing a closed public enum that callers must exhaustively match.
Additive artifact compile/load/render/export entry points share internal
validation and runtime machinery; existing `VersionedPlan` and `Processor`
enums are unchanged. V4 wire data types remain internal; artifact accessors and
strict JSON serialization provide inspection without committing callers to new
closed enums. No kit is represented as a fake sine or instrument node.

## Acceptance evidence

- Asset tests cover package-root resolution, containment, hash/length/format
  rejection, finite sample preservation, zero frames, and allocation bounds.
- Compilation and imported plans agree on hit identity, transforms, overrides,
  step/ramp scheduling, receiver/key validation, and onset-only timing.
- Analytical sample vectors cover mono/stereo playback, equal and unequal rates,
  interpolation toward zero, overlapping voices, same-frame reclamation,
  overflow, level automation, score-end tails, and reset replay.
- Installed CLI source builds and saved-plan renders produce identical WAV bytes
  after copied source/assets are removed. Invalid input and failed export
  preserve existing destinations. Legacy source/plans retain behavior.
- Formatting, Clippy, the full suite including the explicit metering audits,
  release/install checks, and independent review pass on the final snapshot.

WAV importing, arranged `audio` transports, raw protocol messages, choke groups,
sample-kit library exports, and new sample packs are outside this first slice.
Automated waveform evidence does not assert human listening acceptance.

## Try the example

The [drum example](../examples/core-kit.maac) contains three original samples,
24 hits, tempo ramps and kit level automation. From the repository root:

```sh
maac check examples/core-kit.maac --project-root .
maac build examples/core-kit.maac --project-root . -o core-kit.wav --format pcm16
maac compile examples/core-kit.maac --project-root . -o core-kit.plan.json
maac render core-kit.plan.json -o core-kit-from-plan.wav --format pcm16
```

The result is 208,158 mono frames at 48 kHz, including a quarter-second tail.
The retained plan embeds all samples. Its original source files and PCM assets
are unnecessary for rendering or delivery.

For a new sample, prepare the raw bytes outside MaaC, use `maac hash FILE` to
obtain its exact pin, and declare the rate, channels and frame count. Map the
asset to a string key in `config.samples`; a hit selects that key:

```maac
hit kick_one { at = 0q; key = "kick"; velocity = 4/5; }
```

The example's [Python generator](../examples/sounds/core-kit/generate_samples.py)
is an optional authoring tool. Neither compilation nor replay executes it.
