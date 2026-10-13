# Strums and voicings: proposal

**Status:** accepted 2026-10-13, when the owner took every recommended
[decision](#decisions), and implemented the same day. The normative contract
is [§8.2](../../MaaC-1-Specification.md#82-chords); [Results](#results)
compares it with this proposal. This is item 3 of
[what to build next](../reports/ai-production-trial-t3.md#what-to-build-next-in-order)
in the T3 trial: F18, a strum and voicing shorthand. It is a language
change. It follows the
[authoring density proposal](authoring-density-proposal.md), which added the
chord leaf and left strums to tools.

## Why

T3's first draft was folk-pop with a strummed steel guitar. In draft 4, the
version the owner called "fine", the strums take 35 non-blank lines and 388
field values. Each of its 25 strokes is a chord leaf with three parallel
lists:

```maac
chord d1 { at = 0q; dur = 1q; pitches = [G2, B2, D3, G3, B3, G4]; velocity = [0.78, 0.74, 0.72, 0.7, 0.68, 0.66]; onset_offset = [0ms, 8ms, 16ms, 24ms, 32ms, 40ms]; }
```

- **The strum is spelled out.** A downstroke is a list of offsets
  counting up by 8 ms, and an upstroke counts down. Nothing in the source says
  "down" or "up", or how fast.
- **The chord shape is spelled out.** Draft 4 writes 8 distinct pitch lists
  25 times. A G chord's six pitches appear in every G stroke, and its
  upstrokes repeat the top four.
- **The rhythm is spelled out per chord.** The G, D, Em and C patterns play
  the same six strokes. They differ only in their pitches, so changing the
  strum rhythm means editing four patterns.

The rock ballad that replaced the draft strums only once. Its verse picking
shows the same gap from the other side. The AI wrote the G-shape pattern once
and reused it for D and C with `transpose = -500ct` and `-700ct`. Transposing
a G shape keeps the shape, so those bars play D2 and C2, below a guitar's
lowest string in standard tuning (E2). `transpose` is the only way the
language reuses a pattern with other pitches.

## The existing position, and why this proposal revisits it

[§8.2](../../MaaC-1-Specification.md#82-chords) says: "Lists make a strum
explicit; no strum or voicing rule is inferred." The authoring density
proposal kept strums for tools, with probability, humanizing and Euclidean
rhythms. It revisited swing for three reasons, and they apply to strums too:

1. **The long form stays in the source.** A tool that writes the strum
   offsets writes the same 388 values back, and the author reads and edits
   them all.
2. **The intent is lost.** Lowered offsets do not say which way the stroke
   goes or which chord it is. A later edit can break one string's offset or
   one pitch in one copy of the G shape, and nothing shows the mistake.
3. **These are not generators.** A strum is a fixed, exact offset per string.
   A voicing is a finite list of pitches. Neither needs randomness, control
   flow or a runtime.

The proposal adds three finite declarative structures. A chord is still
simultaneous notes, so
[§19](../../MaaC-1-Specification.md#19-regions-and-non-rendering-information)
stays true. The one rule it reverses is §8.2's sentence about strums and
voicings.

## Proposed contract

This is a language change to `maac 1`. Every addition expands at compile
time to the notes a chord leaf already produces. The plan format, the
renderer, the MIDI adapter and `maac query-events` see ordinary chord members
with onset offsets and velocities.

### A. Strum fields on the chord leaf

```maac
chord d1 { at = 0q; dur = 1q; pitches = [G2, B2, D3, G3, B3, G4]; strum = down; velocity = 0.78; tilt = -0.12; }
chord u1 { at = 3/2q; dur = 1q; pitches = [D3, G3, B3, G4]; strum = up; velocity = 0.46; tilt = -0.06; }
```

**Fields.**

- `strum` is `down` or `up`. Without it, the chord is struck at once, as
  today.
- `stagger` is a nonnegative physical duration: the time from one string to
  the next. It defaults to 8 ms, and only a strummed chord may set it.
- `tilt` is a dimensionless velocity change from the first string struck to
  the last. It defaults to 0, and only a strummed chord with a single
  `velocity` may set it.

**Meaning.** Let a chord have `n` members. A downstroke strikes them in list
order and an upstroke in reverse, so member `k` is struck at position
`i = k` (down) or `i = n - 1 - k` (up). Member `k` then has:

- an onset offset of `i * stagger`, added to its own `onset_offset`;
- a velocity of `velocity + tilt * i / (n - 1)`, or `velocity` when `n` is 1.

Both are exact rationals. A velocity outside `[0, 1]` is `E_RANGE`, as for a
note. `stagger` or `tilt` without `strum`, and `tilt` with a velocity list,
are `E_RANGE`.

- **Per string, not per sweep.** A hand moving at one speed takes as long
  per string on a four-string upstroke as on a six-string downstroke. Draft 4
  wrote 8 ms per string on its six-string G and Em chords and on their
  four-string upstrokes. The 8 ms default is that value. A slow final strum is
  `stagger = 30ms`, as in draft 4's last chord.
- **Tilt follows the hand.** Draft 4 made the first string struck the
  loudest in both directions: 0.78 down to 0.66 on a downstroke, 0.46 down to
  0.40 on an upstroke. A linear tilt reproduces those lists within 0.016.
- **Physical offsets.** Like `onset_offset`, the stagger does not stretch with
  `stretch` and is applied after the groove.

### B. Voicings

A `voicing` is a named, reusable list of pitches, such as a chord shape.

```maac
voicing g { pitches = [G2, B2, D3, G3, B3, G4]; }
voicing d { pitches = [D3, A3, D4, F#4]; }

pattern st_g { length = 4q;
  chord d1 { at = 0q; dur = 1q; voicing = &g; strum = down; velocity = 0.78; tilt = -0.12; }
  chord u1 { at = 3/2q; dur = 1q; voicing = &g; top = 4; strum = up; velocity = 0.46; tilt = -0.06; }
}
```

**Fields.**

- `voicing` is a new top-level kind. It requires `pitches`, a nonempty list
  in any form a chord's `pitches` accepts, and may carry `label`.
- A chord names its pitches with either `pitches` or `voicing = &name`, not
  both.
- `top = n` keeps the last `n` pitches, and `bottom = n` the first `n`, with
  `1 <= n <= ` the number of pitches. A chord takes at most one of them. They
  work with `pitches` too, but they matter for a shared voicing: an upstroke
  that catches the top four strings is `top = 4`.

**Meaning.** The chord's members are the selected pitches, in list order.
Member `k` is entry `k` of that selection, so addresses such as `d1.2` count
from the selection's first pitch. Transposition, overrides and queries treat
members exactly as before. Editing a voicing changes every chord that uses
it, as editing a shared pattern changes every use.

### C. Voicings supplied by a use or a placement

`use` and `place` gain an optional `voicing` reference. A chord with neither
`pitches` nor `voicing` takes the voicing of its nearest enclosing `use` or
`place` that names one. One rhythm then serves every chord:

```maac
pattern strum_bar { length = 4q;
  chord d1 { at = 0q;   dur = 1q;   strum = down; velocity = 0.78; tilt = -0.12; }
  chord d2 { at = 1q;   dur = 1/2q; strum = down; velocity = 0.6;  tilt = -0.08; }
  chord u1 { at = 3/2q; dur = 1q;   top = 4; strum = up; velocity = 0.46; tilt = -0.06; }
  // ...
}
pattern prog_verse { length = 16q;
  use b1 { pattern = &strum_bar; at = 0q;  voicing = &g; }
  use b2 { pattern = &strum_bar; at = 4q;  voicing = &d; }
  use b3 { pattern = &strum_bar; at = 8q;  voicing = &em; }
  use b4 { pattern = &strum_bar; at = 12q; voicing = &c; }
}
```

**Rules.**

- **Nearest wins.** An inner `use` that names a voicing replaces an outer
  one for the chords below it.
- **Explicit pitches win.** A chord's own `pitches` or `voicing` ignores any
  supplied voicing.
- **No silent default.** A chord with no pitches and no voicing in scope is
  `E_REFERENCE` at the chord. A `top` or `bottom` larger than the supplied
  voicing is `E_RANGE` at the chord, naming the use.
- **Transposition** applies to the supplied pitches like any other.
- **Identity.** Member addresses keep their form. One rhythm pattern can
  give a different member count under each voicing, so an override that
  addresses `d1.5` under a four-pitch voicing is `E_INSTANCE_TARGET`, as for
  any missing occurrence.

## Effect on T3 draft 4

Draft 4's five stroke patterns, rewritten with each part. With C, the 16
existing `use` lines of the progression patterns each gain a `voicing`
field; the table counts those 16 values.

| | Non-blank lines | Field values | Characters |
| --- | --- | --- | --- |
| Today | 35 | 388 | 3,869 |
| A and B | 39 | 195 | 2,924 |
| A, B and C | 15 | 79 | about 1,150 |

- **A and B** halve the values. The line count rises by the four voicing
  declarations, but each stroke line reads as what it does:
  `voicing = &g; top = 4; strum = up`.
- **C** writes the rhythm once. Changing the strum pattern is then one edit
  instead of four.
- **Exactness.** A rewrite with A and B that keeps draft 4's velocity lists
  and its 10 ms D and C staggers expands to the same events. The tilt rewrite
  changes velocities by at most 0.016. The single-rhythm rewrite also plays the
  D and C chords at 8 ms per string, and their upstrokes on four strings where
  draft 4 used three.

## Rejected alternatives

- **Keep strums for tools.** This is today's position. It keeps the 388
  values in the source, and T3's AI wrote them by hand because no tool
  existed.
- **A total sweep time** (`spread = 40ms`) instead of a per-string stagger.
  It makes a four-string upstroke slower per string than a six-string
  downstroke at the same setting. Draft 4 kept the per-string time constant.
- **Fret shapes**, such as `frets = [3, 2, 0, 0, 0, 3]` over a tuning. They
  are guitar-specific, and the AI wrote correct pitch lists in T3. A voicing
  is a pitch list, so it serves keyboard voicings too.
- **A strum rhythm string**, such as `"D-DU-UDU"`. It is a second notation
  inside a string, and its strokes would have no IDs to override.
- **An explicit marker for a supplied voicing**, such as
  `voicing = from_use`. It would make a rhythm pattern's dependence visible
  in the chord. Omitting both `pitches` and `voicing` already says the same,
  and the compiler rejects a chord with nothing in scope.

## Not proposed

- **Picking by string.** Ferry's picking would need notes that name a member
  of the supplied voicing instead of a pitch. Voicings differ in size, a D
  chord on four strings and a G on six, so this needs a string model with
  muted strings. That is its own proposal. Until then a picking pattern per
  shape is the safe form; the [producer guide](../ai-producer-guide.md) now
  warns that a transposed shape can leave the instrument's range.
- **Voicings in libraries.** A library could export common shapes. This
  waits until compositions use voicings.
- **Humanizing.** Random variation in stagger or velocity stays a tool
  operation, as §9 requires.
- **Velocity curves other than a straight tilt.** A velocity list stays
  available for anything else.

## Versioning

As with the chord leaf, these additions make invalid source valid and change
the meaning of no valid source. Under
[§26](../../MaaC-1-Specification.md#26-versioning-and-extensions) they can
join `maac 1`. The plan format does not change, because members compile to
the same note events with offsets and velocities.

## Acceptance

**Fixtures**

- A four-pitch downstroke and upstroke with an 8 ms and a 30 ms stagger give
  the exact offsets. A tilt gives the exact velocities, and one that leaves
  `[0, 1]` is `E_RANGE`.
- `stagger` or `tilt` without `strum`, `tilt` with a velocity list, both
  `pitches` and `voicing`, both `top` and `bottom`, and an out-of-range `top`
  are each rejected with the stated code.
- A voicing supplied by a `place`, overridden by a nested `use`, and ignored
  by a chord with its own pitches resolves as stated. A chord with nothing in
  scope is `E_REFERENCE`.
- Transposition, `stretch`, `cut`, the groove and member overrides behave as
  they do for written pitch lists.

**T3 draft 4 (outside the repository)**

- The A-and-B rewrite with the original velocity lists and staggers gives the
  same `maac query-events` output and a byte-identical render.
- The A, B and C rewrite compiles, and its counts match the table above.

**Specification and documentation**

- §4 lists `voicing`. §8.2 gains the strum fields, `voicing`, `top` and
  `bottom`, and loses "no strum or voicing rule is inferred". §9 and §10 give
  `use` and `place` their `voicing` field.
- The editing protocol, canonical form and conformance vectors cover the new
  kind and fields.
- The [tutorial](../tutorial.md), the [reference](../reference.md) and the
  producer guide show a strummed progression with voicings. The T3 F18 row
  and the [language plan](language-specification-plan.md) are updated.

## Cost and risk

- **Cost: medium.** A and B are compile-time expansions inside the chord
  leaf. C adds the use-site scope, the larger part, because the voicing must
  travel through expansion as `transpose` does. The editing protocol and
  canonical hashing need the new kind and fields.
- **Risk: a rhythm whose chords change under it.** With C, the members of a
  stroke depend on where it is used. Error messages must name both the chord
  and the use that supplied the voicing. The acceptance fixtures cover the
  scope rules.
- **Risk: one more way to write a chord.** `pitches` stays the plain form.
  The guide recommends voicings only when a shape repeats.

## Decisions

Recommendations first:

1. **Strum fields (A):** `strum`, `stagger` and `tilt` on the chord leaf.
   *Recommended* over keeping offset and velocity lists as the only form.
2. **Stagger:** the time per string, with an 8 ms default. *Recommended* over
   a total sweep time, and over no default.
3. **Voicings (B):** a top-level `voicing` kind, with `top` and `bottom` on
   the chord. *Recommended* over repeating pitch lists.
4. **Supplied voicings (C):** `voicing` on `use` and `place`, taken by chords
   with no pitches of their own. *Recommended* over voicings named only on
   each chord, which keeps one stroke pattern per chord.
5. **Versioning:** additive `maac 1` core, with no plan change.
   *Recommended*, as for the chord leaf.
6. **Order:** A, then B, then C, each landing with its fixtures.

The owner accepted all six recommendations on 2026-10-13.

## Results

A, B and C were built and tested in that order: A with
[`tests/chord_strum.rs`](../../tests/chord_strum.rs), B and C with
[`tests/chord_voicing.rs`](../../tests/chord_voicing.rs). They landed in one
commit, because all three change the same chord lowering.

**T3 draft 4.**

- The A-and-B rewrite that keeps draft 4's velocity lists and its 10 ms D
  and C staggers gives the same `maac query-events` output, 2,025 events, and
  a byte-identical render (SHA-256 `dfb131be27864925…`).
- The A, B and C rewrite compiles. It has 2,091 events: 66 more, because its
  single rhythm plays the D and C upstrokes on four strings where draft 4
  used three. Its four voicings and two stroke patterns take the 15 lines
  counted above.

**Listening.** On 2026-10-13 the owner compared the two renders on a phone
as 192 kbit/s AAC. A was draft 4 (`dfb131be27864925…`); B was the A, B and C
rewrite (`6542e5e0715ab995…`). Only the strummed guitar differs, in three
ways. B plays the D and C downstrokes 8 ms apart instead of 10 ms, and their
upstrokes on four strings instead of three. It also replaces draft 4's
hand-set velocity lists with an even `tilt`, which differs by at most 0.02.
The masters measure −14.43 and −14.35 LUFS.

- A against B, with file names that showed which was draft 4: "B도 괜찮은데 A가
  확실히 풍부한 느낌이 있네" ("B is fine too, but A clearly feels richer").
- To find the cause, X took A's strum timing and strings with B's
  velocities (`eea4de04d1409a9b…`), and Y took B's with A's
  (`77857368d80d71a5…`). Their compiled plans differ from A and from B only
  there. They were sent as P and Q, with the order drawn at random and
  revealed after the verdict: "거의 구별이 안 되네" ("I can barely tell them
  apart").

So no single difference accounts for A's richness. Either the timing and
the dynamics each add a little, or the open A/B comparison made the
difference sound larger than it is. The owner stopped there. No language
change follows: a voicing carries no stagger and no upstroke string count.
A shape that needs its own can still have its own stroke pattern.

**Differences from the proposal.**

- A chord with no pitches, no voicing and nothing supplying one is
  `E_REFERENCE` where it is expanded, so a pattern that no placement reaches
  is not checked for it. A `top` or `bottom` too large for a supplied voicing,
  and a per-member list of the wrong length, are checked at each occurrence;
  the diagnostic ends with the voicing and what supplied it, such as
  "(voicing `d` from use `b2`)".
- Editing: an edit that changes the number of pitches a chord plays, through
  its `pitches`, its voicing, its `top` or `bottom`, or the voicing a use or
  placement supplies, is refused with `E_INSTANCE_TARGET` when an existing
  override would move to another member. A voicing edit reports only the
  occurrences that play it. Materializing a placement copies a voicing that a
  chord names, as it copies curves; a supplied voicing stays on its use or
  placement.
- The [reference](../reference.md) covers the CLI and the Rust library, not
  the language, so the example is in the [tutorial](../tutorial.md) and the
  [producer guide](../ai-producer-guide.md) only.
