# Authoring density: chord leaves, grooves, and drum kits

**Status:** accepted and implemented, 2026-10-05. The chord leaf is
[§8.2](../MaaC-1-Specification.md#82-chords), the groove
[§9.2](../MaaC-1-Specification.md#92-grooves), and kit instruments are in the
[instrument contract](instruments.md#kits). [Results](#results) compares the
estimates below with what landed.

## Why

The [T2 production trial](ai-production-trial.md) ranked authoring density
third among the things to build (F1–F3). An AI producer writes MaaC source
directly and reads it back on every edit, so source length is its working
cost. In "Late Window" ([`examples/lofi/lofi.maac`](../examples/lofi/lofi.maac)),
150 of the 318 non-blank lines are note objects:

- **Chords (F1).** 81 of the 150 notes are members of 21 chords. Each chord
  member repeats the same `at` and `dur`.
- **Swing (F3).** 59 notes sit at hand-calculated swung positions such as
  `8/3q` and `11/6q`. Nothing in the source says the part is swung. In the
  trial, parts written on different grids went unnoticed until the owner heard
  them (F12).
- **Drums (F2).** Each synthesized drum needs its own node, track, pattern,
  placement and connection, and every hit is a note with a placeholder pitch
  and gate: `note k1 { at = 0q; dur = 3/20q; pitch = C2; velocity = 0.85; }`.

## The existing position, and why this proposal revisits it

MaaC already has an answer for these needs, and this proposal departs from
part of it.

- [§9](../MaaC-1-Specification.md#9-finite-patterns-and-composition) says
  quantization and swing "may be authoring operations; their materialized
  events or explicit finite uses are stored".
- The [production language review](professional-production-language-review.md)
  ranks deterministic groove fifth: "Groove, harmony, and strum tools should
  first lower to finite notes".
- [§19](../MaaC-1-Specification.md#19-regions-and-non-rendering-information)
  says "a chord in core is simultaneous notes".

Lowering through a tool keeps the core small, but the trial shows what it
costs an AI author:

1. **The long form stays in the source.** A tool that expands a chord or
   swings a part writes the same 81 or 59 objects back. The author still reads
   and edits all of them.
2. **The intent is lost.** Lowered swing is a set of positions. A later edit
   written straight breaks the feel, and the source cannot say which grid
   applies. That is exactly how F12 happened.
3. **These are not generators.** A chord is a finite list of pitches. Swing is
   an exact, piecewise-linear map of score time, in the same class as the
   `stretch` that `use` and `place` already apply. A drum kit is an instrument
   that routes keys to sounds. None needs control flow, randomness or a
   runtime. The [design principles](design-principles.md) forbid hidden
   computation, not finite declared structure.

The proposal therefore keeps tool lowering for open-ended generation
(probability, humanizing, Euclidean rhythms, strums). It adds three finite
declarative structures. A chord leaf is still simultaneous notes, so §19
stays true; only §9's sentence about swing changes.

## 1. Chord leaf

A `chord` is a pattern leaf that denotes simultaneous notes with one onset and
one gate.

```maac
// Straight positions: under the groove of section 2, `5/2q` sounds at `8/3q`.
pattern ch_fmaj9 { length = 4q;
  chord a { at = 0q;   dur = 7/5q; pitches = [A3, C4, E4, G4]; velocity = [0.5, 0.45, 0.45, 0.42]; }
  chord b { at = 5/2q; dur = 6/5q; pitches = [A3, C4, E4, G4]; velocity = 0.36; }
}
```

**Fields.**

- `at`, `dur` and `pitches` are required. `pitches` is a nonempty list of
  pitch values, in any form a note's `pitch` accepts.
- `velocity`, `release_velocity`, `onset_offset` and `release_offset` are
  either one value for every member or a list with one entry per member. A
  list of the wrong length is `E_RANGE`.
- `order` and `label` are single values.
- A chord may contain `expression` children. Each one applies to every member.

**Meaning.** Member `k` behaves exactly like a note with the chord's `at` and
`dur`, pitch `pitches[k]`, and the member's per-note fields. Expansion,
stretch, transposition, `cut`, overrides and queries treat members as notes.
Lists make strums expressible with explicit offsets, such as
`onset_offset = [0ms, 12ms, 24ms, 36ms]`; no strum rule is inferred.

**Identity.**

- The structured event address gains an optional final member index:
  `(placement, repetition, [use, repetition ...], leaf, member)`. Its display
  form is `a.2`; identifiers cannot contain `.`.
- An override addresses one member: `event = "3/a.2"`. Changing a member's
  pitch keeps its identity, as it does for a note.
- Appending a pitch keeps the existing indices. Inserting or removing a pitch
  in the middle shifts later members. That is a structural change: the edit
  protocol must update or reject overrides that address the shifted members,
  as it already must when a leaf moves between patterns.

**Boundaries.** Members sharing a pitch are allowed, as they are for notes.
Members compile to ordinary plan note events. Their addresses carry the
member suffix, which today's plan validation rejects: accepting it is the one
plan-format change, and it needs a plan revision.

## 2. Groove

A `groove` is a named, exact swing map. Placements opt in by reference, so
every part shares one declared feel.

```maac
groove lazy { grid = 1/2q; ratio = 2/3; }
place hats_main { pattern = &hat_bar; track = &hat_notes; at = 16q; count = 22; groove = &lazy; }
```

The author writes straight positions. Here an eighth at `1/2q` sounds at
`2/3q`, and a sixteenth at `3/4q` sounds at `5/6q`. These are the positions
"Late Window" calculates by hand today.

**Fields.**

- `grid` is a positive musical duration: `1/2q` swings eighths, and `1/4q`
  swings sixteenths.
- `ratio` is a rational from 1/2 (straight) through 3/4 (dotted). It is the
  fraction of each pair of grid steps that the first step takes: 2/3 is
  triplet swing.

**The map.** Let `P = 2*grid` and `r = ratio`. For a final score position `t`,
measure `p = t - bar_start` from the start of the bar containing `t`. Let
`k = floor(p/P)` and `f = p - k*P`:

```
f <= grid:  f' = 2*r*f
otherwise:  f' = 2*r*grid + 2*(1-r)*(f - grid)
t' = bar_start + k*P + f'
```

The map is continuous and increasing. It fixes every period boundary and bar
line, and it is exact in rationals. A ratio of 1/2 is the identity.

**Rules.**

- **Bar fit.** `P` must divide the length of every bar in which a grooved
  position falls. Otherwise it is `E_RANGE` at the placement's `groove`
  field. A 7/8 bar with eighth swing therefore fails rather than drifting off
  the grid.
- **Order of operations.** Grooving happens after expansion, stretch,
  transposition and `cut`, and before overrides and inserts. Physical offsets
  come after all of these.
- **What moves.** Note onsets and gate ends are mapped independently, so
  legato stays legato. Hit and message onsets are mapped.
- **What does not move.**
  - Override `set.at` values and inserts keep their final coordinates, as the
    §11 final-state rule requires.
  - Audio leaves keep their positions: a recording carries its own feel.
- **Scope.**
  - `groove` is a field of `place` only. Nested `use` grooves would compose
    maps and are not proposed.
  - Event identity, addresses and the plan format are unchanged. Plans carry
    the final positions.

`maac analyze` keeps its `groove_mismatch` finding for parts that still
disagree. The guide changes from "calculate swung positions" to "write
straight and reference one groove".

## 3. Drum kit instrument

A kit is an instrument whose voices are other instruments, selected by `hit`
keys. It reuses the existing `hit` leaf and the existing drum sounds.

```maac
// In a library, or in the composition itself.
instrument boom_bap {
  channels = 2;
  piece kick  { instrument = &kick;       key = "kick";  gate = 100ms; }
  piece snare { instrument = &snare;      key = "snare"; gate = 100ms; }
  piece hat   { instrument = &closed_hat; key = "hat";   gate = 30ms; choke = "hats"; }
  piece open  { instrument = &open_hat;   key = "open";  gate = 200ms; choke = "hats"; }
  control kick_level  { target = &kick.params.level;  default = 1; }
  control snare_level { target = &snare.params.level; default = 1; }
  control hat_level   { target = &hat.params.level;   default = 1; }
  control hat_pan     { target = &hat.params.pan;     default = 0; }
}
```

```maac
node drums { instrument = &boom_bap; params = { kick_level = 0.22; snare_level = 0.28; hat_level = 0.4; hat_pan = 0.35; }; }
track drum_hits { target = &drums:events; }
pattern beat { length = 4q;
  hit k1 { at = 0q;   key = "kick";  velocity = 0.85; }
  hit s1 { at = 1q;   key = "snare"; velocity = 0.7; }
  hit k2 { at = 7/4q; key = "kick";  velocity = 0.5; }
  // ...
}
```

**Pieces.**

- A kit instrument has `channels` and one or more `piece` children, and no
  `voice` or `shared` graph.
- Each piece has:
  - a required `instrument` reference, with optional `preset` and `params`;
  - a required unique string `key`;
  - a required physical `gate`;
  - an optional `pitch` (default `C4`);
  - an optional `voices` (default 8);
  - an optional `choke` group string.
- A piece's instrument must have the kit's channel count. Pieces cannot be
  kits.

**Dispatch.**

- A hit with key `k` starts a note on the matching piece: at the hit's
  scheduled onset, with the piece's `pitch` and the hit's velocity, released
  after `gate`. This is the "declared trigger behavior" that §8 leaves to the
  receiver.
- An unknown key is an error, as in `core.kit/1`. A kit accepts hits only; a
  note sent to it is an error.
- **Choke.** A hit in a choke group releases the sounding voices of the
  group's other pieces at that frame. The existing rule that note-offs precede
  note-ons orders this.
- **Output.** The kit's output is the sum of its pieces' outputs, in piece ID
  order.

**Controls.** Kit controls target a piece's public controls
(`&kick.params.level`). They follow the existing control rules.

**Library.** `std/basic/1.0.0` stays frozen. A new `std/basic/1.1.0` exports
a ready kit built from its eight drum instruments, with open and closed hats
in one choke group. A composition may still declare its own kit.

**Cost.** This is the largest of the three. It needs:

- a kit form in the instrument contract;
- a plan representation for kits;
- key dispatch in the renderer;
- a hit-to-key mapping in the MIDI adapter. Until that mapping exists, hits
  are reported as losses.

## Effect on "Late Window"

| | Today | With all three |
| --- | --- | --- |
| Leaf objects | 150 notes | 90: 21 chords, 56 notes and 13 hits |
| Non-blank lines | 318 | about 250 |
| Hand-calculated swung positions | 59 | 0, and one `groove` object |
| Drum objects (nodes, tracks, patterns, placements, connections), with the std kit | 16 | 7 |
| Edits to change the swing feel | 59 | 1 |

## Results

"Late Window" after all three:

| | Before | Estimate | Landed |
| --- | --- | --- | --- |
| Leaf objects | 150 notes | 90 | 90: 21 chords, 58 notes and 11 hits |
| Non-blank lines | 318 | about 250 | 266 |
| Hand-calculated swung positions | 59 | 0 | 0, and one `groove` object |
| Drum objects | 16 | 7 | 13 |
| Edits to change the swing feel | 59 | 1 | 1 |

- **Fewer drum objects than estimated.** The kit saves fewer objects because
  a kit has one output. The snare alone feeds the reverb send, so it stays
  its own node; the kick and hats play from the `std/basic/1.1.0` kit. Kit
  piece outputs for per-drum processing would need multiple output ports per
  node, which the engine does not have.
- **The same music.** Every change kept the event timing: the chord and
  groove rewrites produced the same 629 events and a byte-identical master.
  The kit plays the same kick and hat onsets and velocities. Its closed-hat
  gate is the kit's 35 ms in place of the earlier 33 ms.

### Implementation notes

- **Kits run at render time.** A plan note needs a positive gate in score
  time, and a physical gate cannot be expressed exactly in q under every
  tempo map. Hits therefore stay hits in the plan, and the kit node starts and
  releases piece notes itself. A kit renders bit-identically to its pieces as
  separate nodes playing notes with the same gates.
- **Not yet supported.** Native archives and the generic renderer refuse kits
  explicitly, and a kit control cannot expose a reset-rate control.

## Versioning

These additions make previously invalid source valid and change the meaning
of no valid source. [§26](../MaaC-1-Specification.md#26-versioning-and-extensions)
requires a new language version or a capability only for changes to the
meaning of valid source, so they can join `maac 1`. The precedent is
[audio leaves in patterns](pattern-audio.md), added to core §9.1. The
alternative is a required capability such as `maac.authoring/1`, which makes
older hosts fail with `E_CAPABILITY` rather than `E_UNKNOWN_FIELD`.

## Acceptance

- **Chord.** "Late Window" rewritten with chord leaves expands to the same
  pitches, times and velocities in `maac query-events`. The render differs at
  most by voice-summation order, because member addresses sort differently.
  - Overrides and deletions address single members.
  - A structural edit that shifts an addressed member is rejected or updated.
- **Groove.**
  - Straight hats with `ratio = 2/3` produce exactly the hand-swung
    positions.
  - `ratio = 1/2` changes nothing.
  - An override's `set.at` stays unswung.
  - A 7/8 bar under eighth swing is `E_RANGE`.
- **Kit.**
  - The "Late Window" drums as one kit node with hits match the three-node
    version at equal gates, up to summation order.
  - Choke and unknown-key cases behave as specified.

## Decisions

The owner accepted every recommendation on 2026-10-05:

1. **Chord leaf:** a `chord` leaf with member-index identity.
2. **Groove:** a named `groove` object referenced by `place`, bar-aligned,
   mapping onsets and gate ends but not overrides, inserts or audio leaves.
3. **Drum kit:** a kit instrument with `piece` children, hit keys, gates and
   choke groups, plus `std/basic/1.1.0`.
4. **Versioning:** additive `maac 1` core.
5. **Order:** chord, then groove, then kit.
