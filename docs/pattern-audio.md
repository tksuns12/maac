# Audio leaves inside patterns

Implemented. This contract implements MaaC/1
[§9.1](../MaaC-1-Specification.md#91-audio-leaves). An `audio` object may be a
child of a `pattern` or of a placement `insert`. It then repeats, nests,
stretches, and takes occurrence edits like other pattern leaves. Each
occurrence is played by the existing [rate](audio-clips.md) or
[warp](warp-rate.md) transport.

Try the [runnable example](../examples/pattern-audio.maac):

```sh
maac build examples/pattern-audio.maac --project-root . -o pattern-audio.wav
```

It builds a one-bar drum loop from the core-kit PCM files, with a nested hat
pattern and a warped snare. The loop repeats four times under a tempo ramp and
uses one deleted occurrence, one softened occurrence, and one inserted leaf.

## Source contract

```maac
pattern groove {
  length = 4q;
  audio kick { asset = &drums; at = 0q; source = [0frame, 24000frame]; mode = rate; }
}
track drums {}
place drums_main { pattern = &groove; track = &drums; at = 0q; count = 8; }
connect drums_route { from = &drums_main:out; to = &mix:in; }
```

An audio leaf accepts the fields of a top-level `audio` object except `track`:
`asset`, `at`, `source`, `mode`, and, by mode, `speed`, `reverse`, `warp`, and
`processor`, plus `gain`, `fade_in`, `fade_out`, and `fade_shape`. Its `at` is
a nonnegative local q position before the pattern length. Seconds and
`bar(b,u)` are `E_UNIT`, and a `track` field is `E_UNKNOWN_FIELD`. Mode, source
slice, warp endpoint, and stretch-processor rules are the same as at top level.
The leaf's defaults expand identically in the editing and execution
normalizations.

## Expansion

- **Onset:** each occurrence starts at its §9 position,
  `use.at + stretch*(i*length + at)` through every enclosing use and placement.
- **Warp anchors:** their local q values are multiplied by the product of the
  inherited stretches; source frames stay the same.
- **Rate playback:** `speed`, `reverse`, duration, and fades are physical and
  are not stretched.
- **Transposition and `cut`:** neither applies, as for hits. Shorten a leaf with
  its source slice or warp map.
- **Transport rules:** every occurrence then follows the full clip contract,
  including the start interval, continuation into the tail, and output crop.

## Output and routing

A placement that reaches an audio leaf, through its pattern's `use` graph or
its inserts, has an audio output port `out`, summing every occurrence. Connect
it like any clip; nothing is routed implicitly. All leaves reached by one
placement need the same channel count (`E_PORT_TYPE` otherwise). A placement
without audio leaves has no `out` port. Its `track` may lack an event target
when the placement expands no note, hit, or message.

## Occurrence edits

Occurrences use the ordinary structured addresses, such as `groove/2/kick` or
`groove/0/pulse/3/closed`. An `override` may `delete = true` an occurrence or
`set` its `at` (placement-local final q), `gain`, `fade_in`, `fade_out`,
`fade_shape`, or `label`. Other replacements are `E_UNKNOWN_FIELD`, and note-,
hit-, or message-only fields are refused on audio and the reverse. An `insert`
may hold one audio leaf at placement-local final q; it is not repeated.

Editing tools treat audio occurrences as occurrences:

- Protocol 2 validates their override targets.
- Edit impact reports their addresses.
- `materialize-instance` copies audio leaves and their overrides. The copies
  keep sharing the immutable, hash-pinned asset, and the render is unchanged.

## Plans

The bundle artifact path compiles audio leaves. It selects a version 5 plan
for rate leaves, and version 6 (or 7 with controls) when any leaf warps. The
single-document and version 2/3 plan APIs refuse them with `E_CAPABILITY`, as
they refuse top-level audio.

Each placement with an audio output becomes one `clips` plan node with the
placement's ID and channel count. The node has no inputs and no parameters.
Its `clips` list holds one member per surviving occurrence, in expansion
order, then inserts. Each member is an ordinary rate (`audio`) or `warp_rate`
clip record:

- **Member `source` mapping:** names the leaf and its pattern path.
- **Member `track`:** the placement's track.
- **Member frames:** each member carries its own certified active interval.

```json
{"id": "loops", "processor": {"kind": "clips", "channels": 1, "clips": [
  {"kind": "audio", "clip": {"asset": "tone", "at": {"q": "0/1"}, "...": "..."}},
  {"kind": "audio", "clip": {"asset": "tone", "at": {"q": "2/1"}, "...": "..."}}
]}}
```

The node's output is the sum of its members, added in list order from zero.
A member contributes exact zeros outside its active interval. The output
equals routing each member as a separate clip node into one `core.sum/1`
node in the same order. A placement whose every occurrence was deleted keeps
an empty, silent `clips` node, so connections to its `out` port stay valid.

Plan validation checks every member as it checks a standalone clip. It also
checks that every member has the node's channel count (`E_PORT_TYPE`). A
version 5 plan may hold only rate members; a warp member is `E_VERSION`.

A placement uses one of the 256 plan nodes, whatever its occurrence count.
Occurrences are bounded instead by:

- the 4 MiB plan JSON limit, shared with inline asset bytes. A rate member
  takes about 400 bytes, so a plan holds roughly 10,000 occurrences;
- the execution work budget, which charges each member for its active frames
  plus a fixed lookup cost;
- the shared warp point budget, which counts every warped member's anchors.

Occurrences are transports, not events. Score-window queries, event counts,
and event dispatch do not include them.

The renderer indexes members by fixed 4,096-frame output buckets. Each frame
visits only the members whose interval touches its bucket.

## Limits

- A library document declares no assets and cannot reference a composition's
  assets, so a library pattern with an audio leaf is `E_REFERENCE` when used.
- Mixed channel counts under one placement are refused, never remixed.
- Overrides cannot replace an occurrence's recording, slice, mode, or warp map.
  Edit the leaf, or materialize the placement and edit the copy.

See the [public vectors](../tests/pattern_audio.rs).
