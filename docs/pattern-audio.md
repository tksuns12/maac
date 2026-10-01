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
they refuse top-level audio. Each placement output becomes a `core.sum/1` plan
node with the placement's ID. Each surviving occurrence becomes an ordinary
clip node:

- **Clip node:** `__clip_<place>_<k>`, in expansion order, then inserts.
- **Connection:** `__route_<place>_<k>`, to the placement's `in` port.
- **Clip `source` mapping:** names the leaf and its pattern path.
- **Clip `track`:** the placement's track.

A source declaration that uses a reserved ID is `E_DUPLICATE_ID`.

Every occurrence is a plan node, so the plan node limit (256 in total,
including all other nodes) bounds the number of occurrences. Every warped
occurrence also counts its anchors against the shared warp point budget.
Occurrences are transports, not events. Score-window queries, event counts,
and event dispatch do not include them.

## Limits

- A library document declares no assets and cannot reference a composition's
  assets, so a library pattern with an audio leaf is `E_REFERENCE` when used.
- Mixed channel counts under one placement are refused, never remixed.
- Overrides cannot replace an occurrence's recording, slice, mode, or warp map.
  Edit the leaf, or materialize the placement and edit the copy.

See the [public vectors](../tests/pattern_audio.rs).
