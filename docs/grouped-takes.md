# Synchronized microphone-file takes

`maac.takes/2` extends the take contract to a fixed set of named microphone
lanes. A comp region chooses one take for every lane together. Each selected
lane is checked against an explicit native audio clip; routing, gain, and fades
remain ordinary authored graph and clip behavior.

This is an offline authoring contract for existing files. Source origins
express the author's alignment choices. MaaC does not infer acoustic phase,
measure device latency, capture from devices, or correct drift. Playback,
warped/reversed comps, and automatic crossfades remain outside this capability.

## Version and descriptor

A project must require `"maac.takes/2"` and provide one extension with that
namespace, `render_affecting=true`, and a top-level descriptor reference. The
schema pin and bytes must match [`takes-v2.schema.json`](../takes-v2.schema.json).
The descriptor path resolves from the package root, including for nested source
entries. The schema describes tagged syntax data; semantic validation also
checks assets, timing, references, bounds, and selected clips.

The original [`maac.takes/1`](takes-and-comping.md) schema and meaning remain
unchanged. One extension of each take version may coexist with the existing
production delivery extension. Each present version needs its own matching
required capability and descriptor. A requirement without its extension fails.
No grammar, Protocol 2, saved-plan, or archive version changes are introduced.

## Group, take, and region records

```maac
extension microphones {
  namespace = "maac.takes/2";
  schema = &microphone_schema;
  render_affecting = true;
  data = {
    groups = {
      drums = {
        origin = 1s;
        takes = {
          first = {
            lanes = {
              close = { asset = &first_close; source_origin = 12frame; };
              room = { asset = &first_room; source_origin = 20frame; };
            };
          };
          second = {
            lanes = {
              close = { asset = &second_close; source_origin = 4frame; };
              room = { asset = &second_room; source_origin = 9frame; };
            };
          };
        };
        regions = {
          phrase = {
            take = first;
            range = [0frame, 48000frame];
            clips = { close = &close_phrase; room = &room_phrase; };
          };
        };
      };
    };
  };
}
```

The snippet assumes declared assets, schema, and audio clips. The complete
[synthetic example](../examples/grouped-takes.maac) renders two lanes to separate
left and right output channels.

All records are closed: groups contain exactly `origin`, `takes`, and `regions`;
takes contain exactly `lanes`; lane members contain `asset` and `source_origin`;
regions contain `take`, `range`, and `clips`. Group, take, region, and lane keys
use ASCII identifiers of at most 128 characters. Maps must be nonempty.

Every take and region in a group must contain exactly the same lane keys,
with one to sixteen lanes. All files in a group share one sample rate. A given
lane keeps the same channel count across takes; different lanes may be mono or
stereo independently. Each membership names a distinct top-level native PCM
asset within the group. Files may have different lengths.

`source_origin` is the unsigned integer frame aligned to common group frame
zero, and must precede its file's end. `origin` is nonnegative physical time in
seconds or milliseconds under `T(0)=0`. A region's `range=[a frame,b frame]` is a
nonempty unsigned half-open interval in that common frame coordinate system.
Regions within a group must not overlap; adjacent intervals and gaps are valid.

For the selected take, every lane's clip must satisfy:

| Clip field | Required value |
| --- | --- |
| `asset` | The selected take's asset for that lane |
| `source` | `[source_origin+a frame, source_origin+b frame]`, within that file |
| `at` | `group.origin + a/R` seconds, where `R` is the shared sample rate |
| `mode` | `rate` |
| `speed` | Omitted or exactly `1` |
| `reverse` | Omitted or `false` |

All frame additions are checked. Every selected lane must cover the entire
region. A shorter inactive lane is allowed until a selection would exceed its
available frames. Every declared alternate file remains hash-pinned and retained
in the source closure, even when inactive. A clip may belong to only one region
and lane across both take extensions.

Fades, gain, track membership, and routing remain independently authored. The
contract synchronizes source selection and clip coordinates; it does not force
identical processing, infer routing, fill missing lanes, or silently resample
a member with a different rate.

## Atomic editing and reopen

Select another take with one Protocol 2 transaction that changes the region's
`take` and every lane clip's `asset` and `source`. A region-range or common-origin
change must update every affected clip in that transaction. Intermediate
operations may disagree, but the complete candidate must validate before
publication. A one-lane switch, missing lane, incorrect crop, or incorrect
placement fails without overwriting the destination.

```text
maac check PROJECT --disk-media
maac patch PROJECT SELECT.json --disk-media -o PROJECT/main.maac --force
maac build PROJECT --disk-media -o comp.wav
```

Existing revision conflicts, source-preserving edits, returned inverses, and
archive journals apply. Tagged asset and clip references follow `rename_id`;
local lane and take keys require coordinated record edits. Saved plans carry
the resolved selected audio, while source archives preserve every alternate,
the descriptors, authored lane membership, and selection history after
relocation. Production delivery can expose lanes through explicit output ports
and stems.

## Bounds and evidence

Across both versions combined, at most 64 groups, 64 takes, and 256 regions are
accepted. V2 permits at most 16 lanes per group; the resulting metadata maxima
are 1,024 file memberships and 4,096 clip bindings. Existing limits still apply,
including 64 total bundle assets with descriptors, native graph limits, work
budgets, and media byte limits. These maxima are not a promise that every
combination fits those other limits.

See the [verification record](verification.md) for exact-sample switching,
atomic rejection, undo, compatibility, and relocated archive evidence. The
separate [`play`](playback.md) command auditions rendered output on macOS.
Device capture, low-latency transport, listening acceptance, and the
representative production workload remain in the
[roadmap](end-to-end-production-plan.md).
