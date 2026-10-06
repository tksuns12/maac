# Take groups and explicit comp selections

`maac.takes/1` is a bounded source capability for alternate mono/stereo audio
takes and comp regions. It checks that a selected take agrees exactly with an
existing rate-mode audio clip. Ordinary clips provide the audible result;
source records preserve take membership, alignment coordinates, and selection.

The capability uses existing `extension`, asset, audio, and Protocol 2 syntax.
It adds no processor or performance-plan version. Recording from devices,
monitoring, separate synchronized microphone files, playback transport,
warped/reversed comp clips, and automatic crossfades remain outside this v1
slice. [`maac.takes/2`](grouped-takes.md) separately implements synchronized
microphone-file lanes while preserving this schema and v1 behavior.

## Source contract

A project using this capability must list `"maac.takes/1"` in `project.requires`
and contain exactly one extension with that namespace. The extension requires
`render_affecting=true`, a `schema` reference to a top-level descriptor asset,
and `data`. The descriptor's hash and bytes must match the shipped
[`takes.schema.json`](../takes.schema.json). Its path is relative to the package
root, including when the composition entry is in a subdirectory. One v1 take
extension can coexist with one v2 take extension and the existing production
delivery extension. Each version needs its matching required capability.

```maac
extension takes {
  namespace = "maac.takes/1";
  schema = &takes_schema;
  render_affecting = true;
  data = {
    groups = {
      vocals = {
        origin = 0s;
        takes = {
          first = { asset = &recording_a; source_origin = 0frame; };
          second = { asset = &recording_b; source_origin = 240frame; };
        };
        regions = {
          phrase = {
            take = first;
            range = [0frame, 48000frame];
            clip = &phrase_audio;
          };
        };
      };
    };
  };
}
```

The example assumes declared assets and an audio clip. A complete runnable
example is [`examples/take-comp.maac`](../examples/take-comp.maac).

Each group contains exactly `origin`, `takes`, and `regions`, with nonempty
member and region records. `origin` is nonnegative absolute physical time in
seconds or milliseconds, using the existing `T(0)=0` convention. Group, take,
and region keys are stable local record identifiers.

Each take contains exactly `asset` and `source_origin`. The asset is a
top-level hash-pinned native mono/stereo audio asset. All members of a group
have the same sample rate and channel count, and reference distinct assets.
`source_origin` is an unsigned integer source-frame index below the asset's
frame count. It identifies the source frame aligned to common group frame zero.
Assets may have different lengths. The declared origin is an authored alignment
choice; MaaC does not measure capture latency or infer synchronization.

Each region contains exactly `take`, `range`, and `clip`. `take` is a symbol
naming a member of that group. `range=[a frame,b frame]` is a nonempty half-open
interval in the common group coordinates, with unsigned integer endpoints.
`clip` references a top-level `audio` object without a port suffix. A clip is
managed by at most one region across the extension. Region intervals in a
group must not overlap; touching intervals and gaps are allowed.

For a selected take with source origin `o` and sample rate `R`, validation
requires exact agreement:

| Clip field | Required value |
| --- | --- |
| `asset` | Selected take's asset reference |
| `source` | `[o+a frame, o+b frame]`, within the selected asset |
| `at` | `group.origin + a/R` seconds, expressed in seconds or milliseconds |
| `mode` | `rate` |
| `speed` | Omitted or exactly `1` |
| `reverse` | Omitted or `false` |

Existing clip gain, explicit fades, tracks, and routing keep their ordinary
meaning. No routing or silence filling is inferred. An inactive alternate may
be shorter than the selected interval; attempting to select an unavailable
interval fails. All declared alternate assets remain in the verified source
closure, including inactive takes.

## Selection, editing, and identity

Selecting another take uses one Protocol 2 transaction updating the region's
`take` plus its clip's `asset` and `source`. Changing a common range or origin
must update the corresponding clip fields in the same transaction. Temporary
disagreement between operations is permitted; the final candidate must satisfy
the complete contract before publication. A metadata-only or clip-only change
that leaves them inconsistent fails.

Use the existing commands:

```text
maac check PROJECT --disk-media
maac patch PROJECT SELECT.json --disk-media -o PROJECT/main.maac --force
maac build PROJECT --disk-media -o comp.wav
```

The normal `song` work profile is available where supported. Transactions
retain revision conflicts, source-preserving forward projection, returned
inverses, and conservative render invalidation. Asset and clip `rename_id`
operations rewrite the tagged references in take records. Local take/region
keys are edited through ordinary atomic record-field changes.

Editable archives preserve exact source, the pinned descriptor, every take
asset, selection history, and inverse transactions. Saved plans carry the
validated selected audio for replay; editable take membership lives in source.
Unknown extensions, unavailable dependencies, and mismatches fail explicitly.

## Bounds and evidence

At most 64 groups, 64 total take entries, and 256 total regions are accepted
across v1 and v2 combined. Clip ownership is also shared across both versions.
Existing source, asset, graph, exact-arithmetic, duration, and work limits also
apply. Frame offset addition is checked before use. Schema shape validation
alone does not establish asset identity, alignment, or clip agreement.

The [verification record](verification.md) describes executable acceptance,
including exact selected samples, atomic switching and inverse restoration,
production delivery coexistence, and archive relocation. This capability
provides the original single-file comp-selection contract.
